//! The engine the four parallax worlds share (`models::parallax`): one float
//! block, one uniform, one WGSL (`shaders/parallax.wgsl`) with a world switch.
//!
//! Five layers of procedural 2D stickers stand at depths `prof_1..prof_5`
//! (1 = the farthest by default, 5 the nearest) inside a real 3D scene. The
//! camera block places the eye; `vaiven` bobs it; head tracking's reserved
//! `cam_x/cam_y/cam_z` are added to it. Because every layer is a plane the
//! ray actually hits, any eye movement shifts a layer by 1/depth — the
//! parallax H4 asks 2D layered models to map head offsets to.
//!
//! The scroll is a world-space offset per layer, `time * velocidad *
//! direccion * vel_i`, so `direccion` at 0 stands still, negative scrolls the
//! other way, and a layer's apparent speed is its own speed over its depth.
//! The offset is computed in f64 and only narrowed at the end; f32 layer
//! coordinates keep sub-pixel precision for several hours of continuous
//! scroll at default speeds.

use glam::Vec3;

use crate::model::{spec, Ctx, FloatSpec, Schema};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::{CameraGpu, ColorSlotGpu};

/// The world a model draws; the number is the shader's switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum World {
    Cueva = 0,
    Cielo = 1,
    Campos = 2,
    Ciudad = 3,
}

/// How many sticker layers every world has.
pub const LAYERS: usize = 5;

/// World units of eye travel per unit of `cam_x/cam_y/cam_z`.
pub const HEAD_SCALE: f32 = 0.25;

/// The model's own floats, identical across the four worlds.
pub const FLOATS: &[FloatSpec] = &[
    // Scroll direction: -1 one way, +1 the other, 0 stands still. Scales
    // every layer's speed, so one float reverses the whole world.
    spec("direction", -1.0, 1.0, 1.0),
    // Global scroll speed, world units per second at layer speed 1.
    spec("speed", 0.0, 3.0, 0.5),
    // Per-layer speed multipliers, 1 = far … 5 = near. Signed, so one layer
    // can drift against the others. At 1 each, depth alone gives parallax.
    spec("speed_1", -2.0, 3.0, 1.0),
    spec("speed_2", -2.0, 3.0, 1.0),
    spec("speed_3", -2.0, 3.0, 1.0),
    spec("speed_4", -2.0, 3.0, 1.0),
    spec("speed_5", -2.0, 3.0, 1.0),
    // Per-layer depth: the distance of each layer's plane from the camera.
    // Draw order follows depth, so moving a layer past another reorders them.
    spec("depth_1", 0.5, 40.0, 18.0),
    spec("depth_2", 0.5, 40.0, 9.0),
    spec("depth_3", 0.5, 40.0, 5.0),
    spec("depth_4", 0.5, 40.0, 2.8),
    spec("depth_5", 0.5, 40.0, 1.6),
    // How many stickers per strip, and how big they are.
    spec("density", 0.0, 1.0, 0.6),
    spec("size", 0.3, 2.5, 1.0),
    // Light sources (crystals, sun, stars, windows, neon, lanterns): their
    // intensity, how far their halo reaches, and how much they flicker.
    spec("light", 0.0, 3.0, 1.2),
    spec("halo", 0.0, 2.0, 0.6),
    spec("blink", 0.0, 1.0, 0.25),
    // Atmosphere: how much far layers sink into `niebla_c`.
    spec("fog", 0.0, 1.0, 0.45),
    // Pixel-art blocks: 0 smooth, 1 eight-pixel blocks.
    spec("pixel", 0.0, 1.0, 0.18),
    // Camera bob (amplitude and rate), on top of the camera block.
    spec("swing", 0.0, 1.0, 0.15),
    spec("swing_speed", 0.0, 3.0, 0.4),
    // Where the ground and the sun/moon sit.
    spec("horizon", -1.0, 1.0, 0.0),
    // Which strip: another seed is another, equally endless, world.
    spec("seed", 0.0, 100.0, 0.0),
];

/// Reserved for `cam.track` (§13.2 / H4), Turing's convention.
pub const CAMERA_TRACK_FLOATS: &[FloatSpec] = &[
    spec("cam_x", -2.0, 2.0, 0.0),
    spec("cam_y", -2.0, 2.0, 0.0),
    spec("cam_z", -2.0, 2.0, 0.0),
];

/// The colour slots, in schema order, after the camera block.
pub const SLOTS: [&str; 5] = ["background", "layers", "objects", "lights", "fog_c"];

/// Builds a world's schema with its own colour defaults. A macro rather than
/// a `const fn` because `Schema` holds a `&'static [Block]`.
#[macro_export]
macro_rules! parallax_schema {
    ($background:expr, $layers:expr, $objects:expr, $lights:expr, $fog:expr) => {
        $crate::model::Schema {
            blocks: &[
                $crate::model::Block::Floats($crate::shared::parallax::FLOATS),
                // Level, looking straight down -z at the layers.
                $crate::model::camera([0.0, 0.0, 0.0], [0.0, 0.0, -10.0], 60.0),
                $crate::model::color_at("background", $background),
                $crate::model::color_at("layers", $layers),
                $crate::model::color_at("objects", $objects),
                $crate::model::color_at("lights", $lights),
                $crate::model::color_at("fog_c", $fog),
                $crate::model::Block::Floats($crate::shared::parallax::CAMERA_TRACK_FLOATS),
                $crate::model::SIGNAL_FLOATS_BLOCK,
                $crate::model::EFFECTS_FLOATS_BLOCK,
                $crate::model::EFFECTS_COLOR_BLOCK,
            ],
        }
    };
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    slots: [ColorSlotGpu; 5],
    eye: [f32; 4],
    fwd: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    f0: [f32; 4],
    off0: [f32; 4],
    dep0: [f32; 4],
    f1: [f32; 4],
    look: [f32; 4],
    look2: [f32; 4],
}

