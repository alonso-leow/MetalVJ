//! `fx.brush` -- the frame repainted from strokes, after fogleman/primitive
//! (behaviour only; nothing is ported).
//!
//! # Why this is not an ordinary chain step
//!
//! The recipe (`the effect notes`) says a chain step is a pure function
//! of the frame: "no previous-frame texture exists in the post pass". A brush
//! painting is the opposite, it is the sum of every stroke laid down so far.
//! **This effect breaks that limit on purpose**, the way ghosting does
//! (`shared::trail`): it owns a persistent canvas texture per model, outside the
//! post pass, and the post pass never sees it as anything but "the frame".
//!
//! * **Placement.** `fx.brush` is a chain step in the schema (so the graph, the
//!   inspector and the AI treat it like any other, and `cant` is its amount),
//!   but it is applied *before* the post pass, between the model and ghost:
//!   whatever its place in the chain, it is first, because it redraws what
//!   every other step then receives. The `case` for its kind in `post.wgsl` is
//!   a deliberate no-op.
//! * **Off means off.** At `cant == 0` the step is not in the resolved chain, no
//!   canvas is allocated, no pass runs and the frame is bit-identical to a
//!   renderer that never heard of brush. Turning it off throws the canvas away
//!   ([`Brushes::clear`]), like ghost's trail.
//! * **Per model, per size.** Like [`Trails`](super::trail::Trails): one scratch
//!   texture is shared by every tile, so a canvas keyed any other way would
//!   paint one tile's strokes over the next one's. A size change reallocates.
//! * **Determinism.** The same `time` twice advances nothing (a paused preview
//!   must not keep painting); a jump over a second or backwards is a seek and
//!   repaints from scratch. Every random number is a hash of the stroke number
//!   and the frame counter, so a cold renderer replaying the same instants
//!   draws the same pixels.
//!
//! # State
//!
//! `canvas` (Rgba16Float, linear, the frame's size, so slow fades do not stall
//! in 8-bit quantisation) holds the painting; `out` is where the result is
//! mixed with the live frame by `cant` when `cant < 1` (otherwise the canvas is
//! returned as is). Strokes are *blended onto* the canvas (load, never clear),
//! so one texture is enough; the renew/decay fade (`renueva`/`decae`) is also a blend (a
//! fullscreen quad drawn with alpha = the fade fraction), which is why no
//! ping-pong pair is needed. Search mode adds a 64x64 proxy pair (the frame, and
//! a mirror of the canvas) plus three small storage buffers.
//!
//! # Dab mode (default, cheap)
//!
//! The frame's strokes are a grid of `n_trazos` cells visited in a scrambled
//! order (a permutation step coprime to the cell count). Every frame draws
//! `cobertura` of them as one instanced draw: the vertex shader derives each
//! stroke from its number (position with `aleatoriedad` jitter, size from its
//! *rank in the frame*: `tam_ini` for the first, `tam_fin` for the last, so a
//! frame is coarse strokes first and detail after, as in primitive's
//! progression), samples the frame at its centre (five taps) for the colour and
//! four more for the luminance gradient it turns along (`sigue`), and the
//! fragment shader fills an ellipse / rect / triangle / bezier SDF. No CPU
//! buffer, no readback. The first frame after a reset draws the whole grid
//! (a finished painting), later ones only `cobertura` of it.
//!
//! # Search mode (opt-in, needs compute)
//!
//! As primitive's `model.go`/`core.go`: per stroke, `candidatos` random strokes
//! are scored by the error they would remove between the frame and the canvas
//! (a compute workgroup per candidate reduces the 64x64 proxy: first the best
//! colour, `computeColor`, then the error change); the best is refined once
//! (candidate 0 is the winner itself, the rest are mutations of it, primitive's
//! hill-climb with one step) and drawn on the proxy so the next round sees it.
//! Rounds per frame come from `cobertura` (1..24, 96 on a reset; about 0.3 ms each on a loaded desktop GPU); the chosen
//! strokes then draw at full resolution in one instanced draw. Full-resolution
//! scoring is not real time, which is why the proxy exists. Without compute
//! (WebGL2) the mode falls back to dab.
//!
//! # Not covered
//!
//! `app.rs` (the window path) keeps no per-model history of any kind, so there
//! the step is a no-op, like ghost. `fx.estela`'s ring reads the model's raw
//! frame, not the painting.

