//! The browser's preview painter in a Web Worker (plan §6, item 1).
//!
//! Two halves in the one wasm module, told apart by where they run:
//!
//! - **The worker** (`web/wasm/worker.js` instantiates this same module):
//!   [`worker_boot`] builds a wgpu device on the `OffscreenCanvas` the page
//!   transferred, [`worker_draw`] draws one model as a texture and reads it
//!   back *without blocking* (a browser cannot wait on a buffer map, so the
//!   readback is polled with a yield between turns).
//! - **The page**: [`try_worker`] feature-detects once at startup
//!   (`Worker`, `OffscreenCanvas`, then the worker's own answer about its
//!   device), and on success returns a [`WorkerPaint`], a `Paint` whose
//!   pixel draws go to the worker. Everything else (the model list, names,
//!   schemas) is the CPU stand-in's, which lists the same models.
//!
//! The wire is `metal_perform::worker_proto` (tested natively). A frame the
//! worker has not landed yet is answered with the stand-in's, so the first
//! frames are figures rather than black; after that the newest landed frame
//! answers, one request in flight per `(model, size)`.
//!
//! The desktop's own preview worker (`shell/frames.rs`, a thread) is native
//! and untouched. A real browser run (WebGPU in a worker, `OffscreenCanvas`
//! transfer, the frame cadence) is for a person: see `docs/MULTIPLATFORM.md`.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use metal_perform::paint::Paint;
use metal_perform::stand::Stand;
use metal_perform::worker_proto::{choose_mode, DrawRequest, FrameReply, Key, Mailbox, Mode};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

// -- the worker's half ----------------------------------------------------------

thread_local! {
    static WORKER_PAINT: RefCell<Option<Box<dyn Paint>>> = const { RefCell::new(None) };
}

/// Builds the worker's painter on the transferred canvas. `false` when the
/// worker has neither WebGPU nor WebGL2 (the page then paints inline).
#[wasm_bindgen]
pub async fn worker_boot(canvas: web_sys::OffscreenCanvas) -> bool {
    let Some(gpu) = metal_render::gpu::web_worker(canvas).await else { return false };
    let renderer = metal_render::headless::Renderer::with_gpu(gpu);
    let paint = metal_perform::paint::Gpu::from_renderer(renderer);
    WORKER_PAINT.with(|p| *p.borrow_mut() = Some(Box::new(paint)));
    true
}

/// The render scale, as the page's quality slider sets it.
#[wasm_bindgen]
pub fn worker_quality(quality: f32) {
    WORKER_PAINT.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.set_quality(quality);
        }
    });
}

/// One draw: the encoded [`FrameReply`]. Calls must not overlap (`worker.js`
/// chains them); an overlapping call answers "no frame".
#[wasm_bindgen]
pub async fn worker_draw(header: String, floats: Vec<f32>) -> Vec<u8> {
    let req = match DrawRequest::parse(&header, floats) {
        Ok(r) => r,
        Err(e) => {
            web_sys::console::warn_1(&format!("metalvj worker: {e}").into());
            return FrameReply::none(0).encode();
        }
    };
    let Some(mut paint) = WORKER_PAINT.with(|p| p.borrow_mut().take()) else {
        return FrameReply::none(req.id).encode();
    };
    let reply = draw(paint.as_mut(), &req).await;
    WORKER_PAINT.with(|p| *p.borrow_mut() = Some(paint));
    reply.encode()
}

async fn draw(paint: &mut dyn Paint, req: &DrawRequest) -> FrameReply {
    let Some(frame) = paint.solo_texture(req.index, &req.floats, req.time, req.w, req.h, req.background_alpha, &req.chain) else {
        return FrameReply::none(req.id);
    };
    let Some(cx) = paint.gpu_cx() else { return FrameReply::none(req.id) };
    let slot: std::sync::Arc<std::sync::Mutex<Option<Vec<u8>>>> = Default::default();
    let fill = std::sync::Arc::clone(&slot);
    cx.readback_with(&frame, move |bytes| *fill.lock().expect("readback slot") = Some(bytes));
    for _ in 0..3000 {
        if let Some(bytes) = slot.lock().expect("readback slot").take() {
            let (w, h) = (frame.width(), frame.height());
            if bytes.len() != (w * h * 4) as usize {
                return FrameReply::none(req.id);
            }
            return FrameReply { id: req.id, w, h, rgba: bytes };
        }
        sleep_ms(1).await;
    }
    FrameReply::none(req.id)
}

/// Yields to the event loop for about `ms` (`setTimeout`, from a window or a
/// worker alike).
async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let global = js_sys::global();
        if let Some(w) = global.dyn_ref::<web_sys::Window>() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        } else if let Some(w) = global.dyn_ref::<web_sys::WorkerGlobalScope>() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        } else {
            let _ = resolve.call0(&JsValue::NULL);
        }
    });
    let _ = JsFuture::from(promise).await;
}

// -- the page's half --------------------------------------------------------------

fn has_global(name: &str) -> bool {
    js_sys::Reflect::has(&js_sys::global(), &JsValue::from_str(name)).unwrap_or(false)
}

fn message(kind: &str) -> js_sys::Object {
    let o = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&o, &"kind".into(), &kind.into());
    o
}

/// A `Paint` whose pixel draws run in a Web Worker. Built by [`try_worker`].
pub struct WorkerPaint {
    stand: Stand,
    worker: web_sys::Worker,
    mail: Rc<RefCell<Mailbox>>,
    label: String,
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
}

