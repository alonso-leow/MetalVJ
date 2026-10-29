//! The headless gate every model has to pass without opening a window: draw
//! one frame at its schema defaults, and check it is not black, has no NaN
//! or Inf, and renders byte-identical again given the same floats and time.
//! This is the only way to know all ten renderers are still alive without
//! opening the app ten times by hand — `docs/SUITE.md` verification §1.

use metal_render::gpu::{offscreen_target, Gpu};
use metal_render::model::{Ctx, Model};
use metal_render::registry::all_models;
use metal_render::shared::post::{ChainFx, EstelaParams, GradeUniform, PostChain, PostParams};
use metal_render::shared::trail::fallback_ring;
use metal_render::shared::ColorSlotGpu;
use metal_schema::{EffectStep, CHAIN_KIND_ESTELA};

const W: u32 = 64;
const H: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

fn render_and_readback(model: &mut dyn Model, gpu: &Gpu, floats: &[f32], time: f32) -> Vec<f32> {
    render_at(model, gpu, floats, time, W, H)
}

fn render_at(
    model: &mut dyn Model,
    gpu: &Gpu,
    floats: &[f32],
    time: f32,
    w: u32,
    h: u32,
) -> Vec<f32> {
    let (texture, view) = offscreen_target(&gpu.device, w, h, FORMAT);
    let ctx = Ctx {
        device: &gpu.device,
        queue: &gpu.queue,
        time,
        width: w,
        height: h,
        fondo_alpha: false, detail: 1.0,
    };
    model.render(&ctx, floats, &view, FORMAT);
    readback_at(gpu, &texture, w, h)
}

fn readback(gpu: &Gpu, texture: &wgpu::Texture) -> Vec<f32> {
    readback_at(gpu, texture, W, H)
}

fn readback_at(gpu: &Gpu, texture: &wgpu::Texture, w: u32, h: u32) -> Vec<f32> {
    let bytes_per_pixel = 8u32; // Rgba16Float
    let unpadded = w * bytes_per_pixel;
    let bytes_per_row = unpadded.next_multiple_of(256);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (bytes_per_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback encoder") });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    gpu.queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).expect("readback channel closed");
    });
    gpu.device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
        .expect("device poll failed");
    rx.recv().expect("readback never completed").expect("buffer map failed");

    let data = slice.get_mapped_range().expect("get_mapped_range failed");
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let row_start = (row * bytes_per_row) as usize;
        let row_bytes = &data[row_start..row_start + (w * bytes_per_pixel) as usize];
        for px in row_bytes.chunks_exact(2) {
            out.push(half::f16::from_le_bytes([px[0], px[1]]).to_f32());
        }
    }
    drop(data);
    buffer.unmap();
    out
}

#[test]
fn every_model_renders_a_live_deterministic_frame() {
    let gpu = Gpu::new_headless();
    let mut failures = Vec::new();

    for mut model in all_models() {
        if model.retired() {
            // A tombstone keeps a table position but draws nothing, so the
            // live-frame gate has nothing to say about it — see `registry.rs`.
            continue;
        }
        let name = model.name();
        let floats = model.schema().defaults();
        assert_eq!(
            floats.len(),
            model.schema().width(),
            "{name}: defaults() length does not match schema width()"
        );

        let a = render_and_readback(model.as_mut(), &gpu, &floats, 1.7);
        let b = render_and_readback(model.as_mut(), &gpu, &floats, 1.7);

        if a.iter().any(|v| v.is_nan() || v.is_infinite()) {
            failures.push(format!("{name}: NaN or Inf in the rendered frame"));
        }
        // RGB channels only — alpha may legitimately sit at a fixed 1.0.
        let rgb_nonzero = a.chunks_exact(4).any(|px| px[0] != 0.0 || px[1] != 0.0 || px[2] != 0.0);
        if !rgb_nonzero {
            failures.push(format!("{name}: rendered an all-black frame at schema defaults"));
        }
        if a != b {
            failures.push(format!("{name}: not deterministic for identical floats and time"));
        }

        // Everything above is square, and a square target hides an entire
        // class of bug: an offset or a noise field that is right in UV is
        // stretched in pixels the moment the output is not 1:1. Both the
        // window and the preview page are wide, so the gate is too.
        let wide = render_at(model.as_mut(), &gpu, &floats, 1.7, 96, 54);
        if wide.iter().any(|v| v.is_nan() || v.is_infinite()) {
            failures.push(format!("{name}: NaN or Inf on a non-square target"));
        }
        if !wide.chunks_exact(4).any(|px| px[0] != 0.0 || px[1] != 0.0 || px[2] != 0.0) {
            failures.push(format!("{name}: all-black frame on a non-square target"));
        }

        println!("ok: {name} ({} floats)", floats.len());
    }

    assert!(failures.is_empty(), "render gate failures:\n{}", failures.join("\n"));
}

/// Turing is the one model that keeps a field between frames, so the promise
/// the others get for free has to be checked directly: time standing still
/// advances nothing, time moving forward changes the picture, and a rewind
/// reseeds rather than integrating backwards into nonsense.
#[test]
fn turing_advances_with_time_and_holds_still_without_it() {
    let gpu = Gpu::new_headless();
    let mut model = metal_render::models::turing::Turing::new();
    let floats = model.schema().defaults();

    let a = render_and_readback(&mut model, &gpu, &floats, 4.0);
    let again = render_and_readback(&mut model, &gpu, &floats, 4.0);
    assert_eq!(a, again, "the same instant twice must not step the reaction");

    let later = render_and_readback(&mut model, &gpu, &floats, 4.5);
    assert_ne!(a, later, "half a second later the pattern must have moved");

    let rewound = render_and_readback(&mut model, &gpu, &floats, 0.0);
    assert!(
        rewound.iter().all(|v| !v.is_nan() && !v.is_infinite()),
        "rewinding must reseed, not integrate backwards"
    );
    assert!(rewound.chunks_exact(4).any(|px| px[0] != 0.0 || px[1] != 0.0 || px[2] != 0.0));
}

/// DLA keeps state the same way Turing does, so it owes the same direct
/// check. The byte-equality is doing double duty here: the sparks are a
/// readout of walker state baked fresh every frame, so a frame that advances
/// nothing must show the *same* sparks, not a cleared ring — and the
/// readback comparison is what proves it.
#[test]
fn dla_advances_with_time_and_holds_still_without_it() {
    let gpu = Gpu::new_headless();
    let mut model = metal_render::models::dla::Dla::new();
    let floats = model.schema().defaults();

    let a = render_and_readback(&mut model, &gpu, &floats, 4.0);
    let again = render_and_readback(&mut model, &gpu, &floats, 4.0);
    assert_eq!(a, again, "the same instant twice must not step the aggregation");

    let later = render_and_readback(&mut model, &gpu, &floats, 4.5);
    assert_ne!(a, later, "half a second later the coral must have grown");

    let rewound = render_and_readback(&mut model, &gpu, &floats, 0.0);
    assert!(
        rewound.iter().all(|v| !v.is_nan() && !v.is_infinite()),
        "rewinding must reseed, not integrate backwards"
    );
    assert!(rewound.chunks_exact(4).any(|px| px[0] != 0.0 || px[1] != 0.0 || px[2] != 0.0));
}

/// Ghosting is the first effect in this project that remembers anything, and
/// the property it must not break is the one every other gate rests on: the
/// same instant asked for twice advances nothing and returns the same image.
///
/// So this checks all three things at once — that a trail exists (a frame with
/// history is not the frame a cold renderer draws), that it does not run away
/// (asking twice is idempotent), and that turning it off restores the old
/// behaviour exactly.
#[test]
fn a_trail_remembers_the_frames_before_it_and_still_holds_still() {
    use metal_render::headless::Renderer;
    use metal_render::model::EFFECTS_WIDTH;

    let mut renderer = Renderer::new();
    // Geoterrain: it moves visibly with time, which is what a trail needs to
    // have anything to leave behind.
    let index = 0;
    let plain = renderer.schemas()[index].defaults();
    let mut row = plain.clone();
    let at = row.len() - EFFECTS_WIDTH + 2; // ghost, third of the four effects floats
    row[at] = 0.8;

    // A cold renderer asked for one instant, versus one walked up to it.
    let cold = Renderer::new().solo(index, &row, 1.4, 96, 54, false).0;
    for step in 0..5 {
        renderer.solo(index, &row, 1.0 + step as f32 * 0.1, 96, 54, false);
    }
    let warm = renderer.solo(index, &row, 1.4, 96, 54, false).0;
    assert_ne!(cold, warm, "the trail carried nothing forward from earlier frames");

    // Twice at the same instant. Without this, a paused preview dissolves.
    let again = renderer.solo(index, &row, 1.4, 96, 54, false).0;
    assert_eq!(warm, again, "asking for the same instant twice advanced the trail");

    // And off is off: a renderer that has had a trail and then has it turned
    // off must draw what a renderer that never had one draws.
    let after = renderer.solo(index, &plain, 2.0, 96, 54, false).0;
    let never = Renderer::new().solo(index, &plain, 2.0, 96, 54, false).0;
    assert_eq!(after, never, "a released trail left something behind");
}

/// The kaleidoscope fold, as a common effect. It is stateless, so the only
/// things to check are that it compiles into the post chain, that it changes
/// the picture, and that folding does not produce a hole — the fold pushes UVs
/// outside the frame near the corners and the clamp is what catches them.
#[test]
fn the_kaleidoscope_effect_folds_any_model_without_tearing_a_hole_in_it() {
    use metal_render::headless::Renderer;
    use metal_render::model::EFFECTS_WIDTH;

    let mut renderer = Renderer::new();
    for index in [0usize, 1, 6] {
        let row = renderer.schemas()[index].defaults();
        let mut kaleido_row = row.clone();
        kaleido_row[row.len() - EFFECTS_WIDTH + 3] = 6.0; // kaleido, fourth effects float
        let plain = renderer.solo(index, &row, 3.0, 96, 54, false).0;
        let folded = renderer.solo(index, &kaleido_row, 3.0, 96, 54, false).0;
        assert_ne!(plain, folded, "model {index} looks the same folded six ways");
        assert!(
            folded.chunks_exact(4).any(|px| px[0].max(px[1]).max(px[2]) > 24),
            "model {index} folded to nothing"
        );
    }
}