use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

use crate::gpu::offscreen_target;

/// A gap longer than this (seconds) is a seek: repaint from scratch.
const SEEK: f32 = 1.0;
const STATE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The search proxy is `PROXY` x `PROXY` (the shader hard-codes 64).
pub const PROXY: u32 = 64;
/// The most strokes search lays down in one frame.
const MAX_ROUNDS: u32 = 128;
const MAX_CANDS: u32 = 32;
/// Uniform entries sit this far apart (dynamic offsets need 256).
const STRIDE: usize = 256;

/// The `fx_brush_*` floats as the shader wants them (angles in radians).
#[derive(Clone, Debug, PartialEq)]
pub struct BrushParams {
    pub cant: f32,
    pub shape: u32,
    pub n_trazos: u32,
    pub tam_ini: f32,
    pub tam_fin: f32,
    pub alfa: f32,
    pub aleatoriedad: f32,
    pub search: bool,
    pub candidatos: u32,
    pub cobertura: f32,
    pub renueva: f32,
    pub outline: f32,
    pub paper: f32,
    pub papel_tono: f32,
    pub angle: f32,
    pub sigue: f32,
    pub alargado: f32,
    pub color_jitter: f32,
    pub saturation: f32,
    pub decae: f32,
}

impl BrushParams {
    /// Reads the group out of `Post.advanced`: defaults and clamps applied, a
    /// short or missing slice reads as the defaults (so `cant == 0`).
    pub fn from_advanced(advanced: &[f32]) -> BrushParams {
        let f = super::post::group_floats(advanced, "brush");
        let at = |i: usize| f.get(i).copied().unwrap_or(0.0);
        BrushParams {
            cant: at(0),
            shape: at(1).round().max(0.0) as u32,
            n_trazos: at(2).round().max(1.0) as u32,
            tam_ini: at(3),
            tam_fin: at(4),
            alfa: at(5),
            aleatoriedad: at(6),
            search: at(7) >= 0.5,
            candidatos: (at(8).round() as u32).clamp(2, MAX_CANDS),
            cobertura: at(9),
            renueva: at(10),
            outline: at(11),
            paper: at(12),
            papel_tono: at(13),
            angle: at(14).to_radians(),
            sigue: at(15),
            alargado: at(16),
            color_jitter: at(17),
            saturation: at(18),
            decae: at(19),
        }
    }
}

/// The uniform block, the twin of `U` in `brush_common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uni {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
    e: [u32; 4],
    f: [f32; 4],
    g: [u32; 4],
    h: [f32; 4],
}

/// One model's painting.
struct Canvas {
    size: (u32, u32),
    canvas: (wgpu::Texture, wgpu::TextureView),
    out: (wgpu::Texture, wgpu::TextureView),
    proxy_tgt: (wgpu::Texture, wgpu::TextureView),
    proxy_cvs: (wgpu::Texture, wgpu::TextureView),
    /// The stroke counter: dab `k` is the k-th stroke ever laid on this canvas.
    cursor: u32,
    /// Frames drawn, seeds the search candidates.
    frame: u32,
    last_time: Option<f32>,
    /// The canvas was last painted in search mode.
    searching: bool,
    search: Option<SearchBufs>,
}

struct SearchBufs {
    scores: wgpu::Buffer,
    cols: wgpu::Buffer,
    chosen: wgpu::Buffer,
    chosen_bg: wgpu::BindGroup,
}

