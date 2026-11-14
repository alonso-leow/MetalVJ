//! Print where every fixture of a venue sits in the stage matrix.
//!
//! cargo run -p metal-perform --example stage_places -- <venue.toml> <data root>
//!
//! One JSON object: `{fixture id: {"spot": "l2.pro.x.beta", "name", "address", "kind"}}`,
//! the spot being the fixture's place and variant (`metal_stage::Spot`).
use metal_perform::{stage, venue::Venue};
use std::path::Path;

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        return Err("usage: stage_places <venue.toml> <data root>".into());
    }
    let venue = Venue::parse(&std::fs::read_to_string(&a[0]).map_err(|e| e.to_string())?)?;
    let patch = stage::patch_of(&venue, Path::new(&a[1]));
    let mut out = serde_json::Map::new();
    for (f, p) in patch.fixtures.iter().zip(patch.placements()) {
        let v = venue.fixtures.iter().find(|x| x.id == f.id);
        out.insert(
            f.id.clone(),
            serde_json::json!({
                "spot": format!("{}.{}", p.point.word(), p.variant.word()),
                "name": v.map(|x| x.name.clone()),
                "address": v.and_then(|x| x.address),
                "kind": f.kind.word(),
            }),
        );
    }
    println!("{}", serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?);
    Ok(())
}
