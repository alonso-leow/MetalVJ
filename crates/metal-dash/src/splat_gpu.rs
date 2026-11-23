//! The GPU layer of the venue viewport's splat view.
//!
//! The venue's splats are drawn into an offscreen target inside egui-wgpu's
//! `prepare` (its own command buffer), then blitted into the surface pass in
//! `paint`. The CPU only builds the splats' world-space data when the clouds
//! or their placement change; per frame the GPU does the rest:
//!
//! 1. a compute pass projects every splat to a screen ellipse (the same EWA
//!    maths as `Splats::project`, constants shared with `metal_geom::splat`),
//!    lights it by the fixtures' beams (`splat::beam_light`), and writes a
//!    depth key;
//! 2. a bitonic sort of the keys (one dispatch per step) puts them far to
//!    near; culled splats sort last;
//! 3. a depth prepass of the venue's occluding triangles (surfaces and
//!    `occludes` objects, minus the objects a splat set is attached to), so a
//!    splat behind a wall is hidden;
//! 4. the splats as instanced screen-space quads in sorted order, with
//!    premultiplied "over" blending and a Gaussian falloff. Depth is tested
//!    but not written.
//!
//! View-dependent colour (SH degree > 0) is the CPU layer's only; here every
//! splat keeps its base colour. Everything is `gpu`-gated.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Painter, Rect};
use glam::{Mat3, Mat4, Vec3};
use metal_geom::splat::{self, BeamLight, Splats};
use metal_geom::Mesh;
use metal_perform::venue::Venue;

/// The colour format of the surface pass egui-wgpu draws into. window.rs
/// inserts it into the renderer's `callback_resources`.
pub(crate) struct SplatTargetFormat(pub wgpu::TextureFormat);

/// One splat set in the venue's frame: `splats` are in file space, `model`
/// maps them to world, and `object` names the venue object they belong to
/// (its mesh does not occlude its own splats).
pub(crate) struct SetRef<'a> {
    pub splats: &'a Splats,
    pub model: Mat4,
    pub object: &'a str,
}

/// Floats per splat in the scene buffer: position and opacity, the six
/// unique entries of the world covariance (two vec4s), and the colour.
const SPLAT_FLOATS: usize = 16;
/// Floats per projected instance: centre and axis a, axis b and depth, colour.
const INSTANCE_FLOATS: usize = 12;
/// Beams the compute pass lights splats with; more are dropped.
const MAX_LIGHTS: usize = 16;
const WORKGROUP: u32 = 256;
/// The largest padded splat count: `N / WORKGROUP` workgroups must fit one
/// dispatch dimension.
const MAX_SORTED: u32 = 1 << 23;
const GLOBALS_BYTES: u64 = 80;
/// view, proj (128), size (16), counts (16), lights (48 each).
const FRAME_BYTES: u64 = 160 + 48 * MAX_LIGHTS as u64;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Tessellation of curved surfaces that occlude splats.
const SURFACE_U: usize = 24;
const SURFACE_V: usize = 12;

const SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    size: vec4<f32>,
};
@group(0) @binding(0) var<uniform> g: Globals;

@vertex
fn vs_depth(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
    return g.view_proj * vec4<f32>(p, 1.0);
}

@fragment
fn fs_depth() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}

struct Inst {
    centre_a: vec4<f32>,
    b_z: vec4<f32>,
    rgba: vec4<f32>,
};
@group(1) @binding(0) var<storage, read> insts: array<Inst>;
@group(1) @binding(1) var<storage, read> order: array<u32>;

struct SplatOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) rgba: vec4<f32>,
};

@vertex
fn vs_splat(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> SplatOut {
    let s = insts[order[ii]];
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );
    let c = corners[vi];
    var o: SplatOut;
    o.local = c;
    o.rgba = s.rgba;
    if (s.rgba.a <= 0.0) {
        // Culled: outside the clip volume.
        o.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return o;
    }
    let px = s.centre_a.xy + c.x * s.centre_a.zw + c.y * s.b_z.xy;
    let ndc = vec2<f32>(px.x / g.size.x * 2.0 - 1.0, 1.0 - px.y / g.size.y * 2.0);
    o.pos = vec4<f32>(ndc, s.b_z.z, 1.0);
    return o;
}

@fragment
fn fs_splat(i: SplatOut) -> @location(0) vec4<f32> {
    let a = i.rgba.a * exp(-4.5 * dot(i.local, i.local));
    if (a < 1.0 / 255.0) {
        discard;
    }
    return vec4<f32>(i.rgba.rgb * a, a);
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct BlitOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) vi: u32) -> BlitOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: BlitOut;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@fragment
fn fs_blit(i: BlitOut) -> @location(0) vec4<f32> {
    return textureSample(src, samp, i.uv);
}
"#;

