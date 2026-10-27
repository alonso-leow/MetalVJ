//! Model — Scene 3D (`scene3d`), the template `the design notes` §H.1
//! names: a raymarched SDF scene the future AI harness reads as its starting
//! point for a personalised shader. One object (a box) instanced across a
//! finite grid of tiles, kept deliberately simple and heavily `//`-commented in
//! `shaders/escena3d.wgsl` — the harness's `read_shader` tool (§H.4) hands that
//! file to the model verbatim, so the comments *are* the documentation it gets.
//!
//! # Why raymarching, and why the grid is the model's, not the scene's
//!
//! A `create_node` with its own shader (§H.2) is meant to replace *only* the
//! object's SDF — `sd_object()` in the WGSL — and keep everything around it:
//! the camera, the grid layout, the background, the material terms. That
//! split only holds if the grid tiles instance one shared SDF rather than
//! each carrying its own, which is why every tile spins together off the
//! same `rot_x/rot_y/rot_z` rather than each tile getting its own pose — see
//! `map()` in the WGSL. "Put a different mask on each tile" (§H.1) is a
//! request about the one object every tile already shares.
//!
//! # The float block, in the exact order §H.1 specifies
//!
//! ```text
//! rot_x, rot_y, rot_z        // the object's rotation
//! orbit, dist, fov           // the camera around the scene
//! grid_cols, grid_rows       // the grid of tiles: how many
//! grid_gap, grid_scale       // the spacing and size of each tile
//! bg_a, bg_b, bg_mix         // background — two tones and their mix
//! metal, rough, light_dir    // the material and where the light comes from
//! ```
//!
//! `backdrop()` is `true` — this model draws its own background, so it is
//! the twelve-model list `Ctx::fondo_alpha` matters to (`the design notes`
//! PARTE IV §17.3), and the WGSL answers it directly rather than through
//! `shared::backdrop::Backdrop`. `camera_track()` is `true` — `cam_x/cam_y/
//! cam_z` are appended right after this model's own floats, Turing's
//! convention (`models::turing`), so a `cam.track` node can pan/dolly the
//! orbiting eye without knowing this model computes its camera from
//! `orbit`/`dist`/`fov` instead of a fixed `camera(...)` block.

use glam::Vec3;

use crate::model::{spec, Block, Ctx, FloatSpec, Model, Schema, SurfaceSpec};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::surface::{MaterialU, PartSet};

/// The one part a material can be laid on: the tile every instance shares.
const SURFACES: &[&str] = &["body"];

/// The shared surface-sampling rule and the material kinds, then this file.
const WGSL: &str = concat!(
    include_str!("../../shaders/surface.wgsl"),
    "\n",
    include_str!("../../shaders/material.wgsl"),
    "\n",
    include_str!("../../shaders/escena3d.wgsl")
);

const FLOATS: &[FloatSpec] = &[
    // The object's spin, in radians/second around each world axis — a
    // speed, not a fixed pose, so the default already looks alive rather
    // than posed. Multiplied by elapsed time in the shader (`map()`), so
    // asking for the same instant twice still returns the same frame.
    spec("rot_x", -4.0, 4.0, 0.35),
    spec("rot_y", -4.0, 4.0, 0.55),
    spec("rot_z", -4.0, 4.0, 0.0),
    // The camera: `orbit` is the azimuth's angular speed around the scene
    // (radians/second, so 0 holds the camera still on one side), `dist` is
    // how far it orbits at, `fov` is the lens in degrees. Together they are
    // this model's whole camera — there is no fixed `camera(...)` schema
    // block underneath, unlike most 3D models in this suite, because §H.1
    // asks for the camera itself to be a driven float triple rather than a
    // static eye/target an operator nudges with `cam.track` alone.
    spec("orbit", -1.5, 1.5, 0.22),
    spec("distance", 1.5, 16.0, 5.5),
    spec("fov", 20.0, 110.0, 55.0),
    // The tiling: how many objects along x and along z. Rounded to the
    // nearest integer in the shader — a slider mid-way between 4 and 5 tiles
    // is not a fractional tile, it is 4 or 5 of them.
    spec("grid_columns", 1.0, 9.0, 4.0),
    spec("grid_rows", 1.0, 9.0, 4.0),
    // Spacing: `grid_gap` is the extra room between tiles on top of each
    // tile's own footprint, `grid_scale` is that footprint's half-extent —
    // both feed the same `spacing = grid_scale*2 + grid_gap` the WGSL keys
    // the whole grid off.
    spec("grid_gap", 0.0, 2.0, 0.35),
    spec("grid_scale", 0.08, 1.2, 0.42),
    // The background: two tones and where the gradient between them sits.
    // Grayscale on purpose (0..1, not a colour slot) — a template has no
    // colour choice to make for the harness, that is what the effects
    // chain's `EFFECTS_COLOR_BLOCK` is for, appended after every model.
    spec("bg_a", 0.0, 1.0, 0.04),
    spec("bg_b", 0.0, 1.0, 0.32),
    spec("bg_mix", 0.0, 1.0, 0.5),
    // The material: `metal` walks the surface from matte diffuse to a
    // tinted mirror-ish specular, `rough` narrows or widens that highlight,
    // and `light_direction` is the single free azimuth (0..1 turns) the light
    // orbits the scene at — one dial for "where the light comes from"
    // rather than a three-float vector an operator has to keep normalised.
    spec("metal", 0.0, 1.0, 0.35),
    spec("rough", 0.02, 1.0, 0.45),
    spec("light_direction", 0.0, 1.0, 0.15),
];

