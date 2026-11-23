#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let event_loop = winit::event_loop::EventLoop::new().expect("event loop init failed");
    let mut app = metal_render::app::App::new();
    event_loop.run_app(&mut app).expect("event loop exited with an error");
}

// The browser has no window: this binary is the native viewer only.
#[cfg(target_arch = "wasm32")]
fn main() {}