/// Projection, beam light and sort. `{NAME}` placeholders are filled from
/// `metal_geom::splat`'s constants by [`compute_shader`], so both layers
/// project and light alike.
const COMPUTE: &str = r#"
struct Light {
    apex_cos: vec4<f32>,
    dir_len: vec4<f32>,
    rgb: vec4<f32>,
};
struct Frame {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    // width, height, 1 when orthographic, unused
    size: vec4<f32>,
    // splats, padded length, lights, unused
    counts: vec4<u32>,
    lights: array<Light, {MAX_LIGHTS}>,
};
struct Splat {
    pos_opacity: vec4<f32>,
    cov_a: vec4<f32>,
    cov_b: vec4<f32>,
    rgb: vec4<f32>,
};
struct Inst {
    centre_a: vec4<f32>,
    b_z: vec4<f32>,
    rgba: vec4<f32>,
};
struct Step {
    j: u32,
    k: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var<uniform> f: Frame;
@group(0) @binding(1) var<storage, read> splats: array<Splat>;
@group(0) @binding(2) var<storage, read_write> insts: array<Inst>;
@group(0) @binding(3) var<storage, read_write> keys: array<u32>;
@group(0) @binding(4) var<storage, read_write> order: array<u32>;
@group(1) @binding(0) var<uniform> st: Step;

const CULLED: u32 = 0xffffffffu;

fn cull(i: u32) {
    keys[i] = CULLED;
    insts[i] = Inst(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
}

fn clamp_len(v: vec2<f32>) -> vec2<f32> {
    let l = length(v);
    if (l > {MAX_AXIS_PX}) {
        return v * ({MAX_AXIS_PX} / l);
    }
    return v;
}

fn beam_light(p: vec3<f32>) -> vec3<f32> {
    var out = vec3<f32>(0.0);
    for (var n = 0u; n < f.counts.z; n = n + 1u) {
        let l = f.lights[n];
        let v = p - l.apex_cos.xyz;
        let along = dot(v, l.dir_len.xyz);
        let dist = length(v);
        if (along <= 0.0 || along > l.dir_len.w || dist <= 0.0) {
            continue;
        }
        let c = along / dist;
        let ch = l.apex_cos.w;
        if (c <= ch) {
            continue;
        }
        let edge = min((c - ch) / max((1.0 - ch) * {BEAM_EDGE}, 1e-6), 1.0);
        out = out + l.rgb.xyz * (edge * (1.0 - 0.5 * along / l.dir_len.w));
    }
    return out;
}

@compute @workgroup_size({WORKGROUP})
fn cs_project(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= f.counts.y) {
        return;
    }
    order[i] = i;
    if (i >= f.counts.x) {
        cull(i);
        return;
    }
    let s = splats[i];
    let t = (f.view * vec4<f32>(s.pos_opacity.xyz, 1.0)).xyz;
    if (t.z > -{NEAR_CULL}) {
        cull(i);
        return;
    }
    let clip = f.proj * vec4<f32>(t, 1.0);
    if (clip.w <= 0.0) {
        cull(i);
        return;
    }
    let vw = f.size.x;
    let vh = f.size.y;
    let cx = (clip.x / clip.w * 0.5 + 0.5) * vw;
    let cy = (1.0 - (clip.y / clip.w * 0.5 + 0.5)) * vh;
    let fx = f.proj[0][0] * vw * 0.5;
    let fy = f.proj[1][1] * vh * 0.5;
    let d = -t.z;
    var j0 = vec3<f32>(fx / d, 0.0, fx * t.x / (d * d));
    var j1 = vec3<f32>(0.0, -fy / d, -fy * t.y / (d * d));
    if (f.size.z > 0.5) {
        j0 = vec3<f32>(fx, 0.0, 0.0);
        j1 = vec3<f32>(0.0, -fy, 0.0);
    }
    let w = mat3x3<f32>(f.view[0].xyz, f.view[1].xyz, f.view[2].xyz);
    let cw = mat3x3<f32>(
        vec3<f32>(s.cov_a.x, s.cov_a.y, s.cov_a.z),
        vec3<f32>(s.cov_a.y, s.cov_a.w, s.cov_b.x),
        vec3<f32>(s.cov_a.z, s.cov_b.x, s.cov_b.y),
    );
    let sig = w * cw * transpose(w);
    let a = dot(j0, sig * j0) + {LOW_PASS};
    let b = dot(j0, sig * j1);
    let c = dot(j1, sig * j1) + {LOW_PASS};
    let mid = 0.5 * (a + c);
    let rad = sqrt(max(mid * mid - (a * c - b * b), 0.0));
    let l1 = mid + rad;
    let l2 = max(mid - rad, 0.0);
    var v = vec2<f32>(0.0, 1.0);
    if (abs(b) > 1e-12) {
        v = normalize(vec2<f32>(b, l1 - a));
    } else if (a >= c) {
        v = vec2<f32>(1.0, 0.0);
    }
    let axis_a = clamp_len(v * (3.0 * sqrt(l1)));
    let axis_b = clamp_len(vec2<f32>(-v.y, v.x) * (3.0 * sqrt(l2)));
    let r = max(length(axis_a), length(axis_b));
    if (cx < -r || cx > vw + r || cy < -r || cy > vh + r) {
        cull(i);
        return;
    }
    let light = beam_light(s.pos_opacity.xyz);
    let rgb = clamp(s.rgb.xyz * (vec3<f32>(1.0) + {BEAM_GAIN} * light) + {BEAM_ADD} * light, vec3<f32>(0.0), vec3<f32>(1.0));
    let ndc_z = clamp(clip.z / clip.w, 0.0, 1.0);
    insts[i] = Inst(vec4<f32>(cx, cy, axis_a), vec4<f32>(axis_b, ndc_z, 0.0), vec4<f32>(rgb, s.pos_opacity.w));
    // Ascending keys run far to near: a larger distance has larger bits.
    keys[i] = CULLED - 1u - bitcast<u32>(length(t));
}

@compute @workgroup_size({WORKGROUP})
fn cs_sort(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let l = i ^ st.j;
    if (l <= i || l >= f.counts.y) {
        return;
    }
    let ki = keys[i];
    let kl = keys[l];
    let up = (i & st.k) == 0u;
    if ((up && ki > kl) || (!up && ki < kl)) {
        keys[i] = kl;
        keys[l] = ki;
        let oi = order[i];
        order[i] = order[l];
        order[l] = oi;
    }
}
"#;

/// [`COMPUTE`] with the shared constants filled in.
pub(crate) fn compute_shader() -> String {
    let f = |v: f32| format!("{v:?}");
    COMPUTE
        .replace("{MAX_LIGHTS}", &MAX_LIGHTS.to_string())
        .replace("{WORKGROUP}", &WORKGROUP.to_string())
        .replace("{MAX_AXIS_PX}", &f(splat::MAX_AXIS_PX))
        .replace("{NEAR_CULL}", &f(splat::NEAR_CULL))
        .replace("{LOW_PASS}", &f(splat::LOW_PASS))
        .replace("{BEAM_EDGE}", &f(splat::BEAM_EDGE))
        .replace("{BEAM_GAIN}", &f(splat::BEAM_GAIN))
        .replace("{BEAM_ADD}", &f(splat::BEAM_ADD))
}

/// True when the device can run this layer: compute, and two storage buffers
/// in the vertex stage. window.rs checks it before turning the layer on.
pub(crate) fn supported(device: &wgpu::Device) -> bool {
    let l = device.limits();
    l.max_storage_buffers_per_shader_stage >= 4
        && l.max_compute_workgroup_size_x >= WORKGROUP
        && l.max_compute_invocations_per_workgroup >= WORKGROUP
        && l.max_compute_workgroups_per_dimension >= MAX_SORTED / WORKGROUP
}

/// The splats of every set in world space, `SPLAT_FLOATS` each: centre and
/// opacity, the world covariance `M Σ Mᵀ` (xx xy xz yy, yz zz), the colour.
pub(crate) fn scene_floats(sets: &[SetRef<'_>]) -> Vec<f32> {
    let n: usize = sets.iter().map(|s| s.splats.len()).sum();
    let mut out = Vec::with_capacity(n * SPLAT_FLOATS);
    for set in sets {
        let m = Mat3::from_mat4(set.model);
        for i in 0..set.splats.len() {
            let p = set.model.transform_point3(Vec3::from_array(set.splats.pos[i]));
            let c = m * set.splats.covariance3(i) * m.transpose();
            let rgb = set.splats.rgb[i];
            out.extend_from_slice(&[
                p.x, p.y, p.z, set.splats.opacity[i],
                c.x_axis.x, c.y_axis.x, c.z_axis.x, c.y_axis.y,
                c.z_axis.y, c.z_axis.z, 0.0, 0.0,
                rgb[0], rgb[1], rgb[2], 0.0,
            ]);
        }
    }
    out
}

/// The frame uniform of the compute pass.
pub(crate) fn frame_bytes(view: Mat4, proj: Mat4, size_px: [u32; 2], count: u32, lights: &[BeamLight]) -> Vec<u8> {
    let ortho = proj.w_axis.w == 1.0 && proj.z_axis.w == 0.0;
    let mut out = Vec::with_capacity(FRAME_BYTES as usize);
    let mut put = |v: f32| out.extend_from_slice(&v.to_ne_bytes());
    for v in view.to_cols_array().into_iter().chain(proj.to_cols_array()) {
        put(v);
    }
    for v in [size_px[0] as f32, size_px[1] as f32, if ortho { 1.0 } else { 0.0 }, 0.0] {
        put(v);
    }
    let lights = &lights[..lights.len().min(MAX_LIGHTS)];
    for v in [count, padded(count), lights.len() as u32, 0] {
        out.extend_from_slice(&v.to_ne_bytes());
    }
    for l in lights {
        for v in [l.apex[0], l.apex[1], l.apex[2], l.cos_half, l.dir[0], l.dir[1], l.dir[2], l.length, l.rgb[0], l.rgb[1], l.rgb[2], 0.0] {
            out.extend_from_slice(&v.to_ne_bytes());
        }
    }
    out.resize(FRAME_BYTES as usize, 0);
    out
}

/// The sort's length: the splat count rounded up to a power of two, at
/// least one workgroup.
fn padded(count: u32) -> u32 {
    count.max(WORKGROUP).next_power_of_two()
}

/// Every `(j, k)` step of a bitonic sort of `n` (a power of two) keys.
pub(crate) fn bitonic_steps(n: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut k = 2;
    while k <= n {
        let mut j = k / 2;
        while j > 0 {
            out.push((j, k));
            j /= 2;
        }
        k *= 2;
    }
    out
}

/// Occluding world-space triangles: every surface, and every `occludes`
/// object not in `skip` (the objects the splat sets belong to).
pub(crate) fn occluder_tris(venue: &Venue, skip: &[&str]) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    let mut push = |m: &Mesh| {
        for tri in m.indices.chunks_exact(3) {
            for &k in tri {
                if let Some(p) = m.positions.get(k as usize) {
                    out.push(*p);
                }
            }
        }
    };
    for s in &venue.surfaces {
        push(&s.tessellate(SURFACE_U, SURFACE_V));
    }
    for o in &venue.objects {
        if o.occludes && !skip.contains(&o.id.as_str()) {
            push(&o.mesh());
        }
    }
    out
}

/// The key of the splat sets as placed: equal keys mean the scene buffer
/// needs no rebuild or upload.
pub(crate) fn scene_key(sets: &[SetRef<'_>]) -> u64 {
    let mut h = DefaultHasher::new();
    for s in sets {
        (s.splats as *const Splats as usize).hash(&mut h);
        s.splats.len().hash(&mut h);
        for f in s.model.to_cols_array() {
            f.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// The key of the occluding triangles.
pub(crate) fn tris_key(tris: &[[f32; 3]]) -> u64 {
    let mut h = DefaultHasher::new();
    tris.len().hash(&mut h);
    for p in tris {
        for f in p {
            f.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// The world-space splat data, kept in egui temp memory under its key.
#[derive(Clone)]
struct Scene {
    key: u64,
    floats: Arc<Vec<f32>>,
}

/// Paints the splat sets through the GPU layer. Returns `false` when there
/// is nothing to paint or too much to sort, so the caller falls back to the
/// CPU layer.
pub(crate) fn paint_layer(
    painter: &Painter,
    rect: Rect,
    view: Mat4,
    proj: Mat4,
    sets: &[SetRef<'_>],
    venue: &Venue,
    lights: &[BeamLight],
) -> bool {
    if sets.is_empty() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return false;
    }
    let count: usize = sets.iter().map(|s| s.splats.len()).sum();
    if count == 0 || count > MAX_SORTED as usize {
        return false;
    }
    let ppp = painter.ctx().pixels_per_point();
    let w = (rect.width() * ppp).round().max(1.0) as u32;
    let h = (rect.height() * ppp).round().max(1.0) as u32;
    let skip: Vec<&str> = sets.iter().map(|s| s.object).collect();
    let tris = occluder_tris(venue, &skip);
    let key = scene_key(sets);

    let id = egui::Id::new("viewport_splats_gpu_scene");
    let scene = match painter.ctx().data_mut(|d| d.get_temp::<Scene>(id)) {
        Some(s) if s.key == key => s,
        _ => {
            let s = Scene { key, floats: Arc::new(scene_floats(sets)) };
            painter.ctx().data_mut(|d| d.insert_temp(id, s.clone()));
            s
        }
    };

    painter.add(egui_wgpu::Callback::new_paint_callback(
        rect,
        SplatCallback {
            scene: scene.floats,
            scene_key: key,
            count: count as u32,
            tris_key: tris_key(&tris),
            tris: Arc::new(tris),
            view,
            proj,
            size_px: [w, h],
            lights: lights.to_vec(),
        },
    ));
    true
}

/// The egui-wgpu callback. `prepare` renders the offscreen target; `paint`
/// blits it into the surface pass.
pub(crate) struct SplatCallback {
    pub scene: Arc<Vec<f32>>,
    pub scene_key: u64,
    pub count: u32,
    pub tris: Arc<Vec<[f32; 3]>>,
    pub tris_key: u64,
    pub view: Mat4,
    pub proj: Mat4,
    pub size_px: [u32; 2],
    pub lights: Vec<BeamLight>,
}

struct Offscreen {
    size: [u32; 2],
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    blit_bg: wgpu::BindGroup,
}

/// The splat buffers for one padded length: the scene (input), the projected
/// instances, the sort keys and order, and their bind groups.
struct SortBuffers {
    padded: u32,
    splats: wgpu::Buffer,
    splats_cap: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    insts: wgpu::Buffer,
    #[cfg_attr(not(test), allow(dead_code))]
    order: wgpu::Buffer,
    compute_bg: wgpu::BindGroup,
    draw_bg: wgpu::BindGroup,
    steps_bg: wgpu::BindGroup,
    step_count: u32,
}

/// Pipelines and buffers, created once per device in `callback_resources`.
struct SplatGpu {
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    frame: wgpu::Buffer,
    depth_pipe: wgpu::RenderPipeline,
    splat_pipe: wgpu::RenderPipeline,
    blit_pipe: wgpu::RenderPipeline,
    project_pipe: wgpu::ComputePipeline,
    sort_pipe: wgpu::ComputePipeline,
    blit_bgl: wgpu::BindGroupLayout,
    compute_bgl: wgpu::BindGroupLayout,
    draw_bgl: wgpu::BindGroupLayout,
    steps_bgl: wgpu::BindGroupLayout,
    step_stride: u64,
    sampler: wgpu::Sampler,
    target: Option<Offscreen>,
    sort: Option<SortBuffers>,
    tris_buf: wgpu::Buffer,
    tris_cap: u64,
    tri_count: u32,
    scene_uploaded: Option<u64>,
    tris_uploaded: Option<u64>,
}

fn floats_to_bytes(data: impl Iterator<Item = f32>) -> Vec<u8> {
    let mut out = Vec::new();
    for f in data {
        out.extend_from_slice(&f.to_ne_bytes());
    }
    out
}

fn entry(binding: u32, b: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: b.as_entire_binding() }
}

fn storage_entry(binding: u32, visibility: wgpu::ShaderStages, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages, dynamic: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: if dynamic { wgpu::BufferSize::new(16) } else { None },
        },
        count: None,
    }
}

impl SplatGpu {
    fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport-splats"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let compute = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport-splats-compute"),
            source: wgpu::ShaderSource::Wgsl(compute_shader().into()),
        });
        let bgl = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let globals_bgl = bgl(
            "viewport-splats-globals",
            &[uniform_entry(0, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT, false)],
        );
        let draw_bgl = bgl(
            "viewport-splats-draw",
            &[
                storage_entry(0, wgpu::ShaderStages::VERTEX, true),
                storage_entry(1, wgpu::ShaderStages::VERTEX, true),
            ],
        );
        let cs = wgpu::ShaderStages::COMPUTE;
        let compute_bgl = bgl(
            "viewport-splats-compute",
            &[
                uniform_entry(0, cs, false),
                storage_entry(1, cs, true),
                storage_entry(2, cs, false),
                storage_entry(3, cs, false),
                storage_entry(4, cs, false),
            ],
        );
        let steps_bgl = bgl("viewport-splats-steps", &[uniform_entry(0, cs, true)]);
        let blit_bgl = bgl(
            "viewport-splats-blit",
            &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        );
        let globals = buffer(device, "viewport-splats-globals", GLOBALS_BYTES, wgpu::BufferUsages::UNIFORM);
        let frame = buffer(device, "viewport-splats-frame", FRAME_BYTES, wgpu::BufferUsages::UNIFORM);
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport-splats-globals"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let layout = |label: &str, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let globals_pl = layout("viewport-splats-globals", &[Some(&globals_bgl)]);
        let splat_pl = layout("viewport-splats-splat", &[Some(&globals_bgl), Some(&draw_bgl)]);
        let blit_pl = layout("viewport-splats-blit", &[Some(&blit_bgl)]);
        let project_pl = layout("viewport-splats-project", &[Some(&compute_bgl)]);
        let sort_pl = layout("viewport-splats-sort", &[Some(&compute_bgl), Some(&steps_bgl)]);

        // Depth prepass: positions only. The colour target exists so the
        // pass attachment count matches, but its write mask is empty.
        let depth_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport-splats-depth"),
            layout: Some(&globals_pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_depth"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_depth"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Less)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let splat_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport-splats-splat"),
            layout: Some(&splat_pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_splat"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_splat"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth_state(false, wgpu::CompareFunction::LessEqual)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let blit_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport-splats-blit"),
            layout: Some(&blit_pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_blit"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_blit"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let compute_pipe = |label: &str, layout: &wgpu::PipelineLayout, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                module: &compute,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let project_pipe = compute_pipe("viewport-splats-project", &project_pl, "cs_project");
        let sort_pipe = compute_pipe("viewport-splats-sort", &sort_pl, "cs_sort");

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viewport-splats"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        SplatGpu {
            globals,
            globals_bg,
            frame,
            depth_pipe,
            splat_pipe,
            blit_pipe,
            project_pipe,
            sort_pipe,
            blit_bgl,
            compute_bgl,
            draw_bgl,
            steps_bgl,
            step_stride: 16u64.div_ceil(align) * align,
            sampler,
            target: None,
            sort: None,
            tris_buf: buffer(device, "viewport-splats-tris", 48, wgpu::BufferUsages::VERTEX),
            tris_cap: 48,
            tri_count: 0,
            scene_uploaded: None,
            tris_uploaded: None,
        }
    }

    /// Recreates the offscreen colour and depth targets when the size changes.
    fn ensure_target(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let extent = wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        };
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport-splats-color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport-splats-depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color = color.create_view(&wgpu::TextureViewDescriptor::default());
        let blit_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport-splats-blit"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.target = Some(Offscreen {
            size,
            color,
            depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            blit_bg,
        });
    }

    /// Recreates the sort buffers when the padded length changes; the scene
    /// buffer is kept when it is big enough.
    fn ensure_sort(&mut self, device: &wgpu::Device, count: u32) {
        let n = padded(count);
        let scene_bytes = (count as u64 * SPLAT_FLOATS as u64 * 4).max(64);
        if self.sort.as_ref().is_some_and(|s| s.padded == n && s.splats_cap >= scene_bytes) {
            return;
        }
        let st = wgpu::BufferUsages::STORAGE;
        let splats_cap = grow(scene_bytes);
        let splats = buffer(device, "viewport-splats-scene", splats_cap, st);
        let insts = buffer(device, "viewport-splats-insts", n as u64 * INSTANCE_FLOATS as u64 * 4, st | wgpu::BufferUsages::COPY_SRC);
        let keys = buffer(device, "viewport-splats-keys", n as u64 * 4, st);
        let order = buffer(device, "viewport-splats-order", n as u64 * 4, st | wgpu::BufferUsages::COPY_SRC);
        let steps = bitonic_steps(n);
        let mut bytes = vec![0u8; (steps.len() as u64 * self.step_stride) as usize];
        for (s, (j, k)) in steps.iter().enumerate() {
            let at = s * self.step_stride as usize;
            bytes[at..at + 4].copy_from_slice(&j.to_ne_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&k.to_ne_bytes());
        }
        let steps_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport-splats-steps"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        steps_buf.slice(..).get_mapped_range_mut().expect("mapped at creation").copy_from_slice(&bytes);
        steps_buf.unmap();
        let compute_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport-splats-compute"),
            layout: &self.compute_bgl,
            entries: &[entry(0, &self.frame), entry(1, &splats), entry(2, &insts), entry(3, &keys), entry(4, &order)],
        });
        let draw_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport-splats-draw"),
            layout: &self.draw_bgl,
            entries: &[entry(0, &insts), entry(1, &order)],
        });
        let steps_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport-splats-steps"),
            layout: &self.steps_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &steps_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(16),
                }),
            }],
        });
        self.sort = Some(SortBuffers {
            padded: n,
            splats,
            splats_cap,
            insts,
            order,
            compute_bg,
            draw_bg,
            steps_bg,
            step_count: steps.len() as u32,
        });
        self.scene_uploaded = None;
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, cb: &SplatCallback) {
        if self.tris_uploaded != Some(cb.tris_key) {
            let tris = floats_to_bytes(cb.tris.iter().flat_map(|p| *p));
            self.tri_count = (tris.len() / 12) as u32;
            if (tris.len() as u64) > self.tris_cap {
                self.tris_cap = grow(tris.len() as u64);
                self.tris_buf = buffer(device, "viewport-splats-tris", self.tris_cap, wgpu::BufferUsages::VERTEX);
            }
            if !tris.is_empty() {
                queue.write_buffer(&self.tris_buf, 0, &tris);
            }
            self.tris_uploaded = Some(cb.tris_key);
        }
        self.ensure_sort(device, cb.count);
        if self.scene_uploaded != Some(cb.scene_key) {
            if let Some(sort) = &self.sort {
                queue.write_buffer(&sort.splats, 0, &floats_to_bytes(cb.scene.iter().copied()));
            }
            self.scene_uploaded = Some(cb.scene_key);
        }
    }

    /// Records the projection and the sort.
    fn record_compute(&self, enc: &mut wgpu::CommandEncoder) {
        let Some(sort) = &self.sort else {
            return;
        };
        let groups = sort.padded / WORKGROUP;
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("viewport-splats-sort"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.project_pipe);
        pass.set_bind_group(0, &sort.compute_bg, &[]);
        pass.dispatch_workgroups(groups, 1, 1);
        pass.set_pipeline(&self.sort_pipe);
        pass.set_bind_group(0, &sort.compute_bg, &[]);
        for s in 0..sort.step_count {
            pass.set_bind_group(1, &sort.steps_bg, &[(s as u64 * self.step_stride) as u32]);
            pass.dispatch_workgroups(groups, 1, 1);
        }
    }
}

