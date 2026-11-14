//! Build the scene review set: every desk scene and every look (the scenes a timeline
//! holds on at once), on a target venue, with the cells the scenes leave empty filled
//! by `metal_stage::voices`.
//!
//! cargo run -p metal-perform --example scene_review -- \
//!   <scenes dir> <placements.json> <fixtures.json> <songs.json> <target venue.toml> <data root> <out dir>
//!
//! Writes `<out>/venue.json` (the target's fixtures: id, kind, at, aim, spot, pixels),
//! `<out>/index.json` (one row per item) and `<out>/items/<id>.json`: per key, every
//! fixture's signal (`level hue sat strobe pan tilt zoom`) and which values are synthetic.
use metal_perform::{stage, venue::Venue};
use metal_stage::{common, scene, voices, FloatName, Kind, Spot};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// The longest a look plays before it loops in review.
const LOOK_MS: u32 = 16_000;

fn read(p: &Path) -> Result<Value, String> {
    serde_json::from_str(&std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?).map_err(|e| e.to_string())
}

/// Floats at `t` of a clip (the key at or before `t`, looped).
fn at(clip: &scene::Clip, t: u32) -> BTreeMap<String, f32> {
    if clip.keys.is_empty() {
        return BTreeMap::new();
    }
    let t = if clip.length_ms > 0 { t % clip.length_ms } else { 0 };
    let k = clip.keys.iter().rev().find(|k| k.ms <= t).unwrap_or(&clip.keys[0]);
    clip.columns.iter().cloned().zip(k.floats.iter().copied()).collect()
}

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 7 {
        return Err("usage: scene_review <scenes dir> <placements.json> <fixtures.json> <songs.json> <venue.toml> <data root> <out dir>".into());
    }
    let (scenes_dir, out) = (Path::new(&a[0]), Path::new(&a[6]));
    let places: HashMap<u64, Spot> = read(Path::new(&a[1]))?["by_id"]
        .as_object()
        .ok_or("placements: no by_id")?
        .iter()
        .filter_map(|(k, v)| Some((k.parse().ok()?, Spot::parse(v.as_str()?)?)))
        .collect();
    let kinds: HashMap<u64, Kind> = read(Path::new(&a[2]))?
        .as_array()
        .ok_or("fixtures: not a list")?
        .iter()
        .filter_map(|f| Some((f["id"].as_u64()?, scene::kind_of_desk_type(f["type"].as_str()?)?)))
        .collect();
    let venue = Venue::parse(&std::fs::read_to_string(&a[4]).map_err(|e| e.to_string())?)?;
    let patch = stage::patch_of(&venue, Path::new(&a[5]));
    let m = patch.matrix();
    std::fs::create_dir_all(out.join("items")).map_err(|e| e.to_string())?;

    // The target rig, for the viewer.
    let rig: Vec<Value> = patch
        .fixtures
        .iter()
        .zip(patch.placements())
        .map(|(f, p)| {
            json!({ "id": f.id, "kind": f.kind.word(), "at": f.at, "yaw": p.yaw, "pitch": p.pitch,
                    "spot": format!("{}.{}", p.point.word(), p.variant.word()),
                    "segments": common::segments(f) })
        })
        .collect();
    std::fs::write(out.join("venue.json"), json!({ "id": venue.venue.id, "frame": patch.frame, "fixtures": rig }).to_string())
        .map_err(|e| e.to_string())?;

    // Every scene, by slug and by name.
    let mut clips: BTreeMap<String, (scene::Scene, scene::Clip)> = BTreeMap::new();
    let mut by_name: HashMap<String, String> = HashMap::new();
    for e in std::fs::read_dir(scenes_dir).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        let Some(slug) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
        let Ok(s) = serde_json::from_str::<scene::Scene>(&std::fs::read_to_string(&p).unwrap_or_default()) else { continue };
        let c = scene::to_clip(&s, &places, &kinds);
        by_name.entry(s.name.trim().to_lowercase()).or_insert_with(|| slug.clone());
        clips.insert(slug, (s, c));
    }

    // Looks: per timeline, the scene each track holds at every cue; one look per set.
    let songs = read(Path::new(&a[3]))?;
    let mut looks: BTreeMap<Vec<String>, Vec<Value>> = BTreeMap::new();
    for (song, s) in songs["songs"].as_object().into_iter().flatten() {
        let mut per_tl: BTreeMap<String, Vec<(f64, String, String)>> = BTreeMap::new();
        for c in s["scenes"].as_array().into_iter().flatten() {
            let name = c["scene"].as_str().unwrap_or_default().trim().to_lowercase();
            if let Some(slug) = by_name.get(&name) {
                per_tl.entry(c["timeline"].as_str().unwrap_or_default().to_string()).or_default().push((
                    c["start_s"].as_f64().unwrap_or(0.0),
                    c["track"].as_str().unwrap_or_default().to_string(),
                    slug.clone(),
                ));
            }
        }
        for (_, mut cues) in per_tl {
            cues.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut held: BTreeMap<String, String> = BTreeMap::new();
            for (t, track, slug) in cues {
                held.insert(track, slug);
                let set: Vec<String> = held.values().cloned().collect::<BTreeSet<_>>().into_iter().collect();
                if set.len() > 1 {
                    looks.entry(set).or_default().push(json!({ "song": song, "t": t }));
                }
            }
        }
    }

    // One item: floats per key time → retarget → fill the empty cells → per-fixture signals.
    let render = |id: &str, times: Vec<u32>, floats_at: &dyn Fn(u32) -> BTreeMap<String, f32>| -> Value {
        let mut keys = Vec::new();
        for t in times {
            let r = common::retarget(&m, &patch.frame, &floats_at(t));
            let mut vals: Vec<Option<f32>> = vec![None; m.columns.len()];
            let mut written = BTreeMap::new();
            for (n, v) in &r.floats {
                if let Some(i) = FloatName::parse(n).and_then(|f| m.index(&f)) {
                    vals[i] = Some(*v);
                    written.insert(m.columns[i], *v);
                }
            }
            // A cell the scene touched keeps its own values; only untouched cells are filled.
            let touched: BTreeSet<(metal_stage::Point, Kind)> = written.keys().map(|c| (c.point, c.kind)).collect();
            let empty: Vec<usize> = (0..m.columns.len()).filter(|&i| !touched.contains(&(m.columns[i].point, m.columns[i].kind))).collect();
            let mut synth = BTreeSet::new();
            for (i, v) in voices::fill(&m, &written, &empty) {
                vals[i] = Some(v);
                synth.insert(metal_stage::common::base_id(&m.drives[i][0].0).to_string());
            }
            let rest = metal_stage::Channel::rest();
            let row: Vec<f32> = vals.iter().enumerate().map(|(i, v)| v.unwrap_or_else(|| m.columns[i].channel.get(&rest))).collect();
            let sig: BTreeMap<String, [f32; 7]> = m
                .expand(&row)
                .into_iter()
                .map(|(k, s)| (k, [s.level, s.hue, s.sat, s.strobe, s.pan, s.tilt, s.zoom]))
                .collect();
            keys.push(json!({ "ms": t, "fixtures": sig, "synthetic": synth, "moved": r.moved.len(), "dropped": r.dropped.len() }));
        }
        json!({ "id": id, "keys": keys })
    };

    let mut index = Vec::new();
    for (slug, (s, c)) in &clips {
        let times: Vec<u32> = c.keys.iter().map(|k| k.ms).collect::<BTreeSet<_>>().into_iter().take(32).collect();
        let item = render(slug, times.clone(), &|t| at(c, t));
        std::fs::write(out.join("items").join(format!("{slug}.json")), item.to_string()).map_err(|e| e.to_string())?;
        index.push(json!({ "id": slug, "type": "scene", "name": s.name, "folder": s.folder, "length_ms": c.length_ms,
                           "keys": times.len(), "tags": scene::tags(s), "dropped": c.dropped.len() }));
    }
    for (i, (set, uses)) in looks.iter().enumerate() {
        let parts: Vec<&scene::Clip> = set.iter().filter_map(|s| clips.get(s).map(|(_, c)| c)).collect();
        let len = parts.iter().map(|c| c.length_ms).max().unwrap_or(0).clamp(1, LOOK_MS);
        let mut times: BTreeSet<u32> = BTreeSet::new();
        for c in &parts {
            let l = c.length_ms.max(1);
            let mut base = 0;
            while base < len && times.len() < 64 {
                times.extend(c.keys.iter().map(|k| base + k.ms).filter(|t| *t < len));
                base += l;
            }
        }
        times.insert(0);
        let times: Vec<u32> = times.into_iter().take(48).collect();
        // Layers: a later scene in the set wins a float; levels take the highest.
        let merged = |t: u32| {
            let mut f: BTreeMap<String, f32> = BTreeMap::new();
            for c in &parts {
                for (k, v) in at(c, t) {
                    let e = f.entry(k.clone()).or_insert(v);
                    *e = if k.ends_with(".level") { e.max(v) } else { v };
                }
            }
            f
        };
        let id = format!("look-{i:04}");
        let item = render(&id, times.clone(), &merged);
        std::fs::write(out.join("items").join(format!("{id}.json")), item.to_string()).map_err(|e| e.to_string())?;
        let names: Vec<&str> = set.iter().filter_map(|s| clips.get(s).map(|(sc, _)| sc.name.as_str())).collect();
        index.push(json!({ "id": id, "type": "look", "name": names.join(" + "), "scenes": set, "uses": uses.len(),
                           "songs": uses.iter().filter_map(|u| u["song"].as_str()).collect::<BTreeSet<_>>(), "length_ms": len, "keys": times.len() }));
    }
    let looks_n = looks.len();
    std::fs::write(out.join("index.json"), Value::Array(index).to_string()).map_err(|e| e.to_string())?;
    eprintln!("{} scenes, {looks_n} looks → {}", clips.len(), out.display());
    Ok(())
}
