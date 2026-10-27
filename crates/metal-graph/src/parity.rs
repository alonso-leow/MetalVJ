//! Parity lote P-A (2026-10-11): the image nodes TouchDesigner/Resolume/OBS
//! have and we lacked — `fx.chroma_key`, `fx.level`, `fx.lookup`, `fx.math`,
//! `fx.projection` — plus the `gen.ramp` gradient source and the `ctl.math`
//! control-row node.
//!
//! Each `fx.*` here follows the `fx.warp`/`fx.flash` pattern: its own floats,
//! each under its own config key (no widening of any model's schema, no
//! `Post` float), one image in, one image out. The non-float settings (a hex
//! key colour, a swizzle, a palette id, a `.cube` path, a ramp's stops) are
//! plain config strings beside the floats. `metal_perform::fx::parity` reads
//! both. Nothing here takes a second image input.
//!
//! `gen.ramp` is a *source*, not a model: it is not in
//! `registry::all_models()` (rule 1 untouched), so the painter rasterises it
//! itself and every `gen.*` reader that looks a model up finds none.
//!
//! `ctl.math` is a native script preset ([`crate::script_preset_floats`]): a
//! row in on `control`, the same row out, remapped — `Stage::Modulate` like
//! every `script.*`.

/// One float parameter: `(name, min, max, default)`.
pub type FloatDef = (&'static str, f32, f32, f32);

pub const CHROMA_KEY_EFFECT_ID: &str = "chroma_key";
pub const LEVEL_EFFECT_ID: &str = "level";
pub const LOOKUP_EFFECT_ID: &str = "lookup";
pub const MATH_EFFECT_ID: &str = "math";
pub const PROJECTION_EFFECT_ID: &str = "projection";

pub const CHROMA_KEY_KIND: &str = "fx.chroma_key";
pub const LEVEL_KIND: &str = "fx.level";
pub const LOOKUP_KIND: &str = "fx.lookup";
pub const MATH_KIND: &str = "fx.math";
pub const PROJECTION_KIND: &str = "fx.projection";

/// The five image effects, in catalog order (appended after `fx.flash`).
pub const PARITY_EFFECT_IDS: &[&str] =
    &[CHROMA_KEY_EFFECT_ID, LEVEL_EFFECT_ID, LOOKUP_EFFECT_ID, MATH_EFFECT_ID, PROJECTION_EFFECT_ID];

/// The gradient source.
pub const RAMP_KIND: &str = "gen.ramp";

/// The control-row math node.
pub const CTL_MATH_KIND: &str = "ctl.math";

/// `fx.chroma_key`: config `key` (`"#rrggbb"`, default green) plus these.
pub const CHROMA_KEY_FLOATS: &[FloatDef] = &[
    ("tolerance", 0.0, 1.0, 0.3),
    ("softness", 0.0, 1.0, 0.1),
    ("spill", 0.0, 1.0, 0.5),
    ("invert", 0.0, 1.0, 0.0),
];

/// `fx.level` (TD Level TOP). `channel`: 0 all, 1 r, 2 g, 3 b.
pub const LEVEL_FLOATS: &[FloatDef] = &[
    ("black_in", 0.0, 1.0, 0.0),
    ("white_in", 0.0, 1.0, 1.0),
    ("black_out", 0.0, 1.0, 0.0),
    ("white_out", 0.0, 1.0, 1.0),
    ("gamma", 0.1, 4.0, 1.0),
    ("brightness", -1.0, 1.0, 0.0),
    ("contrast", 0.0, 2.0, 1.0),
    ("opacity", 0.0, 1.0, 1.0),
    ("invert", 0.0, 1.0, 0.0),
    ("channel", 0.0, 3.0, 0.0),
];

/// `fx.lookup`: config `palette` (an id in `data/palettes`) or `cube` (a
/// `.cube` path under `data/`), plus these. `source`: 0 palette, 1 cube.
/// `channel` (palette only): 0 luma, 1 r, 2 g, 3 b, 4 a.
pub const LOOKUP_FLOATS: &[FloatDef] =
    &[("source", 0.0, 1.0, 0.0), ("channel", 0.0, 4.0, 0.0), ("mix", 0.0, 1.0, 1.0)];

/// `fx.math`: config `swizzle` (`"rgba"` → any order of `r g b a 0 1`) plus
/// these. Order of operations: swizzle, × mul, + add, abs, pow, invert,
/// clamp, luma → alpha.
pub const MATH_FLOATS: &[FloatDef] = &[
    ("mul_r", 0.0, 4.0, 1.0),
    ("mul_g", 0.0, 4.0, 1.0),
    ("mul_b", 0.0, 4.0, 1.0),
    ("mul_a", 0.0, 4.0, 1.0),
    ("add_r", -1.0, 1.0, 0.0),
    ("add_g", -1.0, 1.0, 0.0),
    ("add_b", -1.0, 1.0, 0.0),
    ("add_a", -1.0, 1.0, 0.0),
    ("pow", 0.1, 4.0, 1.0),
    ("abs", 0.0, 1.0, 0.0),
    ("invert", 0.0, 1.0, 0.0),
    ("clamp", 0.0, 1.0, 1.0),
    ("luma_alpha", 0.0, 1.0, 0.0),
];

/// `fx.projection` (Cube Map). `from`/`to`: 0 equirect, 1 cubemap cross
/// (4×3, horizontal), 2 fisheye (dome). Angles in degrees; `fov` is the
/// fisheye's field of view.
pub const PROJECTION_FLOATS: &[FloatDef] = &[
    ("from", 0.0, 2.0, 0.0),
    ("to", 0.0, 2.0, 2.0),
    ("yaw", -180.0, 180.0, 0.0),
    ("pitch", -90.0, 90.0, 0.0),
    ("roll", -180.0, 180.0, 0.0),
    ("fov", 10.0, 360.0, 180.0),
];

/// `gen.ramp`: config `palette` (an id) or `stops` (`[[r,g,b], …]`, 0..1,
/// evenly spaced; default black → white), plus these. `shape`: 0 linear,
/// 1 radial, 2 circular (angular), 3 diamond. A binding (or a `control`
/// cable's first value) drives `phase`.
pub const RAMP_FLOATS: &[FloatDef] = &[
    ("shape", 0.0, 3.0, 0.0),
    ("angle", 0.0, 360.0, 0.0),
    ("center_x", 0.0, 1.0, 0.5),
    ("center_y", 0.0, 1.0, 0.5),
    ("phase", 0.0, 1.0, 0.0),
    ("repeat", 1.0, 16.0, 1.0),
];

/// `ctl.math` (CHOP Math): per value of the row, remap `in` → `0..1`,
/// invert, curve (pow), quantize (`steps`, 0 = off), smooth (lag), then to
/// `out`.
pub const CTL_MATH_FLOATS: &[FloatDef] = &[
    ("in_low", 0.0, 1.0, 0.0),
    ("in_high", 0.0, 1.0, 1.0),
    ("out_low", 0.0, 1.0, 0.0),
    ("out_high", 0.0, 1.0, 1.0),
    ("curve", 0.1, 8.0, 1.0),
    ("smooth", 0.0, 0.99, 0.0),
    ("steps", 0.0, 64.0, 0.0),
    ("invert", 0.0, 1.0, 0.0),
];

/// The floats a P-A kind declares, `None` for every other kind.
pub fn floats_of(kind: &str) -> Option<&'static [FloatDef]> {
    Some(match kind {
        CHROMA_KEY_KIND => CHROMA_KEY_FLOATS,
        LEVEL_KIND => LEVEL_FLOATS,
        LOOKUP_KIND => LOOKUP_FLOATS,
        MATH_KIND => MATH_FLOATS,
        PROJECTION_KIND => PROJECTION_FLOATS,
        RAMP_KIND => RAMP_FLOATS,
        CTL_MATH_KIND => CTL_MATH_FLOATS,
        _ => return None,
    })
}