/// The shared systems live in `shared::post` and are included by every
/// model's WGSL, but nothing above actually *runs* the post chain — each
/// model draws straight to its own offscreen texture. A reserved-keyword
/// bug in `post.wgsl` (`docs/CORRECTIONS.md` §42) shipped past that gap and
/// was only caught by opening the real window. This closes it: build the
/// pipeline, run one model through it, and check the result the same way
/// the per-model gate does.
#[test]
fn the_post_chain_compiles_and_runs() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);

    let mut model = all_models().into_iter().next().expect("at least one model");
    let floats = model.schema().defaults();
    let (_src_texture, src_view) = offscreen_target(
        &gpu.device,
        (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        FORMAT,
    );
    let ctx = Ctx {
        device: &gpu.device,
        queue: &gpu.queue,
        time: 2.3,
        width: (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        height: (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        fondo_alpha: false, detail: 1.0,
    };
    model.render(&ctx, &floats, &src_view, FORMAT);

    let (dst_texture, dst_view) = offscreen_target(&gpu.device, W, H, FORMAT);
    post.apply(
        &gpu.device,
        &gpu.queue,
        &src_view,
        &dst_view,
        W,
        H,
        &PostParams {
            shake: 0.6,
            light_leak: 0.8,
            leak_color: ColorSlotGpu::from_floats(&[1.0, 0.6, 0.2, 1.0, 0.9, 0.5, 0.8, 0.3, 0.9, 0.6, 0.2, 1.0]),
            time: 2.3,
            kaleido: 0.0,
            wake: EstelaParams::default(),
            grade: GradeUniform::default(),
            chain_fx: ChainFx::default(),
        },
        &[],
        &fallback_ring(&src_view),
    );

    let out = readback(&gpu, &dst_texture);
    assert!(out.iter().all(|v| !v.is_nan() && !v.is_infinite()), "post chain produced NaN or Inf");
    assert!(
        out.chunks_exact(4).any(|px| px[0] != 0.0 || px[1] != 0.0 || px[2] != 0.0),
        "post chain produced an all-black frame"
    );
}

/// The advanced block actually reaches the GPU: `grade_exposure` +100 must
/// brighten the frame relative to the same model through a neutral grade, and
/// the grade must never produce NaN/Inf. `GradeUniform::default()` is the
/// inactive path every other test takes, so without this the `effect_grade`
/// body would only ever be proven to *compile*.
#[test]
fn an_active_colour_grade_moves_the_frame_on_the_gpu() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);

    let mut model = all_models().into_iter().next().expect("at least one model");
    let floats = model.schema().defaults();
    let (_src_texture, src_view) = offscreen_target(
        &gpu.device,
        (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        FORMAT,
    );
    let ctx = Ctx {
        device: &gpu.device,
        queue: &gpu.queue,
        time: 2.3,
        width: (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        height: (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        fondo_alpha: false, detail: 1.0,
    };
    model.render(&ctx, &floats, &src_view, FORMAT);

    let mean_rgb = |gpu: &Gpu, grade: GradeUniform| -> [f32; 3] {
        let (dst_texture, dst_view) = offscreen_target(&gpu.device, W, H, FORMAT);
        post.apply(
            &gpu.device,
            &gpu.queue,
            &src_view,
            &dst_view,
            W,
            H,
            &PostParams {
                shake: 0.0,
                light_leak: 0.0,
                leak_color: ColorSlotGpu::solid(0.0, 0.0, 0.0),
                time: 2.3,
                kaleido: 0.0,
                wake: EstelaParams::default(),
                grade,
                chain_fx: ChainFx::default(),
            },
            &[],
            &fallback_ring(&src_view),
        );
        let out = readback(gpu, &dst_texture);
        assert!(out.iter().all(|v| v.is_finite()), "advanced grade produced NaN/Inf");
        let mut sum = [0.0f32; 3];
        for px in out.chunks_exact(4) {
            sum[0] += px[0];
            sum[1] += px[1];
            sum[2] += px[2];
        }
        let n = (W * H) as f32;
        [sum[0] / n, sum[1] / n, sum[2] / n]
    };
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];

    let neutral = mean_rgb(&gpu, GradeUniform::default());

    // Exposure: +100 is +2 stops, so the frame must brighten.
    let mut exposed = metal_schema::Post::default().advanced;
    exposed[2] = 100.0; // grade_exposure
    let bright = mean_rgb(&gpu, GradeUniform::from_advanced(&exposed));
    assert!(
        luma(bright) > luma(neutral),
        "grade_exposure +100 must brighten: neutral {neutral:?}, exposed {bright:?}"
    );

    // Temperature: +100 is warm — R up, B down. This pins `tone_a.x` (index 0)
    // to the uniform component the shader reads, not just "some grade ran".
    let mut warm = metal_schema::Post::default().advanced;
    warm[0] = 100.0; // grade_temperature
    let w = mean_rgb(&gpu, GradeUniform::from_advanced(&warm));
    assert!(
        w[0] - w[2] > neutral[0] - neutral[2],
        "grade_temperature +100 must warm: neutral {neutral:?}, warm {w:?}"
    );
}

/// **E5 (the design notes PART IV §18.11).** Two of the ten common effects, enabled
/// together, must run one through the other's output — not cross-fade two
/// independent reads of the original frame. Before the fix, every
/// `effect_*` in `post.wgsl`'s chain re-sampled `src_tex` fresh and was
/// folded in via `mix(color, effect(...), amount)`; with two effects both at
/// `amount == 1.0`, that collapses to exactly whichever effect runs *last*
/// in the fixed order (`mix(x, y, 1.0) == y`, discarding `x` — and whatever
/// produced it — entirely). That collapse is the tell of a crossfade, and is
/// what this test rules out.
///
/// `posterize` (effect 6) then `grain` (effect 13), given explicitly as a
/// two-step `chain` — the order is data-driven now, not a fixed line of
/// source, so this pins one concrete order rather than "whatever `fs_main`
/// happens to run third and last". Both fully on must therefore differ from
/// `grain` alone — proof `grain`'s noise landed on `posterize`'s quantized
/// output, not the original frame — and from `posterize` alone — proof
/// `grain` still ran on top of it.
#[test]
fn two_effects_enabled_together_chain_instead_of_crossfading() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);

    let mut model = all_models().into_iter().next().expect("at least one model");
    let floats = model.schema().defaults();
    let (_src_texture, src_view) = offscreen_target(
        &gpu.device,
        (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        FORMAT,
    );
    let ctx = Ctx {
        device: &gpu.device,
        queue: &gpu.queue,
        time: 2.3,
        width: (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        height: (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        fondo_alpha: false, detail: 1.0,
    };
    model.render(&ctx, &floats, &src_view, FORMAT);

    let run = |posterize: f32, grain: f32| {
        let (dst_texture, dst_view) = offscreen_target(&gpu.device, W, H, FORMAT);
        post.apply(
            &gpu.device,
            &gpu.queue,
            &src_view,
            &dst_view,
            W,
            H,
            &PostParams {
                shake: 0.0,
                light_leak: 0.0,
                leak_color: ColorSlotGpu::from_floats(&[
                    1.0, 0.6, 0.2, 1.0, 0.9, 0.5, 0.8, 0.3, 0.9, 0.6, 0.2, 1.0,
                ]),
                time: 2.3,
                kaleido: 0.0,
                wake: EstelaParams::default(),
                grade: GradeUniform::default(),
            chain_fx: ChainFx::default(),
            },
            &[
                EffectStep { effect: 6, amount: posterize, slot: 0 },
                EffectStep { effect: 13, amount: grain, slot: 0 },
            ],
            &fallback_ring(&src_view),
        );
        readback(&gpu, &dst_texture)
    };

    let both_on = run(0.3, 1.0);
    let grain_alone = run(0.0, 1.0);
    let posterize_alone = run(0.3, 0.0);
    let neither = run(0.0, 0.0);

    assert!(
        both_on.iter().all(|v| !v.is_nan() && !v.is_infinite()),
        "chained posterize+grain produced NaN or Inf"
    );
    assert_ne!(
        both_on, grain_alone,
        "posterize+grain together matched grain alone bit-for-bit — grain is still \
         cross-fading against the original frame instead of running on posterize's output"
    );
    assert_ne!(
        both_on, posterize_alone,
        "posterize+grain together matched posterize alone bit-for-bit — grain did not run"
    );
    assert!(
        neither.iter().all(|v| !v.is_nan() && !v.is_infinite())
            && neither.chunks_exact(4).any(|px| px[0] != 0.0 || px[1] != 0.0 || px[2] != 0.0),
        "both effects at 0 must still render a live frame"
    );
}

/// A stress case E5's fix makes possible for the first time: `chain_uv` can
/// now drift far outside `[0, 1]` before the chain finishes (zoom-out
/// composed with a lorenz warp compounds both), which no combination of
/// these effects could do to a shared coordinate before this fix — there was
/// no shared coordinate. Everything downstream of that drift (vignette/
/// blur/sharpen/grain) has to stay finite regardless.
#[test]
fn every_effect_on_at_once_still_renders_a_finite_frame() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);

    let mut model = all_models().into_iter().next().expect("at least one model");
    let floats = model.schema().defaults();
    let (_src_texture, src_view) = offscreen_target(
        &gpu.device,
        (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        FORMAT,
    );
    let ctx = Ctx {
        device: &gpu.device,
        queue: &gpu.queue,
        time: 5.5,
        width: (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        height: (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        fondo_alpha: false, detail: 1.0,
    };
    model.render(&ctx, &floats, &src_view, FORMAT);

    let (dst_texture, dst_view) = offscreen_target(&gpu.device, W, H, FORMAT);
    post.apply(
        &gpu.device,
        &gpu.queue,
        &src_view,
        &dst_view,
        W,
        H,
        &PostParams {
            shake: 0.8,
            light_leak: 0.7,
            leak_color: ColorSlotGpu::from_floats(&[
                1.0, 0.6, 0.2, 1.0, 0.9, 0.5, 0.8, 0.3, 0.9, 0.6, 0.2, 1.0,
            ]),
            time: 5.5,
            kaleido: 6.0,
            wake: EstelaParams::default(),
            grade: GradeUniform::default(),
            // fm at its most extreme: every float at its declared maximum.
            chain_fx: ChainFx::from_advanced(&fm_advanced(&[
                ("fx_wavewarp_largo", 0.02),
                ("fx_wavewarp_alto", 0.4),
                ("fx_wavewarp_tipo", 7.0),
                ("fx_wavewarp_pin", 6.0),
                ("fx_wavewarp_asimetria", 1.0),
                ("fx_wavewarp_capa2", 1.0),
                ("fx_wavewarp_capa2_largo", 0.25),
                ("fx_wavewarp_cruce", 1.0),
                ("fx_wavewarp_borde", 2.0),
                ("fx_fm_portadora_freq", 40.0),
                ("fx_fm_moduladora_freq", 40.0),
                ("fx_fm_indice", 10.0),
                ("fx_fm_forma_onda", 3.0),
                ("fx_fm_color_cant", 1.0),
                ("fx_fm_realim", 1.0),
                // ascii likewise: smallest cells, every ramp mixed, all the
                // jitter, edges and weight at once.
                ("fx_ascii_celda", 4.0),
                ("fx_ascii_rampa", 4.0),
                ("fx_ascii_contraste", 3.0),
                ("fx_ascii_color_modo", 2.0),
                ("fx_ascii_color_mezcla", 1.0),
                ("fx_ascii_fondo", 1.0),
                ("fx_ascii_invierte", 1.0),
                ("fx_ascii_ruido", 1.0),
                ("fx_ascii_gamma", 0.3),
                ("fx_ascii_parpadeo", 1.0),
                ("fx_ascii_proporcion", 0.5),
                ("fx_ascii_bordes", 1.0),
                ("fx_ascii_deriva", 4.0),
                ("fx_ascii_grosor", 1.0),
            ])),
        },
        // The legacy order, all ten given explicitly, plus Wake (`wake`)
        // — effect 15, set F — last:
        // zoom (effect 7) at a negative amount to push `chain_uv` outside [0,
        // 1] before lorenz (effect 9) compounds on top of that same drift, same
        // as before, and now Wake's own texture reads have to
        // tolerate that drift too.
        &[
            EffectStep { effect: 4, amount: 0.8, slot: 0 },  // crt
            EffectStep { effect: 5, amount: 0.7, slot: 0 },  // glitch
            EffectStep { effect: 6, amount: 0.6, slot: 0 },  // posterize
            EffectStep { effect: 7, amount: -0.9, slot: 0 }, // zoom-out
            EffectStep { effect: 8, amount: 0.5, slot: 0 },  // fire
            EffectStep { effect: 9, amount: 0.9, slot: 0 },  // lorenz
            EffectStep { effect: 10, amount: 0.6, slot: 0 }, // vignette
            EffectStep { effect: 11, amount: 0.7, slot: 0 }, // blur
            EffectStep { effect: 12, amount: 0.6, slot: 0 }, // sharpen
            EffectStep { effect: 13, amount: 0.5, slot: 0 }, // grain
            EffectStep { effect: CHAIN_KIND_ESTELA, amount: 0.7, slot: 0 }, // estela
            EffectStep { effect: fm_kind(), amount: 0.9, slot: 0 }, // fm
            EffectStep { effect: wavewarp_kind(), amount: 0.9, slot: 0 }, // wavewarp
            EffectStep { effect: metal_schema::advanced_chain_kind("hardstutter").unwrap(), amount: 0.9, slot: 0 }, // hardstutter (no-op case here)
            EffectStep { effect: ascii_kind(), amount: 0.9, slot: 0 }, // ascii
        ],
        &fallback_ring(&src_view),
    );

    let out = readback(&gpu, &dst_texture);
    assert!(
        out.iter().all(|v| !v.is_nan() && !v.is_infinite()),
        "every effect on at once produced NaN or Inf — likely chain_uv drifting \
         outside [0, 1] after zoom/lorenz and something downstream not tolerating it"
    );
}

/// The regression gate for making the chain order data-driven (2026-09-03):
/// an empty `chain` has to keep meaning exactly what it always meant,
/// effect-for-effect and pixel-for-pixel, for a row with several of the
/// pre-chain fields (`shake`, `light_leak`, `kaleido`) and several of the
/// ten chain effects on at once — see `Renderer::solo_chain`'s doc comment
/// for why an empty slice falls back to the legacy order. `solo` is now
/// defined as `solo_chain` with an empty slice, so this doubles as a pin
/// against that delegation ever drifting if someone later gives the two
/// their own separate bodies.
#[test]
fn solo_and_an_empty_explicit_chain_render_the_same_picture() {
    use metal_render::headless::Renderer;
    use metal_render::model::EFFECTS_WIDTH;

    let mut renderer = Renderer::new();
    for index in [0usize, 3, 7] {
        let defaults = renderer.schemas()[index].defaults();
        let mut row = defaults.clone();
        let at = row.len() - EFFECTS_WIDTH;
        // shake, light_leak, kaleido, crt, zoom, vignette, grain — `ghost`
        // (at + 2) stays 0 so neither call below grows a trail history and
        // the two renderers diverge on that instead of on the chain.
        row[at] = 0.3; // shake
        row[at + 1] = 0.4; // light_leak
        row[at + 3] = 6.0; // kaleido
        row[at + 4] = 0.5; // crt
        row[at + 7] = -0.6; // zoom
        row[at + 10] = 0.7; // vignette
        row[at + 13] = 0.4; // grain

        let plain = renderer.solo(index, &row, 2.5, 96, 54, false).0;
        let chained = renderer
            .solo_chain(
                index,
                &row,
                2.5,
                96,
                54,
                false,
                &[],
                &EstelaParams::default(),
            )
            .0;
        assert_eq!(
            plain, chained,
            "model {index}: solo and solo_chain(.., &[], ..) disagree on an empty chain"
        );
    }
}

/// **The test that proves the feature works.** Same two effects, same
/// amounts, opposite order, must render two different pictures — the whole
/// point of `chain` is that "vignette then zoom" and "zoom then vignette"
/// are different images now, not two names for the one fixed line of source
/// `fs_main` used to run regardless of what a graph asked for.
///
/// `zoom` (7) and `vignette` (10) were picked because they provably do not
/// commute, not because any pair would do: `zoom` relocates `chain_uv` (see
/// `zoom_uv`) *and* zeroes the pixel outright wherever the zoomed-out sample
/// position lands outside `[0, 1]` (`effect_zoom`'s `in_bounds` factor),
/// while `vignette`'s falloff is a function of `chain_uv`'s position. Run
/// vignette after zoom and its falloff — and zoom's own out-of-bounds
/// blackening — are both measured in the *warped* frame; run it before and
/// vignette darkens the *original* framing, then zoom separately blends in
/// a fresh, independently-blackened sample on top. Different arithmetic, not
/// just a different blend weight, which is what makes this pair a proof
/// rather than a coincidence — and unlike most pairs in this shader, it does
/// not depend on what the source model happens to draw.
#[test]
fn reordering_two_interacting_effects_changes_the_rendered_picture() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);

    let mut model = all_models().into_iter().next().expect("at least one model");
    let floats = model.schema().defaults();
    let (_src_texture, src_view) = offscreen_target(
        &gpu.device,
        (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        FORMAT,
    );
    let ctx = Ctx {
        device: &gpu.device,
        queue: &gpu.queue,
        time: 1.1,
        width: (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        height: (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        fondo_alpha: false, detail: 1.0,
    };
    model.render(&ctx, &floats, &src_view, FORMAT);

    let run = |chain: &[EffectStep]| {
        let (dst_texture, dst_view) = offscreen_target(&gpu.device, W, H, FORMAT);
        post.apply(
            &gpu.device,
            &gpu.queue,
            &src_view,
            &dst_view,
            W,
            H,
            &PostParams {
                shake: 0.0,
                light_leak: 0.0,
                leak_color: ColorSlotGpu::from_floats(&[
                    1.0, 0.6, 0.2, 1.0, 0.9, 0.5, 0.8, 0.3, 0.9, 0.6, 0.2, 1.0,
                ]),
                time: 1.1,
                kaleido: 0.0,
                wake: EstelaParams::default(),
                grade: GradeUniform::default(),
            chain_fx: ChainFx::default(),
            },
            chain,
            &fallback_ring(&src_view),
        );
        readback(&gpu, &dst_texture)
    };

    let zoom_then_vignette = run(&[
        EffectStep { effect: 7, amount: -0.7, slot: 0 },
        EffectStep { effect: 10, amount: 0.8, slot: 0 },
    ]);
    let vignette_then_zoom = run(&[
        EffectStep { effect: 10, amount: 0.8, slot: 0 },
        EffectStep { effect: 7, amount: -0.7, slot: 0 },
    ]);

    assert!(
        zoom_then_vignette.iter().all(|v| !v.is_nan() && !v.is_infinite())
            && vignette_then_zoom.iter().all(|v| !v.is_nan() && !v.is_infinite()),
        "reordering zoom and vignette produced NaN or Inf"
    );
    assert_ne!(
        zoom_then_vignette, vignette_then_zoom,
        "zoom then vignette matched vignette then zoom bit-for-bit — the chain order \
         is not actually reaching the shader, or zoom/vignette do not interact the way \
         this test's doc comment claims"
    );
}

/// A chain step naming a kind neither a row float nor a footage kind (here,
/// one past Wake's own position — the first number that is neither) must
/// be dropped the same way `Post::resolved_chain` already drops it on the
/// Rust side (see its own metal-schema tests), not merely "happen to render
/// something" the way `an_oversized_or_out_of_range_chain_does_not_panic_or_produce_garbage`
/// checks. This pins the *value*: the unknown step contributes nothing, so
/// a chain with only an unknown step must render byte-identical to an empty
/// chain — proof it was actually dropped, not merely tolerated.
#[test]
fn a_chain_step_naming_an_unknown_kind_is_dropped_rather_than_changing_the_picture() {
    use metal_render::headless::Renderer;

    let mut renderer = Renderer::new();
    let index = 0usize;
    let row = renderer.schemas()[index].defaults();

    let plain = renderer.solo_chain(
        index,
        &row,
        4.1,
        64,
        36,
        false,
        &[],
        &EstelaParams::default(),
    );
    let unknown = renderer.solo_chain(
        index,
        &row,
        4.1,
        64,
        36,
        false,
        &[EffectStep { effect: CHAIN_KIND_ESTELA + 1, amount: 1.0, slot: 0 }],
        &EstelaParams::default(),
    );

    assert_eq!(
        plain.0, unknown.0,
        "a chain step naming a kind this build does not declare changed the rendered picture"
    );
}

/// `Post::resolved_chain` already guards `MAX_CHAIN` and drops an
/// out-of-range effect index (see its own tests in `metal-schema`), but
/// `solo_chain`/`apply_tile` accept *any* `&[EffectStep]`, not only ones
/// that went through it. This exercises the belt-and-suspenders clamp in
/// `apply_tile` directly: a chain twice `MAX_CHAIN` long, naming an effect
/// index no schema declares partway through, has to render a live, finite
/// frame rather than panicking or indexing past the GPU's fixed-size array.
#[test]
fn an_oversized_or_out_of_range_chain_does_not_panic_or_produce_garbage() {
    use metal_render::headless::Renderer;

    let mut renderer = Renderer::new();
    let index = 0usize;
    let row = renderer.schemas()[index].defaults();

    let mut chain = Vec::new();
    for i in 0..(metal_schema::MAX_CHAIN * 2) {
        // Cycles through the twelve positions the chain dispatch knows (the
        // ten common effects 4..=13, the reserved gap at 14, and `fx.estela`
        // at 15) at a comfortably non-zero
        // amount, so a silently truncated tail would show up as a visible
        // difference rather than nothing to notice.
        chain.push(EffectStep { effect: 4 + (i as u32 % 12), amount: 0.5, slot: 0 });
    }
    // One index this build's schema does not declare, dropped into the
    // middle rather than truncated away, so the out-of-range guard is
    // exercised somewhere the length guard alone would not reach.
    chain[3] = EffectStep { effect: 9999, amount: 1.0, slot: 0 };

    let (pixels, w, h) = renderer.solo_chain(
        index,
        &row,
        3.0,
        64,
        36,
        false,
        &chain,
        &EstelaParams::default(),
    );
    assert_eq!(pixels.len(), (w * h * 4) as usize, "solo_chain returned the wrong buffer size");
    assert!(
        pixels.chunks_exact(4).any(|px| px[0] != 0 || px[1] != 0 || px[2] != 0),
        "an oversized/out-of-range chain rendered an all-black frame"
    );
}

/// River is the first model in the suite that displaces a vertex, so it gets the
/// gate that only it can fail: **that the wind reaches the bushes and the
/// current reaches the water, and that neither reaches anything else.**
///
/// Every other check in this file passes just as well on a river frozen solid.
/// A displacement silently multiplied to nothing is lit, deterministic, free of
/// NaN, and renders a perfectly convincing still photograph of a valley — which
/// is exactly the failure a suite of *visuals* cannot afford and no general
/// gate can see.
///
/// The four checks are one argument in four parts, and the order matters. Every
/// float that can animate anything else is turned off first — `variacion` and
/// `pulso` (the flicker and the moon's breathing), `barrido` (the sweep),
/// `deriva` (the camera drift) — so that the frames below differ for exactly
/// one reason each and there is nowhere for a difference to come from by
/// accident.
#[test]
fn rio_moves_where_it_says_it_moves_and_nowhere_else() {
    let gpu = Gpu::new_headless();
    let (w, h) = (128u32, 72u32);
    let index = all_models()
        .iter()
        .position(|m| m.id() == "river")
        .expect("rio is in the registry");
    let mut model = metal_render::models::rio::Rio::new();

    // variacion, pulso, barrido, deriva off; the water flat *and* stopped; the
    // wind still. `corriente` has to go as well as `oleaje` — the surface
    // slides downstream as well as rising and falling, and it is the current
    // that carries it, not the swell.
    let mut still = metal_render::registry::all_models()[index].schema().defaults();
    for (slot, value) in [(1, 0.0), (2, 0.0), (3, 0.0), (4, 0.0), (7, 0.0), (8, 0.0), (10, 0.0)] {
        still[slot] = value;
    }

    // 1. With all of that off, the valley is a photograph: two different
    //    instants are the same image, down to the byte. This is what makes the
    //    three tests below mean anything.
    let a = render_at(&mut model, &gpu, &still, 2.0, w, h);
    let b = render_at(&mut model, &gpu, &still, 9.5, w, h);
    assert_eq!(a, b, "with every animated float at zero the frame still moves");

    // 2. The wind reaches the bushes.
    let mut blown = still.clone();
    blown[10] = 1.5;
    let windy = render_at(&mut model, &gpu, &blown, 2.0, w, h);
    assert_ne!(a, windy, "the wind does not move the bushes");

    // 3. And reaches nothing else. With no bushes in the valley and the water
    //    flat, `viento` has nothing left it is allowed to touch — the chop it
    //    puts on the surface is multiplied by `oleaje`, which is zero here. A
    //    difference at this point means the wind is leaking into the ground,
    //    the stones or the moon.
    let mut bare = still.clone();
    bare[16] = 0.0;
    let mut bare_windy = bare.clone();
    bare_windy[10] = 1.5;
    assert_eq!(
        render_at(&mut model, &gpu, &bare, 2.0, w, h),
        render_at(&mut model, &gpu, &bare_windy, 2.0, w, h),
        "the wind is moving something that is not a bush or the water"
    );

    // 4. The current reaches the water. No bushes and no wind, so the surface
    //    is the only thing in the frame left that can differ between two
    //    instants — and it has to.
    let mut running = bare;
    running[7] = 0.9;
    running[8] = 0.6;
    let t0 = render_at(&mut model, &gpu, &running, 2.0, w, h);
    let t1 = render_at(&mut model, &gpu, &running, 2.4, w, h);
    assert_ne!(t0, t1, "the river is not running");

    // 5. And the ends of the four structural floats, which the defaults gate
    //    never visits. These rebuild the valley rather than shade it, and two
    //    of them can degenerate: an empty river at `caudal = 0` is a shoreline
    //    search that has to find water in a bed it very nearly does not fit
    //    in, and `lineas = 4` is a sheet coarse enough that a stone's own cell
    //    count clamps to its floor.
    let defaults = metal_render::registry::all_models()[index].schema().defaults();
    for (slot, value) in [
        (13, 4.0),  // lineas, at both ends
        (13, 40.0),
        (14, 0.0), // caudal: the river at its lowest
        (14, 1.0), // and at its highest
        (15, 0.0), // no stones
        (15, 24.0),
        (16, 0.0), // no bushes
        (16, 16.0),
    ] {
        let mut row = defaults.clone();
        row[slot] = value;
        let frame = render_at(&mut model, &gpu, &row, 3.0, w, h);
        let name = ["lines", "flow", "stones", "bushes"][slot - 13];
        assert!(
            frame.iter().all(|v| !v.is_nan() && !v.is_infinite()),
            "{name} = {value} renders NaN or Inf"
        );
        assert!(
            frame.chunks_exact(4).any(|px| px[0] != 0.0 || px[1] != 0.0 || px[2] != 0.0),
            "{name} = {value} renders an empty valley"
        );
    }
}

/// Terrarium is the second model in the suite that keeps state, so it gets the
/// same three checks Turing does — and one more that only it can fail.
///
/// The extra one is the whole model: **the sand has to end up at the bottom**.
/// Every other property here would hold just as well for a box of grains that
/// hang in mid-air — the frame would be lit, deterministic, and free of NaN —
/// and a gravity bug leaves all of those true. It is checked as brightness
/// down the box rather than by reading the grid back, because the grid is a
/// private texture and the thing that has to be right is what an operator sees.
///
/// The pile is measured with the model turned down to one still mouth and the
/// drain shut, not at its defaults. Five sweeping mouths paint the whole height
/// of the box with falling sand, so at the defaults a pile and no pile look
/// nearly the same from a brightness profile — the check would pass on a broken
/// model. One mouth over a closed floor has to build a cone or the model is
/// wrong.
#[test]
fn terrario_piles_its_sand_up_at_the_bottom_and_holds_still_without_time() {
    let gpu = Gpu::new_headless();
    let mut model = metal_render::models::terrario::Terrario::new();
    let floats = model.schema().defaults();
    // 16:9, so the box fills the frame and the bands below are all box.
    let (w, h) = (128u32, 72u32);

    let a = render_at(&mut model, &gpu, &floats, 4.0, w, h);
    let again = render_at(&mut model, &gpu, &floats, 4.0, w, h);
    assert_eq!(a, again, "the same instant twice must not step the sand");

    let later = render_at(&mut model, &gpu, &floats, 4.5, w, h);
    assert_ne!(a, later, "half a second later the sand must have moved");

    let rewound = render_at(&mut model, &gpu, &floats, 0.0, w, h);
    assert!(
        rewound.iter().all(|v| !v.is_nan() && !v.is_infinite()),
        "rewinding must empty the box, not step it backwards"
    );
    assert!(rewound.chunks_exact(4).any(|px| px[0] != 0.0 || px[1] != 0.0 || px[2] != 0.0));

    // One mouth, centred, not sweeping, over a floor with no drain in it.
    let mut one = floats.clone();
    one[0] = 1.0; // fuentes
    one[3] = 0.05; // ancho
    one[4] = 0.8; // caudal
    one[5] = 0.0; // vaiven
    one[12] = 0.0; // vibracion
    one[13] = 0.0; // drenaje
    one[20] = 0.0; // estorbos

    let mut pile = metal_render::models::terrario::Terrario::new();
    let mut frame = Vec::new();
    for i in 0..60 {
        frame = render_at(&mut pile, &gpu, &one, 4.0 + i as f32 / 60.0, w, h);
    }
    // The middle of the frame only: the glass runs the full height of both
    // edges, and counting it would put the same constant in both bands and
    // flatten the very ratio being measured.
    let band = |frame: &[f32], from: f32, to: f32| {
        let (y0, y1) = ((h as f32 * from) as usize, (h as f32 * to) as usize);
        let (x0, x1) = (w as usize * 2 / 10, w as usize * 8 / 10);
        let mut sum = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                let px = (y * w as usize + x) * 4;
                sum += frame[px] + frame[px + 1] + frame[px + 2];
            }
        }
        sum / ((y1 - y0) * (x1 - x0)) as f32
    };
    let upper = band(&frame, 0.30, 0.60);
    let lower = band(&frame, 0.85, 0.98);
    assert!(
        lower > upper * 3.0,
        "the sand is not piling: the floor of the box reads {lower:.4} against \
         {upper:.4} in the middle of it, where only the stream should be"
    );
}

/// The `fondo_alpha` parameter (the design notes PART IV §17.3) tells the shared
/// Backdrop pass whether to emit alpha 0 instead of 1, allowing sources to
/// stack without the background covering lower layers. This verifies that
/// only the alpha channel is affected, and that RGB stays the same.
#[test]
fn fondo_alpha_affects_only_the_alpha_channel() {
    let gpu = Gpu::new_headless();
    let (w, h) = (64u32, 64u32);

    // rio is one of the twelve models that draw through the Backdrop pass.
    let index = all_models()
        .iter()
        .position(|m| m.id() == "river")
        .expect("rio is in the registry");
    let mut model = metal_render::models::rio::Rio::new();
    let mut floats = metal_render::registry::all_models()[index].schema().defaults();

    // Flatten the water completely so the background is guaranteed to be
    // visible across the frame: set oleaje to zero, variacion to zero.
    floats[2] = 0.0; // variacion
    floats[7] = 0.0; // oleaje

    fn render_with_alpha(
        model: &mut dyn Model,
        gpu: &Gpu,
        floats: &[f32],
        w: u32,
        h: u32,
        fondo_alpha: bool,
    ) -> Vec<f32> {
        let (texture, view) = offscreen_target(&gpu.device, w, h, FORMAT);
        let ctx = Ctx {
            device: &gpu.device,
            queue: &gpu.queue,
            time: 1.0,
            width: w,
            height: h,
            fondo_alpha,
            detail: 1.0,
        };
        model.render(&ctx, floats, &view, FORMAT);
        readback_at(&gpu, &texture, w, h)
    }

    // Render the same frame twice: once with fondo_alpha=false (opaque
    // background, alpha=1), once with fondo_alpha=true (transparent, alpha=0).
    let opaque = render_with_alpha(&mut model, &gpu, &floats, w, h, false);
    let transparent = render_with_alpha(&mut model, &gpu, &floats, w, h, true);

    // Pick a pixel from the corner — guaranteed to be background-only.
    let corner_idx = (0 * w + 0) as usize * 4; // Top-left, RGBA

    // Alpha channel is the 4th component (indices 3, 7, 11, ...).
    let opaque_alpha = opaque[corner_idx + 3];
    let transparent_alpha = transparent[corner_idx + 3];

    // With fondo_alpha=false, the alpha should be ~1.0 (in f32: ~1.0 → close to 1.0).
    // With fondo_alpha=true, the alpha should be ~0.0.
    assert!(
        opaque_alpha > 0.9,
        "fondo_alpha=false should produce opaque backdrop (alpha > 0.9), got {opaque_alpha}"
    );
    assert!(
        transparent_alpha < 0.1,
        "fondo_alpha=true should produce transparent backdrop (alpha < 0.1), got {transparent_alpha}"
    );

    // RGB should be identical (or very close) — only alpha differs.
    for i in 0..3 {
        let r = (opaque[corner_idx + i] - transparent[corner_idx + i]).abs();
        assert!(
            r < 0.01,
            "RGB channel {i} differs between opaque and transparent: {}, {}",
            opaque[corner_idx + i],
            transparent[corner_idx + i]
        );
    }
}

// ---------------------------------------------------------------------------
// `fx.estela` (`CHAIN_KIND_ESTELA`, 15) — set F's promotion, the
// ring `shaders/post.wgsl`'s own block comment above `effect_crt` costs out.
// Three properties, each pinned separately: a ring that
// exists but panics before it fills, an effect that is not exactly off at
// amount 0, and a `case 15u` that is unreachable or an identity would each
// be a different bug, and a single combined test would only say "something
// is wrong" rather than which of the three it is.

/// "The ring's fallback before it has filled is sane and doesn't panic" —
/// the very first frame a chain naming `fx.estela` ever draws, before
/// `EstelaRing::new` has anywhere to source real history from but the
/// current frame itself (`EstelaRing::reset`'s own fallback). This is the
/// one property [`PostChain::apply`] alone cannot exercise — that function
/// always receives an already-decided ring — so this goes through
/// `Renderer::solo_chain`, which owns the ring and decides when to allocate
/// it.
#[test]
fn the_estela_ring_is_sane_on_the_very_first_frame_it_is_asked_for() {
    use metal_render::headless::Renderer;

    let mut renderer = Renderer::new();
    let index = 0usize;
    let row = renderer.schemas()[index].defaults();

    let (pixels, w, h) = renderer.solo_chain(
        index,
        &row,
        0.7,
        96,
        54,
        false,
        &[EffectStep { effect: CHAIN_KIND_ESTELA, amount: 1.0, slot: 0 }],
        &EstelaParams::default(),
    );

    assert_eq!(pixels.len(), (w * h * 4) as usize, "solo_chain returned the wrong buffer size");
    assert!(
        pixels.chunks_exact(4).any(|px| px[0].max(px[1]).max(px[2]) > 24),
        "the very first fx.estela frame — ring unfilled, every tap a copy of the live frame \
         — rendered blank"
    );
}

/// The same "0 is exactly off" guarantee every chain effect makes, for
/// Wake: the outer `mix(chain_color, effect_estela(...), clamp(amt, 0, 1))`
/// in `case 15u` has to return `chain_color` bit-for-bit at `amt == 0`,
/// whatever the ring holds and whatever `effect_estela` computed against it.
#[test]
fn an_estela_chain_step_at_zero_renders_identical_to_no_chain_at_all() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);

    let mut model = all_models().into_iter().next().expect("at least one model");
    let floats = model.schema().defaults();
    let (_src_texture, src_view) = offscreen_target(
        &gpu.device,
        (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        FORMAT,
    );
    let ctx = Ctx {
        device: &gpu.device,
        queue: &gpu.queue,
        time: 9.2,
        width: (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        height: (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
        fondo_alpha: false, detail: 1.0,
    };
    model.render(&ctx, &floats, &src_view, FORMAT);

    let run = |chain: &[EffectStep]| {
        let (dst_texture, dst_view) = offscreen_target(&gpu.device, W, H, FORMAT);
        post.apply(
            &gpu.device,
            &gpu.queue,
            &src_view,
            &dst_view,
            W,
            H,
            &PostParams {
                shake: 0.0,
                light_leak: 0.0,
                leak_color: ColorSlotGpu::from_floats(&[
                    1.0, 0.6, 0.2, 1.0, 0.9, 0.5, 0.8, 0.3, 0.9, 0.6, 0.2, 1.0,
                ]),
                time: 9.2,
                kaleido: 0.0,
                wake: EstelaParams::default(),
                grade: GradeUniform::default(),
            chain_fx: ChainFx::default(),
            },
            chain,
            // A fallback ring (the current frame, repeated) rather than a
            // real one — irrelevant to this property: `amt == 0` must
            // discard whatever `effect_estela` computed regardless of what
            // it read, so a degenerate ring is if anything the harder case,
            // not a weaker one.
            &fallback_ring(&src_view),
        );
        readback(&gpu, &dst_texture)
    };

    let without = run(&[]);
    let zero_estela = run(&[EffectStep { effect: CHAIN_KIND_ESTELA, amount: 0.0, slot: 0 }]);
    assert_eq!(
        without, zero_estela,
        "a fx.estela step at amount 0 must be exactly as inert as no chain at all"
    );
}

/// **The test that proves the effect works** — check.mjs's own methodology
/// ("render a frame with the step active vs. not, compare the bytes differ
/// and neither is corrupted or blank") done here in Rust instead of over
/// JPEG endpoints, so it runs with `cargo test` rather than needing a live
/// server.
///
/// Unlike the zero-vs-off test above, this needs *real* ring history, not
/// the single-frame fallback: with every tap equal to the live frame (the
/// fallback), a frozen block's drift still moves the sample point, but there
/// is no second frame behind it to reveal — the whole "several different
/// moments at once" look `models::estela`'s own doc describes needs the
/// generator to have actually drawn something different a few taps ago.
/// Model 0 (Geoterrain) is `a_trail_remembers_the_frames_before_it_and_
/// still_holds_still`'s own choice for the same reason: it moves visibly
/// with time, which is what a history needs to have anything to reveal.
#[test]
fn an_estela_chain_step_changes_the_picture_once_the_ring_has_real_history() {
    use metal_render::headless::Renderer;

    let mut renderer = Renderer::new();
    let index = 0usize;
    let row = renderer.schemas()[index].defaults();
    let chain = [EffectStep { effect: CHAIN_KIND_ESTELA, amount: 1.0, slot: 0 }];

    // Ten draws, spaced well past `models::estela::TAP_DT` (1/12s) so each
    // one actually advances the ring by a slot (`EstelaRing::advance`'s own
    // gate) — ten to fill all `ESTELA_RING_TAPS` (8) slots with genuinely
    // distinct frames and then some, so by the final draw nothing in the
    // ring is still the allocation-time fallback copy.
    let mut t = 0.0f32;
    for _ in 0..10 {
        renderer.solo_chain(index, &row, t, 96, 54, false, &chain, &EstelaParams::default());
        t += 0.15;
    }
    let t_final = t - 0.15;

    // Same instant again, with the step: determinism (this module's own
    // render gate) says this must not advance the ring further, so this is
    // reading exactly the history just built, not a fresh eleventh frame of
    // it.
    let with_estela =
        renderer.solo_chain(index, &row, t_final, 96, 54, false, &chain, &EstelaParams::default());
    // An empty chain at the same instant: `estela_active` is false for this
    // call (see `draw_tile`'s own gate), so it neither reads nor disturbs
    // the ring built above — this is simply the model's own frame with no
    // chain step at all, the baseline "without" this property needs.
    let without_estela =
        renderer.solo_chain(index, &row, t_final, 96, 54, false, &[], &EstelaParams::default());

    assert!(
        with_estela.0.chunks_exact(4).any(|px| px[0].max(px[1]).max(px[2]) > 24),
        "fx.estela with real ring history rendered blank"
    );
    assert!(
        without_estela.0.chunks_exact(4).any(|px| px[0].max(px[1]).max(px[2]) > 24),
        "the baseline frame with no chain step rendered blank"
    );
    assert_ne!(
        with_estela.0, without_estela.0,
        "a fx.estela step with real ring history rendered identically to no chain at all — \
         case 15u is not being reached, effect_estela computed an identity, or the ring \
         never accumulated any history distinct from the live frame"
    );
}

/// Loops T12: `fx.warp` through the real GPU pass — amount 0 hands the frame
/// back exactly, any amount > 0 moves pixels in every mode, the GPU field
/// agrees with the CPU reference (`metal_schema::warp::apply_cpu`) to within
/// filtering rounding, and mosh pulls from the node's previous output.
#[test]
fn the_warp_is_the_identity_at_zero_and_moves_pixels_above_it() {
    use metal_schema::warp::{apply_cpu, Warp, MODES};
    let (w, h) = (48u32, 32u32);
    let frame: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x * 5) as u8, (y * 7) as u8, ((x + y) * 3) as u8, 255]
        })
        .collect();
    let mut r = metal_render::headless::Renderer::new();
    for k in 0..MODES.len() {
        let mode = (k as f32 + 0.5) / MODES.len() as f32;
        let still = Warp { mode, amount: 0.0, ..Warp::default() };
        let out = r.warp_fx(&format!("still-{k}"), &still, &frame, w, h).unwrap();
        assert_eq!(out, frame, "{}: amount 0 is the identity", MODES[k]);
        // Through the shader itself: a vanishing field samples every pixel
        // at its own centre.
        let tiny = Warp { amount: 1e-6, ..still };
        let out = r.warp_fx(&format!("tiny-{k}"), &tiny, &frame, w, h).unwrap();
        let worst = out.iter().zip(&frame).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 1, "{}: the GPU pass is not the identity at ~0 (worst {worst})", MODES[k]);
        let on = Warp { mode, amount: 0.8, frequency: 0.5, phase: 0.2, ..Warp::default() };
        let out = r.warp_fx(&format!("on-{k}"), &on, &frame, w, h).unwrap();
        let changed = out.chunks(4).zip(frame.chunks(4)).filter(|(a, b)| a != b).count();
        assert!(changed > (w * h / 10) as usize, "{}: only {changed} pixels moved", MODES[k]);
        let cpu = apply_cpu(&on, &frame, None, w, h);
        let worst = out.iter().zip(&cpu).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        let close = out.iter().zip(&cpu).filter(|(a, b)| (**a as i32 - **b as i32).abs() <= 8).count();
        assert!(close * 100 >= out.len() * 97, "{}: GPU and CPU fields disagree (worst {worst})", MODES[k]);
    }
    // Mosh: a black frame after a white one, mosh 0.5 → about half grey.
    let white = vec![255u8; (w * h * 4) as usize];
    let black: Vec<u8> = (0..w * h).flat_map(|_| [0u8, 0, 0, 255]).collect();
    let m = Warp { mosh: 0.5, ..Warp::default() };
    r.warp_fx("mosh", &m, &white, w, h).unwrap();
    let out = r.warp_fx("mosh", &m, &black, w, h).unwrap();
    let mid = out[((h / 2 * w + w / 2) * 4) as usize];
    assert!((100..=200).contains(&mid), "moshed pixel {mid}");
}


