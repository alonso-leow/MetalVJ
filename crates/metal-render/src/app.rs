//! The windowed app: a live-rendering surface on the left, an egui panel
//! generated straight from the active model's `Schema` on the right. This
//! is the Dashboard/Studio in one window for now — picking a model and
//! dragging its sliders is "the Studio configures, the Dashboard runs it
//! live" collapsed into a single view, which is enough to see all ten
//! renderers taking floats and becoming pictures. Splitting Studio and
//! Dashboard into their own surfaces is a later step, not a blocker for
//! this one.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::gpu::{offscreen_target, Gpu};
use crate::model::{Block, Ctx, Model};
use crate::shared::post::{ChainFx, EstelaParams, GradeUniform, PostChain, PostParams};
use crate::shared::trail::fallback_ring;
use crate::shared::ColorSlotGpu;
use crate::{preset, registry};
use metal_schema::EFFECTS_VERSION;

const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

struct Rendering {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    post: PostChain,
    offscreen: (wgpu::Texture, wgpu::TextureView, u32, u32),
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
}

/// Everything the panel reads and writes. Kept separate from `Rendering` so
/// the two can be borrowed independently inside `redraw` — `egui_ctx.run_ui`
/// needs `&mut Rendering` for its own bookkeeping at the same time the
/// closure needs `&mut UiState` for the widgets, and a single `App` struct
/// holding both directly cannot satisfy both borrows at once.
struct UiState {
    models: Vec<Box<dyn Model>>,
    model_floats: Vec<Vec<f32>>,
    selected: usize,
    shake: f32,
    /// Mirrors the frame is folded into; below 2 is off. The window app has
    /// no ghosting — a trail needs a history per model and this path draws
    /// one model to the surface — but the fold is stateless and free here.
    kaleido: f32,
    light_leak: f32,
    leak_color: [f32; 12],
    // Six radiance picks (2026-08-25) — see `Post`'s field comments in
    // `metal-schema` for what each does.
    crt: f32,
    glitch: f32,
    posterize: f32,
    zoom: f32,
    fire: f32,
    lorenz: f32,
    // Four more, added the same way — see `Post`'s field comments in
    // `metal-schema`.
    vignette: f32,
    blur: f32,
    sharpen: f32,
    grain: f32,
    preset_name: String,
    status: String,
}

impl UiState {
    /// The shared effects this window's sliders currently ask for, as the
    /// same `Post` both a saved preset and the live post chain each frame
    /// need — built fresh from `self` rather than kept as one, since a
    /// slider frame has fifteen scattered fields and only one place
    /// (`redraw`, and the save button here) ever wants them assembled.
    ///
    /// This window has sliders, not a node graph, so `chain` is always
    /// empty: there is no order for it to record beyond the one
    /// `Post::resolved_chain` already derives from the fields below.
    fn post(&self) -> preset::Post {
        preset::Post {
            version: EFFECTS_VERSION,
            shake: self.shake,
            light_leak: self.light_leak,
            leak_color: self.leak_color.to_vec(),
            // Ghosting is a headless-path effect: it needs one frame history
            // per model and this window draws whichever model is selected
            // straight to the surface. A preset saved here records it off.
            ghost: 0.0,
            kaleido: self.kaleido,
            crt: self.crt,
            glitch: self.glitch,
            posterize: self.posterize,
            zoom: self.zoom,
            fire: self.fire,
            lorenz: self.lorenz,
            vignette: self.vignette,
            blur: self.blur,
            sharpen: self.sharpen,
            grain: self.grain,
            // This window has sliders, not a node graph — there is no order
            // for it to record, so the preset it saves says "the fixed one".
            chain: Vec::new(),
            // The advanced effects have no sliders here; they start at their
            // declared defaults, the same a fresh `Post` does.
            advanced: metal_schema::Post::default().advanced,
        }
    }

