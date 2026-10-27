//! Parallax worlds — four models, one engine (`shared::parallax`).
//!
//! Endless parallax of procedural 2D stickers in five layers at different
//! depths and speeds, inside a 3D scene the camera (and head tracking) moves
//! through. Four registry entries so the router can pick a world:
//!
//! - `parallax-cave` — Cave: stalactites, moss rims, glowing vines, crystal
//!   clusters, bioluminescent mushrooms, drifting spores.
//! - `parallax-sky` — Sky: stars, sun/moon, layered clouds lit from above,
//!   flapping birds, rising sky lanterns, a cloud bank.
//! - `parallax-fields` — Fields: sun and snowy ridges, rolling hills, round
//!   trees and pines, a fence with lanterns, flowers, tall grass, fireflies.
//! - `parallax-city` — City: moon, ruined skyline, lit windows, neon signs,
//!   antennas with blinking lights, rubble, rebar, street lamps.
//!
//! The four share every float; only the colour defaults differ, each chosen
//! so the world opens in its own mood (and never on a white screen).

use crate::model::{Ctx, Model, Schema};
use crate::shared::parallax::{Parallax, World};

macro_rules! world {
    ($ty:ident, $id:literal, $name:literal, $world:expr, $schema:ident) => {
        pub struct $ty {
            engine: Parallax,
        }

        impl $ty {
            pub fn new() -> $ty {
                $ty { engine: Parallax::new($world) }
            }
        }

        impl Default for $ty {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Model for $ty {
            fn id(&self) -> &'static str {
                $id
            }

            fn name(&self) -> &'static str {
                $name
            }

            fn schema(&self) -> &'static Schema {
                &$schema
            }

            /// Draws its own sky from `fondo`, and answers `fondo_alpha`.
            fn backdrop(&self) -> bool {
                true
            }

            /// Head tracking moves the eye; layers shift by 1/depth.
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
                self.engine.render(&$schema, ctx, floats, target, format);
            }
        }
    };
}