// ---------------------------------------------------------------------------
// fx.fm — frequency-modulation warp (the first multi-float chain effect)
// ---------------------------------------------------------------------------

fn fm_kind() -> u32 {
    metal_schema::advanced_chain_kind("fm").expect("fm is an advanced chain effect")
}

/// `Post::default().advanced` with the named fm floats overridden.
fn fm_advanced(set: &[(&str, f32)]) -> Vec<f32> {
    let mut adv = metal_schema::Post::default().advanced;
    for (name, v) in set {
        let i = metal_schema::effect_index(name).unwrap_or_else(|| panic!("no float {name}"));
        adv[i - metal_schema::CHAIN_FLOATS] = *v;
    }
    adv
}

/// One post pass over the first model's frame with `fm` driven by `adv`
/// (its `fx_fm_cant` is the step amount) -- or with no chain at all when
/// `chain` is false.
fn run_fm(gpu: &Gpu, post: &PostChain, src_view: &wgpu::TextureView, adv: &[f32], chain: bool) -> Vec<f32> {
    let amount = adv[metal_schema::effect_index("fx_fm_cant").unwrap() - metal_schema::CHAIN_FLOATS];
    let steps = [EffectStep { effect: fm_kind(), amount, slot: 0 }];
    let (dst_texture, dst_view) = offscreen_target(&gpu.device, W, H, FORMAT);
    post.apply(
        &gpu.device,
        &gpu.queue,
        src_view,
        &dst_view,
        W,
        H,
        &PostParams {
            shake: 0.0,
            light_leak: 0.0,
            leak_color: ColorSlotGpu::solid(0.0, 0.0, 0.0),
            time: 2.3,
            kaleido: 0.0,
            wake: EstelaParams::default(),
            grade: GradeUniform::default(),
            chain_fx: ChainFx::from_advanced(adv),
        },
        if chain { &steps } else { &[] },
        &fallback_ring(src_view),
    );
    readback(gpu, &dst_texture)
}