fn depth_state(write: bool, compare: wgpu::CompareFunction) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

fn buffer(device: &wgpu::Device, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn grow(needed: u64) -> u64 {
    needed.next_power_of_two().max(48)
}

impl egui_wgpu::CallbackTrait for SplatCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(format) = resources.get::<SplatTargetFormat>().map(|f| f.0) else {
            return Vec::new();
        };
        if !resources.contains::<SplatGpu>() {
            resources.insert(SplatGpu::new(device, format));
        }
        let Some(gpu) = resources.get_mut::<SplatGpu>() else {
            return Vec::new();
        };

        gpu.ensure_target(device, self.size_px);
        let mut globals = Vec::with_capacity(20);
        globals.extend((self.proj * self.view).to_cols_array());
        globals.extend([self.size_px[0] as f32, self.size_px[1] as f32, 0.0, 0.0]);
        queue.write_buffer(&gpu.globals, 0, &floats_to_bytes(globals.into_iter()));
        queue.write_buffer(
            &gpu.frame,
            0,
            &frame_bytes(self.view, self.proj, self.size_px, self.count, &self.lights),
        );
        gpu.upload(device, queue, self);

        let (Some(target), Some(sort)) = (gpu.target.as_ref(), gpu.sort.as_ref()) else {
            return Vec::new();
        };
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("viewport-splats"),
        });
        gpu.record_compute(&mut enc);
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport-splats"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.color,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if gpu.tri_count > 0 {
                pass.set_pipeline(&gpu.depth_pipe);
                pass.set_bind_group(0, &gpu.globals_bg, &[]);
                pass.set_vertex_buffer(0, gpu.tris_buf.slice(..));
                pass.draw(0..gpu.tri_count, 0..1);
            }
            if self.count > 0 {
                pass.set_pipeline(&gpu.splat_pipe);
                pass.set_bind_group(0, &gpu.globals_bg, &[]);
                pass.set_bind_group(1, &sort.draw_bg, &[]);
                pass.draw(0..4, 0..self.count);
            }
        }
        vec![enc.finish()]
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(gpu) = resources.get::<SplatGpu>() else {
            return;
        };
        let Some(target) = gpu.target.as_ref() else {
            return;
        };
        // egui-wgpu sets the viewport to the callback rect; the scissor is the
        // clip rect, which it does not set for callbacks.
        let clip = info.clip_rect_in_pixels();
        pass.set_scissor_rect(
            clip.left_px.max(0) as u32,
            clip.top_px.max(0) as u32,
            clip.width_px.max(0) as u32,
            clip.height_px.max(0) as u32,
        );
        pass.set_pipeline(&gpu.blit_pipe);
        pass.set_bind_group(0, &target.blit_bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(eye: Vec3) -> (Mat4, Mat4) {
        let view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        let proj = Mat4::perspective_rh(55f32.to_radians(), 1.5, 0.05, 500.0);
        (view, proj)
    }

    fn splats(pos: &[[f32; 3]], rgb: &[[f32; 3]]) -> Splats {
        let n = pos.len();
        Splats {
            pos: pos.to_vec(),
            scale: vec![[0.1, 0.1, 0.1]; n],
            rot: vec![[0.0, 0.0, 0.0, 1.0]; n],
            opacity: vec![0.9; n],
            rgb: rgb.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn scene_floats_are_in_world_space() {
        let s = splats(&[[1.0, 0.0, 0.0]], &[[0.2, 0.4, 0.6]]);
        let model = Mat4::from_scale_rotation_translation(Vec3::splat(2.0), glam::Quat::IDENTITY, Vec3::new(0.0, 3.0, 0.0));
        let f = scene_floats(&[SetRef { splats: &s, model, object: "x" }]);
        assert_eq!(f.len(), SPLAT_FLOATS);
        assert_eq!(&f[0..4], &[2.0, 3.0, 0.0, 0.9]);
        // Scale 0.1 doubled: variance (0.2)² on the diagonal, none off it.
        for (k, want) in [(4, 0.04), (5, 0.0), (6, 0.0), (7, 0.04), (8, 0.0), (9, 0.04)] {
            assert!((f[k] - want).abs() < 1e-6, "cov[{k}] = {}", f[k]);
        }
        assert_eq!(&f[12..15], &[0.2, 0.4, 0.6]);
    }

    /// The kernel's compare-and-swap run on the CPU over every step sorts
    /// any keys ascending: the step list and the direction rule are right.
    #[test]
    fn bitonic_steps_sort_ascending() {
        let n = 64u32;
        assert_eq!(bitonic_steps(n).len(), 21);
        let mut keys: Vec<u32> = (0..n).map(|i| i.wrapping_mul(2_654_435_761u32) >> 7).collect();
        for (j, k) in bitonic_steps(n) {
            for i in 0..n {
                let l = i ^ j;
                if l <= i {
                    continue;
                }
                let (ki, kl) = (keys[i as usize], keys[l as usize]);
                let up = i & k == 0;
                if (up && ki > kl) || (!up && ki < kl) {
                    keys.swap(i as usize, l as usize);
                }
            }
        }
        assert!(keys.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn frame_bytes_carry_the_ortho_flag_and_cap_the_lights() {
        let (view, proj) = cam(Vec3::new(0.0, 0.0, 5.0));
        let light = BeamLight { apex: [0.0; 3], dir: [0.0, -1.0, 0.0], cos_half: 0.9, length: 4.0, rgb: [1.0; 3] };
        let b = frame_bytes(view, proj, [640, 480], 300, &vec![light; 40]);
        assert_eq!(b.len() as u64, FRAME_BYTES);
        let word = |at: usize| u32::from_ne_bytes(b[at..at + 4].try_into().unwrap());
        let float = |at: usize| f32::from_ne_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!(float(136), 0.0, "perspective");
        assert_eq!([word(144), word(148), word(152)], [300, 512, MAX_LIGHTS as u32]);
        let ortho = Mat4::orthographic_rh(-4.0, 4.0, -3.0, 3.0, 0.05, 100.0);
        assert_eq!(f32::from_ne_bytes(frame_bytes(view, ortho, [640, 480], 1, &[])[136..140].try_into().unwrap()), 1.0);
        assert_eq!(padded(1), WORKGROUP);
        assert_eq!(padded(257), 512);
    }

    #[test]
    fn shaders_parse_and_validate() {
        use wgpu::naga;
        let check = |src: &str, want: &[&str]| {
            let module = naga::front::wgsl::parse_str(src).expect("WGSL parses");
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
                .validate(&module)
                .expect("WGSL validates");
            let names: Vec<&str> = module.entry_points.iter().map(|e| e.name.as_str()).collect();
            for w in want {
                assert!(names.contains(w), "missing entry point {w}");
            }
        };
        check(SHADER, &["vs_depth", "fs_depth", "vs_splat", "fs_splat", "vs_blit", "fs_blit"]);
        let compute = compute_shader();
        for name in ["MAX_LIGHTS", "WORKGROUP", "MAX_AXIS_PX", "NEAR_CULL", "LOW_PASS", "BEAM_EDGE", "BEAM_GAIN", "BEAM_ADD"] {
            assert!(!compute.contains(&format!("{{{name}}}")), "{name} is filled in");
        }
        check(&compute, &["cs_project", "cs_sort"]);
    }

    /// On a real adapter (skipped without one): the compute pass projects and
    /// sorts like `Splats::project` and `sort_back_to_front`, and lights a
    /// splat inside a beam.
    #[test]
    fn the_gpu_projects_and_sorts_like_the_cpu() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            eprintln!("no adapter: skipped");
            return;
        };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) else {
            eprintln!("no device: skipped");
            return;
        };
        if !supported(&device) {
            eprintln!("device too small: skipped");
            return;
        }
        // Eye at z = 5: distances 5, 8, 6 and one behind the eye.
        let s = splats(
            &[[0.0, 0.0, 0.0], [0.5, 0.0, -3.0], [-0.5, 0.2, -1.0], [0.0, 0.0, 10.0]],
            &[[0.2, 0.2, 0.2], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 1.0]],
        );
        let (view, proj) = cam(Vec3::new(0.0, 0.0, 5.0));
        let size = [640u32, 480];
        let set = SetRef { splats: &s, model: Mat4::IDENTITY, object: "x" };
        let light = BeamLight { apex: [0.0, 3.0, 0.0], dir: [0.0, -1.0, 0.0], cos_half: 0.95, length: 6.0, rgb: [1.0, 0.0, 0.0] };
        let cb = SplatCallback {
            scene: Arc::new(scene_floats(std::slice::from_ref(&set))),
            scene_key: 1,
            count: 4,
            tris: Arc::new(Vec::new()),
            tris_key: 1,
            view,
            proj,
            size_px: size,
            lights: vec![light],
        };
        let mut gpu = SplatGpu::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        queue.write_buffer(&gpu.frame, 0, &frame_bytes(view, proj, size, 4, &cb.lights));
        gpu.upload(&device, &queue, &cb);
        let sort = gpu.sort.as_ref().unwrap();
        let n = sort.padded as u64;
        let read_order = buffer(&device, "order-read", n * 4, wgpu::BufferUsages::MAP_READ);
        let read_insts = buffer(&device, "insts-read", n * 48, wgpu::BufferUsages::MAP_READ);
        let mut enc = device.create_command_encoder(&Default::default());
        gpu.record_compute(&mut enc);
        enc.copy_buffer_to_buffer(&sort.order, 0, &read_order, 0, n * 4);
        enc.copy_buffer_to_buffer(&sort.insts, 0, &read_insts, 0, n * 48);
        queue.submit([enc.finish()]);
        for b in [&read_order, &read_insts] {
            b.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        }
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("poll");
        let order: Vec<u32> = read_order.slice(..).get_mapped_range().expect("mapped").chunks_exact(4).map(|c| u32::from_ne_bytes(c.try_into().unwrap())).collect();
        let insts: Vec<f32> = read_insts.slice(..).get_mapped_range().expect("mapped").chunks_exact(4).map(|c| f32::from_ne_bytes(c.try_into().unwrap())).collect();
        // Farthest first: splat 1 (8 m), 2 (6 m), 0 (5 m); 3 is culled.
        assert_eq!(&order[..3], &[1, 2, 0]);
        for i in 0..3usize {
            let e = s.project(i, view, proj, [640.0, 480.0]).expect("visible");
            let g = &insts[i * 12..i * 12 + 12];
            assert!((g[0] - e.centre[0]).abs() < 0.05 && (g[1] - e.centre[1]).abs() < 0.05, "centre {i}: {g:?} vs {e:?}");
            assert!((g[2] - e.axis_a[0]).abs() < 0.05 && (g[3] - e.axis_a[1]).abs() < 0.05, "axis {i}: {g:?} vs {e:?}");
            assert!((g[11] - 0.9).abs() < 1e-6);
        }
        assert_eq!(insts[3 * 12 + 11], 0.0, "the splat behind the eye is culled");
        // Splat 0 sits in the red beam: lit like `splat::lit`.
        let want = splat::lit([0.2; 3], splat::beam_light(&[light], Vec3::ZERO));
        assert!((insts[8] - want[0]).abs() < 1e-4 && (insts[9] - want[1]).abs() < 1e-4, "{:?} vs {want:?}", &insts[8..11]);
        assert!(insts[8] > 0.3, "the beam brightens red");
    }
}