/// Four stops, written as the eye reads them (sRGB-ish) and squared into the
/// linear values a slot holds — the frame is encoded to sRGB on write, so a
/// slot of 0.5 shows as a light grey, not a mid one.
const fn c4(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> [f32; 12] {
    const fn q(x: f32) -> f32 {
        x * x
    }
    [
        q(a[0]), q(a[1]), q(a[2]), q(b[0]), q(b[1]), q(b[2]),
        q(c[0]), q(c[1]), q(c[2]), q(d[0]), q(d[1]), q(d[2]),
    ]
}

// --- cueva: deep teal, bioluminescent ------------------------------------
const CUEVA_FONDO: [f32; 12] = c4([0.01, 0.05, 0.07], [0.03, 0.16, 0.17], [0.04, 0.22, 0.21], [0.01, 0.07, 0.08]);
const CUEVA_CAPAS: [f32; 12] = c4([0.12, 0.42, 0.40], [0.06, 0.26, 0.27], [0.02, 0.11, 0.12], [0.005, 0.025, 0.03]);
const CUEVA_OBJETOS: [f32; 12] = c4([0.25, 0.75, 0.50], [0.35, 0.90, 0.65], [0.55, 0.95, 0.80], [0.25, 0.65, 0.80]);
const CUEVA_LUCES: [f32; 12] = c4([0.40, 1.00, 0.80], [0.30, 0.85, 1.00], [0.70, 1.00, 0.55], [0.55, 0.75, 1.00]);
const CUEVA_NIEBLA: [f32; 12] = c4([0.04, 0.26, 0.28], [0.05, 0.32, 0.32], [0.05, 0.30, 0.30], [0.03, 0.18, 0.20]);

// --- cielo: dusk, pastel clouds -------------------------------------------
const CIELO_FONDO: [f32; 12] = c4([0.10, 0.12, 0.35], [0.36, 0.26, 0.56], [0.85, 0.46, 0.55], [1.00, 0.70, 0.45]);
const CIELO_CAPAS: [f32; 12] = c4([0.80, 0.55, 0.72], [0.70, 0.46, 0.70], [0.55, 0.36, 0.62], [0.36, 0.25, 0.50]);
const CIELO_OBJETOS: [f32; 12] = c4([0.10, 0.07, 0.18], [0.16, 0.10, 0.24], [0.22, 0.12, 0.28], [0.12, 0.08, 0.20]);
const CIELO_LUCES: [f32; 12] = c4([1.00, 0.86, 0.55], [1.00, 0.95, 0.80], [1.00, 0.68, 0.45], [0.90, 0.90, 1.00]);
const CIELO_NIEBLA: [f32; 12] = c4([0.45, 0.35, 0.62], [0.75, 0.48, 0.62], [0.95, 0.62, 0.58], [1.00, 0.72, 0.52]);

// --- campos: late afternoon over the fields -------------------------------
const CAMPOS_FONDO: [f32; 12] = c4([0.30, 0.48, 0.82], [0.52, 0.66, 0.90], [0.95, 0.78, 0.72], [1.00, 0.84, 0.58]);
const CAMPOS_CAPAS: [f32; 12] = c4([0.42, 0.52, 0.74], [0.32, 0.58, 0.42], [0.20, 0.44, 0.25], [0.08, 0.26, 0.14]);
const CAMPOS_OBJETOS: [f32; 12] = c4([0.98, 0.42, 0.62], [1.00, 0.84, 0.32], [0.74, 0.46, 0.96], [0.42, 0.26, 0.16]);
const CAMPOS_LUCES: [f32; 12] = c4([1.00, 0.92, 0.60], [1.00, 0.78, 0.40], [0.88, 1.00, 0.55], [1.00, 0.66, 0.48]);
const CAMPOS_NIEBLA: [f32; 12] = c4([0.62, 0.72, 0.92], [0.72, 0.80, 0.94], [0.92, 0.84, 0.82], [0.95, 0.86, 0.72]);

// --- ciudad: violet night, neon in the ruins ------------------------------
const CIUDAD_FONDO: [f32; 12] = c4([0.08, 0.08, 0.24], [0.20, 0.20, 0.46], [0.30, 0.30, 0.60], [0.22, 0.21, 0.46]);
const CIUDAD_CAPAS: [f32; 12] = c4([0.28, 0.28, 0.56], [0.20, 0.20, 0.44], [0.13, 0.13, 0.31], [0.06, 0.06, 0.15]);
const CIUDAD_OBJETOS: [f32; 12] = c4([0.14, 0.12, 0.28], [0.20, 0.17, 0.36], [0.90, 0.35, 0.48], [0.35, 0.75, 0.95]);
const CIUDAD_LUCES: [f32; 12] = c4([0.35, 0.88, 1.00], [1.00, 0.36, 0.58], [1.00, 0.86, 0.58], [0.62, 0.42, 1.00]);
const CIUDAD_NIEBLA: [f32; 12] = c4([0.26, 0.26, 0.54], [0.30, 0.30, 0.60], [0.32, 0.31, 0.60], [0.24, 0.23, 0.48]);

pub static CUEVA_SCHEMA: Schema =
    crate::parallax_schema!(&CUEVA_FONDO, &CUEVA_CAPAS, &CUEVA_OBJETOS, &CUEVA_LUCES, &CUEVA_NIEBLA);
pub static CIELO_SCHEMA: Schema =
    crate::parallax_schema!(&CIELO_FONDO, &CIELO_CAPAS, &CIELO_OBJETOS, &CIELO_LUCES, &CIELO_NIEBLA);
pub static CAMPOS_SCHEMA: Schema =
    crate::parallax_schema!(&CAMPOS_FONDO, &CAMPOS_CAPAS, &CAMPOS_OBJETOS, &CAMPOS_LUCES, &CAMPOS_NIEBLA);
pub static CIUDAD_SCHEMA: Schema =
    crate::parallax_schema!(&CIUDAD_FONDO, &CIUDAD_CAPAS, &CIUDAD_OBJETOS, &CIUDAD_LUCES, &CIUDAD_NIEBLA);

world!(ParallaxCueva, "parallax-cave", "Parallax Cave", World::Cueva, CUEVA_SCHEMA);
world!(ParallaxCielo, "parallax-sky", "Parallax Sky", World::Cielo, CIELO_SCHEMA);
world!(ParallaxCampos, "parallax-fields", "Parallax Fields", World::Campos, CAMPOS_SCHEMA);
world!(ParallaxCiudad, "parallax-city", "Parallax City", World::Ciudad, CIUDAD_SCHEMA);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Block;
    use crate::shared::parallax::{CAMERA_TRACK_FLOATS, FLOATS, SLOTS};

    fn worlds() -> Vec<Box<dyn Model>> {
        vec![
            Box::new(ParallaxCueva::new()),
            Box::new(ParallaxCielo::new()),
            Box::new(ParallaxCampos::new()),
            Box::new(ParallaxCiudad::new()),
        ]
    }

    #[test]
    fn the_four_worlds_share_one_width_and_one_float_block() {
        let w = CUEVA_SCHEMA.width();
        for m in worlds() {
            let s = m.schema();
            assert_eq!(s.width(), w, "{}", m.id());
            assert_eq!(s.defaults().len(), w);
            assert!(matches!(s.blocks[0], Block::Floats(f) if f.len() == FLOATS.len()));
        }
        assert_eq!(
            w,
            FLOATS.len() + 7 + 12 * SLOTS.len() + CAMERA_TRACK_FLOATS.len()
                + crate::model::SIGNAL_WIDTH
                + crate::model::EFFECTS_WIDTH
        );
    }

    #[test]
    fn every_world_declares_camera_track_and_backdrop_and_has_a_camera_block() {
        for m in worlds() {
            assert!(m.camera_track(), "{}", m.id());
            assert!(m.backdrop(), "{}", m.id());
            assert!(m.schema().blocks.iter().any(|b| matches!(b, Block::Camera(_))));
            let names: Vec<&str> = m
                .schema()
                .blocks
                .iter()
                .filter_map(|b| if let Block::Floats(f) = b { Some(*f) } else { None })
                .flatten()
                .map(|f| f.name)
                .collect();
            for n in ["cam_x", "cam_y", "cam_z"] {
                assert!(names.contains(&n), "{} lacks {n}", m.id());
            }
        }
    }

    #[test]
    fn no_world_opens_with_a_white_slot() {
        for m in worlds() {
            for b in m.schema().blocks {
                if let Block::ColorSlot(name, c) = b {
                    if SLOTS.contains(name) {
                        assert!(**c != crate::model::WHITE4, "{} {name} opens white", m.id());
                        let mean = c.iter().sum::<f32>() / 12.0;
                        assert!(mean < 0.9, "{} {name} is nearly white", m.id());
                    }
                }
            }
        }
    }

    #[test]
    fn ids_are_the_four_worlds() {
        let ids: Vec<&str> = worlds().iter().map(|m| m.id()).collect();
        assert_eq!(ids, ["parallax-cave", "parallax-sky", "parallax-fields", "parallax-city"]);
    }
}