fn fm_source(gpu: &Gpu) -> wgpu::TextureView {
    let mut model = all_models().into_iter().next().expect("at least one model");
    let floats = model.schema().defaults();
    let (w, h) = (
        (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
    );
    let (_t, view) = offscreen_target(&gpu.device, w, h, FORMAT);
    let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: 2.3, width: w, height: h, fondo_alpha: false, detail: 1.0 };
    model.render(&ctx, &floats, &view, FORMAT);
    view
}

/// `cant = 0` must leave the frame bit-identical, however wild the other ten
/// floats are, whether or not the step is in the chain.
#[test]
fn fm_at_zero_amount_leaves_the_frame_bit_identical() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);
    let src = fm_source(&gpu);
    let none = run_fm(&gpu, &post, &src, &fm_advanced(&[]), false);
    let wild = fm_advanced(&[
        ("fx_fm_cant", 0.0),
        ("fx_fm_indice", 10.0),
        ("fx_fm_color_cant", 1.0),
        ("fx_fm_realim", 1.0),
        ("fx_fm_forma_onda", 3.0),
    ]);
    assert_eq!(run_fm(&gpu, &post, &src, &wild, true), none, "fm with cant=0 changed the frame");
    assert_eq!(run_fm(&gpu, &post, &src, &wild, false), none);
}

