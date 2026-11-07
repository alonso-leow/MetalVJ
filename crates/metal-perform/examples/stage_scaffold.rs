//! Cut stage-fixture authoring examples from real audio ticks on a real patch.
//!
//! cargo run -p metal-perform --example stage_scaffold -- \
//!   <venue.toml> <data root> <songs dir> <parts.json> <out dir> [stride=8] [part-id ...]
//!
//! One `<part>.json` per part (all parts when none is named): music rows from the song's
//! canonical digest, `stage` pre-filled with `metal_stage::dataset::draft`, `approved:false`.
use metal_perform::{stage, venue::Venue};
use std::path::Path;

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 5 {
        return Err("usage: stage_scaffold <venue.toml> <data root> <songs dir> <parts.json> <out dir> [stride] [part ...]".into());
    }
    let (root, songs) = (Path::new(&a[1]), Path::new(&a[2]));
    let venue = Venue::parse(&std::fs::read_to_string(&a[0]).map_err(|e| e.to_string())?)?;
    let patch = stage::patch_of(&venue, root);
    let parts: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[3]).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let stride: u32 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(8);
    let want: Vec<&String> = a.iter().skip(6).collect();
    std::fs::create_dir_all(&a[4]).map_err(|e| e.to_string())?;
    let mut n = 0;
    for (song, s) in parts["songs"].as_object().ok_or("no songs")? {
        let Some(list) = s["parts"].as_array() else { continue };
        let mut rows: Option<Vec<Vec<f32>>> = None;
        for p in list {
            let id = p["id"].as_str().unwrap_or_default();
            if !want.is_empty() && !want.iter().any(|w| w.as_str() == id) {
                continue;
            }
            if rows.is_none() {
                match metal_router::npy::load_song(songs, song, metal_schema::TickRate::CANONICAL) {
                    Ok(r) => rows = Some(r.iter().map(|x| x.to_vec()).collect()),
                    Err(e) => {
                        eprintln!("skip {song}: {e}");
                        break;
                    }
                }
            }
            let span = [p["start_tick"].as_u64().unwrap_or(0) as u32, p["end_tick"].as_u64().unwrap_or(0) as u32];
            let ex = metal_stage::dataset::scaffold(&patch, song, id, p["kind"].as_str().unwrap_or(""), 32.0, rows.as_ref().unwrap(), span, stride)?;
            std::fs::write(Path::new(&a[4]).join(format!("{id}.json")), serde_json::to_string_pretty(&ex).unwrap()).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    println!("{n} examples on patch `{}` ({} floats)", patch.name, patch.matrix().columns.len());
    Ok(())
}
