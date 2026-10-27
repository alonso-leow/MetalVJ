//! The browser app: the engine, the same one the desktop runs, over the
//! bundled data tree, shown through eframe's glow backend (egui_glow over
//! WebGL2).
//!
//! The desktop's own shell (`shell::Shell`, the egui chrome: nodes, properties,
//! lanes, library, preview) hosted by eframe's WebRunner on WebGL2. The shell
//! calls the engine through a real [`call::Door`] built here in-process over a
//! real [`Shared`] (`State`, painter, listener pool), exactly as the desktop
//! window does. The data is `store::BUNDLED`, mounted into `metal_vfs` under
//! [`store::WEB_ROOT`]; every write the engine makes lands in that tree and is
//! mirrored to `localStorage` by the hook `store::install_web_tree` sets.
//! Nothing is fetched at run time. The author side is the `Author` stub (no
//! `gpu`): the rig/curator/router routes answer as they do on a no-GPU build.
//!
//! Absent on purpose, and listed in `the product notes`: the HTTP server, real
//! audio capture, the AI harness, the Bluetooth radio, and the sources that
//! need a process or a socket.
//!
//! Built by `scripts/build-wasm.sh`; the page is `web/wasm/index.html`.

#![cfg(target_arch = "wasm32")]

use crate::call;
use crate::shell::classes::Theme;
use crate::shell::Shell;
use crate::store;
use crate::{Author, Shared};
use eframe::egui;
use metal_perform::engine::State;
use metal_perform::listeners::Listeners;
use metal_perform::paint::Paint;
use metal_perform::stand::Stand;
use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// The page's title, as `web/wasm/scene.json` names it.
const SCENE: &str = include_str!("../../../web/wasm/scene.json");

/// The page's canvas, as `web/wasm/index.html` names it.
const CANVAS_ID: &str = "the_canvas_id";

struct App {
    /// The engine, shared with the door the shell calls through.
    shared: Arc<Mutex<Shared>>,
    shell: Shell,
    /// Which painter the page got: the device's backend, or the CPU stand-in.
    painter: String,
    /// Per model: its id and the painter that draws it. Logged at start; the
    /// shell's own panels name the models a source runs.
    model_painters: Vec<(String, String)>,
    title: String,
}

/// Builds the engine the way the desktop's `serve_forever` does: the data
/// tree first, then the state (which loads `outputs.json` and the rest from
/// it), then the one-time migrations, then the listener pool.
fn boot(paint: Box<dyn Paint>) -> Shared {
    store::install_web_tree();
    metal_perform::output::set_data_root(std::path::Path::new(store::WEB_ROOT));
    metal_graph::owner::set_local_identity("browser");

    let state = State::new(paint.as_ref());
    let mut shared = Shared {
        state,
        paint,
        sim: Listeners::stand_in(),
    };
    crate::migrate_collections_to_graphs_once();
    crate::migrate_device_modes_to_graphs_once(&shared.state);
    let ids = shared.paint.ids();
    crate::sync_device_graphs_once(&shared.state, &ids);
    crate::adopt_device_graphs_at(&crate::output::data_root());
    crate::merge_device_collections_at(&crate::output::data_root());
    shared.state.port = 0;
    shared
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, paint: Box<dyn Paint>, painter: String) -> Result<Self, String> {
        // The page's own colours decide nothing: egui follows the system
        // light/dark preference unless told, and the light default put the
        // labels in a grey close to the page's ground. Dark, as the desktop
        // opens (`Theme::phosphor`).
        crate::shell::components::install_icon_font(&cc.egui_ctx);
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(egui::Color32::from_rgb(0xec, 0xe9, 0xe2));
        cc.egui_ctx.set_visuals(visuals);
        web_sys::console::info_1(&format!("metalvj painter: {painter}").into());
        let scene: serde_json::Value =
            serde_json::from_str(SCENE).map_err(|e| format!("web/wasm/scene.json: {e}"))?;
        let title = scene["title"].as_str().unwrap_or_default().to_string();

        let shared = Arc::new(Mutex::new(boot(paint)));
        let model_painters: Vec<(String, String)> = {
            let guard = shared.lock().map_err(|_| "engine lock poisoned".to_string())?;
            guard
                .paint
                .ids()
                .iter()
                .enumerate()
                .map(|(i, id)| (id.to_string(), guard.paint.painter_of(i)))
                .collect()
        };
        for (id, painter) in &model_painters {
            web_sys::console::info_1(&format!("metalvj model {id}: {painter}").into());
        }

        // The door the shell walks: the same type the desktop window builds,
        // over the same `Shared` and the no-GPU `Author` stub. No fetcher: the
        // browser has no threads, so previews fetch inline through the door.
        let author = Arc::new(Author::new());
        let door = call::Door::new(Arc::clone(&shared), author);
        let shell = Shell::new(Theme::phosphor(), Box::new(door));

        Ok(App {
            shared,
            shell,
            painter,
            model_painters,
            title,
        })
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.label(format!("{} · painter: {}", self.title, self.painter));
        self.shell.ui(ui);
        ui.ctx().request_repaint();
    }
}

/// The painter the page draws with. The wgpu models on the browser's device
/// (WebGPU, or WebGL2 where WebGPU is absent) when the browser gives one,
/// awaited because a blocked future never resolves on this thread; otherwise
/// the CPU stand-in for everything. On a WebGL2 device the models that need
/// compute shaders draw on the stand-in, one by one (`Paint::painter_of`).
async fn choose_painter() -> (Box<dyn Paint>, String) {
    if let Some(gpu) = metal_render::gpu::web().await {
        let renderer = metal_render::headless::Renderer::with_gpu(gpu);
        let paint = metal_perform::paint::Gpu::from_renderer(renderer);
        let label = paint.describe();
        return (Box::new(paint), label);
    }
    (
        Box::new(Stand::new()),
        "CPU stand-in (the browser gave no WebGPU or WebGL2 adapter)".to_string(),
    )
}

/// Runs when the page instantiates the module (`#[wasm_bindgen(start)]`).
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    // A panic on wasm32 only traps; without this hook the message never
    // reaches the console and the page just goes blank.
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("metalvj panic: {info}").into());
    }));
    let canvas = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(CANVAS_ID))
        .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| JsValue::from_str("web/wasm/index.html has no <canvas id=the_canvas_id>"))?;

    wasm_bindgen_futures::spawn_local(async move {
        let (paint, painter) = choose_painter().await;
        let runner = eframe::WebRunner::new();
        let created = runner
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| {
                    let app = App::new(cc, paint, painter)?;
                    Ok(Box::new(app))
                }),
            )
            .await;
        if let Err(e) = created {
            web_sys::console::error_1(&e);
        }
    });
    Ok(())
}
