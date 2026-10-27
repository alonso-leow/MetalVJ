//! Emits `data/schema.json` — the float manifest `fw-corpus`
//! authors examples against.
//!
//! **Why generated and not hand-written.** `docs/SUITE_STUDIO.md` chose a
//! shared JSON file over a shared Rust type so the two crates could be built
//! and iterated on independently, and flagged reconciling the two by hand as
//! a task still to do. By the time anyone looked, the hand-written file and
//! the renderers disagreed about three models — and one of those, Membrane,
//! disagreed only about *block order* while the total width still matched, so
//! nothing anywhere could have caught it: every row authored against it would
//! have been silently scrambled on the way into the renderer, camera floats
//! landing in a colour slot.
//!
//! Generating the file keeps the decoupling that mattered (the corpus crate
//! still reads plain JSON and has no dependency on this one) and removes the
//! part that was never going to hold: two people keeping two lists in step by
//! remembering to. The renderers are the truth here because they are what
//! actually consumes the row.
//!
//! `cargo run -p metal-render --bin emit-schema` rewrites the file;
//! `tests/schema.rs` fails if the file on disk has drifted from the models.

use serde_json::{json, Value};

use crate::model::Block;
use crate::registry::all_models;

/// The manifest as `fw-corpus::schema::Manifest` deserialises it:
/// `{rate, models: [{id, label, technique, blocks}]}`, blocks internally
/// tagged by `type` in snake_case.
pub fn manifest() -> Value {
    let models: Vec<Value> = all_models()
        .iter()
        // Tombstones keep a table position but are not offered — see
        // `registry.rs`. `data/schema.json` is the catalog `metal-graph` reads,
        // so a retired row must not appear here.
        .filter(|m| !m.retired())
        .map(|m| {
            let blocks: Vec<Value> = m
                .schema()
                .blocks
                .iter()
                .map(|b| match b {
                    Block::Floats(specs) => {
                        let fields: Vec<Value> = specs
                            .iter()
                            .map(|s| {
                                let mut f = json!({
                                    "name": s.name,
                                    "min": s.min,
                                    "max": s.max,
                                    "default": s.default,
                                    // `drive` was missing from this file for as long as
                                    // it has existed on `FloatSpec`, and the omission
                                    // was invisible because the only reader was the
                                    // corpus, which does not drive anything. It stopped
                                    // being invisible when `metal-dash` learned to run
                                    // without the renderers: a manifest with no `drive`
                                    // reads every float as freely drivable, and Turing
                                    // driven freely renders a flat blank sheet with
                                    // nothing out of range and nothing in any log (see
                                    // `FloatSpec::drive`). Emitted for the same reason
                                    // the two keys below are: a consumer that wants it
                                    // should not have to reconstruct it.
                                    "drive": s.drive,
                                });
                                // Only a rig float carries a control, and only
                                // `monster` has rig floats so far. Written
                                // when present so a no-GPU consumer's
                                // `face::has_rig` sees it too; omitted
                                // otherwise, so the file stays small and old
                                // consumers ignore a key they do not know.
                                if let Some(c) = s.rig {
                                    f["rig"] = json!(c);
                                }
                                f
                            })
                            .collect();
                        json!({ "type": "floats", "fields": fields })
                    }
                    // `default` is extra here for the same reason it is on a camera:
                    // the corpus crate's own `Block::ColorSlot` carries only a
                    // name and falls back to white, which serde ignores this
                    // key to keep doing. It is emitted because a slot that
                    // starts somewhere other than white is now a thing a model
                    // can say, and a manifest that dropped it would hand the
                    // corpus a different opening row than the renderer uses.
                    Block::ColorSlot(name, start) => {
                        json!({ "type": "color_slot", "name": name, "default": start })
                    }
                    // `default` is extra: the corpus crate's `Block::Camera`
                    // carries no fields and falls back to one hardcoded view,
                    // which serde ignores this key to keep doing. It is
                    // emitted anyway because the per-model camera default is
                    // now the difference between a model opening framed and
                    // opening edge-on (`docs/CORRECTIONS.md` §43), and the
                    // corpus side can start reading it whenever it wants to.
                    Block::Camera(view) => json!({ "type": "camera", "default": view }),
                })
                .collect();
            json!({
                "id": m.id(),
                "label": m.name(),
                "technique": "",
                // Whether this is a transmission — a model that draws over
                // source material rather than generating a picture. It is in
                // the file because the consumer that most needs it is the one
                // with no renderers: `metal-dash` on a host with no adapter still
                // has to draw the surface that says a transmission cannot be
                // selected until a folder has been chosen.
                "footage": m.footage(),
                // Whether this model draws its background via the shared
                // `Backdrop` pass — see `Model::backdrop`. The twelve models
                // where marking `fondo` as alpha does anything.
                "backdrop": m.backdrop(),
                // Whether a `cam.track` node may drive this model's view —
                // see `Model::camera_track`. `metal-graph`'s `cam.track`
                // resolution reads this off the manifest, the same way it
                // reads `footage`, so a no-GPU consumer can answer "which
                // generators are valid targets" without linking the
                // renderers.
                "camera_track": m.camera_track(),
                // The parts a surface picture can be laid on — see
                // `Model::surfaces`. Empty for most models; each name becomes a
                // `tex.<part>` input on the generated node in `metal-graph`.
                "surfaces": m.surfaces(),
                "blocks": blocks,
            })
        })
        .collect();

    json!({ "rate": 8.0, "models": models })
}

/// Pretty JSON with a trailing newline — the shape a diff of the file reads
/// well in, and what `emit-schema` writes.
pub fn manifest_json() -> String {
    let mut text = serde_json::to_string_pretty(&manifest()).expect("manifest serialises");
    text.push('\n');
    text
}

/// Where the file lives, resolved from this crate rather than the working
/// directory so it is the same path from a test, a binary, or a different cwd.
pub fn schema_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json")
}