    fn build(&mut self, ui: &mut egui::Ui) {
        ui.heading("metal-render — floats by hand");
        ui.separator();

        let names: Vec<&'static str> = self.models.iter().map(|m| m.name()).collect();
        egui::ComboBox::from_label("model")
            .selected_text(names[self.selected])
            .show_ui(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    ui.selectable_value(&mut self.selected, i, *name);
                }
            });

        ui.separator();
        ui.label("global effects (all models)");
        ui.add(egui::Slider::new(&mut self.shake, 0.0..=1.0).text("shake"));
        ui.add(egui::Slider::new(&mut self.kaleido, 0.0..=16.0).text("kaleidoscope"));
        ui.add(egui::Slider::new(&mut self.light_leak, 0.0..=1.0).text("light_leak"));
        color_slot_ui(ui, "light_leak color", &mut self.leak_color);
        ui.add(egui::Slider::new(&mut self.crt, 0.0..=1.0).text("crt"));
        ui.add(egui::Slider::new(&mut self.glitch, 0.0..=1.0).text("glitch"));
        ui.add(egui::Slider::new(&mut self.posterize, 0.0..=1.0).text("posterize"));
        ui.add(egui::Slider::new(&mut self.zoom, -1.0..=1.0).text("zoom"));
        ui.add(egui::Slider::new(&mut self.fire, 0.0..=1.0).text("fire"));
        ui.add(egui::Slider::new(&mut self.lorenz, 0.0..=1.0).text("lorenz"));
        ui.add(egui::Slider::new(&mut self.vignette, 0.0..=1.0).text("vignette"));
        ui.add(egui::Slider::new(&mut self.blur, 0.0..=1.0).text("blur"));
        ui.add(egui::Slider::new(&mut self.sharpen, 0.0..=1.0).text("sharpen"));
        ui.add(egui::Slider::new(&mut self.grain, 0.0..=1.0).text("grain"));

        ui.separator();
        ui.label(format!("{} floats", names[self.selected]));
        let schema = self.models[self.selected].schema();
        // Copied out before `floats` takes a mutable borrow of `self`, so the
        // save button below can still write them into the preset.
        let post = self.post();
        let floats = &mut self.model_floats[self.selected];
        let mut offset = 0usize;
        for block in schema.blocks {
            match block {
                Block::Floats(specs) => {
                    for spec in specs.iter() {
                        ui.add(
                            egui::Slider::new(&mut floats[offset], spec.min..=spec.max)
                                .text(spec.name),
                        );
                        offset += 1;
                    }
                }
                Block::ColorSlot(name, _) => {
                    color_slot_ui(ui, name, (&mut floats[offset..offset + 12]).try_into().unwrap());
                    offset += 12;
                }
                Block::Camera(_) => {
                    ui.label("camera: pos / target / fov");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut floats[offset]).prefix("x "));
                        ui.add(egui::DragValue::new(&mut floats[offset + 1]).prefix("y "));
                        ui.add(egui::DragValue::new(&mut floats[offset + 2]).prefix("z "));
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut floats[offset + 3]).prefix("tx "));
                        ui.add(egui::DragValue::new(&mut floats[offset + 4]).prefix("ty "));
                        ui.add(egui::DragValue::new(&mut floats[offset + 5]).prefix("tz "));
                    });
                    ui.add(egui::Slider::new(&mut floats[offset + 6], 10.0..=120.0).text("fov"));
                    offset += 7;
                }
            }
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("preset:");
            ui.text_edit_singleline(&mut self.preset_name);
        });
        ui.horizontal(|ui| {
            if ui.button("save").clicked() {
                let model_name = names[self.selected];
                self.status = match preset::save(model_name, &self.preset_name, floats, &post) {
                    Ok(()) => format!("saved: {}/{}", model_name, self.preset_name),
                    Err(e) => format!("save failed: {e}"),
                };
            }
            if ui.button("load").clicked() {
                let model_name = names[self.selected];
                match preset::load(model_name, &self.preset_name) {
                    Ok(loaded) if loaded.floats.len() == floats.len() => {
                        floats.copy_from_slice(&loaded.floats);
                        self.shake = loaded.post.shake;
                        self.kaleido = loaded.post.kaleido;
                        self.light_leak = loaded.post.light_leak;
                        if loaded.post.leak_color.len() == 12 {
                            self.leak_color.copy_from_slice(&loaded.post.leak_color);
                        }
                        self.crt = loaded.post.crt;
                        self.glitch = loaded.post.glitch;
                        self.posterize = loaded.post.posterize;
                        self.zoom = loaded.post.zoom;
                        self.fire = loaded.post.fire;
                        self.lorenz = loaded.post.lorenz;
                        self.vignette = loaded.post.vignette;
                        self.blur = loaded.post.blur;
                        self.sharpen = loaded.post.sharpen;
                        self.grain = loaded.post.grain;
                        self.status = format!("loaded: {}/{}", model_name, self.preset_name);
                    }
                    Ok(_) => self.status = "the preset does not have this model's width".into(),
                    Err(e) => self.status = format!("load failed: {e}"),
                }
            }
            if ui.button("reset to defaults").clicked() {
                floats.copy_from_slice(&schema.defaults());
            }
        });
        for name in preset::list(names[self.selected]) {
            ui.label(format!("  · {name}"));
        }
        if !self.status.is_empty() {
            ui.separator();
            ui.label(&self.status);
        }
    }
}