/// Every one of the eleven fm floats changes a rendered frame.
#[test]
fn every_fm_float_changes_the_rendered_frame() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);
    let src = fm_source(&gpu);
    let base_set = [("fx_fm_cant", 0.8), ("fx_fm_indice", 3.0)];
    let base = run_fm(&gpu, &post, &src, &fm_advanced(&base_set), true);
    let plain = run_fm(&gpu, &post, &src, &fm_advanced(&[]), false);
    assert_ne!(base, plain, "fm on did not move the frame at all");
    assert!(base.iter().all(|v| v.is_finite()));

    for spec in metal_schema::advanced_effect_floats("fm").unwrap() {
        // Move each float 37% of its range toward the farther end (not to the
        // end itself: `fase` and `angulo` wrap, so min and max are the same
        // picture).
        let here = fm_advanced(&base_set)[metal_schema::effect_index(spec.name).unwrap() - metal_schema::CHAIN_FLOATS];
        let step = (spec.max - spec.min) * 0.37;
        let to = if (spec.max - here) >= (here - spec.min) { here + step } else { here - step };
        let mut set = base_set.to_vec();
        set.push((spec.name, if spec.name == "fx_fm_cant" { 0.3 } else { to }));
        let moved = run_fm(&gpu, &post, &src, &fm_advanced(&set), true);
        assert!(moved.iter().all(|v| v.is_finite()), "{} produced NaN/Inf", spec.name);
        assert_ne!(moved, base, "{} did not change the rendered frame", spec.name);
    }
}