/// Where each block starts in the row, read off the schema rather than
/// counted by hand.
const B_FLOATS: usize = 0;
const B_CAMERA: usize = 1;
const B_SLOTS: usize = 2;
const B_TRACK: usize = 7;

pub struct Parallax {
    world: World,
    pass: Fullscreen,
}

impl Parallax {
    pub fn new(world: World) -> Parallax {
        Parallax { world, pass: Fullscreen::new("parallax", include_str!("../../shaders/parallax.wgsl")) }
    }

    pub fn render(
        &mut self,
        schema: &Schema,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let f = &floats[schema.offset(B_FLOATS)..];
        let named = |name: &str| -> f32 {
            let i = FLOATS.iter().position(|s| s.name == name).expect("a parallax float");
            f[i]
        };
        let time = ctx.time as f64;
        let travel = time * named("speed") as f64 * named("direction") as f64;
        let mut off = [0f32; LAYERS];
        let mut dep = [0f32; LAYERS];
        for i in 0..LAYERS {
            off[i] = (travel * named(&format!("speed_{}", i + 1)) as f64) as f32;
            dep[i] = named(&format!("depth_{}", i + 1)).max(0.05);
        }

        let cam = CameraGpu::from_floats(&floats[schema.offset(B_CAMERA)..schema.offset(B_CAMERA) + 7]);
        let t0 = schema.offset(B_TRACK);
        let track = Vec3::new(floats[t0], floats[t0 + 1], floats[t0 + 2]);
        let (va, vv) = (named("swing"), named("swing_speed"));
        let ph = ctx.time * vv * std::f32::consts::TAU * 0.25;
        let bob = Vec3::new(0.12 * va * (ph * 0.5).sin(), 0.08 * va * ph.sin(), 0.0);
        // The look direction comes from the camera block alone; head tracking
        // and the bob translate the eye without re-aiming it, which is what
        // makes near layers slide more than far ones.
        let fwd = (cam.target() - cam.pos()).normalize_or(Vec3::NEG_Z);
        let right = fwd.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = right.cross(fwd);
        // A head offset of 2 (the float's end) moves the eye half a world unit:
        // the nearest layer (1.6 away) slides about a third of the frame,
        // the far ones barely — enough depth without leaving the strip.
        let eye = cam.pos() + track * HEAD_SCALE + bob;
        let tan_half = (cam.fov_radians().clamp(0.1, 3.0) * 0.5).tan();

        let pixel = named("pixel");
        let block = 1.0 + (pixel * 7.0).round();
        let px_layer = 2.0 * tan_half / ctx.height.max(1) as f32 * block;

        let mut slots = [ColorSlotGpu::solid(0.0, 0.0, 0.0); 5];
        for (k, slot) in slots.iter_mut().enumerate() {
            let o = schema.offset(B_SLOTS + k);
            *slot = ColorSlotGpu::from_floats(&floats[o..o + 12]);
        }

        let uniform = Uniform {
            slots,
            eye: [eye.x, eye.y, eye.z, ctx.time],
            fwd: [fwd.x, fwd.y, fwd.z, tan_half],
            right: [right.x, right.y, right.z, 0.0],
            up: [up.x, up.y, up.z, 0.0],
            f0: [
                ctx.aspect(),
                self.world as u32 as f32,
                if ctx.fondo_alpha { 1.0 } else { 0.0 },
                named("seed"),
            ],
            off0: [off[0], off[1], off[2], off[3]],
            dep0: [dep[0], dep[1], dep[2], dep[3]],
            f1: [off[4], dep[4], px_layer, ctx.height as f32],
            look: [named("density"), named("size"), named("light"), named("halo")],
            look2: [named("blink"), named("fog"), pixel, named("horizon")],
        };
        self.pass.draw(ctx, target, format, &uniform);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Block;

    #[test]
    fn every_layer_has_a_speed_and_a_depth() {
        for i in 1..=LAYERS {
            assert!(FLOATS.iter().any(|f| f.name == format!("speed_{i}")));
            assert!(FLOATS.iter().any(|f| f.name == format!("depth_{i}")));
        }
    }

    #[test]
    fn direction_can_go_either_way_or_stand_still() {
        let d = FLOATS.iter().find(|f| f.name == "direction").unwrap();
        assert!(d.min < 0.0 && d.max > 0.0);
    }

    #[test]
    fn default_depths_run_far_to_near() {
        let d: Vec<f32> = (1..=LAYERS)
            .map(|i| FLOATS.iter().find(|f| f.name == format!("depth_{i}")).unwrap().default)
            .collect();
        assert!(d.windows(2).all(|w| w[0] > w[1]), "{d:?}");
    }

    #[test]
    fn block_indices_point_at_the_blocks_they_name() {
        static S: Schema = crate::parallax_schema!(
            &crate::model::BLACK4,
            &crate::model::BLACK4,
            &crate::model::BLACK4,
            &crate::model::BLACK4,
            &crate::model::BLACK4
        );
        assert!(matches!(S.blocks[B_FLOATS], Block::Floats(f) if f.len() == FLOATS.len()));
        assert!(matches!(S.blocks[B_CAMERA], Block::Camera(_)));
        for (k, name) in SLOTS.iter().enumerate() {
            assert!(matches!(S.blocks[B_SLOTS + k], Block::ColorSlot(n, _) if n == *name));
        }
        assert!(matches!(S.blocks[B_TRACK], Block::Floats(f) if f[0].name == "cam_x"));
    }
}
