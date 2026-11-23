//! `POST /stage/frame {floats: {"l2.back.l.wash.level": 0.8, …}, venue?}` —
//! the lanes' scene clips at the playhead, sent to DMX.
//!
//! The floats are hitbox matrix names (`metal_stage::FloatName`). The venue
//! (the body's, else the one projected last) becomes a stage patch
//! (`metal_perform::stage::patch_of`), its [`metal_stage::Matrix`] expands the
//! floats to one signal per fixture, and each fixture's profile packs the
//! slots (`fixture::drive_venue_fixture_signal`) onto the DMX bus as the
//! source [`SOURCE`]. Only fixtures some float reaches are written: a scene
//! that names the back wash leaves the front alone.
//!
//! The floats may be written against the common matrix (`metal_stage::common`):
//! a name this venue lacks lands on a stand-in type or the nearest point holding
//! its type, so a scene cut on another venue still plays; `moved` reports where.
//!
//! The bus drops a source that stops writing after its timeout, so the lanes
//! post while they play or scrub and the scene lets go when they stop.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use metal_perform::dmx_bus::{DmxBus, SourceMode, DEFAULT_PRIORITY};
use metal_stage::{FloatName, Matrix};
use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

/// The bus source the lanes' scene clips write as.
pub const SOURCE: &str = "lanes/scenes";

pub fn post(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("body: {floats}".into()))?;
    let root = crate::output::data_root();
    let id = body["venue"]
        .as_str()
        .map(str::to_string)
        .or_else(|| metal_perform::venue::last(&root))
        .ok_or_else(|| CallError::Bad("no venue: project one first or name it".into()))?;
    let venue = metal_perform::venue::load(&root, &id).map_err(CallError::Bad)?;
    let floats: BTreeMap<String, f32> = body["floats"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
        .collect();
    Ok(send(&root, &venue, &floats, &mut ctx.state.dmx_bus, Instant::now()))
}

/// `GET|POST /stage/matrix {op?: "get"|"retarget", venue?, floats?}` — a venue
/// read as the common matrix. `get`: its cells (the fixtures each fans out to),
/// the dedup report and its float names; `retarget`: where `floats` land.
pub fn matrix(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().unwrap_or(Value::Null);
    let root = crate::output::data_root();
    let id = body["venue"]
        .as_str()
        .or(params.param("venue"))
        .map(str::to_string)
        .or_else(|| metal_perform::venue::last(&root))
        .ok_or_else(|| CallError::Bad("no venue: project one first or name it".into()))?;
    let venue = metal_perform::venue::load(&root, &id).map_err(CallError::Bad)?;
    let patch = metal_perform::stage::patch_of(&venue, &root);
    let op = body["op"].as_str().or(params.param("op")).unwrap_or("get");
    match op {
        "get" => {
            let d = metal_stage::common::dedup(&patch);
            Ok(json!({ "venue": id, "dedup": d, "floats": patch.matrix().names() }))
        }
        "retarget" => {
            let floats: BTreeMap<String, f32> = body["floats"]
                .as_object()
                .ok_or_else(|| CallError::Bad("retarget needs floats: {name: value}".into()))?
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
                .collect();
            let r = metal_stage::common::retarget(&patch.matrix(), &patch.frame, &floats);
            Ok(json!({ "venue": id, "retarget": r }))
        }
        other => Err(CallError::Bad(format!("op must be get or retarget, not {other}"))),
    }
}