/// Reserved for `cam.track` (`the design notes` §13.2), Turing's
/// convention: appended right after the model's own floats, each ranging
/// wide enough to be useful without immediately leaving the grid behind.
/// At the schema default `[0,0,0]` `render()` below adds exactly nothing to
/// the orbiting eye it already computed.
const CAMERA_TRACK_FLOATS: &[FloatSpec] = &[
    spec("cam_x", -4.0, 4.0, 0.0),
    spec("cam_y", -4.0, 4.0, 0.0),
    spec("cam_z", -4.0, 4.0, 0.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        Block::Floats(CAMERA_TRACK_FLOATS),
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    f0: [f32; 4],
    eye: [f32; 4],
    fwd: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    rot: [f32; 4],
    grid: [f32; 4],
    bg: [f32; 4],
    mat: [f32; 4],
    /// `body`'s picture and `mat_*`, see `material.wgsl`.
    mat_body: MaterialU,
}

pub struct Escena3d {
    pass: Fullscreen,
    surfaces: PartSet<1>,
}

impl Escena3d {
    pub fn new() -> Escena3d {
        Escena3d {
            pass: Fullscreen::with_textures("scene3d", WGSL, SURFACES.len()),
            surfaces: PartSet::new(SURFACES),
        }
    }
}

impl Model for Escena3d {
    fn id(&self) -> &'static str {
        "scene3d"
    }

    fn name(&self) -> &'static str {
        "Scene 3D"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn is_3d(&self) -> bool {
        true
    }

    fn surfaces(&self) -> &'static [&'static str] {
        self.surfaces.surfaces()
    }

    fn set_surface(&mut self, part: &str, spec: Option<SurfaceSpec>) {
        self.surfaces.set(part, spec);
    }

    /// This model draws its own background (the grid floats over a two-tone
    /// gradient it composites itself) rather than covering the frame with
    /// `shared::backdrop::Backdrop` — see the WGSL's `fondo_alpha` handling.
    fn backdrop(&self) -> bool {
        true
    }

    /// The camera is already a driven orbit (`orbit`/`dist`/`fov`, not a
    /// fixed `camera(...)` block); `cam.track` still has three reserved
    /// floats to pan/dolly it, appended right after this model's own.
    fn camera_track(&self) -> bool {
        true
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let rot_x = floats[0];
        let rot_y = floats[1];
        let rot_z = floats[2];
        let orbit = floats[3];
        let distance = floats[4].max(0.1);
        let fov = floats[5].to_radians();
        let grid_columns = floats[6];
        let grid_rows = floats[7];
        let grid_gap = floats[8];
        let grid_scale = floats[9];
        let bg_a = floats[10];
        let bg_b = floats[11];
        let bg_mix = floats[12];
        let metal = floats[13];
        let rough = floats[14];
        let light_direction = floats[15];
        let cam_track = Vec3::new(floats[16], floats[17], floats[18]);

        // The camera orbits the scene at a fixed, slightly-above-level
        // elevation — `orbit` is an angular *speed*, so the azimuth is the
        // integral of it over elapsed time, the same convention `rot_x/y/z`
        // use for the object itself.
        let azimuth = ctx.time * orbit;
        const ELEVATION: f32 = 0.35;
        let eye = Vec3::new(
            distance * ELEVATION.cos() * azimuth.sin(),
            distance * ELEVATION.sin(),
            distance * ELEVATION.cos() * azimuth.cos(),
        ) + cam_track;
        let look_at = Vec3::ZERO;
        let fwd = (look_at - eye).normalize_or_zero();
        let world_up = Vec3::Y;
        let right = fwd.cross(world_up).normalize_or_zero();
        let up = right.cross(fwd);

        self.surfaces.sync(ctx.device, ctx.queue);
        let uniform = Uniform {
            f0: [ctx.aspect(), ctx.time, fov, 0.0],
            eye: [eye.x, eye.y, eye.z, 0.0],
            fwd: [fwd.x, fwd.y, fwd.z, 0.0],
            right: [right.x, right.y, right.z, 0.0],
            up: [up.x, up.y, up.z, 0.0],
            rot: [rot_x, rot_y, rot_z, 0.0],
            grid: [grid_columns, grid_rows, grid_gap, grid_scale],
            bg: [bg_a, bg_b, bg_mix, if ctx.fondo_alpha { 1.0 } else { 0.0 }],
            mat: [metal, rough, light_direction, 0.0],
            mat_body: self.surfaces.uniform(0),
        };
        self.pass.draw_textured(ctx, target, format, &uniform, &self.surfaces.bind_pairs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Vec<f32> {
        SCHEMA.defaults()
    }

    #[test]
    fn the_schema_and_the_slices_render_reads_are_the_same_width() {
        // 16 own floats + 3 cam.track floats. `render` slices `[0..19]`.
        assert_eq!(SCHEMA.width(), 19 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 19 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn floats_are_named_in_the_exact_order_h1_specifies() {
        let names: Vec<&str> = FLOATS.iter().map(|f| f.name).collect();
        assert_eq!(
            names,
            vec![
                "rot_x", "rot_y", "rot_z", "orbit", "distance", "fov", "grid_columns", "grid_rows",
                "grid_gap", "grid_scale", "bg_a", "bg_b", "bg_mix", "metal", "rough", "light_direction",
            ]
        );
    }

    #[test]
    fn backdrop_and_camera_track_are_both_declared() {
        let m = Escena3d::new();
        assert!(m.backdrop());
        assert!(m.camera_track());
    }

    #[test]
    fn cam_track_floats_are_appended_right_after_the_model_s_own() {
        let names: Vec<&str> = CAMERA_TRACK_FLOATS.iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["cam_x", "cam_y", "cam_z"]);
    }

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    /// Every kind is a surface: at `mat_mix` 1 it changes the frame, and
    /// clearing the `body` part restores the unpainted frame exactly — the
    /// migrated all-zero material group's contract.
    #[test]
    fn a_body_material_changes_the_frame_and_clearing_it_restores_it() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (128u32, 128u32);
        let floats = SCHEMA.defaults();
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Escena3d::new();
        assert_eq!(model.surfaces(), &["body"]);
        let draw = |model: &mut Escena3d| {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: 1.0,
                width: w,
                height: h,
                fondo_alpha: false,
            };
            model.render(&ctx, &floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };
        let plain = draw(&mut model);
        let kinds = ["matte", "concrete", "wood", "leather", "metal", "glass", "matrix"];
        for (kind, name) in kinds.iter().enumerate() {
            model.set_surface(
                "body",
                Some(SurfaceSpec {
                    image: None,
                    material: Some(MaterialSpec {
                        kind: kind as f32,
                        mix: 1.0,
                        scale: 1.0,
                        ..Default::default()
                    }),
                }),
            );
            let painted = draw(&mut model);
            assert_ne!(plain, painted, "{name} did not change the frame");
            model.set_surface("body", None);
            let restored = draw(&mut model);
            assert_eq!(plain, restored, "clearing {name} did not restore the frame");
        }
    }
}
