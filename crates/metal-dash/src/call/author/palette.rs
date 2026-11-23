//! Saved colour slots — `author::palette`'s three routes, moved.

use serde_json::json;

use crate::author::palette;
use crate::call::{CallError, Params};

use super::Ctx;

pub fn list(_params: &Params) -> Result<serde_json::Value, CallError> {
    Ok(palette::listing(crate::author::PALETTES))
}

pub fn save(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let Some(posted) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    // K: `{op:"set", id, slot, value, relative}` edits one slot of a stored
    // palette — relationally when `relative` is true, so the whole palette
    // moves with it (`metal_config::palettes::shift`, a pure function). Without
    // it, this is the same `save` the route always was, and a `set` would be
    // sent to it by mistake.
    if posted.get("op").and_then(serde_json::Value::as_str) == Some("set") {
        return set_slot(&posted);
    }
    match palette::save(crate::author::PALETTES, &posted) {
        Ok(n) => {
            println!("  palettes {n} → {}", crate::author::PALETTES);
            Ok(json!({ "ok": true }))
        }
        Err(e) => Err(CallError::Bad(e)),
    }
}

/// The `op: "set"` body of `POST /author/palettes`: move one slot of a stored
/// palette, relationally or not, and write the result back through the same
/// [`palette::save`] every other write uses (one writer, so the file's shape
/// cannot drift).
fn set_slot(body: &serde_json::Value) -> Result<serde_json::Value, CallError> {
    let Some(id) = body["id"].as_str() else {
        return Err(CallError::Bad("expected {id}".into()));
    };
    let Some(slot) = body["slot"].as_u64() else {
        return Err(CallError::Bad("expected {slot}".into()));
    };
    let Some(value) = body["value"].as_array() else {
        return Err(CallError::Bad("expected {value: [r, g, b]}".into()));
    };
    let rgb: Vec<f32> = value.iter().filter_map(serde_json::Value::as_f64).map(|v| v as f32).collect();
    if rgb.len() != 3 {
        return Err(CallError::Bad("value is three floats, r g b".into()));
    }
    let relative = body["relative"].as_bool().unwrap_or(false);

    let all = palette::load(crate::author::PALETTES);
    let Some(found) = all.iter().find(|p| p.name == id) else {
        return Err(CallError::Bad(format!("no palette named {id:?}")));
    };
    let Some(mut p) = as_config_palette(found) else {
        return Err(CallError::Bad(format!(
            "{id} is not a palette — {} floats",
            crate::author::palette::SLOT
        )));
    };
    if (slot as usize) >= p.slots.len() {
        return Err(CallError::Bad(format!("{id} has no slot {slot}")));
    }
    if relative {
        p = metal_config::shift(&p, slot as usize, [rgb[0], rgb[1], rgb[2]]);
    } else {
        p.slots[slot as usize] = [rgb[0], rgb[1], rgb[2]];
    }
    let colors = to_flat(&p);
    match palette::save(
        crate::author::PALETTES,
        &json!({ "name": id, "colors": colors }),
    ) {
        Ok(n) => Ok(json!({ "ok": true, "palettes": n, "colors": colors })),
        Err(e) => Err(CallError::Bad(e)),
    }
}

/// One stored palette (twelve flat floats, `crate::author::palette::Palette`)
/// as the four-slot [`metal_config::Palette`] the relational shift works on.
/// `None` when the stored `colors` is not a whole number of RGB triples.
fn as_config_palette(p: &palette::Palette) -> Option<metal_config::Palette> {
    if p.colors.len() % 3 != 0 {
        return None;
    }
    let slots: Vec<[f32; 3]> = p.colors.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    Some(metal_config::Palette { name: p.name.clone(), slots })
}

/// [`as_config_palette`]'s inverse — the shifted slots back into the twelve
/// flat floats `data/palettes.json` stores.
fn to_flat(p: &metal_config::Palette) -> Vec<f32> {
    p.slots.iter().flat_map(|c| c.iter().copied()).collect()
}

pub fn delete(_ctx: &mut Ctx, params: &Params) -> Result<serde_json::Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(name) = body["name"].as_str() else {
        return Err(CallError::Bad("expected {name}".into()));
    };
    match palette::remove(crate::author::PALETTES, name) {
        Ok(gone) => Ok(json!({ "ok": true, "removed": gone })),
        Err(e) => Err(CallError::Bad(e)),
    }
}
