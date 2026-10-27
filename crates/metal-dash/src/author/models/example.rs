//! The sparse example: authored float keyframes for one model against one
//! song, and the disk format they round-trip through.
//!
//! Mirrors the rig's own sparse-tick convention (`metal_rig::frame::Track`) —
//! only authored ticks are stored, and everything between them is filled in
//! at export time by [`crate::author::dense`]. This is the file the studio's `save`
//! writes and `load` reads back; it is also the source of truth an author
//! keeps editing, which is why it stays sparse and human-sized rather than a
//! dense row per tick.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::author::models::schema::ModelSchema;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tick {
    pub tick: usize,
    pub floats: Vec<f32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Example {
    pub model: String,
    pub song: String,
    pub rate: f32,
    pub ticks: Vec<Tick>,
}

impl Example {
    /// Refuses anything whose shape does not match the model's own schema —
    /// the same discipline `Track::validate()` applies in `fw-studio`: a
    /// file this writes is a file the next reader, human or training script,
    /// can trust without re-checking it.
    pub fn validate(&self, schema: &ModelSchema) -> Result<(), String> {
        if self.model != schema.id {
            return Err(format!(
                "example says model {:?}, schema is {:?}",
                self.model, schema.id
            ));
        }
        let width = schema.width();
        for t in &self.ticks {
            if t.floats.len() != width {
                return Err(format!(
                    "tick {} has {} floats, {} ({}) takes {width}",
                    t.tick,
                    t.floats.len(),
                    schema.id,
                    schema.label
                ));
            }
            if !t.floats.iter().all(|v| v.is_finite()) {
                return Err(format!("tick {} has a NaN or infinite float in it", t.tick));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for t in &self.ticks {
            if !seen.insert(t.tick) {
                return Err(format!("tick {} is authored twice", t.tick));
            }
        }
        Ok(())
    }

    /// Sorted ascending by tick — the order [`crate::author::dense::interpolate`]
    /// assumes, and the order worth keeping on disk so a diff of the file
    /// reads as a timeline.
    fn sorted(mut self) -> Example {
        self.ticks.sort_by_key(|t| t.tick);
        self
    }

    /// Move a legacy 8 Hz example onto the canonical 32 Hz grid, **exactly**:
    /// every authored tick index ×4 and `rate` set to canonical. Returns
    /// whether it changed anything, and is idempotent — the `rate` field is the
    /// marker, so a file already on the canonical grid (or on an unknown one)
    /// is returned untouched rather than guessed at. This is the one-time
    /// upgrade the voice design notes § "Two complete levels, one switch" point 5
    /// describes for keys and segments; nothing is edited in place after it.
    pub fn upgrade_to_canonical(&mut self) -> bool {
        if (self.rate - metal_schema::TickRate::Base.hz() as f32).abs() > f32::EPSILON {
            return false;
        }
        for t in &mut self.ticks {
            t.tick = metal_schema::TickRate::Base.to_canonical(t.tick as i64) as usize;
        }
        self.rate = metal_schema::TickRate::CANONICAL.hz() as f32;
        true
    }
}

fn path_for(root: &Path, model: &str, song: &str) -> PathBuf {
    root.join(model).join(format!("{song}.json"))
}

/// One layer's override of the shared `(model, song)` corpus — a sibling
/// file, never a replacement. Addressed by `(device, layer)` because that is
/// the same addressing `POST /layers` already uses (`Layer::id`,
/// `layers.rs:352`), so two sources on the same model stop sharing one
/// keyframe file — see `the design notes` §15.3 for the bug this closes.
fn override_path_for(root: &Path, model: &str, song: &str, device: &str, layer: &str) -> PathBuf {
    root.join(model)
        .join(format!("{song}@{device}-{layer}.json"))
}

/// A device/layer id, as it lands in a file name. Ids are server-generated
/// (`l{z}-{n}` — `main.rs`'s layer-id comment) and safe today, but a request
/// body is not the place to trust that: the same discipline as `safe_stem`
/// in `models/mod.rs`, applied here rather than shared, because this guards
/// a path *component* (no `@` either, since that is the separator) and that
/// guards a full *stem*.
pub fn safe_component(raw: &str) -> Result<&str, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("empty device/layer id".into());
    }
    if name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('@')
        || name.contains('\0')
    {
        return Err(format!("{name:?} is not a valid device/layer id"));
    }
    Ok(name)
}

