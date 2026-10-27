//! Layer previews: one model drawn at a size and time, as RGBA8. The browser
//! app shows whatever a [`Preview`] returns, so the painter behind it is a
//! choice: [`CpuPreview`] (the `metal_perform` stand-in, runs anywhere) today,
//! and a WebGL2 renderer from `metal-render` later, without touching the app.

use metal_perform::paint::Paint;
use metal_perform::stand::Stand;

/// One rendered frame: RGBA8, row-major, `w * h * 4` bytes.
pub struct Frame {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// Draws a model by its index in [`Preview::ids`].
pub trait Preview {
    /// The model ids this painter can draw, in index order.
    fn ids(&self) -> Vec<&'static str>;
    /// `floats` is the model's parameter row (`Schema::defaults` for a fresh
    /// one). `time` is seconds. The frame is `size` by `size`.
    fn frame(&mut self, model: usize, floats: &[f32], time: f32, size: u32) -> Frame;
}

/// The CPU painter. Any [`Paint`] works; [`CpuPreview::stand`] is the one
/// the browser build uses.
pub struct CpuPreview {
    paint: Box<dyn Paint>,
}

impl CpuPreview {
    pub fn new(paint: Box<dyn Paint>) -> Self {
        CpuPreview { paint }
    }

    pub fn stand() -> Self {
        Self::new(Box::new(Stand::new()))
    }
}

impl Preview for CpuPreview {
    fn ids(&self) -> Vec<&'static str> {
        self.paint.ids()
    }

    fn frame(&mut self, model: usize, floats: &[f32], time: f32, size: u32) -> Frame {
        let (rgba, w, h) = self.paint.solo(model, floats, time, size, size, false);
        Frame { rgba, w, h }
    }
}