fn color_slot_ui(ui: &mut egui::Ui, label: &str, floats: &mut [f32; 12]) {
    ui.label(label);
    ui.horizontal(|ui| {
        for i in 0..4 {
            let mut rgb = [floats[i * 3], floats[i * 3 + 1], floats[i * 3 + 2]];
            egui::widgets::color_picker::color_edit_button_rgb(ui, &mut rgb);
            floats[i * 3] = rgb[0];
            floats[i * 3 + 1] = rgb[1];
            floats[i * 3 + 2] = rgb[2];
        }
    });
}

pub struct App {
    rendering: Option<Rendering>,
    ui: UiState,
    start_time: Instant,
}

impl App {
    pub fn new() -> App {
        let models = registry::all_models();
        let model_floats = models.iter().map(|m| m.schema().defaults()).collect();
        App {
            rendering: None,
            ui: UiState {
                models,
                model_floats,
                selected: 0,
                shake: 0.0,
                kaleido: 0.0,
                light_leak: 0.0,
                leak_color: [1.0, 0.9, 0.6, 1.0, 0.3, 0.9, 0.9, 0.2, 0.6, 0.6, 0.8, 1.0],
                crt: 0.0,
                glitch: 0.0,
                posterize: 0.0,
                zoom: 0.0,
                fire: 0.0,
                lorenz: 0.0,
                vignette: 0.0,
                blur: 0.0,
                sharpen: 0.0,
                grain: 0.0,
                preset_name: String::from("example"),
                status: String::new(),
            },
            start_time: Instant::now(),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.rendering.is_some() {
            return;
        }

        let window_attrs = Window::default_attributes()
            .with_title("metal-render")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(event_loop.create_window(window_attrs).expect("create_window failed"));

        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::PRIMARY;
        let instance = wgpu::Instance::new(instance_desc);
        let surface = instance.create_surface(window.clone()).expect("create_surface failed");

        // The one place in `metal-render` that asks for an adapter for a
        // window: `Gpu::new_windowed` runs the `HighPerformance` →
        // `LowPower` → `force_fallback_adapter` ladder before giving up.
        // `S.U15` is what teaches this call site to fall back to
        // `egui_glow` on `None` instead of `.expect()`-panicking; today's
        // behaviour (open exactly as before, on this machine) is
        // unchanged because the first rung still succeeds here.
        let (adapter, gpu) = pollster::block_on(Gpu::new_windowed(&instance, &surface))
            .expect("no wgpu adapter (windowed), even after LowPower/force_fallback retries");

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface unsupported by this adapter");
        // Uncapped output: Immediate, then Mailbox, else vsync.
        let modes = surface.get_capabilities(&adapter).present_modes;
        config.present_mode = [wgpu::PresentMode::Immediate, wgpu::PresentMode::Mailbox]
            .into_iter()
            .find(|m| modes.contains(m))
            .unwrap_or(wgpu::PresentMode::AutoVsync);
        surface.configure(&gpu.device, &config);
        let surface_format = config.format;

        let post = PostChain::new(&gpu.device, surface_format);
        let overscan_w = ((size.width.max(1) as f32) * crate::shared::post::OVERSCAN) as u32;
        let overscan_h = ((size.height.max(1) as f32) * crate::shared::post::OVERSCAN) as u32;
        let (otex, oview) = offscreen_target(&gpu.device, overscan_w, overscan_h, OFFSCREEN_FORMAT);

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&gpu.device, surface_format, egui_wgpu::RendererOptions::default());

        self.rendering = Some(Rendering {
            window,
            surface,
            surface_config: config,
            gpu,
            post,
            offscreen: (otex, oview, overscan_w, overscan_h),
            egui_ctx,
            egui_state,
            egui_renderer,
        });
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        let Some(r) = &mut self.rendering else { return };

        let response = r.egui_state.on_window_event(&r.window, &event);
        if response.repaint {
            r.window.request_redraw();
        }
        if response.consumed {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    r.surface_config.width = size.width;
                    r.surface_config.height = size.height;
                    r.surface.configure(&r.gpu.device, &r.surface_config);

                    let ow = ((size.width as f32) * crate::shared::post::OVERSCAN) as u32;
                    let oh = ((size.height as f32) * crate::shared::post::OVERSCAN) as u32;
                    let (t, v) = offscreen_target(&r.gpu.device, ow.max(1), oh.max(1), OFFSCREEN_FORMAT);
                    r.offscreen = (t, v, ow.max(1), oh.max(1));
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(r) = &self.rendering {
            r.window.request_redraw();
        }
    }
}

impl App {
    fn redraw(&mut self) {
        let App { rendering, ui, start_time } = self;
        let Some(r) = rendering else { return };

        let raw_input = r.egui_state.take_egui_input(&r.window);
        let mut full_output = r.egui_ctx.clone().run_ui(raw_input, |root_ui| {
            egui::Panel::left("controls").min_size(340.0).show(root_ui, |panel_ui| {
                egui::ScrollArea::vertical().show(panel_ui, |scroll_ui| {
                    ui.build(scroll_ui);
                });
            });
        });
        r.egui_state.handle_platform_output(&r.window, full_output.platform_output);

        let surface_texture = match r.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => return,
        };
        let surface_view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let time = start_time.elapsed().as_secs_f32();
        let (_, offscreen_view, ow, oh) = &r.offscreen;
        // No compositor here — the dev window shows one model alone.
        let ctx = Ctx {
            device: &r.gpu.device,
            queue: &r.gpu.queue,
            time,
            width: *ow,
            height: *oh,
            fondo_alpha: false,
        };
        ui.models[ui.selected].render(&ctx, &ui.model_floats[ui.selected], offscreen_view, OFFSCREEN_FORMAT);

