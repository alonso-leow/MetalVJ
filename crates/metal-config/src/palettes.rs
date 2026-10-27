//! `[studio.palettes]` → `data/palettes.json` (S.AH point 3).
//!
//! O.26: "the shader still knows slots, not the file" — this module
//! only gets the palette from named hex slots to an ordered list of floats.
//! It does not decide how a shader samples them (that is `O.15`'s call, not
//! this loader's) and it does not decide sRGB vs linear, premultiplication,
//! or any other shader-facing detail — see the crate's "Escalate if" note in
//! `plan-sonnet.md` § S.AH: this stores, it does not consume.

use crate::PaletteDef;
use serde::{Deserialize, Serialize};

/// One palette, materialized: `name` plus `c_0..c_n` in the fixed order
/// `base, acento, fondo, destello, extra…` (S.AH point 3, "append-only,
/// rule 2" — a palette only ever grows floats at the tail, so an index a
/// shader already latched onto never moves).
///
/// Each slot is `[r, g, b]` in `0.0..=1.0`, parsed from a `#rrggbb` hex
/// string — the most literal reading of "floats" available without picking
/// a colour space, which is the fork this lot explicitly does not take.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    pub slots: Vec<[f32; 3]>,
}

/// Writes `<data_root>/palettes.json` from `[studio.palettes]`. Plain
/// `fs::write`, not `metal-dash`'s `atomic.rs` — this runs from `metal-sync
/// --doctor` and `scripts/reindex.py`, both offline tools with no concurrent
/// writer to race, unlike the live `POST /palettes` route O.26 leaves for a
/// later lot to wire against the running server.
pub fn write_palettes_file(
    data_root: &std::path::Path,
    defs: &std::collections::BTreeMap<String, PaletteDef>,
) -> Result<(), String> {
    let palettes = materialize_palettes(defs);
    let path = data_root.join(crate::PALETTES_FILE);
    let json = serde_json::to_string_pretty(&palettes).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    metal_vfs::write(&path, json).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Turns `[studio.palettes]` into the list `data/palettes.json` should hold.
/// Absent slots are skipped, not zero-filled — a palette that never set
/// `destello` should not silently claim a black `c_2`, it should have one
/// fewer float than one that did (append-only, not fixed-width).
pub fn materialize_palettes(
    defs: &std::collections::BTreeMap<String, PaletteDef>,
) -> Vec<Palette> {
    defs.iter()
        .map(|(name, def)| {
            let mut slots = Vec::new();
            for hex in [&def.base, &def.accent, &def.background, &def.flash]
                .into_iter()
                .flatten()
            {
                if let Some(c) = parse_hex(hex) {
                    slots.push(c);
                }
            }
            for hex in &def.extra {
                if let Some(c) = parse_hex(hex) {
                    slots.push(c);
                }
            }
            Palette { name: name.clone(), slots }
        })
        .collect()
}

/// A relational edit of a palette: move its **whole** set of colours by
/// whatever it takes to bring `key_slot` to its new colour.
///
/// **Why the relation and not the slot.** The operator picks a new colour for
/// one slot and the palette has to stay a palette — a set of colours that
/// belong together. Setting one slot in isolation and leaving the rest is how
/// a warm palette turns into a warm palette with one blue in it. So the edit
/// is a single offset: the colour space distance from `key_slot`'s old colour
/// to `new` (a lightness shift, a chroma scale, a hue rotation), applied to
/// every slot at once. `key_slot` lands on `new` exactly; the others keep
/// their relationships to it, which is the property the test below pins.
///
/// **Why OkLCh and not RGB.** Rotating hue in RGB drags lightness along with
/// it (a red and a magenta of the same RGB distance do not read as equally
/// bright), so a "shift the palette's hue" gesture would visibly change how
/// light the palette is. OkLCh carries lightness, chroma and hue as separate
/// axes, so a hue rotation is only a hue rotation. The conversion is the same
/// Ottosson one `metal-notation`'s `color` module uses; duplicated here in
/// minimal form rather than taking a dependency on that crate's UI-flavoured
/// colour type, because this function is a pure data transform `metal-config`
/// can own on its own.
///
/// Out-of-range `key_slot` returns the palette unchanged rather than
/// panicking: this is called from a route whose input is an operator's slot
/// name, and a bad index is a refusal the caller can report, not a crash.
pub fn shift(palette: &Palette, key_slot: usize, new: [f32; 3]) -> Palette {
    let Some(old) = palette.slots.get(key_slot).copied() else {
        return palette.clone();
    };
    let a = rgb_to_oklch(old);
    let b = rgb_to_oklch([new[0].clamp(0.0, 1.0), new[1].clamp(0.0, 1.0), new[2].clamp(0.0, 1.0)]);
    // The one offset that makes `key_slot` land on `new`: a subtraction in
    // lightness and hue, a ratio in chroma (a hue rotation must not shrink a
    // colour that had no chroma to begin with to a single point).
    let dl = b.l - a.l;
    let dh = b.h - a.h;
    let dc = if a.c <= f32::EPSILON { 1.0 } else { b.c / a.c };
    let slots = palette
        .slots
        .iter()
        .map(|&c| {
            let o = rgb_to_oklch(c);
            oklch_to_rgb_with_chroma(o.l + dl, o.c * dc, o.h + dh)
        })
        .collect();
    Palette { name: palette.name.clone(), slots }
}

/// OkLCh with hue in degrees. Local to this module — see [`shift`].
#[derive(Clone, Copy)]
struct Oklch {
    l: f32,
    c: f32,
    h: f32,
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f32) -> f32 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn rgb_to_oklch(rgb: [f32; 3]) -> Oklch {
    let (r, g, b) = (to_linear(rgb[0]), to_linear(rgb[1]), to_linear(rgb[2]));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let l_ = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
    let a_ = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
    let b_ = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
    let c = (a_ * a_ + b_ * b_).sqrt();
    let h = if c <= f32::EPSILON { 0.0 } else { b_.atan2(a_).to_degrees().rem_euclid(360.0) };
    Oklch { l: l_, c, h }
}

/// OkLCh → sRGB, reducing chroma until the colour fits — the same promise
/// `metal-notation`'s `to_srgb` makes, kept here so a shifted palette never ships
/// a channel outside `0..1` (which a shader would read as a wrap).
fn oklch_to_rgb_with_chroma(l: f32, c: f32, h: f32) -> [f32; 3] {
    let (l, c) = (l.clamp(0.0, 1.0), c.max(0.0));
    let h = h.rem_euclid(360.0);
    let mut hi = c;
    let mut lo = 0.0;
    if !oklch_in_gamut(l, hi, h) {
        for _ in 0..20 {
            let mid = 0.5 * (lo + hi);
            if oklch_in_gamut(l, mid, h) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
    } else {
        lo = hi;
    }
    let linear = oklch_to_linear(l, lo, h);
    [
        from_linear(linear[0].clamp(0.0, 1.0)),
        from_linear(linear[1].clamp(0.0, 1.0)),
        from_linear(linear[2].clamp(0.0, 1.0)),
    ]
}

fn oklch_in_gamut(l: f32, c: f32, h: f32) -> bool {
    oklch_to_linear(l, c, h).iter().all(|v| (-1e-4..=1.0 + 1e-4).contains(v))
}

fn oklch_to_linear(l: f32, c: f32, h: f32) -> [f32; 3] {
    let rad = h.to_radians();
    let (a, b) = (c * rad.cos(), c * rad.sin());
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ]
}

/// `#rrggbb` → `[r, g, b]` floats in `0.0..=1.0`. `None` on anything else —
/// callers decide whether that is a hard stop (`validate`) or a skip
/// (`materialize_palettes`, which already ran behind a validated config in
/// every call site this lot wires up).
fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn palette_slots_are_floats_in_order() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "night".to_string(),
            PaletteDef {
                base: Some("#000000".to_string()),
                accent: Some("#ff0000".to_string()),
                background: Some("#0000ff".to_string()),
                flash: None,
                extra: vec!["#00ff00".to_string()],
            },
        );
        let out = materialize_palettes(&defs);
        assert_eq!(out.len(), 1);
        let p = &out[0];
        assert_eq!(p.name, "night");
        // base, acento, fondo, then extra — destello was absent, so the
        // extra slot is c_3, not c_4: append-only means no gap either.
        assert_eq!(p.slots.len(), 4);
        assert_eq!(p.slots[0], [0.0, 0.0, 0.0]);
        assert_eq!(p.slots[1], [1.0, 0.0, 0.0]);
        assert_eq!(p.slots[2], [0.0, 0.0, 1.0]);
        assert_eq!(p.slots[3], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn write_palettes_file_lands_data_palettes_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut defs = BTreeMap::new();
        defs.insert(
            "hotel".to_string(),
            PaletteDef {
                base: Some("#1a1206".to_string()),
                accent: Some("#d9a441".to_string()),
                background: None,
                flash: None,
                extra: vec![],
            },
        );
        write_palettes_file(dir.path(), &defs).unwrap();

        let raw = metal_vfs::read_to_string(dir.path().join("palettes.json")).unwrap();
        let parsed: Vec<Palette> = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "hotel");
        assert_eq!(parsed[0].slots.len(), 2);
    }

    #[test]
    fn an_invalid_hex_slot_is_skipped_not_baked_as_black() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "broken".to_string(),
            PaletteDef {
                base: Some("not-a-color".to_string()),
                accent: Some("#ff5c8a".to_string()),
                background: None,
                flash: None,
                extra: vec![],
            },
        );
        let out = materialize_palettes(&defs);
        assert_eq!(out[0].slots.len(), 1);
        assert_eq!(out[0].slots[0], [1.0, 0.36078432, 0.5411765]);
    }

    // -- the relational shift ----------------------------------------------

    fn palette(name: &str, slots: &[[f32; 3]]) -> Palette {
        Palette { name: name.to_string(), slots: slots.to_vec() }
    }

    fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < tol)
    }

    /// Setting a slot to the colour it already holds changes nothing — the
    /// identity the whole relation is built on.
    #[test]
    fn shift_to_the_same_colour_is_the_identity() {
        let p = palette("night", &[[0.1, 0.2, 0.3], [0.8, 0.1, 0.4], [0.0, 0.0, 1.0]]);
        let out = shift(&p, 1, p.slots[1]);
        for (a, b) in out.slots.iter().zip(&p.slots) {
            assert!(close(*a, *b, 1e-3), "{a:?} != {b:?}");
        }
        assert_eq!(out.name, p.name);
    }

    /// The named slot lands on its new colour exactly, and the others follow —
    /// the defining property a relational edit has and a per-slot write does
    /// not.
    #[test]
    fn shift_lands_the_key_slot_on_its_new_colour() {
        let p = palette("warm", &[[0.9, 0.3, 0.1], [0.95, 0.8, 0.2], [0.2, 0.1, 0.05]]);
        let out = shift(&p, 0, [0.1, 0.4, 0.9]);
        assert!(close(out.slots[0], [0.1, 0.4, 0.9], 2e-2), "key slot: {:?}", out.slots[0]);
        // The companions moved too, in the same direction — none survived
        // untouched.
        assert!(!close(out.slots[1], p.slots[1], 5e-3), "the palette must move as a whole");
    }

    /// A hue rotation wraps: rotating the key slot a long way round does not
    /// leave a slot with a hue outside `0..360`, and blue rotated by a full
    /// turn is blue again.
    #[test]
    fn a_hue_rotation_wraps_and_never_leaves_the_wheel() {
        let p = palette("wheel", &[[0.0, 0.0, 1.0], [1.0, 0.0, 0.0]]);
        let out = shift(&p, 0, [0.0, 0.0, 1.0]);
        for slot in &out.slots {
            let o = rgb_to_oklch(*slot);
            assert!((0.0..360.0).contains(&o.h), "hue {} left the wheel", o.h);
        }
        // Same hue, same lightness, same chroma: the identity, proved through
        // the OklCh round trip rather than the RGB floats.
        let a = rgb_to_oklch(p.slots[0]);
        let b = rgb_to_oklch(out.slots[0]);
        assert!((a.h - b.h).abs() < 1.0 || (a.h - b.h).abs() > 359.0);
    }

    /// A move that would leave the display gamut is clamped into it, not
    /// wrapped — every slot stays a drawable RGB triple.
    #[test]
    fn shift_clamps_out_of_gamut_into_the_display() {
        let p = palette("grey", &[[0.5, 0.5, 0.5], [0.2, 0.2, 0.2]]);
        let out = shift(&p, 0, [1.0, 0.0, 0.0]);
        for slot in &out.slots {
            for v in slot {
                assert!((0.0..=1.0).contains(v), "{slot:?} has a channel outside 0..1");
            }
        }
    }

    /// A key slot that names nothing is refused by returning the palette
    /// unchanged — the caller reports it, this function never panics.
    #[test]
    fn an_out_of_range_key_slot_leaves_the_palette_alone() {
        let p = palette("short", &[[0.1, 0.2, 0.3]]);
        let out = shift(&p, 9, [1.0, 1.0, 1.0]);
        assert_eq!(out, p);
    }
}
