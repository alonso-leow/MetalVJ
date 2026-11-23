//! G1 — the types the GPU-centric compose hands around, compiled in every
//! build so [`crate::paint::Paint`] has one signature. Without the `gpu`
//! feature [`GpuFrame`] is uninhabited: no value exists, every `Texture` arm is
//! dead code, and the CPU path is the only one.

use crate::layers::{Blend, ClipMode, Sector};

#[cfg(feature = "gpu")]
pub use crate::gpu_compose::GpuFrame;

#[cfg(not(feature = "gpu"))]
#[derive(Clone, Debug)]
pub enum GpuFrame {}

#[cfg(not(feature = "gpu"))]
impl GpuFrame {
    pub fn width(&self) -> u32 {
        match *self {}
    }
    pub fn height(&self) -> u32 {
        match *self {}
    }
}

/// A device-sized single-channel coverage map on the GPU (a contour clip, a
/// mask cut, a stroke band). Cheap to clone; built by
/// [`crate::paint::Paint::cover_gpu`].
#[cfg(feature = "gpu")]
pub use crate::gpu_cover::CoverRef;

#[cfg(not(feature = "gpu"))]
#[derive(Clone, Debug)]
pub enum CoverRef {}

/// A tile's warp as the GPU compose shader reads it: the inverse map of
/// [`crate::compose::warp_matrix`] and the sampling.
#[derive(Clone, Copy, Debug)]
pub struct GpuWarp {
    pub m: [f32; 9],
    pub nearest: bool,
}

/// One whole-frame pass over a finished device frame (G3): the mask cut, the
/// overlay tint, a painted RGBA layer (grid, calibration, stroke material).
pub enum Shade<'a> {
    /// `rgb *= coverage`.
    Cut(&'a CoverRef),
    /// `rgb = mix(rgb, colour, k * coverage)`; no coverage is the whole frame.
    Tint { rgb: [f32; 3], k: f32, cover: Option<&'a CoverRef> },
    /// `rgb = mix(rgb, layer.rgb, layer.a)` for a device-sized RGBA8 `layer`.
    Over { rgba: &'a [u8] },
}

/// A layer's picture on its way into compose: still on the GPU (a model's
/// output, no readback) or a CPU buffer (a still, a capture, a peer's frame,
/// a transition's result) that compose uploads.
pub enum TileSource<'a> {
    Texture(&'a GpuFrame),
    Cpu { pixels: &'a [u8], w: u32, h: u32 },
}

/// [`crate::compose::Tile`] with a [`TileSource`] in place of the pixel slice.
/// Same placement, opacity, blend and `giro` semantics; `cover` is the contour
/// clip (device-sized coverage, `None` for no clip) and `warp` the homography.
pub struct GpuTile<'a> {
    pub source: TileSource<'a>,
    pub sector: Sector,
    pub opacity: f32,
    pub blend: Blend,
    pub turn: f32,
    pub giro_mode: ClipMode,
    pub cover: Option<&'a CoverRef>,
    pub warp: Option<GpuWarp>,
}