/// Whether `(model, song)` has its own `(device, layer)` override on disk —
/// distinct from what `load` tells you, because `load`'s `None` already means
/// two different things at once ("no file at all" and "not this one, fell
/// back to the shared corpus") and a caller that needs to tell those apart —
/// `Timeline::export`, to decide whether an override's own span should bound
/// which of its ticks count, a question the shared corpus is not asked — has
/// to ask *before* `load` already collapsed the answer, not after.
pub fn has_override(root: &Path, model: &str, song: &str, device: &str, layer: &str) -> bool {
    match (safe_component(device), safe_component(layer)) {
        (Ok(device), Ok(layer)) => metal_vfs::is_file(&override_path_for(root, model, song, device, layer)),
        _ => false,
    }
}

/// `None` covers both "no file yet" and "the file on disk is not valid
/// JSON" — either way there is nothing to hand back, and the caller's
/// response is the same: start from the schema's own defaults.
///
/// `target`, when given, is tried first as the per-layer override; falling
/// through to the shared file when the override does not exist (not yet
/// authored for that lane) keeps a brand-new layer showing the same picture
/// as the shared corpus until someone authors an override on purpose.
/// `target: None` is byte-identical to before this axis existed — same path,
/// same call.
pub fn load(root: &Path, model: &str, song: &str, target: Option<(&str, &str)>) -> Option<Example> {
    if let Some((device, layer)) = target {
        if let (Ok(device), Ok(layer)) = (safe_component(device), safe_component(layer)) {
            let path = override_path_for(root, model, song, device, layer);
            if let Ok(text) = metal_vfs::read_to_string(&path) {
                if let Some(ex) = read_back(&path, &text) {
                    return Some(ex);
                }
            }
        }
    }
    let path = path_for(root, model, song);
    let text = metal_vfs::read_to_string(&path).ok()?;
    read_back(&path, &text)
}

/// Parse, run the one-time 8→32 Hz key migration, and persist it once if it
/// changed anything. The `rate` field is the marker, so an already-canonical
/// file is not rewritten and the upgrade never runs twice.
fn read_back(path: &Path, text: &str) -> Option<Example> {
    let mut ex = serde_json::from_str::<Example>(text).ok()?;
    if ex.upgrade_to_canonical() {
        if let Ok(json) = serde_json::to_string_pretty(&ex) {
            let _ = crate::atomic::write(path, json);
        }
    }
    Some(ex.sorted())
}

