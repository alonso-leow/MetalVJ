//! Drawing *over* footage instead of generating a picture — the pipeline half
//! of a transmission, and the cache that keeps it affordable.
//!
//! # What a transmission needs that no other model does
//!
//! Thirty renderers take a uniform and draw. A transmission takes a uniform and
//! **several source frames at once**, because the first effect built on this is
//! a trail and a trail is made of the frames behind the one you are on.
//!
//! So the bind group is one uniform, one `texture_2d_array`, one sampler, and
//! the model says which layers it wants in the uniform. `Fullscreen`'s header
//! argues that five hand-rolled copies of a pipeline is where one of them
//! quietly differs; this is a sixth shape rather than a sixth copy, and the
//! vertex half is `fullscreen::VERTEX` unchanged.
//!
//! # The trail is a lookup, not a memory, and that is the whole design
//!
//! `shared::trail` — the one effect in this project that remembers anything —
//! needs two textures, a ping-pong, a `dt`, a decay per second and a `SEEK`
//! threshold, all so that asking for the same instant twice does not advance
//! it. It needs all of that because a generated model's past frames **do not
//! exist any more**.
//!
//! A transmission's do. Every frame that can be shown is a rung of a ladder at
//! a known address, so the trail at time `t` is the effect's own position
//! function evaluated at `t - Δ`, `t - 2Δ`, … — arithmetic, not history.
//! Wake therefore stays a pure function of `(floats, time)` with no seek
//! threshold and no state, which is the property the render gates and the
//! automix both rest on. Sandstorm made the same move against a particle buffer
//! and its header calls it inverting the fall.
//!
//! [`Frames`] is a **cache**, not state: it changes what is resident on the GPU
//! and never what is drawn. Two requests for one instant give one image whether
//! the second one hit or missed.
//!
//! # What it costs
//!
//! A miss is a JPEG decode plus an upload — 2 to 4 ms at the default proxy
//! width. A frame asks for at most [`MAX_TAPS`] rungs, so a cold cue costs
//! about 25 ms once. It is nearly free afterwards and *especially* free while
//! the stutter is holding, because a held frame is the same rung several taps
//! over. The residency is [`LAYERS`] rungs, which is about 15 MB.


use wgpu::util::DeviceExt;

use crate::model::{Built, Ctx};

/// How many source frames one draw may read.
///
/// Eight is what a trail needs to read as a trail rather than as a double
/// exposure, and it is the width of the tap array in the shader — the two
/// numbers must agree and this is the one they both come from.
pub const MAX_TAPS: usize = 8;

/// How many rungs stay resident on the GPU.
///
/// Twice [`MAX_TAPS`] **per folder in play**. The doubling is so the taps of the
/// frame being drawn never evict the taps of the one before it, which would
/// make every frame a full set of misses and turn the cache into a fixed cost
/// instead of a saving. The four is because a library is per source now: two
/// sources on two folders are two full tap sets alive at once, and at sixteen
/// they evict each other every single frame — the same thrash the doubling
/// exists to prevent, one level up.
///
/// Four folders at the default proxy width is about 60 MB. A venue running more
/// than four folders at once gets misses rather than wrong frames: the key
/// carries the library, so an evicted rung is re-decoded, never confused with
/// another folder's.
pub const LAYERS: u32 = 64;

/// The format rungs are uploaded in. sRGB, because the ladder is JPEGs and the
/// rest of this crate's targets are sRGB: sampling decodes and writing encodes,
/// so the shader works in linear light without a single conversion by hand.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// One tap, as the shader reads it: which layer, how much of it, and where.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Tap {
    /// Layer index in the array, as a float because a uniform's scalars are
    /// four bytes either way and `i32` mixed into a `vec4<f32>` is a padding
    /// problem for no gain.
    pub layer: f32,
    /// 0 switches the tap off. The shader always loops [`MAX_TAPS`] times —
    /// a dynamic bound would be uniform control flow the compiler has to prove,
    /// and a zero weight is one multiply.
    pub weight: f32,
    /// How far along the trail this tap is, 0..1. Walks the ghost's colour
    /// slot, so the wake changes colour as it fades.
    pub age: f32,
    /// The source time this tap reads. Only the slate uses it — with no
    /// library there is no layer to sample, and a test card that ignored the
    /// clock would show neither the stutter nor the trail.
    pub at: f32,
}

/// The GPU-resident rungs, and which is which.
///
/// Built without a device on purpose: a model is constructed by
/// `registry::all_models()`, which runs before any GPU exists on the GPU-free
/// build and would otherwise have to become fallible. Everything that needs a
/// device is behind `Built`, exactly as a model's pipeline is.
#[derive(Default)]
pub struct Frames {
    array: Option<Resident>,
    /// Bumped on every lookup; a layer's own stamp is when it was last wanted.
    clock: u64,
    /// Each clip's open HAP proxy, or `None` once it is known to have none.
    haps: std::collections::HashMap<(String, String), Option<metal_reel::hap::Hap>>,
}