/// The whole path a row takes: fm written into the row's effects tail reaches
/// the legacy chain, the uniform and the picture; the default row does not.
#[test]
fn an_fm_row_moves_the_picture_through_the_renderer_and_a_default_row_does_not() {
    use metal_render::headless::Renderer;
    let mut renderer = Renderer::new();
    let row = renderer.schemas()[0].defaults();
    let at = row.len() - metal_schema::EFFECTS_WIDTH;
    let mut on = row.clone();
    let cant = metal_schema::effect_index("fx_fm_cant").unwrap();
    on[at + cant] = 0.9;
    let draw = |r: &mut Renderer, row: &[f32]| r.solo_chain(0, row, 2.3, 96, 54, false, &[], &EstelaParams::default()).0;
    let off_a = draw(&mut renderer, &row);
    let on_a = draw(&mut renderer, &on);
    assert_ne!(off_a, on_a, "an fm row did not change the picture");
    assert_eq!(off_a, draw(&mut renderer, &row), "the default row is not deterministic");
}


// ---------------------------------------------------------------------------
// fx.wavewarp -- AE-style wave warp (second multi-float chain effect)
// ---------------------------------------------------------------------------

fn wavewarp_kind() -> u32 {
    metal_schema::advanced_chain_kind("wavewarp").expect("wavewarp is an advanced chain effect")
}

/// One post pass with `wavewarp` driven by `adv` (its `cant` is the amount),
/// or with no chain at all when `chain` is false.
fn run_wavewarp(gpu: &Gpu, post: &PostChain, src_view: &wgpu::TextureView, adv: &[f32], chain: bool) -> Vec<f32> {
    let amount = adv[metal_schema::effect_index("fx_wavewarp_cant").unwrap() - metal_schema::CHAIN_FLOATS];
    let steps = [EffectStep { effect: wavewarp_kind(), amount, slot: 0 }];
    let (dst_texture, dst_view) = offscreen_target(&gpu.device, W, H, FORMAT);
    post.apply(
        &gpu.device,
        &gpu.queue,
        src_view,
        &dst_view,
        W,
        H,
        &PostParams {
            shake: 0.0,
            light_leak: 0.0,
            leak_color: ColorSlotGpu::solid(0.0, 0.0, 0.0),
            time: 2.3,
            kaleido: 0.0,
            wake: EstelaParams::default(),
            grade: GradeUniform::default(),
            chain_fx: ChainFx::from_advanced(adv),
        },
        if chain { &steps } else { &[] },
        &fallback_ring(src_view),
    );
    readback(gpu, &dst_texture)
}

#[test]
fn wavewarp_at_zero_amount_leaves_the_frame_bit_identical() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);
    let src = fm_source(&gpu);
    let none = run_wavewarp(&gpu, &post, &src, &fm_advanced(&[]), false);
    let wild = fm_advanced(&[
        ("fx_wavewarp_cant", 0.0),
        ("fx_wavewarp_alto", 0.4),
        ("fx_wavewarp_tipo", 5.0),
        ("fx_wavewarp_capa2", 1.0),
        ("fx_wavewarp_cruce", 1.0),
        ("fx_wavewarp_borde", 2.0),
    ]);
    assert_eq!(run_wavewarp(&gpu, &post, &src, &wild, true), none, "wavewarp with cant=0 changed the frame");
    assert_eq!(run_wavewarp(&gpu, &post, &src, &wild, false), none);
}

#[test]
fn every_wavewarp_float_changes_the_rendered_frame() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);
    let src = fm_source(&gpu);
    // Pin on (centre) and the second layer on, so pin_x/pin_y and the capa2_*
    // floats have something to act on; a tall wave so `borde` reaches past the frame.
    let base_set = [
        ("fx_wavewarp_cant", 0.8),
        ("fx_wavewarp_alto", 0.2),
        ("fx_wavewarp_pin", 6.0),
        ("fx_wavewarp_capa2", 0.5),
        ("fx_wavewarp_cruce", 0.3),
        ("fx_wavewarp_angulo", 30.0),
    ];
    let base = run_wavewarp(&gpu, &post, &src, &fm_advanced(&base_set), true);
    let plain = run_wavewarp(&gpu, &post, &src, &fm_advanced(&[]), false);
    assert_ne!(base, plain, "wavewarp on did not move the frame at all");
    assert!(base.iter().all(|v| v.is_finite()));

    for spec in metal_schema::advanced_effect_floats("wavewarp").unwrap() {
        let here = fm_advanced(&base_set)[metal_schema::effect_index(spec.name).unwrap() - metal_schema::CHAIN_FLOATS];
        let step = (spec.max - spec.min) * 0.37;
        let to = if (spec.max - here) >= (here - spec.min) { here + step } else { here - step };
        let mut set = base_set.to_vec();
        set.push((spec.name, if spec.name == "fx_wavewarp_cant" { 0.3 } else { to }));
        let moved = run_wavewarp(&gpu, &post, &src, &fm_advanced(&set), true);
        assert!(moved.iter().all(|v| v.is_finite()), "{} produced NaN/Inf", spec.name);
        if spec.name == "fx_wavewarp_borde" {
            continue; // needs content at the texture border: checked below
        }
        assert_ne!(moved, base, "{} did not change the rendered frame", spec.name);
    }
}

/// The edge mode only matters where the warp reaches past the source texture,
/// so it needs a model that paints up to the border: some model must tell
/// clamp, mirror and wrap apart.
#[test]
fn the_wavewarp_edge_modes_differ_on_a_frame_that_reaches_the_border() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);
    let set = |b: f32| fm_advanced(&[
        ("fx_wavewarp_cant", 1.0),
        ("fx_wavewarp_alto", 0.4),
        ("fx_wavewarp_cruce", 1.0),
        ("fx_wavewarp_borde", b),
    ]);
    let (w, h) = (
        (W as f32 * metal_render::shared::post::OVERSCAN) as u32,
        (H as f32 * metal_render::shared::post::OVERSCAN) as u32,
    );
    let mut told_apart = false;
    for mut model in all_models() {
        let floats = model.schema().defaults();
        let (_t, view) = offscreen_target(&gpu.device, w, h, FORMAT);
        let ctx = Ctx { device: &gpu.device, queue: &gpu.queue, time: 2.3, width: w, height: h, fondo_alpha: false, detail: 1.0 };
        model.render(&ctx, &floats, &view, FORMAT);
        let clamp = run_wavewarp(&gpu, &post, &view, &set(0.0), true);
        let mirror = run_wavewarp(&gpu, &post, &view, &set(1.0), true);
        let wrap = run_wavewarp(&gpu, &post, &view, &set(2.0), true);
        assert!(mirror.iter().chain(&wrap).all(|v| v.is_finite()));
        if clamp != mirror && mirror != wrap && clamp != wrap {
            told_apart = true;
            break;
        }
    }
    assert!(told_apart, "no model's frame told clamp, mirror and wrap apart");
}

#[test]
fn a_wavewarp_row_moves_the_picture_through_the_renderer_and_a_default_row_does_not() {
    use metal_render::headless::Renderer;
    let mut renderer = Renderer::new();
    let row = renderer.schemas()[0].defaults();
    let at = row.len() - metal_schema::EFFECTS_WIDTH;
    let mut on = row.clone();
    let cant = metal_schema::effect_index("fx_wavewarp_cant").unwrap();
    on[at + cant] = 0.9;
    let draw = |r: &mut Renderer, row: &[f32]| r.solo_chain(0, row, 2.3, 96, 54, false, &[], &EstelaParams::default()).0;
    let off_a = draw(&mut renderer, &row);
    let on_a = draw(&mut renderer, &on);
    assert_ne!(off_a, on_a, "a wavewarp row did not change the picture");
    assert_eq!(off_a, draw(&mut renderer, &row), "the default row is not deterministic");
}


// ---------------------------------------------------------------------------
// fx.ascii -- the frame redrawn as characters (a multi-float chain effect)
// ---------------------------------------------------------------------------

fn ascii_kind() -> u32 {
    metal_schema::advanced_chain_kind("ascii").expect("ascii is an advanced chain effect")
}

/// One post pass over `src_view` with ascii driven by `adv` (its
/// `fx_ascii_cant` is the step amount), or with no chain when `chain` is false.
fn run_ascii(gpu: &Gpu, post: &PostChain, src_view: &wgpu::TextureView, adv: &[f32], chain: bool) -> Vec<f32> {
    let amount = adv[metal_schema::effect_index("fx_ascii_cant").unwrap() - metal_schema::CHAIN_FLOATS];
    let steps = [EffectStep { effect: ascii_kind(), amount, slot: 0 }];
    let (dst_texture, dst_view) = offscreen_target(&gpu.device, W, H, FORMAT);
    post.apply(
        &gpu.device,
        &gpu.queue,
        src_view,
        &dst_view,
        W,
        H,
        &PostParams {
            shake: 0.0,
            light_leak: 0.0,
            leak_color: ColorSlotGpu::solid(0.0, 0.0, 0.0),
            time: 2.3,
            kaleido: 0.0,
            wake: EstelaParams::default(),
            grade: GradeUniform::default(),
            chain_fx: ChainFx::from_advanced(adv),
        },
        if chain { &steps } else { &[] },
        &fallback_ring(src_view),
    );
    readback(gpu, &dst_texture)
}

#[test]
fn ascii_at_zero_amount_leaves_the_frame_bit_identical() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);
    let src = fm_source(&gpu);
    let none = run_ascii(&gpu, &post, &src, &fm_advanced(&[]), false);
    let wild = fm_advanced(&[
        ("fx_ascii_cant", 0.0),
        ("fx_ascii_celda", 4.0),
        ("fx_ascii_rampa", 4.0),
        ("fx_ascii_contraste", 3.0),
        ("fx_ascii_color_modo", 2.0),
        ("fx_ascii_color_mezcla", 1.0),
        ("fx_ascii_fondo", 1.0),
        ("fx_ascii_invierte", 1.0),
        ("fx_ascii_ruido", 1.0),
        ("fx_ascii_parpadeo", 1.0),
        ("fx_ascii_bordes", 1.0),
        ("fx_ascii_deriva", 4.0),
        ("fx_ascii_grosor", 1.0),
    ]);
    assert_eq!(run_ascii(&gpu, &post, &src, &wild, true), none, "ascii with cant=0 changed the frame");
    assert_eq!(run_ascii(&gpu, &post, &src, &wild, false), none);
}

/// Every ascii float changes a rendered frame, each moved 37% of its range.
#[test]
fn every_ascii_float_changes_the_rendered_frame() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);
    let src = fm_source(&gpu);
    let base_set = [("fx_ascii_cant", 0.8)];
    let base = run_ascii(&gpu, &post, &src, &fm_advanced(&base_set), true);
    let plain = run_ascii(&gpu, &post, &src, &fm_advanced(&[]), false);
    assert_ne!(base, plain, "ascii on did not move the frame at all");
    assert!(base.iter().all(|v| v.is_finite()));

    for spec in metal_schema::advanced_effect_floats("ascii").unwrap() {
        let here = fm_advanced(&base_set)[metal_schema::effect_index(spec.name).unwrap() - metal_schema::CHAIN_FLOATS];
        let step = (spec.max - spec.min) * 0.37;
        let to = if (spec.max - here) >= (here - spec.min) { here + step } else { here - step };
        let mut set = base_set.to_vec();
        set.push((spec.name, if spec.name == "fx_ascii_cant" { 0.3 } else { to }));
        let moved = run_ascii(&gpu, &post, &src, &fm_advanced(&set), true);
        assert!(moved.iter().all(|v| v.is_finite()), "{} produced NaN/Inf", spec.name);
        assert_ne!(moved, base, "{} did not change the rendered frame", spec.name);
    }
}