/// `target: None` writes exactly where `save` always has — same path, same
/// bytes, same error shape — so a request with no `device`/`layer` in it
/// keeps producing today's file. `target: Some((device, layer))` writes the
/// sibling override instead, leaving the shared file (and every other
/// layer's own override) untouched.
pub fn save(
    root: &Path,
    example: Example,
    schema: &ModelSchema,
    target: Option<(&str, &str)>,
) -> Result<usize, String> {
    // New keyframes are written on the canonical grid; a caller that still
    // hands over a legacy 8 Hz example is upgraded rather than stored on the
    // old grid, so no new file is born legacy.
    let mut example = example.sorted();
    example.upgrade_to_canonical();
    example.validate(schema)?;
    let path = match target {
        Some((device, layer)) => {
            let device = safe_component(device)?;
            let layer = safe_component(layer)?;
            override_path_for(root, &example.model, &example.song, device, layer)
        }
        None => path_for(root, &example.model, &example.song),
    };
    if let Some(dir) = path.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&example).map_err(|e| e.to_string())?;
    crate::atomic::write(&path, json).map_err(|e| e.to_string())?;
    Ok(example.ticks.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author::models::schema::{Block, FloatSpec, ModelSchema};

    fn schema() -> ModelSchema {
        ModelSchema {
            id: "geoterrain".into(),
            label: "Geoterrain".into(),
            technique: String::new(),
            blocks: vec![
                Block::Floats {
                    fields: vec![FloatSpec {
                        name: "radius".into(),
                        min: 0.0,
                        max: 5.0,
                        default: 1.0,
                    }],
                },
                Block::ColorSlot {
                    name: "principal".into(),
                },
            ],
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-corpus-examples-{name}"));
        let _ = metal_vfs::remove_dir_all(&dir);
        dir
    }

    /// The one-time migration: a legacy 8 Hz keyframe sequence moves to the
    /// canonical 32 Hz grid with every key ×4 **exactly**, and the `rate` field
    /// is the marker that makes a second run a no-op.
    #[test]
    fn a_legacy_8hz_sequence_upgrades_keys_x4_exactly() {
        let mut ex = Example {
            model: "geoterrain".into(),
            song: "click-100".into(),
            rate: 8.0,
            ticks: vec![
                Tick { tick: 0, floats: vec![1.0; 13] },
                Tick { tick: 40, floats: vec![2.0; 13] },
                Tick { tick: 125, floats: vec![3.0; 13] },
            ],
        };
        assert!(ex.upgrade_to_canonical(), "a legacy file upgrades");
        assert_eq!(ex.rate, metal_schema::TickRate::CANONICAL.hz() as f32);
        let ticks: Vec<usize> = ex.ticks.iter().map(|t| t.tick).collect();
        assert_eq!(ticks, vec![0, 160, 500], "every key is exactly ×4");
        // The floats are untouched — only the grid moved.
        assert_eq!(ex.ticks[1].floats, vec![2.0; 13]);

        // Idempotent: the rate is the marker, so a canonical file answers false
        // and is not multiplied again (which would be ×16).
        assert!(!ex.upgrade_to_canonical());
        assert_eq!(ex.ticks[2].tick, 500, "a second run must not multiply again");
    }

    #[test]
    fn a_saved_example_round_trips_sorted_and_intact() {
        let root = scratch("roundtrip");
        let ex = Example {
            model: "geoterrain".into(),
            song: "click-100".into(),
            rate: 32.0,
            ticks: vec![
                Tick {
                    tick: 40,
                    floats: vec![2.0; 13],
                },
                Tick {
                    tick: 0,
                    floats: vec![1.0; 13],
                },
            ],
        };
        let n = save(&root, ex, &schema(), None).expect("save");
        assert_eq!(n, 2);
        let back = load(&root, "geoterrain", "click-100", None).expect("load");
        assert_eq!(back.ticks[0].tick, 0, "sorted ascending on disk");
        assert_eq!(back.ticks[1].tick, 40);
    }

    #[test]
    fn a_tick_with_the_wrong_width_is_refused_rather_than_written() {
        let root = scratch("badwidth");
        let ex = Example {
            model: "geoterrain".into(),
            song: "s".into(),
            rate: 8.0,
            ticks: vec![Tick {
                tick: 0,
                floats: vec![1.0, 2.0],
            }], // schema wants 13
        };
        assert!(save(&root, ex, &schema(), None).is_err());
        assert!(
            load(&root, "geoterrain", "s", None).is_none(),
            "a refused example must not reach disk"
        );
    }

    #[test]
    fn a_nan_float_is_refused() {
        let root = scratch("nan");
        let mut floats = vec![0.0; 13];
        floats[3] = f32::NAN;
        let ex = Example {
            model: "geoterrain".into(),
            song: "s".into(),
            rate: 8.0,
            ticks: vec![Tick { tick: 0, floats }],
        };
        assert!(save(&root, ex, &schema(), None).is_err());
    }

    #[test]
    fn a_duplicated_tick_is_refused() {
        let root = scratch("dup");
        let ex = Example {
            model: "geoterrain".into(),
            song: "s".into(),
            rate: 8.0,
            ticks: vec![
                Tick {
                    tick: 5,
                    floats: vec![0.0; 13],
                },
                Tick {
                    tick: 5,
                    floats: vec![1.0; 13],
                },
            ],
        };
        assert!(save(&root, ex, &schema(), None).is_err());
    }

    #[test]
    fn loading_a_missing_example_is_none_not_an_error() {
        let root = scratch("missing");
        assert!(load(&root, "geoterrain", "nope", None).is_none());
    }

    /// The acceptance test the plan names directly: a layer's override does
    /// not touch a sibling layer on the same `(model, song)`, and does not
    /// touch the shared corpus either.
    #[test]
    fn a_layer_override_does_not_touch_a_sibling_layer_or_the_shared_corpus() {
        let root = scratch("override-isolation");
        let shared = Example {
            model: "geoterrain".into(),
            song: "s".into(),
            rate: 8.0,
            ticks: vec![Tick {
                tick: 0,
                floats: vec![9.0; 13],
            }],
        };
        save(&root, shared, &schema(), None).expect("save shared");

        let lane_a = Example {
            model: "geoterrain".into(),
            song: "s".into(),
            rate: 8.0,
            ticks: vec![Tick {
                tick: 0,
                floats: vec![1.0; 13],
            }],
        };
        save(&root, lane_a, &schema(), Some(("dev1", "l1-1"))).expect("save lane a override");

        let lane_b = Example {
            model: "geoterrain".into(),
            song: "s".into(),
            rate: 8.0,
            ticks: vec![Tick {
                tick: 0,
                floats: vec![2.0; 13],
            }],
        };
        save(&root, lane_b, &schema(), Some(("dev1", "l1-2"))).expect("save lane b override");

        let back_a = load(&root, "geoterrain", "s", Some(("dev1", "l1-1"))).expect("load a");
        let back_b = load(&root, "geoterrain", "s", Some(("dev1", "l1-2"))).expect("load b");
        let back_shared = load(&root, "geoterrain", "s", None).expect("load shared");
        assert_eq!(back_a.ticks[0].floats[0], 1.0, "lane a's own override");
        assert_eq!(
            back_b.ticks[0].floats[0], 2.0,
            "lane b's own override, untouched by a's save"
        );
        assert_eq!(
            back_shared.ticks[0].floats[0], 9.0,
            "the shared corpus, untouched by either override"
        );
    }

    /// A layer with no override yet falls back to the shared corpus — so a
    /// freshly-created source shows the same picture as the song's shared
    /// keyframes until someone authors a change specific to that lane.
    #[test]
    fn a_layer_with_no_override_yet_falls_back_to_the_shared_corpus() {
        let root = scratch("override-fallback");
        let shared = Example {
            model: "geoterrain".into(),
            song: "s".into(),
            rate: 8.0,
            ticks: vec![Tick {
                tick: 0,
                floats: vec![5.0; 13],
            }],
        };
        save(&root, shared, &schema(), None).expect("save shared");
        let back = load(&root, "geoterrain", "s", Some(("dev1", "l9-9"))).expect("falls back");
        assert_eq!(back.ticks[0].floats[0], 5.0);
    }

    #[test]
    fn a_request_with_no_device_or_layer_writes_and_reads_the_exact_pre_fix_path() {
        let root = scratch("byte-identical");
        let ex = Example {
            model: "geoterrain".into(),
            song: "click-100".into(),
            rate: 8.0,
            ticks: vec![Tick {
                tick: 0,
                floats: vec![3.0; 13],
            }],
        };
        save(&root, ex, &schema(), None).expect("save");
        let expected = root.join("geoterrain").join("click-100.json");
        assert!(
            metal_vfs::exists(&expected),
            "must land at the exact pre-fix path with no device/layer"
        );
        assert!(
            !root
                .join("geoterrain")
                .join("click-100@none-none.json")
                .exists(),
            "no override sibling should be created when target is None"
        );
    }
}