/// Whether `kind` is one of the five P-A image effects.
pub fn is_parity_effect(kind: &str) -> bool {
    kind.strip_prefix("fx.").is_some_and(|id| PARITY_EFFECT_IDS.contains(&id))
}

/// Whether `kind` runs as a script (a `script.*` or the `ctl.math` preset):
/// the one test every "is this a script" reader shares.
pub fn is_script_kind(kind: &str) -> bool {
    kind.starts_with("script.") || kind == CTL_MATH_KIND
}

/// The P-A kinds the catalog appends, with their ports — see
/// [`crate::NodeKind::catalog`].
pub(crate) fn kinds() -> Vec<crate::NodeKind> {
    use crate::{NodeFamily, NodeKind, Port, PortType};
    let mut out: Vec<NodeKind> = PARITY_EFFECT_IDS.iter().map(|id| crate::effect_kind(id)).collect();
    out.push(NodeKind::new(
        RAMP_KIND,
        NodeFamily::Generator,
        "Ramp",
        vec![Port::new("control", PortType::Control)],
        vec![Port::new("image", PortType::Image)],
    ));
    out.push(NodeKind::new(
        CTL_MATH_KIND,
        NodeFamily::Script,
        "Math (control)",
        vec![Port::new("control", PortType::Control), Port::new("floats", PortType::Control)],
        vec![Port::new("control", PortType::Control)],
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeFamily, NodeKind, PortType, Stage};

    fn catalog() -> Vec<NodeKind> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        let text = metal_vfs::read_to_string(path).expect("data/schema.json readable");
        NodeKind::catalog(&metal_schema::Manifest::parse(&text).expect("schema.json parses"))
    }

    /// Every P-A kind is in the catalog, once, with exactly the floats this
    /// module declares — counted from the tables, never written down.
    #[test]
    fn every_parity_kind_is_in_the_catalog_with_its_floats() {
        let cat = catalog();
        let mut kinds: Vec<String> = PARITY_EFFECT_IDS.iter().map(|id| format!("fx.{id}")).collect();
        kinds.push(RAMP_KIND.into());
        for kind in &kinds {
            let found: Vec<&NodeKind> = cat.iter().filter(|k| &k.kind == kind).collect();
            assert_eq!(found.len(), 1, "{kind} once in the catalog");
            let names: Vec<&str> = found[0].params.iter().map(|p| p.name.as_str()).collect();
            let want: Vec<&str> = floats_of(kind).unwrap().iter().map(|f| f.0).collect();
            assert_eq!(names, want, "{kind}'s params are its floats, in order");
            for (p, f) in found[0].params.iter().zip(floats_of(kind).unwrap()) {
                assert!(p.min <= f.3 && f.3 <= p.max, "{kind}.{} default inside its range", p.name);
            }
        }
        for id in PARITY_EFFECT_IDS {
            let k = cat.iter().find(|k| k.kind == format!("fx.{id}")).unwrap();
            assert_eq!(k.family, NodeFamily::Effect);
            assert_eq!(k.inputs.iter().filter(|p| p.ty == PortType::Image).count(), 1, "one image input, no second");
        }
        let ramp = cat.iter().find(|k| k.kind == RAMP_KIND).unwrap();
        assert!(ramp.inputs.iter().all(|p| p.ty == PortType::Control), "a source: no image in");
        assert_eq!(ramp.stage(), Stage::Source);
    }

    #[test]
    fn ctl_math_is_a_modulate_script_preset() {
        let cat = catalog();
        let k = cat.iter().find(|k| k.kind == CTL_MATH_KIND).expect("ctl.math in the catalog");
        assert_eq!(k.family, NodeFamily::Script);
        assert_eq!(k.stage(), Stage::Modulate);
        assert!(is_script_kind(CTL_MATH_KIND));
        assert_eq!(crate::script_preset_floats(CTL_MATH_KIND).map(<[_]>::len), Some(CTL_MATH_FLOATS.len()));
    }

    /// Rule 1: the ramp is not a model of the manifest.
    #[test]
    fn the_ramp_is_not_a_model() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        let text = metal_vfs::read_to_string(path).unwrap();
        let m = metal_schema::Manifest::parse(&text).unwrap();
        assert!(!m.models.iter().any(|x| format!("gen.{}", x.id) == RAMP_KIND));
    }
}