struct Pipes {
    l0: wgpu::BindGroupLayout,
    lc: wgpu::BindGroupLayout,
    lch: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    dab: wgpu::RenderPipeline,
    chosen: wgpu::RenderPipeline,
    fade: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    comp: wgpu::RenderPipeline,
    score: Option<wgpu::ComputePipeline>,
    best: Option<wgpu::ComputePipeline>,
}

/// Every model's canvas, allocated on demand (see the module doc).
#[derive(Default)]
pub struct Brushes {
    per_model: Vec<Option<Canvas>>,
    pipes: Option<Pipes>,
}

fn module(device: &wgpu::Device, label: &str, body: &str) -> wgpu::ShaderModule {
    let src = format!("{}\n{}", include_str!("../../shaders/brush_common.wgsl"), body);
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    })
}

fn uniform_entry(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(std::mem::size_of::<Uni>() as u64),
        },
        count: None,
    }
}

fn tex_entry(binding: u32, vis: wgpu::ShaderStages, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_entry(binding: u32, vis: wgpu::ShaderStages, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Pipes {
    fn new(device: &wgpu::Device, compute: bool) -> Pipes {
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let l0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush l0"),
            entries: &[
                uniform_entry(0, vf),
                tex_entry(1, vf, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: vf,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(3, vf, true),
            ],
        });
        let lch = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush chosen"),
            entries: &[storage_entry(0, wgpu::ShaderStages::VERTEX, true)],
        });
        let c = wgpu::ShaderStages::COMPUTE;
        let lc = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush compute"),
            entries: &[
                uniform_entry(0, c),
                tex_entry(1, c, false),
                tex_entry(2, c, false),
                storage_entry(3, c, false),
                storage_entry(4, c, false),
                storage_entry(5, c, false),
            ],
        });
        let draw = module(device, "brush", include_str!("../../shaders/brush.wgsl"));
        let layout0 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush layout"),
            bind_group_layouts: &[Some(&l0)],
            immediate_size: 0,
        });
        let layout1 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush chosen layout"),
            bind_group_layouts: &[Some(&l0), Some(&lch)],
            immediate_size: 0,
        });
        let render = |label: &str, layout: &wgpu::PipelineLayout, vs: &str, fs: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &draw,
                    entry_point: Some(vs),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &draw,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: STATE_FORMAT,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let dab = render("brush dab", &layout0, "vs_dab", "fs_stroke");
        let chosen = render("brush chosen", &layout1, "vs_chosen", "fs_stroke");
        let fade = render("brush fade", &layout0, "vs_full", "fs_fade");
        let down = render("brush down", &layout0, "vs_full", "fs_down");
        let comp = render("brush comp", &layout0, "vs_full", "fs_comp");

        let (mut score, mut best) = (None, None);
        if compute {
            let m = module(device, "brush search", include_str!("../../shaders/brush_search.wgsl"));
            let cl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brush compute layout"),
                bind_group_layouts: &[Some(&lc)],
                immediate_size: 0,
            });
            let mk = |label: &str, entry: &str| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&cl),
                    module: &m,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
            };
            score = Some(mk("brush score", "cs_score"));
            best = Some(mk("brush best", "cs_best"));
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("brush sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Pipes { l0, lc, lch, sampler, dab, chosen, fade, down, comp, score, best }
    }
}

/// What this call has to do to the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// The same instant again: change nothing (only re-mix by `cant`).
    Hold,
    /// First frame, a seek or a new size: repaint from scratch.
    Fresh,
    /// A normal frame `dt` seconds after the last.
    Run(f32),
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A step through `cells` cells that visits each exactly once per sweep.
fn permutation_step(cells: u32) -> u32 {
    let mut p = ((cells as f32 * 0.618) as u32).max(1);
    while gcd(p, cells) != 1 {
        p += 1;
    }
    p
}

