//! G3 — the contour clip, overlays and mask passes keep the frame on the GPU.
//! The CPU backend is the reference every case here is diffed against; the
//! `measure_*` test (ignored) prints the rule-11 numbers.

use crate::backend::Backend;
use crate::engine::{render_device, render_device_frame, DeviceFrame, State};
use crate::layers::{self, Blend, Layer};
use crate::output::{Mask, Output};
use crate::paint::Paint;

fn painter(b: Backend) -> Box<dyn Paint> {
    Box::new(crate::paint::Gpu::with_backend(metal_render::headless::Renderer::new(), b))
}

pub(crate) fn diamond(id: &str, alpha: f32) -> Mask {
    Mask {
        id: id.into(),
        name: id.into(),
        points: vec![[0.5, 0.1], [0.9, 0.5], [0.5, 0.9], [0.1, 0.5]],
        alpha,
        ..Mask::default()
    }
}

/// What a case arms on top of three plain layers.
#[derive(Clone, Copy, Default)]
pub(crate) struct Case {
    pub clip: bool,
    pub solid: bool,
    pub solid_masks: bool,
    pub grid: bool,
}

pub(crate) struct Rig {
    pub paint: Box<dyn Paint>,
    pub sim: crate::listeners::Listeners,
    pub state: State,
    pub device: String,
}

pub(crate) fn rig(b: Backend, case: Case, w: u32, h: u32, tag: &str) -> Rig {
    let paint = painter(b);
    let sim = crate::listeners::Listeners::stand_in();
    let mut state = State::new(paint.as_ref());
    state.paused_at = Some(3.0);
    let device = format!("g3-{tag}-{}-{}", b.as_str(), std::process::id());
    let mut out = Output::new(&device, "g3", w, h);
    out.masks = vec![diamond("m1", 1.0), diamond("m2", 0.5)];
    out.masks[1].points = vec![[0.05, 0.05], [0.3, 0.05], [0.3, 0.3], [0.05, 0.3]];
    state.outputs.list.push(out);
    let mut list = Vec::new();
    for (i, (blend, opacity)) in [(Blend::Normal, 1.0), (Blend::Add, 0.6), (Blend::Screen, 0.8)].into_iter().enumerate() {
        let mut l = Layer::new(&format!("g3-{tag}-{i}"), "l", i);
        l.outputs = vec![device.clone()];
        l.blend = blend;
        l.opacity = opacity;
        list.push(l);
    }
    state.layers = list;
    state.sources.insert(device.clone(), layers::Device::default());
    if case.clip {
        // A contour binds through a `mask` node, so the device gets a graph:
        // three sources into the output, the middle one through the mask.
        use metal_graph::{Domain, Endpoint, Graph};
        let ids = paint.ids();
        let mut graph = Graph::new(&crate::engine::device_graph_id(&device), &device, Domain::Live);
        let out_node = graph.add_node("out", "output.device", [800.0, 0.0]);
        graph.add_node("comp", "compose.layers", [-200.0, 0.0]);
        for i in 0..3 {
            let gen = graph.add_node(&format!("gen{i}"), &format!("gen.{}", ids[i % ids.len()]), [0.0, 100.0 * i as f32]);
            graph.node_mut(&gen).unwrap().config =
                metal_graph::Config::from_json_str(&serde_json::json!({ "source_id": format!("g3-{tag}-{i}") }).to_string()).unwrap();
            if i == 1 {
                let mask = graph.add_node("mask", "mask", [200.0, 100.0]);
                graph.node_mut(&mask).unwrap().config = metal_graph::Config::from_json_str(
                    &serde_json::json!({ "device": device, "mask": "m1" }).to_string(),
                )
                .unwrap();
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mask, "image"));
                graph.add_edge(Endpoint::new(&mask, "image"), Endpoint::new(&out_node, "image"));
            } else {
                graph.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out_node, "image"));
            }
        }
        crate::engine::save_graph_at(&crate::output::data_root(), &graph).unwrap();
    }
    let t = state.time();
    if case.solid_masks {
        state.overlays.start_solid_masks(device.clone(), [200, 20, 20], vec![("m2".into(), [10, 220, 30])], t, 600.0);
    } else if case.solid {
        state.overlays.start_solid_color(device.clone(), [200, 20, 20], t, 600.0);
    }
    if case.grid {
        state.overlays.start_test_pattern(device.clone(), [20, 60, 220], t, 600.0);
    }
    Rig { paint, sim, state, device }
}

impl Rig {
    pub(crate) fn frame(&mut self, w: u32, h: u32) -> DeviceFrame {
        render_device_frame(&mut *self.paint, &mut self.sim, &mut self.state, &self.device, w, h, &mut |_| None)
    }
    pub(crate) fn bytes(&mut self, w: u32, h: u32) -> Vec<u8> {
        render_device(&mut *self.paint, &mut self.sim, &mut self.state, &self.device, w, h).0
    }
}