struct Resident {
    view: wgpu::TextureView,
    texture: wgpu::Texture,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    /// Per layer: which library, which clip, which rung, and when it was last
    /// asked for.
    ///
    /// The library is in the key because two folders can hold the same
    /// filename, and `index::slug` turns both into the same clip id — so
    /// without it, pointing a second source at a second folder draws the first
    /// folder's footage, correctly and silently.
    holds: Vec<Option<(String, String, usize, u64)>>,
}

impl Frames {
    /// Whether anything is resident, and at what size. The model needs the size
    /// to letterbox the footage into a frame of another shape.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.array.as_ref().map(|r| (r.width, r.height))
    }

    /// Makes sure rung `k` of `clip` is on the GPU and says which layer it is
    /// in. `None` means the rung could not be read — a missing file, or a
    /// library whose ladder was deleted underneath it.
    ///
    /// A rung of a different size than what is resident **rebuilds the array**
    /// rather than being scaled into it. Clips of two aspect ratios in one
    /// folder is normal, and stretching one into the other's frame is a
    /// distortion nothing downstream could detect or undo. A cue may only
    /// change every `HOLD_SECONDS`, so this is at worst one reallocation every
    /// twenty-four seconds.
    pub fn resident(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        lib: &metal_reel::index::Library,
        clip: &metal_reel::index::Clip,
        k: usize,
    ) -> Option<u32> {
        let (library, id) = (lib.name.as_str(), clip.id.as_str());
        self.clock += 1;

        if let Some(r) = &mut self.array {
            if let Some(at) = r.holds.iter().position(|h| {
                matches!(h, Some((l, c, rung, _)) if l == library && c == id && *rung == k)
            }) {
                if let Some(Some(slot)) = r.holds.get_mut(at) {
                    slot.3 = self.clock;
                }
                return Some(at as u32);
            }
        }

        // 2026-10-01: the clip's HAP proxy when it has one — its blocks go to
        // the GPU as they are on a device that samples BC, and are decoded on
        // the CPU on one that does not. The JPEG rung is the fallback.
        let hap = self
            .haps
            .entry((library.to_string(), id.to_string()))
            .or_insert_with(|| lib.hap(clip))
            .as_mut()
            .and_then(|h| h.frame(k).ok());
        let (w, h, format, bytes, bytes_per_row, rows) = match hap {
            Some(f) => {
                let bc = device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
                let blocks = (f.width.div_ceil(4), f.height.div_ceil(4));
                match f.texture {
                    metal_reel::hap::Texture::Bc1 if bc => {
                        (f.width, f.height, wgpu::TextureFormat::Bc1RgbaUnormSrgb, f.blocks, blocks.0 * 8, blocks.1)
                    }
                    metal_reel::hap::Texture::Bc3 if bc => {
                        (f.width, f.height, wgpu::TextureFormat::Bc3RgbaUnormSrgb, f.blocks, blocks.0 * 16, blocks.1)
                    }
                    _ => (f.width, f.height, FORMAT, f.rgba(), f.width * 4, f.height),
                }
            }
            None => {
                let frame = metal_reel::index::decode(&lib.rung_path(clip, k)).ok()?;
                // RGB on disk, RGBA on the GPU. The alpha is written rather
                // than left undefined because the post chain blends against it.
                let mut rgba = Vec::with_capacity((frame.width * frame.height * 4) as usize);
                for px in frame.px.chunks_exact(3) {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                (frame.width, frame.height, FORMAT, rgba, frame.width * 4, frame.height)
            }
        };
        if w == 0 || h == 0 {
            return None;
        }

        let stale = !matches!(&self.array, Some(r) if r.width == w && r.height == h && r.format == format);
        if stale {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("footage frames"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: LAYERS },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            self.array = Some(Resident {
                view,
                texture,
                width: w,
                height: h,
                format,
                holds: vec![None; LAYERS as usize],
            });
        }
        let r = self.array.as_mut().expect("just built above");

        // The layer nobody has wanted for longest — or an empty one first,
        // which is what makes a cold cue fill the array rather than thrash one
        // slot.
        let at = r
            .holds
            .iter()
            .position(Option::is_none)
            .unwrap_or_else(|| {
                r.holds
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, h)| h.as_ref().map(|(_, _, _, t)| *t).unwrap_or(0))
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &r.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: at as u32 },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        r.holds[at] = Some((library.to_string(), id.to_string(), k, self.clock));
        Some(at as u32)
    }

    /// The format resident rungs are in — `Rgba8UnormSrgb`, or BC1/BC3 when
    /// a HAP proxy went up compressed.
    pub fn format(&self) -> Option<wgpu::TextureFormat> {
        self.array.as_ref().map(|r| r.format)
    }

    /// Throws every rung away. Called when a library is detached, so pointing
    /// the app at another folder cannot show a frame of the last one.
    pub fn clear(&mut self) {
        self.array = None;
        self.haps.clear();
    }

    pub fn allocated(&self) -> usize {
        self.array
            .as_ref()
            .map(|r| r.holds.iter().filter(|h| h.is_some()).count())
            .unwrap_or(0)
    }
}