/// The stroke grid for `n` strokes on a frame of this aspect: columns, rows.
fn grid(n: u32, aspect: f32) -> (u32, u32) {
    let gx = ((n as f32 * aspect).sqrt().round() as u32).max(1);
    let gy = n.div_ceil(gx).max(1);
    (gx, gy)
}

impl Canvas {
    fn new(device: &wgpu::Device, size: (u32, u32)) -> Canvas {
        let t = |w, h| offscreen_target(device, w, h, STATE_FORMAT);
        Canvas {
            size,
            canvas: t(size.0, size.1),
            out: t(size.0, size.1),
            proxy_tgt: t(PROXY, PROXY),
            proxy_cvs: t(PROXY, PROXY),
            cursor: 0,
            frame: 0,
            last_time: None,
            searching: false,
            search: None,
        }
    }
}

impl SearchBufs {
    fn new(device: &wgpu::Device, pipes: &Pipes) -> SearchBufs {
        let buf = |label: &str, bytes: u64, extra: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | extra,
                mapped_at_creation: false,
            })
        };
        let scores = buf("brush scores", MAX_CANDS as u64 * 16, wgpu::BufferUsages::empty());
        let cols = buf("brush colours", MAX_CANDS as u64 * 16, wgpu::BufferUsages::empty());
        let chosen = buf("brush chosen", MAX_ROUNDS as u64 * 48, wgpu::BufferUsages::empty());
        let chosen_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush chosen bg"),
            layout: &pipes.lch,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: chosen.as_entire_binding() }],
        });
        SearchBufs { scores, cols, chosen, chosen_bg }
    }
}

/// One search round: score and pick random candidates, then score and pick
/// mutations of the winner. Uniform entries 2 + 2r and 3 + 2r.
fn cbg_pass(
    cp: &mut wgpu::ComputePass<'_>,
    score: &wgpu::ComputePipeline,
    best: &wgpu::ComputePipeline,
    bg: &wgpu::BindGroup,
    round: u32,
    candidates: u32,
) {
    for stage in 0..2u32 {
        let off = ((2 + round * 2 + stage) as usize * STRIDE) as u32;
        cp.set_bind_group(0, bg, &[off]);
        cp.set_pipeline(score);
        cp.dispatch_workgroups(candidates, 1, 1);
        cp.set_pipeline(best);
        cp.dispatch_workgroups(1, 1, 1);
    }
}

/// One render pass over `view`: `clear` or load, `pipeline`, group 0 at
/// uniform entry `entry`, optional group 1, `verts` x `instances`.
#[allow(clippy::too_many_arguments)]
fn render_pass(
    enc: &mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    clear: bool,
    pipeline: &wgpu::RenderPipeline,
    bg: &wgpu::BindGroup,
    entry: usize,
    bg1: Option<&wgpu::BindGroup>,
    verts: u32,
    instances: std::ops::Range<u32>,
) {
    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: if clear { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bg, &[(entry * STRIDE) as u32]);
    if let Some(b) = bg1 {
        pass.set_bind_group(1, b, &[]);
    }
    pass.draw(0..verts, instances);
}