        let leak_color = ColorSlotGpu::from_floats(&ui.leak_color);
        let size = r.window.inner_size();
        // No graph in this window, only sliders — `resolved_chain` derives
        // the legacy order (declaration order, zero-amount effects skipped)
        // straight from the same fields `post()` just read off `ui`, which
        // by construction never includes a Wake chain step (see
        // `resolved_chain`'s own doc comment) — so the shipped-look default
        // below is inert here, same as `Renderer::grid`'s.
        let chain = ui.post().resolved_chain();
        // No per-model history in this window at all (see `UiState`'s own
        // module doc) — `fx.estela` cannot appear in `chain` here for the
        // same reason (`ui.post()` never sets one), but the
        // bind group still needs `ESTELA_RING_TAPS` views regardless of
        // whether the shader ever reads them, so every slot gets the current
        // frame's own view — see `fallback_ring`'s own doc comment.
        r.post.apply(
            &r.gpu.device,
            &r.gpu.queue,
            offscreen_view,
            &surface_view,
            size.width,
            size.height,
            &PostParams {
                shake: ui.shake,
                light_leak: ui.light_leak,
                leak_color,
                time,
                kaleido: ui.kaleido,
                wake: EstelaParams::default(),
                grade: GradeUniform::from_advanced(&ui.post().advanced),
                chain_fx: ChainFx::from_advanced(&ui.post().advanced),
            },
            &chain,
            &fallback_ring(offscreen_view),
        );

        let pixels_per_point = full_output.pixels_per_point;
        let paint_jobs = r.egui_ctx.tessellate(full_output.shapes, pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point,
        };
        for (id, deltas) in &full_output.textures_delta.set {
            for delta in deltas {
                r.egui_renderer.update_texture(&r.gpu.device, &r.gpu.queue, *id, delta);
            }
        }
        let mut encoder = r.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("egui encoder"),
        });
        let cmd_bufs =
            r.egui_renderer.update_buffers(&r.gpu.device, &r.gpu.queue, &mut encoder, &paint_jobs, &screen_descriptor);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            r.egui_renderer.render(&mut pass, &paint_jobs, &screen_descriptor);
        }
        for id in &full_output.textures_delta.free {
            r.egui_renderer.free_texture(id);
        }
        // `TexturesDelta::drop` asserts it is empty — iterating `.set`/`.free`
        // above does not count as "handled" in its own bookkeeping, only an
        // explicit `clear()` does.
        full_output.textures_delta.clear();
        r.gpu.queue.submit(cmd_bufs.into_iter().chain(Some(encoder.finish())));
        r.gpu.queue.present(surface_texture);
    }
}
