//! The manifest on disk has to be the manifest the renderers describe.
//!
//! `data/schema.json` is what `fw-corpus` authors examples
//! against: an example is a flat row of floats, and the only thing that makes
//! it mean anything later is that the row's width and block order match the
//! model that will consume it. Those two drifted once already, and the
//! Membrane case drifted in the way that cannot be noticed by hand — same
//! total width, camera block in a different place. So this is a gate, not a
//! convention.

#[test]
fn the_manifest_on_disk_matches_the_renderers() {
    let path = metal_render::manifest::schema_path();
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        on_disk,
        metal_render::manifest::manifest_json(),
        "{} is stale — run `cargo run -p metal-render --bin emit-schema`",
        path.display()
    );
}

// The gate that checked `metal_reel::grade::DEFAULT` against transmission floats
// was removed 2026-10-25 with `wake`, the last transmission: with none
// registered it checks nothing. Restore it with the next footage model.

/// H4: every model whose schema has a `camera` block declares `camera_track`,
/// so a `cam.track` node can offer it as a destination. Computed from the
/// manifest (rule 7), never a hand-written list.
#[test]
fn every_model_with_a_camera_block_declares_camera_track() {
    let text = std::fs::read_to_string(metal_render::manifest::schema_path()).expect("data/schema.json readable");
    let manifest = metal_schema::Manifest::parse(&text).expect("schema.json parses");
    let has_camera = |m: &metal_schema::ManifestModel| {
        m.blocks.iter().any(|b| matches!(b, metal_schema::ManifestBlock::Camera { .. }))
    };
    let with_camera: Vec<&str> = manifest.models.iter().filter(|m| has_camera(m)).map(|m| m.id.as_str()).collect();
    assert!(!with_camera.is_empty(), "no model declares a camera block; this gate checks nothing");
    for m in manifest.models.iter().filter(|m| has_camera(m)) {
        assert!(m.camera_track, "{} has a camera block but does not declare camera_track", m.id);
    }
}

#[test]
fn every_model_id_is_unique_and_url_safe() {
    let models = metal_render::registry::all_models();
    let mut seen = std::collections::HashSet::new();
    for m in &models {
        let id = m.id();
        assert!(seen.insert(id), "two models share the id {id:?}");
        assert!(
            !id.is_empty()
                && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{id:?} is a path segment and a query value; keep it ascii-lowercase"
        );
    }
}