/// The picture really is made of glyph cells: at full amount, mono ink on a
/// black ground, every pixel is one of two levels per cell (ink or ground).
#[test]
fn ascii_at_full_amount_draws_only_ink_and_ground() {
    let gpu = Gpu::new_headless();
    let post = PostChain::new(&gpu.device, FORMAT);
    let src = fm_source(&gpu);
    let adv = fm_advanced(&[
        ("fx_ascii_cant", 1.0),
        ("fx_ascii_color_modo", 0.0),
        ("fx_ascii_color_mezcla", 0.0),
        ("fx_ascii_fondo", 0.0),
    ]);
    let out = run_ascii(&gpu, &post, &src, &adv, true);
    let mut levels: Vec<u32> = out.chunks(4).map(|px| (px[0] * 255.0).round() as u32).collect();
    levels.sort_unstable();
    levels.dedup();
    assert!(levels.len() <= 3, "mono ink on black should be one or two levels, got {levels:?}");
}

#[test]
fn an_ascii_row_moves_the_picture_through_the_renderer_and_a_default_row_does_not() {
    use metal_render::headless::Renderer;
    let mut renderer = Renderer::new();
    let row = renderer.schemas()[0].defaults();
    let at = row.len() - metal_schema::EFFECTS_WIDTH;
    let mut on = row.clone();
    let cant = metal_schema::effect_index("fx_ascii_cant").unwrap();
    on[at + cant] = 0.9;
    let draw = |r: &mut Renderer, row: &[f32]| r.solo_chain(0, row, 2.3, 96, 54, false, &[], &EstelaParams::default()).0;
    let off_a = draw(&mut renderer, &row);
    let on_a = draw(&mut renderer, &on);
    assert_ne!(off_a, on_a, "an ascii row did not change the picture");
    assert_eq!(off_a, draw(&mut renderer, &row), "the default row is not deterministic");
}


// ---------------------------------------------------------------------------
// fx.brush -- the frame repainted from strokes (stateful: it owns a canvas, so
// these render several frames through `Renderer`, not one post pass)
// ---------------------------------------------------------------------------

mod brush {
    use super::*;
    use metal_render::headless::Renderer;

    const TIMES: [f32; 3] = [10.0, 10.04, 10.08];

    /// The model the brush tests paint: the first whose default frame has
    /// real chroma (the colour floats need something to act on).
    fn model(r: &mut Renderer) -> usize {
        static MODEL: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *MODEL.get_or_init(|| {
            let foot = r.footage();
            for i in 0..r.model_count() {
                if foot[i] || !r.runs_here(i) || r.schemas()[i].defaults().len() < metal_schema::EFFECTS_WIDTH {
                    continue;
                }
                let px = r.solo_chain(i, &r.schemas()[i].defaults(), 10.0, 64, 36, false, &[], &EstelaParams::default()).0;
                let chroma: f64 = px
                    .chunks_exact(4)
                    .map(|p| (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) as f64)
                    .sum::<f64>()
                    / (px.len() / 4) as f64;
                if chroma > 20.0 {
                    return i;
                }
            }
            0
        })
    }

    /// The model's default row with the named floats set.
    fn row(r: &Renderer, m: usize, set: &[(&str, f32)]) -> Vec<f32> {
        let mut row = r.schemas()[m].defaults();
        let at = row.len() - metal_schema::EFFECTS_WIDTH;
        for (name, v) in set {
            row[at + metal_schema::effect_index(name).unwrap_or_else(|| panic!("no float {name}"))] = *v;
        }
        row
    }

    /// The frames at `times` for `set`. A start more than a second from where
    /// the previous call left the renderer is a seek, so a call that starts
    /// at `TIMES[0]` begins from a fresh canvas.
    fn frames(r: &mut Renderer, set: &[(&str, f32)], times: &[f32]) -> Vec<Vec<u8>> {
        let m = model(r);
        let row = row(r, m, set);
        times
            .iter()
            .map(|t| r.solo_chain(m, &row, *t, 96, 54, false, &[], &EstelaParams::default()).0)
            .collect()
    }

    fn last(r: &mut Renderer, set: &[(&str, f32)]) -> Vec<u8> {
        frames(r, set, &TIMES).pop().unwrap()
    }

    fn mse(a: &[u8], b: &[u8]) -> f64 {
        a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len() as f64
    }

    #[test]
    fn brush_at_zero_amount_leaves_the_frame_bit_identical_and_allocates_nothing() {
        let mut r = Renderer::new();
        let plain = last(&mut r, &[]);
        let wild = last(
            &mut r,
            &[
                ("fx_brush_cant", 0.0),
                ("fx_brush_forma", 4.0),
                ("fx_brush_n_trazos", 4096.0),
                ("fx_brush_tam_ini", 8.0),
                ("fx_brush_alfa", 1.0),
                ("fx_brush_modo", 1.0),
                ("fx_brush_contorno", 1.0),
                ("fx_brush_papel", 1.0),
                ("fx_brush_decae", 1.0),
            ],
        );
        assert_eq!(wild, plain, "brush with cant=0 changed the frame");
        assert_eq!(r.brush_canvases(), 0, "an off brush must not hold a canvas");
    }

    #[test]
    fn turning_brush_off_releases_its_canvas() {
        let mut r = Renderer::new();
        last(&mut r, &[("fx_brush_cant", 1.0)]);
        assert_eq!(r.brush_canvases(), 1);
        last(&mut r, &[]);
        assert_eq!(r.brush_canvases(), 0);
    }

    #[test]
    fn brush_on_repaints_the_frame_and_stays_finite() {
        let mut r = Renderer::new();
        let plain = last(&mut r, &[]);
        let painted = last(&mut r, &[("fx_brush_cant", 1.0)]);
        assert_ne!(painted, plain, "brush on did not touch the frame");
        assert!(painted.chunks_exact(4).any(|px| px[0] != 0 || px[1] != 0 || px[2] != 0), "the painting is black");
        // Half amount sits between the two.
        let half = last(&mut r, &[("fx_brush_cant", 0.5)]);
        assert!(mse(&half, &plain) < mse(&painted, &plain), "cant 0.5 is not closer to the frame than cant 1");
    }

    /// Strokes are placed by hashes, so the same instants replay the same pixels,
    /// including after a seek back (which repaints from scratch).
    #[test]
    fn a_replayed_sequence_paints_the_same_pixels() {
        let mut r = Renderer::new();
        let set = [("fx_brush_cant", 1.0), ("fx_brush_cobertura", 0.2)];
        let a = frames(&mut r, &set, &TIMES);
        let b = frames(&mut r, &set, &TIMES);
        assert_eq!(a, b);
        let mut cold = Renderer::new();
        assert_eq!(frames(&mut cold, &set, &TIMES), a, "a cold renderer paints differently");
    }

    #[test]
    fn asking_for_the_same_instant_again_paints_nothing_more() {
        let mut r = Renderer::new();
        let set = [("fx_brush_cant", 1.0), ("fx_brush_cobertura", 0.2)];
        let f = frames(&mut r, &set, &[10.0, 10.04, 10.04, 10.04]);
        assert_eq!(f[1], f[2]);
        assert_eq!(f[2], f[3]);
        assert_ne!(f[0], f[1], "a new instant must paint more");
    }

    /// The canvas persists: with no renewal (`fx_brush_renueva` at 0) and few strokes per frame the next
    /// frame is mostly the last one's strokes, unlike a repaint at that instant.
    #[test]
    fn strokes_accumulate_on_a_persistent_canvas() {
        let mut r = Renderer::new();
        let set = [("fx_brush_cant", 1.0), ("fx_brush_cobertura", 0.02), ("fx_brush_renueva", 0.0)];
        let f = frames(&mut r, &set, &[10.0, 10.04, 10.08]);
        let fresh_later = frames(&mut r, &set, &[10.08]).pop().unwrap();
        assert!(mse(&f[1], &f[0]) < mse(&fresh_later, &f[0]), "frame 2 is not built on frame 1's canvas");
    }

    #[test]
    fn the_painting_resembles_the_frame_better_than_blank_paper() {
        let mut r = Renderer::new();
        let plain = last(&mut r, &[]);
        let blank = last(
            &mut r,
            &[("fx_brush_cant", 1.0), ("fx_brush_alfa", 0.05), ("fx_brush_papel", 1.0), ("fx_brush_cobertura", 0.02)],
        );
        let dab = last(&mut r, &[("fx_brush_cant", 1.0), ("fx_brush_papel", 1.0)]);
        assert!(mse(&dab, &plain) < mse(&blank, &plain), "dab painting is no closer to the frame than paper");
        if r.shared_gpu().compute {
            let search = last(&mut r, &[("fx_brush_cant", 1.0), ("fx_brush_papel", 1.0), ("fx_brush_modo", 1.0)]);
            assert!(mse(&search, &plain) < mse(&blank, &plain), "search painting is no closer to the frame than paper");
        }
    }

    const BASE: [(&str, f32); 4] = [
        ("fx_brush_cant", 0.9),
        ("fx_brush_cobertura", 0.5),
        ("fx_brush_contorno", 0.3),
        ("fx_brush_papel", 0.5),
    ];

    /// Move each float 37% of its range toward the farther end and the last of
    /// three frames must change (`candidatos` and `modo` have their own bases).
    fn every_float_moves(base: &[(&str, f32)], skip: &[&str]) {
        let mut r = Renderer::new();
        let reference = last(&mut r, base);
        let specs = metal_schema::advanced_effect_floats("brush").unwrap();
        for spec in specs {
            if skip.contains(&spec.name) {
                continue;
            }
            let here = base.iter().find(|(n, _)| *n == spec.name).map(|(_, v)| *v).unwrap_or(spec.default);
            let step = (spec.max - spec.min) * 0.37;
            let to = if (spec.max - here) >= (here - spec.min) { here + step } else { here - step };
            let mut set = base.to_vec();
            // `cant` stays on (0.3); `modo` is a switch, so it goes all the way.
            let to = match spec.name {
                "fx_brush_cant" => 0.3,
                "fx_brush_modo" => 1.0 - here,
                _ => to,
            };
            set.push((spec.name, to));
            let moved = last(&mut r, &set);
            assert_ne!(moved, reference, "{} did not change the rendered frame", spec.name);
        }
    }

    #[test]
    fn every_brush_float_changes_the_rendered_frame_in_dab_mode() {
        // `candidatos` only exists in search mode.
        every_float_moves(&BASE, &["fx_brush_candidatos"]);
    }

    #[test]
    fn every_brush_float_changes_the_rendered_frame_in_search_mode() {
        let r = Renderer::new();
        if !r.shared_gpu().compute {
            return;
        }
        let mut base = BASE.to_vec();
        base.push(("fx_brush_modo", 1.0));
        every_float_moves(&base, &[]);
    }

    #[test]
    fn dab_and_search_paint_differently_and_each_shape_is_distinct() {
        let mut r = Renderer::new();
        let dab = last(&mut r, &BASE);
        let mut s = BASE.to_vec();
        s.push(("fx_brush_modo", 1.0));
        if r.shared_gpu().compute {
            assert_ne!(last(&mut r, &s), dab);
        }
        let shape = |r: &mut Renderer, f: f32| {
            let mut set = BASE.to_vec();
            set.push(("fx_brush_forma", f));
            last(r, &set)
        };
        let shapes: Vec<_> = (0..5).map(|f| shape(&mut r, f as f32)).collect();
        for i in 0..5 {
            for j in i + 1..5 {
                assert_ne!(shapes[i], shapes[j], "forma {i} and {j} paint the same");
            }
        }
    }

