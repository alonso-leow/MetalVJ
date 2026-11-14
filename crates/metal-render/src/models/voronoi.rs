//! `voronoi`: Stepped-colour cellular mosaic with distortion, gap and glow, on the procedural scaffold (`shared/scaffold.rs`). WGSL written from the Voronoi maths.
//!
//! Own floats (24): the brief's `pasos_color distorsion gap glow`, plus
//! `escala velocidad jitter metrica cheb ancho_borde borde_suave huecos
//! color_celda movimiento nucleo facetas estiramiento angulo_celda ondas
//! parpadeo relieve semilla`. Slots: `celdas`, `hueco` (gap), `resplandor`.

use crate::model::{spec, spec_held, FloatSpec};
use crate::shared::scaffold::ScaffoldDef;

const FLOATS: &[FloatSpec] = &[
    spec_held("steps_color", 1.0, 8.0, 3.0, 0.2),
    spec("distortion", 0.0, 1.0, 0.4),
    spec("gap", 0.0, 0.5, 0.04),
    spec("glow", 0.0, 1.0, 0.0),
    spec("scale", 1.0, 20.0, 5.0),
    spec("speed", -2.0, 2.0, 0.5),
    spec("jitter", 0.0, 1.0, 1.0),
    spec("metric", 0.0, 1.0, 0.0),
    spec("cheb", 0.0, 1.0, 0.0),
    spec("width_edge", 0.0, 1.0, 0.0),
    spec("edge_soft", 0.0, 1.0, 0.1),
    spec("holes", 0.0, 1.0, 0.1),
    spec("color_cell", 0.0, 1.0, 0.8),
    spec("motion", 0.0, 1.0, 0.6),
    spec("core", 0.0, 1.0, 0.0),
    spec("facets", 0.0, 1.0, 0.0),
    spec("stretch", -0.8, 2.0, 0.0),
    spec("angle_cell", 0.0, 1.0, 0.0),
    spec("waves", 0.0, 1.0, 0.0),
    spec("blink", 0.0, 1.0, 0.0),
    spec("relief", 0.0, 1.0, 0.0),
    spec_held("seed", 0.0, 50.0, 3.0, 0.2),
];

/// Linear values: deep red, orange, rust, cream.
const CELDAS: [f32; 12] = [0.25, 0.02, 0.01, 0.80, 0.18, 0.02, 0.55, 0.06, 0.02, 1.00, 0.80, 0.50];
/// Gap: very dark maroon, near black, dark brown, ember.
const HUECO: [f32; 12] = [0.027, 0.0, 0.0, 0.01, 0.0, 0.0, 0.03, 0.01, 0.0, 0.10, 0.02, 0.0];
/// Glow: white, pale yellow, amber, pink.
const RESPLANDOR: [f32; 12] = [1.0, 1.0, 1.0, 1.0, 0.9, 0.5, 1.0, 0.5, 0.05, 1.0, 0.2, 0.4];

pub static SCHEMA: crate::model::Schema = crate::scaffold_schema!(FLOATS;
    "cells" => CELDAS, "hole" => HUECO, "glare" => RESPLANDOR);

pub static DEF: ScaffoldDef = ScaffoldDef::new(
    "voronoi",
    "Voronoi",
    &SCHEMA,
    FLOATS,
    3,
    include_str!("../../shaders/voronoi.wgsl"),
);

crate::scaffold_model!(Voronoi, DEF);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::shared::scaffold::testing;

    fn build() -> Box<dyn Model> {
        Box::new(Voronoi::new())
    }

    const BASE: &[(&str, f32)] = &[
        ("glow", 0.5),
        ("metric", 0.3),
        ("cheb", 0.3),
        ("width_edge", 0.3),
        ("holes", 0.3),
        ("core", 0.5),
        ("facets", 0.5),
        ("stretch", 0.5),
        ("angle_cell", 0.3),
        ("waves", 0.4),
        ("blink", 0.4),
        ("relief", 0.5),
        ("warp_amount", 0.4),
        ("pulse", 0.0),
    ];

    #[test]
    fn standard_checks() {
        testing::check_all(build, BASE);
    }

    #[test]
    fn ms_per_frame() {
        if std::env::var("SCAFFOLD_MEASURE").is_ok() {
            eprintln!("voronoi: {:.2} ms/frame at 1280x720", testing::measure_ms(build));
        }
    }
}