impl Brushes {
    /// Advances model `index`'s painting with the live frame in `source` and
    /// returns the view the post pass should read instead of it. `None` means
    /// "read `source` yourself" (a degenerate size). `p.cant` is the step's
    /// amount.
    #[allow(clippy::too_many_arguments)]
    pub fn advance<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        compute: bool,
        index: usize,
        source: &wgpu::TextureView,
        size: (u32, u32),
        time: f32,
        p: &BrushParams,
    ) -> Option<&'a wgpu::TextureView> {
        let (w, h) = size;
        if w == 0 || h == 0 {
            return None;
        }
        let pipes = self.pipes.get_or_insert_with(|| Pipes::new(device, compute));
        if self.per_model.len() <= index {
            self.per_model.resize_with(index + 1, || None);
        }
        let new_size = !matches!(&self.per_model[index], Some(c) if c.size == size);
        if new_size {
            self.per_model[index] = Some(Canvas::new(device, size));
        }
        let cv = self.per_model[index].as_mut().expect("just built above");

        let search = p.search && compute && pipes.score.is_some();
        let mut step = match cv.last_time.map(|l| time - l) {
            Some(d) if d == 0.0 => Step::Hold,
            None => Step::Fresh,
            Some(d) if !(0.0..=SEEK).contains(&d) => Step::Fresh,
            Some(d) => Step::Run(d.min(0.25)),
        };
        // Switching mode repaints: search starts from blank paper and its proxy
        // must mirror the canvas, which a dab painting is not.
        if search != cv.searching && step != Step::Hold {
            step = Step::Fresh;
        }
        if step == Step::Fresh {
            // A repaint is the same picture whenever it happens.
            cv.cursor = 0;
            cv.frame = 0;
        }
        let fresh = step == Step::Fresh;
        let aspect = w as f32 / h as f32;
        let (gx, gy) = grid(p.n_trazos, aspect);
        let cells = gx * gy;

        // Painting counts for this frame.
        let (fade_k, fade_share) = match step {
            Step::Hold => (0.0, 0.0),
            // Search starts from blank paper, as primitive starts from a flat
            // background: from the frame itself it would have nothing to fix.
            Step::Fresh => (1.0, if search { 1.0 } else { p.paper }),
            Step::Run(dt) => {
                let rate = (p.renueva + p.decae) * 8.0;
                if rate <= 0.0 {
                    (0.0, 0.0)
                } else {
                    (1.0 - (-dt * rate).exp(), p.decae * 8.0 / rate)
                }
            }
        };
        let dabs = match step {
            Step::Hold => 0,
            Step::Fresh => cells,
            Step::Run(_) => ((p.cobertura * cells as f32).ceil() as u32).clamp(1, cells),
        };
        let rounds = match step {
            Step::Hold => 0,
            Step::Fresh => 96,
            Step::Run(_) => ((p.cobertura * 24.0).ceil() as u32).clamp(1, 24),
        }
        .min(MAX_ROUNDS);

        let base = Uni {
            a: [aspect, p.alfa, p.aleatoriedad, p.outline],
            b: [p.tam_ini, p.tam_fin, p.alargado, p.sigue],
            c: [p.angle, p.color_jitter, p.saturation, p.paper],
            d: [p.papel_tono, gx as f32, gy as f32, 1.0 / gy as f32],
            e: [p.shape, cv.cursor, dabs, cells],
            f: [(cv.frame % 100_000) as f32, fade_k, fade_share, p.cant.clamp(0.0, 1.0)],
            g: [0, 0, p.candidatos, permutation_step(cells)],
            h: [0.0, rounds as f32, 0.0, 0.0],
        };
        // Entry 0 full resolution, 1 the proxy, then (round, stage) pairs.
        // A mirror that is stale (search just switched on) is repainted like a
        // fresh canvas; otherwise it fades the way the canvas does.
        let proxy_fresh = search && fresh;
        let (pk, ps) = if proxy_fresh { (1.0, 1.0) } else { (fade_k, fade_share) };
        let mut entries = vec![
            base,
            Uni { f: [base.f[0], pk, ps, base.f[3]], h: [1.0, rounds as f32, 0.0, 0.0], ..base },
        ];
        if search && rounds > 0 {
            for r in 0..rounds {
                for stage in 0..2u32 {
                    entries.push(Uni {
                        g: [r, stage, p.candidatos, base.g[3]],
                        h: [1.0, rounds as f32, 0.0, 0.0],
                        ..base
                    });
                }
            }
        }
        let mut bytes = vec![0u8; entries.len() * STRIDE];
        for (i, e) in entries.iter().enumerate() {
            bytes[i * STRIDE..i * STRIDE + std::mem::size_of::<Uni>()].copy_from_slice(bytemuck::bytes_of(e));
        }
        let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("brush uniforms"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let ubind = wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &ubuf,
            offset: 0,
            size: NonZeroU64::new(std::mem::size_of::<Uni>() as u64),
        });
        let group = |label: &str, t0: &wgpu::TextureView, t1: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &pipes.l0,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ubind.clone() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(t0) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&pipes.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(t1) },
                ],
            })
        };
        let bg_src = group("brush src", source, source);
        let bg_comp = group("brush comp", source, &cv.canvas.1);
        let bg_proxy = group("brush proxy", &cv.proxy_tgt.1, &cv.proxy_tgt.1);

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("brush") });

        if search && step != Step::Hold {
            if cv.search.is_none() {
                cv.search = Some(SearchBufs::new(device, pipes));
            }
            let sb = cv.search.as_ref().expect("just built");
            // The frame at proxy resolution, then the canvas's mirror brought
            // up to date the same way the canvas is.
            render_pass(&mut enc, "brush down", &cv.proxy_tgt.1, true, &pipes.down, &bg_src, 1, None, 3, 0..1);
            render_pass(
                &mut enc, "brush proxy fade", &cv.proxy_cvs.1, proxy_fresh, &pipes.fade, &bg_proxy, 1, None, 3, 0..1,
            );
            // Rounds.
            let cbg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("brush compute bg"),
                layout: &pipes.lc,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ubind.clone() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&cv.proxy_tgt.1) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&cv.proxy_cvs.1) },
                    wgpu::BindGroupEntry { binding: 3, resource: sb.scores.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: sb.cols.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: sb.chosen.as_entire_binding() },
                ],
            });
            let (score, best) = (pipes.score.as_ref().expect("compute"), pipes.best.as_ref().expect("compute"));
            for r in 0..rounds {
                // Both stages of a round in one compute pass: dispatches in a
                // pass are ordered, so the second reads the first's winner.
                let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("brush search"),
                    timestamp_writes: None,
                });
                cbg_pass(&mut cp, score, best, &cbg, r, p.candidatos);
                drop(cp);
                render_pass(
                    &mut enc, "brush proxy stroke", &cv.proxy_cvs.1, false, &pipes.chosen, &bg_proxy, 1,
                    Some(&sb.chosen_bg), 6, r..r + 1,
                );
            }
        }

        // The canvas itself.
        if fade_k > 0.0 {
            render_pass(&mut enc, "brush fade", &cv.canvas.1, fresh, &pipes.fade, &bg_src, 0, None, 3, 0..1);
        }
        if search && step != Step::Hold {
            let sb = cv.search.as_ref().expect("built above");
            render_pass(
                &mut enc, "brush strokes", &cv.canvas.1, false, &pipes.chosen, &bg_src, 0, Some(&sb.chosen_bg), 6,
                0..rounds,
            );
        } else if dabs > 0 {
            render_pass(&mut enc, "brush dabs", &cv.canvas.1, false, &pipes.dab, &bg_src, 0, None, 6, 0..dabs);
        }
        let mixed = p.cant < 0.999;
        if mixed {
            render_pass(&mut enc, "brush mix", &cv.out.1, true, &pipes.comp, &bg_comp, 0, None, 3, 0..1);
        }
        queue.submit(Some(enc.finish()));

        if step != Step::Hold {
            cv.searching = search;
            cv.cursor = cv.cursor.wrapping_add(dabs);
            cv.frame = cv.frame.wrapping_add(1);
            cv.last_time = Some(time);
        }
        Some(if mixed { &cv.out.1 } else { &cv.canvas.1 })
    }

    /// Throws every canvas away (the effect was turned off), so turning it back
    /// on starts a new painting.
    pub fn clear(&mut self) {
        self.per_model.clear();
    }

    /// How many models hold a canvas right now.
    pub fn allocated(&self) -> usize {
        self.per_model.iter().filter(|c| c.is_some()).count()
    }
}