/// The floats → per-fixture slots → the bus. Pure but for the bus, so a test
/// drives it with a tempdir venue and its own bus.
pub fn send(
    root: &Path,
    venue: &metal_perform::venue::Venue,
    floats: &BTreeMap<String, f32>,
    bus: &mut DmxBus,
    now: Instant,
) -> Value {
    let patch = metal_perform::stage::patch_of(venue, root);
    let m: Matrix = patch.matrix();
    let landed = metal_stage::common::retarget(&m, &patch.frame, floats);
    let (values, hit, mut unknown) = align(&m, &landed.floats);
    unknown.extend(landed.dropped);
    let signals = m.expand(&values);
    let mut universes: BTreeMap<u16, Vec<(u16, u8)>> = BTreeMap::new();
    let mut sent = Vec::new();
    for (id, signal) in signals.iter().filter(|(id, _)| hit.contains(*id)) {
        let Some(slots) = metal_perform::fixture::drive_venue_fixture_signal(root, venue, id, None, None, signal) else {
            continue;
        };
        let universe = venue.fixtures.iter().find(|f| &f.id == id).and_then(|f| f.universe).unwrap_or(0);
        universes.entry(universe).or_default().extend(slots);
        sent.push(id.clone());
    }
    for (u, slots) in &universes {
        bus.write(*u, SOURCE, DEFAULT_PRIORITY, SourceMode::Htp, slots, now);
    }
    json!({
        "venue": venue.venue.id,
        "fixtures": sent,
        "universes": universes.keys().collect::<Vec<_>>(),
        "unknown": unknown,
        "moved": landed.moved,
    })
}

/// The matrix's value row from named floats (rest values elsewhere), the
/// fixtures a given float reaches, and the names the patch cannot answer.
fn align(m: &Matrix, floats: &BTreeMap<String, f32>) -> (Vec<f32>, BTreeSet<String>, Vec<String>) {
    let rest = metal_stage::Channel::rest();
    let mut values: Vec<f32> = m.columns.iter().map(|c| c.channel.get(&rest)).collect();
    let mut hit = BTreeSet::new();
    let mut unknown = Vec::new();
    for (name, v) in floats {
        match FloatName::parse(name).and_then(|n| m.index(&n)) {
            Some(i) => {
                values[i] = *v;
                hit.extend(m.drives[i].iter().map(|(id, _)| id.clone()));
            }
            None => unknown.push(name.clone()),
        }
    }
    (values, hit, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_named_floats_reach_fixtures_and_unknown_names_are_reported() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let text = std::fs::read_to_string(root.join("agent/venues/stage.toml")).unwrap();
        let venue = metal_perform::venue::Venue::parse(&text).unwrap();
        let m = metal_perform::stage::patch_of(&venue, &root).matrix();
        let level = m.columns.iter().find(|c| c.channel == metal_stage::Channel::Level).expect("a level float");
        let floats: BTreeMap<String, f32> =
            [(level.to_string(), 1.0), ("l9.nowhere.x.wash.level".to_string(), 1.0)].into_iter().collect();
        let mut bus = DmxBus::new();
        let out = send(&root, &venue, &floats, &mut bus, Instant::now());
        let sent: Vec<&str> = out["fixtures"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        let expect: BTreeSet<&str> = m.drives[m.index(level).unwrap()].iter().map(|(id, _)| id.as_str()).collect();
        assert!(!sent.is_empty());
        assert!(sent.iter().all(|id| expect.contains(id)), "{sent:?} vs {expect:?}");
        assert_eq!(out["unknown"], json!(["l9.nowhere.x.wash.level"]));
        assert!(bus.source_packet_for_now(SOURCE).is_some(), "the bus holds the scene source");
    }

    #[test]
    fn a_common_float_the_venue_lacks_lands_somewhere_real() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let text = std::fs::read_to_string(root.join("agent/venues/stage.toml")).unwrap();
        let venue = metal_perform::venue::Venue::parse(&text).unwrap();
        let m = metal_perform::stage::patch_of(&venue, &root).matrix();
        // Over the audience, a pixel segment: no stage venue hangs one there.
        let far = "l3.house.c.led_bar.p2.level";
        assert!(m.index(&FloatName::parse(far).unwrap()).is_none());
        let floats: BTreeMap<String, f32> = [(far.to_string(), 1.0)].into_iter().collect();
        let out = send(&root, &venue, &floats, &mut DmxBus::new(), Instant::now());
        assert_eq!(out["unknown"], json!([]));
        assert_eq!(out["moved"][0][0], json!(far));
        assert!(!out["fixtures"].as_array().unwrap().is_empty(), "{out}");
    }
}