#[test]
#[ignore = "prints the G3 rule-11 numbers: cargo test -p metal-perform --features gpu --release --lib g3_tests::measure -- --ignored --nocapture"]
fn measure() {
    for (name, case) in [
        ("plain", Case::default()),
        ("clip", Case { clip: true, ..Case::default() }),
        ("solid+mask tint", Case { solid_masks: true, ..Case::default() }),
        ("clip+test pattern", Case { clip: true, grid: true, ..Case::default() }),
    ] {
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            let mut r = rig(Backend::Gpu, case, w, h, "m");
            for _ in 0..3 {
                let _ = r.frame(w, h);
            }
            let n = 12;
            let t = std::time::Instant::now();
            let mut on_gpu = false;
            for _ in 0..n {
                let f = r.frame(w, h);
                on_gpu = matches!(f, DeviceFrame::Gpu(_));
                if let (DeviceFrame::Gpu(_), Some(cx)) = (&f, r.paint.gpu_cx()) {
                    let _ = cx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
                }
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
            println!("MEASURE {name:<18} {w}x{h}: {ms:7.1} ms/frame  (texture out: {on_gpu})");
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = metal_vfs::remove_file(crate::engine::graph_path_at(
            &crate::output::data_root(),
            &crate::engine::device_graph_id(&self.device),
        ));
    }
}

fn stats(a: &[u8], b: &[u8]) -> (f64, u8) {
    let sum: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    (sum as f64 / a.len().max(1) as f64, a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0))
}

/// One case on both backends: same picture within tolerance, and the Gpu one
/// never left the GPU.
fn same(name: &str, case: Case) {
    let (w, h) = (96u32, 54u32);
    let mut cpu = rig(Backend::Cpu, case, w, h, name);
    let mut gpu = rig(Backend::Gpu, case, w, h, name);
    let want = cpu.bytes(w, h);
    let on_gpu = matches!(gpu.frame(w, h), DeviceFrame::Gpu(_));
    let got = gpu.bytes(w, h);
    assert!(on_gpu, "{name}: stayed a texture");
    let (mean, max) = stats(&want, &got);
    assert!(mean < 0.8 && max <= 6, "{name}: mean {mean:.3} max {max}");
    assert!(want.chunks_exact(4).any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0), "{name}: a real picture");
}

#[test]
fn a_contour_clip_composes_on_the_gpu() {
    same("clip", Case { clip: true, ..Case::default() });
}

#[test]
fn a_solid_overlay_and_its_mask_colours_stay_on_the_gpu() {
    same("solid", Case { solid: true, ..Case::default() });
    same("solidm", Case { solid_masks: true, ..Case::default() });
}

#[test]
fn a_test_pattern_and_a_clip_together_stay_on_the_gpu() {
    same("pattern", Case { clip: true, grid: true, ..Case::default() });
}

#[test]
fn the_compose_shader_warps_like_the_cpu() {
    use crate::compose::{compose_warped, Tile, Warp};
    use crate::gpu_compose::{tests::picture, GpuCompose, GpuCx};
    use crate::gpu_frame::{GpuTile, GpuWarp, TileSource};
    use crate::layers::{Composition, Sector};
    let g = metal_render::gpu::Gpu::new_headless();
    let cx = GpuCx::new(g.device, g.queue);
    let (w, h, tw, th) = (160u32, 90u32, 64u32, 36u32);
    let px = picture(tw, th, 3);
    // A mild perspective: a quad pulled in at the top.
    let hom = crate::escena::Homography([0.9, 0.1, 0.05, 0.05, 0.85, 0.08, 0.05, 0.1, 1.0]);
    for mode in [Composition::Overlap, Composition::Sectors] {
        let warp = Warp::new(hom);
        let tile = Tile {
            pixels: &px,
            w: tw,
            h: th,
            sector: Sector::default(),
            opacity: 0.9,
            blend: Blend::Normal,
            turn: 0.0,
            giro_mode: Default::default(),
            clip: None,
            masks: &[],
        };
        let want = compose_warped(w, h, mode, &[tile], &[Some(warp)]);
        let m = crate::compose::warp_matrix(&warp, tw, th, w, h).expect("invertible");
        let f = cx.upload(&px, tw, th);
        let gt = GpuTile {
            source: TileSource::Texture(&f),
            sector: Sector::default(),
            opacity: 0.9,
            blend: Blend::Normal,
            turn: 0.0,
            giro_mode: Default::default(),
            cover: None,
            warp: Some(GpuWarp { m, nearest: false }),
        };
        let out = GpuCompose::new(&cx).compose(w, h, mode, &[gt]);
        let got = cx.readback(&out);
        let (mean, max) = stats(&want, &got);
        assert!(mean < 0.8 && max <= 8, "{mode:?}: mean {mean:.3} max {max}");
    }
}