    /// Every float at both ends, both modes: nothing crashes or goes black.
    #[test]
    fn brush_at_its_extremes_still_renders_a_live_frame() {
        let mut r = Renderer::new();
        for mode in [0.0, 1.0] {
            for extreme in [false, true] {
                let mut set: Vec<(&str, f32)> = Vec::new();
                for spec in metal_schema::advanced_effect_floats("brush").unwrap() {
                    set.push((spec.name, if extreme { spec.max } else { spec.min }));
                }
                set.retain(|(n, _)| *n != "fx_brush_modo" && *n != "fx_brush_cant");
                set.push(("fx_brush_cant", 1.0));
                set.push(("fx_brush_modo", mode));
                let f = frames(&mut r, &set, &TIMES);
                let px = f.last().unwrap();
                assert!(px.chunks_exact(4).any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0), "modo {mode} extreme {extreme}: black");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// fx.hardstutter -- the stateful chain effect (its own pass and held frames,
// see `shared::stutter`); tested through `Renderer`, over sequences of frames
// ---------------------------------------------------------------------------

type Set<'a> = &'a [(&'a str, f32)];

/// A row of model `index` with the given effect floats written into its tail.
fn stutter_row(renderer: &metal_render::headless::Renderer, index: usize, set: Set) -> Vec<f32> {
    let mut row = renderer.schemas()[index].defaults();
    if row.len() < metal_schema::EFFECTS_WIDTH {
        return row; // a retired model's tombstone
    }
    let at = row.len() - metal_schema::EFFECTS_WIDTH;
    for (name, v) in set {
        row[at + metal_schema::effect_index(name).unwrap_or_else(|| panic!("no float {name}"))] = *v;
    }
    row
}

/// A fresh renderer's frames of `row` at each of `times`, in order.
fn stutter_run(index: usize, set: Set, times: &[f32]) -> Vec<Vec<u8>> {
    let mut renderer = metal_render::headless::Renderer::new();
    let row = stutter_row(&renderer, index, set);
    times
        .iter()
        .map(|t| renderer.solo_chain(index, &row, *t, 64, 36, false, &[], &EstelaParams::default()).0)
        .collect()
}

/// A model whose picture moves with time (a stutter on a still is invisible).
fn moving_model() -> usize {
    let mut renderer = metal_render::headless::Renderer::new();
    for index in 0..renderer.model_count() {
        let row = stutter_row(&renderer, index, &[]);
        let a = renderer.solo_chain(index, &row, 0.01, 64, 36, false, &[], &EstelaParams::default()).0;
        let b = renderer.solo_chain(index, &row, 0.30, 64, 36, false, &[], &EstelaParams::default()).0;
        if a != b {
            return index;
        }
    }
    panic!("no model moves between 0.01 s and 0.30 s");
}

/// 120 bpm, 1/16 notes: a 0.125 s step, the gate open for its first half.
const STUTTER_BASE: Set = &[
    ("fx_hardstutter_cant", 1.0),
    ("fx_hardstutter_tempo", 120.0),
    ("fx_hardstutter_ritmo", 3.0),
    ("fx_hardstutter_duty", 0.5),
];

#[test]
fn hardstutter_at_zero_amount_leaves_the_frame_bit_identical() {
    let m = moving_model();
    let times: Vec<f32> = (0..24).map(|i| i as f32 * 0.037).collect();
    let off = stutter_run(m, &[], &times);
    let wild: Set = &[
        ("fx_hardstutter_cant", 0.0),
        ("fx_hardstutter_ritmo", 5.0),
        ("fx_hardstutter_duty", 0.05),
        ("fx_hardstutter_congela", 0.0),
        ("fx_hardstutter_repite_n", 8.0),
        ("fx_hardstutter_jitter", 1.0),
        ("fx_hardstutter_patron", 4.0),
        ("fx_hardstutter_desvanece", 1.0),
        ("fx_hardstutter_tinte", 1.0),
    ];
    assert_eq!(stutter_run(m, wild, &times), off, "cant 0 changed the picture");

    // And turning it off after it ran leaves no state behind: the next frames
    // are exactly a renderer that never stuttered.
    let mut renderer = metal_render::headless::Renderer::new();
    let on = stutter_row(&renderer, m, STUTTER_BASE);
    let row = stutter_row(&renderer, m, &[]);
    let draw = |r: &mut metal_render::headless::Renderer, row: &[f32], t: f32| {
        r.solo_chain(m, row, t, 64, 36, false, &[], &EstelaParams::default()).0
    };
    for t in [0.0, 0.03, 0.09] {
        draw(&mut renderer, &on, t);
    }
    assert_eq!(draw(&mut renderer, &row, 0.12), off_at(m, 0.12));
}

fn off_at(m: usize, t: f32) -> Vec<u8> {
    stutter_run(m, &[], &[t]).remove(0)
}

#[test]
fn hardstutter_holds_the_last_visible_frame_while_the_gate_is_shut() {
    let m = moving_model();
    // 0.0 and 0.03 are inside the open half; 0.09 and 0.11 inside the shut one.
    let on = stutter_run(m, STUTTER_BASE, &[0.0, 0.03, 0.09, 0.11, 0.126]);
    let off = stutter_run(m, &[], &[0.0, 0.03, 0.09, 0.11, 0.126]);
    assert_eq!(on[0], off[0], "the first frame is live");
    assert_eq!(on[1], off[1], "an open gate is the live frame");
    assert_eq!(on[2], on[1], "a shut gate shows the last visible frame");
    assert_eq!(on[3], on[1], "and keeps showing it");
    assert_ne!(on[2], off[2], "while the picture moves on underneath");
    assert_eq!(on[4], off[4], "the next step opens the gate again");
}

#[test]
fn hardstutter_asking_for_one_instant_twice_gives_one_picture() {
    let m = moving_model();
    let seq = stutter_run(m, STUTTER_BASE, &[0.0, 0.03, 0.09, 0.09, 0.126, 0.126]);
    assert_eq!(seq[2], seq[3]);
    assert_eq!(seq[4], seq[5]);
}

#[test]
fn hardstutter_cold_start_in_a_shut_gate_shows_the_live_frame() {
    let m = moving_model();
    let on = stutter_run(m, STUTTER_BASE, &[0.09]);
    assert_eq!(on[0], off_at(m, 0.09), "nothing was held yet, so nothing is replaced");
    // A seek empties the hold the same way.
    let seek = stutter_run(m, STUTTER_BASE, &[0.0, 0.03, 5.09]);
    assert_eq!(seek[2], off_at(m, 5.09));
}

#[test]
fn hardstutter_congela_zero_hides_to_black_and_a_loop_replays_older_frames() {
    let m = moving_model();
    let times = [0.0, 0.02, 0.04, 0.06, 0.08, 0.1, 0.12];
    let black: Set = &[
        ("fx_hardstutter_cant", 1.0),
        ("fx_hardstutter_tempo", 120.0),
        ("fx_hardstutter_ritmo", 3.0),
        ("fx_hardstutter_duty", 0.5),
        ("fx_hardstutter_congela", 0.0),
    ];
    let seq = stutter_run(m, black, &times);
    assert!(seq[5].chunks(4).all(|px| px[..3] == [0, 0, 0]), "a shut gate with congela 0 is black");
    assert!(seq[0].chunks(4).any(|px| px[..3] != [0, 0, 0]), "an open gate is not");

    // A loop of 3 over a long shut span shows frames the freeze never does.
    let long: Set = &[
        ("fx_hardstutter_cant", 1.0),
        ("fx_hardstutter_tempo", 60.0),
        ("fx_hardstutter_ritmo", 0.0),
        ("fx_hardstutter_duty", 0.5),
        ("fx_hardstutter_repite_n", 3.0),
    ];
    let frozen: Set = &[
        ("fx_hardstutter_cant", 1.0),
        ("fx_hardstutter_tempo", 60.0),
        ("fx_hardstutter_ritmo", 0.0),
        ("fx_hardstutter_duty", 0.5),
    ];
    // Open for 0..0.5 s (frames every 0.05), shut 0.5..1.0 s.
    let times: Vec<f32> = (0..20).map(|i| i as f32 * 0.05).collect();
    let looped = stutter_run(m, long, &times);
    let freeze = stutter_run(m, frozen, &times);
    let shut = &freeze[11..19];
    assert!(shut.windows(2).all(|w| w[0] == w[1]), "a freeze shows one frame for the whole span");
    let shut_loop = &looped[11..19];
    assert!(shut_loop.windows(2).any(|w| w[0] != w[1]), "a loop of 3 cycles through different frames");
}

#[test]
fn every_hardstutter_float_changes_a_rendered_sequence() {
    let m = moving_model();
    let base_set: Vec<(&str, f32)> = vec![
        ("fx_hardstutter_cant", 0.8),
        ("fx_hardstutter_tempo", 100.0),
        ("fx_hardstutter_ritmo", 3.0),
        ("fx_hardstutter_duty", 0.5),
        ("fx_hardstutter_tinte", 0.5),
        ("fx_hardstutter_repite_n", 0.0),
    ];
    let times: Vec<f32> = (0..120).map(|i| i as f32 * 0.0137).collect();
    let base = stutter_run(m, &base_set, &times);
    let plain = stutter_run(m, &[], &times);
    assert_ne!(base, plain, "hardstutter on did not change the sequence at all");

    for spec in metal_schema::advanced_effect_floats("hardstutter").unwrap() {
        let here = base_set.iter().find(|(n, _)| *n == spec.name).map_or(spec.default, |(_, v)| *v);
        let step = (spec.max - spec.min) * 0.37;
        let mut to = if (spec.max - here) >= (here - spec.min) { here + step } else { here - step };
        match spec.name {
            "fx_hardstutter_invierte_ritmo" => to = 1.0,
            "fx_hardstutter_repite_n" => to = 4.0,
            "fx_hardstutter_cant" => to = 0.3,
            "fx_hardstutter_tinte" => to = 0.1,
            _ => {}
        }
        let mut set = base_set.clone();
        set.retain(|(n, _)| *n != spec.name);
        set.push((spec.name, to));
        let moved = stutter_run(m, &set, &times);
        assert_ne!(moved, base, "{} did not change the rendered sequence", spec.name);
    }
}

#[test]
fn hardstutter_takes_a_tile_of_the_grid_without_disturbing_its_neighbours() {
    // `grid` draws legacy-order rows into tiles of one canvas: a stutter row
    // lands in its own tile and the other tile is what it always was.
    let mut renderer = metal_render::headless::Renderer::new();
    let m = moving_model();
    let plain: Vec<Vec<f32>> = (0..renderer.model_count()).map(|i| stutter_row(&renderer, i, &[])).collect();
    let mut with = plain.clone();
    with[m] = stutter_row(&renderer, m, STUTTER_BASE);
    let a = renderer.grid(&plain, 0.09, 32, 32, 8, 0, 0).0;
    let _ = renderer.grid(&with, 0.03, 32, 32, 8, 0, 0);
    let b = renderer.grid(&with, 0.09, 32, 32, 8, 0, 0).0;
    let w = 8 * 32 * 4;
    let tile = |img: &[u8], i: usize| -> Vec<u8> {
        let (cx, cy) = ((i % 8) * 32 * 4, (i / 8) * 32);
        (0..32).flat_map(|y| img[(cy + y) * w + cx..(cy + y) * w + cx + 32 * 4].to_vec()).collect()
    };
    let other = (0..plain.len()).find(|i| *i != m && !plain[*i].is_empty()).unwrap();
    assert_eq!(tile(&a, other), tile(&b, other), "a neighbouring tile changed");
    assert_ne!(tile(&a, m), tile(&b, m), "the stuttering tile did not hold");
}