/// The WGSL every transmission gets, concatenated ahead of its own.
///
/// Four things, and each of them is a decision two transmissions must not make
/// differently: how a frame of one shape is fitted into a frame of another, what
/// is drawn when there is no footage, what luminance means, and a hash two GPUs
/// agree on. `shared::PRELUDE` is the same idea for the whole suite; this is the
/// footage-shaped half of it, kept out of that one because thirty models that
/// generate a picture have no use for a slate.
///
/// It arrived when the second transmission was written and would otherwise have
/// copied all four out of `estela.wgsl` — the copy that `fullscreen`'s header
/// calls the place where one of them quietly differs.
pub const PRELUDE: &str = r#"
// An integer hash, for the same reason `estela::draw` is one on the CPU: two
// screens of one venue must break apart identically, and they may not be the
// same GPU. `fract(sin(x))` is a different number depending on whose `sin` it is.
fn hash(cell: vec2<i32>, salt: u32) -> f32 {
    var h = u32(cell.x + 4096) * 374761393u
        + u32(cell.y + 4096) * 668265263u
        + salt * 2246822519u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    h = h ^ (h >> 16u);
    return f32(h & 0xFFFFFFu) / 16777216.0;
}

fn luma709(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Frame uv to source uv, **covering** rather than fitting.
//
// A venue projects onto a surface with no bars on it. Fitting would clear the
// overflow to black and put two hard edges in the middle of a wall; covering
// crops instead, which is what any player does with footage of the wrong shape.
fn cover(uv: vec2<f32>, frame_aspect: f32, source_aspect: f32) -> vec2<f32> {
    var s = vec2<f32>(1.0, 1.0);
    if (source_aspect > frame_aspect) {
        s.x = frame_aspect / max(source_aspect, 1e-4);
    } else {
        s.y = max(source_aspect, 1e-4) / max(frame_aspect, 1e-4);
    }
    return (uv - vec2<f32>(0.5)) * s + vec2<f32>(0.5);
}

// What a transmission draws when no library is attached.
//
// Deliberately a test card and not a plausible picture: an operator who cannot
// tell "no footage loaded" from "this is the footage" will eventually take an
// empty library to a venue, which is the same argument `digest::Simulated`
// makes on its own face. It is a function of the clock, so an effect is visible
// with nothing loaded — which is what makes one tunable before anybody has
// indexed a folder. Seven saturated bars, because a model that distorts colour
// has to be adjustable against something that *has* colour.
fn slate(uv: vec2<f32>, aspect: f32, t: f32) -> vec3<f32> {
    let bar = floor(clamp(uv.x, 0.0, 0.999) * 7.0);
    var c = vec3<f32>(
        step(0.5, fract(bar * 0.5)),
        step(0.5, fract(bar * 0.25)),
        step(0.5, fract(bar * 0.125)),
    ) * 0.62;
    // Something lit that moves, so a frozen block visibly disagrees with a live
    // one. A static card would mosh into an identical card.
    let p = (uv - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0);
    let centre = vec2<f32>(sin(t * 1.1) * 0.34 * aspect, cos(t * 0.73) * 0.24);
    c += vec3<f32>(1.0, 0.97, 0.9) * smoothstep(0.13, 0.03, length(p - centre));
    return c;
}
"#;

/// A fullscreen pass that reads a uniform and the resident rungs.
pub struct Pass {
    label: &'static str,
    wgsl: &'static str,
    state: Built<(wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    /// The sampler, and a 1x1 array bound when no footage is resident.
    ///
    /// **The blank is not an edge case to tidy away.** A bind group must be
    /// complete or the draw does not happen, and "no library attached" is the
    /// state a fresh install is in — so the alternative to this is the one
    /// model in the suite that renders nothing until somebody configures it.
    /// With this, the model draws its own slate instead and the page says why.
    blank: Built<(wgpu::Texture, wgpu::TextureView, wgpu::Sampler)>,
}

impl Pass {
    pub fn new(label: &'static str, wgsl: &'static str) -> Pass {
        Pass { label, wgsl, state: Built::default(), blank: Built::default() }
    }

    pub fn draw<U: bytemuck::Pod>(
        &mut self,
        ctx: &Ctx,
        frames: &Frames,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        uniform: &U,
    ) {
        let (label, wgsl) = (self.label, self.wgsl);
        let (pipeline, layout) = self.state.get(format, || build(ctx.device, format, label, wgsl));
        let blank = self.blank.get(format, || blank_array(ctx.device, ctx.queue));
        let view = frames.array.as_ref().map(|r| &r.view).unwrap_or(&blank.1);
        let sampler = &blank.2;

        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::bytes_of(uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

fn blank_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("footage blank"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: LAYERS },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for layer in 0..LAYERS {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                aspect: wgpu::TextureAspect::All,
            },
            &[0u8, 0, 0, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("footage"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (texture, view, sampler)
}

fn build(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &'static str,
    wgsl: &str,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = crate::shared::build_shader(
        device,
        label,
        &format!("{}\n{PRELUDE}\n{wgsl}", super::fullscreen::VERTEX),
    );

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
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

    (pipeline, bind_group_layout)
}