#[test]
fn the_mask_cut_and_strokes_match_apply_masks_with() {
    use crate::layers::Composition;
    let (w, h) = (96u32, 54u32);
    let mut gpu = rig(Backend::Gpu, Case::default(), w, h, "cut");
    let mut a = diamond("a", 1.0);
    a.stroke = Some(crate::output::MaskStroke { width: 4.0, color: Some([0.0, 1.0, 0.0, 0.8]), source: None });
    let masks = vec![a, diamond("b", 0.5)];
    let mut masks = masks;
    masks[1].points = vec![[0.0, 0.0], [0.3, 0.0], [0.3, 0.3], [0.0, 0.3]];
    let base = crate::gpu_compose::tests::picture(w, h, 5);
    let mut want = base.clone();
    crate::output::apply_masks_with(&mut want, w, h, &masks, &[], Composition::Overlap);
    let cx = gpu.paint.gpu_cx().expect("gpu");
    let f = cx.upload(&base, w, h);
    let out = crate::output::apply_masks_gpu(&mut *gpu.paint, &f, &masks, &[], Composition::Overlap, &Default::default(), true)
        .expect("shades");
    let got = cx.readback(&out);
    let (mean, max) = stats(&want, &got);
    assert!(mean < 0.8 && max <= 6, "mean {mean:.3} max {max}");
    assert_ne!(want, base, "the cut must change the picture");
}

#[test]
#[ignore = "prints the G3 sink numbers: cargo test -p metal-perform --features gpu --release --lib g3_tests::measure_sink -- --ignored --nocapture"]
fn measure_sink() {
    for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
        let mut r = rig(Backend::Gpu, Case { clip: true, solid_masks: true, ..Case::default() }, w, h, "s");
        let n = 15;
        for _ in 0..3 {
            let _ = r.bytes(w, h);
        }
        let t = std::time::Instant::now();
        for _ in 0..n {
            let _ = r.bytes(w, h);
        }
        let blocking = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        let t = std::time::Instant::now();
        let mut got = 0;
        for _ in 0..n {
            if let DeviceFrame::Gpu(f) = r.frame(w, h) {
                if r.paint.readback_async("sink", &f).is_some() {
                    got += 1;
                }
            }
        }
        let asynch = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("MEASURE sink {w}x{h}: frame loop blocking readback {blocking:.1} ms, async readback {asynch:.1} ms ({got}/{n} frames delivered)");
    }
}

/// G5: an `fx.grade` (a baked effect, no pass of its own) after a generator
/// keeps the generator's texture: no readback, the same pixels as the CPU
/// reference path, so the grade node's own picture is a texture too.
#[test]
fn a_grade_node_after_a_generator_stays_a_texture() {
    use metal_graph::{Domain, Endpoint, Graph};
    let mut g = Graph::new("g", "t", Domain::Live);
    let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
    let out = g.add_node("out", "output.device", [3.0, 0.0]);
    g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
    metal_graph::effects::set_effect(
        &mut g,
        &gen,
        "grade",
        metal_schema::EffectState::Fixed(0.0),
        metal_graph::effects::ControlSource::Row,
    )
    .expect("materializes");
    let grade = metal_graph::effects::chain(&g, &gen)
        .into_iter()
        .find(|id| metal_graph::effects::effect_name(&g, id).as_deref() == Some("grade"))
        .expect("the grade node");
    crate::bind::set_config_number(&mut g, &grade, "grade_exposure", 60.0);
    let mut p = painter(Backend::Gpu);
    let rows: std::collections::HashMap<metal_graph::NodeId, Vec<f32>> =
        [(gen.clone(), p.schemas()[0].defaults())].into_iter().collect();
    let plan = g.compile();
    let buf = plan.ops.iter().find(|op| op.node == grade).and_then(|op| op.buffer).expect("grade buffer");
    let (no_backdrop, no_time, no_size) = (
        std::collections::HashMap::<metal_graph::NodeId, bool>::new(),
        std::collections::HashMap::<metal_graph::NodeId, f32>::new(),
        std::collections::HashMap::<metal_graph::NodeId, (u32, u32)>::new(),
    );
    let on_gpu = p.run_full(&g, &plan, &rows, &no_backdrop, &[], 1.0, 64, 36, &no_time, &no_size, true);
    let frame = on_gpu.textures.get(&buf).expect("the grade picture is a texture");
    let texture = p.readback(frame);
    assert_eq!(on_gpu.readbacks, 0, "no readback between the generator and the grade");
    let on_cpu = p.run_full(&g, &plan, &rows, &no_backdrop, &[], 1.0, 64, 36, &no_time, &no_size, false);
    assert_eq!(texture, on_cpu.frames.get(&buf).expect("cpu grade picture").0, "same pixels as the CPU path");
}