impl WorkerPaint {
    fn post(&self, req: &DrawRequest) {
        let o = message("draw");
        let _ = js_sys::Reflect::set(&o, &"header".into(), &req.header().into());
        let _ = js_sys::Reflect::set(&o, &"floats".into(), &js_sys::Float32Array::from(&req.floats[..]).into());
        let _ = self.worker.post_message(&o);
    }
}

/// Feature-detects and boots the worker, once, at startup. `None` (the page
/// paints inline) when `Worker` or `OffscreenCanvas` is missing, the worker
/// does not answer in 15 s, or its device could not be made.
pub async fn try_worker() -> Option<WorkerPaint> {
    let has_offscreen = has_global("OffscreenCanvas");
    if choose_mode(has_global("Worker"), has_offscreen, Some(true)) != Mode::Worker {
        return None;
    }
    let document = web_sys::window()?.document()?;
    let canvas = document.create_element("canvas").ok()?.dyn_into::<web_sys::HtmlCanvasElement>().ok()?;
    let offscreen = canvas.transfer_control_to_offscreen().ok()?;

    let opts = web_sys::WorkerOptions::new();
    opts.set_type(web_sys::WorkerType::Module);
    let worker = web_sys::Worker::new_with_options("./worker.js", &opts).ok()?;

    let mail = Rc::new(RefCell::new(Mailbox::default()));
    let ready: Rc<RefCell<Option<js_sys::Function>>> = Rc::default();
    let (mail_in, ready_in) = (Rc::clone(&mail), Rc::clone(&ready));
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        let data = e.data();
        match js_sys::Reflect::get(&data, &"kind".into()).ok().and_then(|k| k.as_string()).as_deref() {
            Some("ready") => {
                let ok = js_sys::Reflect::get(&data, &"ok".into()).ok().and_then(|v| v.as_bool()).unwrap_or(false);
                if let Some(resolve) = ready_in.borrow_mut().take() {
                    let _ = resolve.call1(&JsValue::NULL, &JsValue::from_bool(ok));
                }
            }
            Some("frame") => {
                if let Ok(rgba) = js_sys::Reflect::get(&data, &"bytes".into()) {
                    let bytes = js_sys::Uint8Array::new(&rgba).to_vec();
                    if let Ok(reply) = FrameReply::decode(&bytes) {
                        mail_in.borrow_mut().land(reply);
                    }
                }
            }
            _ => {}
        }
    });
    worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

    let answered = js_sys::Promise::new(&mut |resolve, _| {
        *ready.borrow_mut() = Some(resolve.clone());
        if let Some(w) = web_sys::window() {
            let give_up = Closure::once_into_js(move || {
                let _ = resolve.call1(&JsValue::NULL, &JsValue::from_bool(false));
            });
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(give_up.unchecked_ref(), 15_000);
        }
    });
    let boot = message("boot");
    let _ = js_sys::Reflect::set(&boot, &"canvas".into(), &offscreen);
    let transfer = js_sys::Array::of1(&offscreen);
    worker.post_message_with_transfer(&boot, &transfer).ok()?;
    let ok = JsFuture::from(answered).await.ok().and_then(|v| v.as_bool());

    if choose_mode(true, has_offscreen, ok) != Mode::Worker {
        worker.terminate();
        return None;
    }
    Some(WorkerPaint {
        stand: Stand::new(),
        worker,
        mail,
        label: "Web Worker on an OffscreenCanvas".to_string(),
        _on_message: on_message,
    })
}

impl Paint for WorkerPaint {
    fn model_count(&self) -> usize {
        self.stand.model_count()
    }
    fn ids(&self) -> Vec<&'static str> {
        self.stand.ids()
    }
    fn names(&self) -> Vec<&'static str> {
        self.stand.names()
    }
    fn schemas(&self) -> Vec<&'static metal_schema::Schema> {
        self.stand.schemas()
    }
    fn footage(&self) -> Vec<bool> {
        self.stand.footage()
    }
    fn backdrop(&self) -> Vec<bool> {
        self.stand.backdrop()
    }
    fn reel(&mut self, index: usize) -> Option<&mut dyn metal_reel::Reel> {
        self.stand.reel(index)
    }
    /// The pictures are the real renderers' (in the worker).
    fn real(&self) -> bool {
        true
    }
    fn set_quality(&mut self, quality: f32) {
        let o = message("quality");
        let _ = js_sys::Reflect::set(&o, &"value".into(), &JsValue::from_f64(f64::from(quality)));
        let _ = self.worker.post_message(&o);
    }
    fn solo(&mut self, index: usize, floats: &[f32], time: f32, w: u32, h: u32, background_alpha: bool) -> (Vec<u8>, u32, u32) {
        self.solo_chain(index, floats, time, w, h, background_alpha, &[])
    }
    fn solo_chain(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        background_alpha: bool,
        chain: &[metal_schema::EffectStep],
    ) -> (Vec<u8>, u32, u32) {
        let key: Key = (index, w, h, background_alpha);
        let asked = self.mail.borrow_mut().begin(key, |id| DrawRequest {
            id,
            index,
            time,
            w,
            h,
            background_alpha,
            chain: chain.to_vec(),
            floats: floats.to_vec(),
        });
        if let Some(req) = asked {
            self.post(&req);
        }
        if let Some((fw, fh, px)) = self.mail.borrow().frame(&key) {
            return (px.clone(), *fw, *fh);
        }
        self.stand.solo_chain(index, floats, time, w, h, background_alpha, chain)
    }
    fn describe(&self) -> String {
        format!("{} ({} models)", self.label, self.stand.model_count())
    }
}
