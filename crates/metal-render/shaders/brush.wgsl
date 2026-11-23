// fx.brush -- draw module (dab strokes, search-chosen strokes, fade, proxy
// downsample, composite). `brush_common.wgsl` is prepended.

@group(0) @binding(1) var tex0: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var tex1: texture_2d<f32>;
@group(1) @binding(0) var<storage, read> chosen: array<Stroke>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) col: vec4<f32>,
    @location(2) @interpolate(flat) ext: vec4<f32>, // hl, hw, bend, shape
    @location(3) uv: vec2<f32>,
};

fn src_at(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(tex0, samp, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb;
}

fn rot(a: f32, v: vec2<f32>) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

// Dab `i` of this frame's `m`: a grid cell in a scrambled order, sized by its
// rank in the frame (coarse first), turned along the frame's edges, coloured
// from the frame at its centre.
fn make_dab(i: u32, m: u32) -> Stroke {
    let aspect = u.a.x;
    let alea = u.a.z;
    let k = u.e.y + i;
    let cells = u.e.w;
    let cell = ((k % cells) * u.g.w) % cells;
    let gx = u32(u.d.y);
    let cx = cell % gx;
    let cy = cell / gx;
    let jx = (rnd(k, 1u) - 0.5) * alea * 1.6;
    let jy = (rnd(k, 2u) - 0.5) * alea * 1.6;
    let c = vec2<f32>((f32(cx) + 0.5 + jx) / u.d.y, (f32(cy) + 0.5 + jy) / u.d.z);
    let t = f32(i) / max(f32(m) - 1.0, 1.0);
    let size = u.d.w * mix(u.b.x, u.b.y, t) * (1.0 + (rnd(k, 3u) - 0.5) * alea);
    let stretch = sqrt(u.b.z);
    let hl = size * 0.5 * stretch;
    let hw = size * 0.5 / stretch;

    // Edge direction from the frame's luminance gradient around the centre.
    let d = max(hw * 0.6, 0.004);
    let ux = vec2<f32>(d / aspect, 0.0);
    let uy = vec2<f32>(0.0, d);
    let g = vec2<f32>(lum(src_at(c + ux)) - lum(src_at(c - ux)), lum(src_at(c + uy)) - lum(src_at(c - uy)));
    let edge_ang = atan2(g.y, g.x) + 0.5 * PI;
    let base = u.c.x + (rnd(k, 4u) - 0.5) * alea * PI;
    let w = u.b.w * smoothstep(0.02, 0.15, length(g));
    let ang = base + wrap_ang(edge_ang - base) * w;

    // Colour: the centre and four taps around it, so a stroke is the mean of
    // what it covers rather than one noisy pixel.
    let tl = rot(ang, vec2<f32>(hl * 0.5, 0.0)) / vec2<f32>(aspect, 1.0);
    let tw = rot(ang, vec2<f32>(0.0, hw * 0.5)) / vec2<f32>(aspect, 1.0);
    var col = (src_at(c) * 2.0 + src_at(c + tl) + src_at(c - tl) + src_at(c + tw) + src_at(c - tw)) / 6.0;
    // Mostly multiplicative, so black stays near black: a shared lightness shift (a little of it additive, so black can move) and a
    // smaller per-channel one.
    let jl = (rnd(k, 5u) - 0.5) * u.c.y * 1.2;
    let jc = (vec3<f32>(rnd(k, 6u), rnd(k, 7u), rnd(k, 10u)) - 0.5) * u.c.y * 0.5;
    col = col * (vec3<f32>(1.0 + jl) + jc) + vec3<f32>(jl * 0.1);
    let l = lum(col);
    col = clamp(vec3<f32>(l) + (col - vec3<f32>(l)) * u.c.z, vec3<f32>(0.0), vec3<f32>(1.0));

    var s: Stroke;
    s.c = c;
    s.ang = ang;
    s.hl = hl;
    s.hw = hw;
    s.bend = (rnd(k, 9u) - 0.5) * 2.0 * (0.3 + alea * 0.7);
    s.shape = pick_shape(u.e.x, k);
    s.pad = 0.0;
    s.col = vec4<f32>(col, u.a.y);
    return s;
}

fn emit(s: Stroke, vi: u32) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let cn = corners[vi];
    let ex = s.hl * 1.06 + 0.012;
    let ey = s.hw * 1.06 + abs(s.bend) * s.hl * 0.5 + 0.012;
    let local = cn * vec2<f32>(ex, ey);
    let p = vec2<f32>(s.c.x * u.a.x, s.c.y) + rot(s.ang, local);
    let uv = vec2<f32>(p.x / u.a.x, p.y);
    var o: VOut;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.local = local;
    o.col = s.col;
    o.ext = vec4<f32>(s.hl, s.hw, s.bend, s.shape);
    o.uv = uv;
    return o;
}

@vertex
fn vs_dab(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    return emit(make_dab(ii, u.e.z), vi);
}

@vertex
fn vs_chosen(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    return emit(chosen[ii], vi);
}

@fragment
fn fs_stroke(in: VOut) -> @location(0) vec4<f32> {
    let d = stroke_d(in.ext.w, in.local, in.ext.x, in.ext.y, in.ext.z);
    let aa = max(fwidth(d), 1e-5);
    let cov = clamp(0.5 - d / aa, 0.0, 1.0);
    var col = in.col.rgb;
    var a = in.col.a * cov;
    if (u.h.x < 0.5) {
        let rim = max(in.ext.y * 0.4, 0.004);
        let edge = 1.0 - smoothstep(0.0, rim, -d);
        // An ink line that reads on either: dark on light strokes, light on dark.
        let ink = select(col * 0.3, vec3<f32>(0.25) + col * 0.6, lum(col) < 0.3);
        col = mix(col, ink, u.a.w * edge);
        a = a * (1.0 - u.c.w * 0.5 * grain(in.uv));
    }
    return vec4<f32>(col, a);
}

struct FVOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FVOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let xy = p[vi];
    var o: FVOut;
    o.pos = vec4<f32>(xy, 0.0, 1.0);
    o.uv = xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return o;
}

// The canvas moves toward `mix(live frame, paper)` by `fade rate`; a fresh
// canvas (rate 1) lands on it.
@fragment
fn fs_fade(in: FVOut) -> @location(0) vec4<f32> {
    let target_col = mix(src_at(in.uv), paper_color(in.uv), u.f.z);
    return vec4<f32>(target_col, u.f.y);
}

// 4x4 box of the frame at the proxy's resolution.
@fragment
fn fs_down(in: FVOut) -> @location(0) vec4<f32> {
    let dim = vec2<f32>(textureDimensions(tex0));
    let stp = dim / 64.0 / 4.0 / dim;
    var acc = vec3<f32>(0.0);
    for (var j = 0; j < 4; j = j + 1) {
        for (var i = 0; i < 4; i = i + 1) {
            acc = acc + src_at(in.uv + (vec2<f32>(f32(i), f32(j)) - 1.5) * stp);
        }
    }
    return vec4<f32>(acc / 16.0, 1.0);
}

// The picture: the frame mixed toward the canvas by `cant`.
@fragment
fn fs_comp(in: FVOut) -> @location(0) vec4<f32> {
    let canvas = textureSampleLevel(tex1, samp, in.uv, 0.0).rgb;
    return vec4<f32>(mix(src_at(in.uv), canvas, u.f.w), 1.0);
}
