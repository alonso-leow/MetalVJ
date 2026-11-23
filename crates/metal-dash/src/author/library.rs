//! The reuse library: named runs of keyframes you can drop into whatever you
//! are authoring.
//!
//! This started as a skeleton-only feature — save a phrase of rig rotations,
//! star the good ones, paste them back in somewhere else — and it is the part
//! of the studio that actually made authoring fast, because almost nothing
//! anyone authors is new. It is now the studio's mechanic rather than the
//! rig's: a movement can be a run of rig frames **or** a run of a model's
//! float rows, and both live in one library.
//!
//! That generalisation is the point of the merge, not a side effect of it.
//! "Copy a set of keyframes and reuse it" is the same operation whether the
//! keyframes are quaternions or floats, and having it only on one side meant
//! the models mode had no way to reuse anything at all — every animation was
//! authored from nothing, every time.
//!
//! ## The file format did not change
//!
//! `data/library/movements.json` is a JSON array of
//! `{name, favourite, ticks}`, exactly as it was. A **model** movement adds
//! one key, `model`, naming the schema it fits. Its absence means rig frames
//! — which is why this is an optional field and not a tag: every movement
//! saved before this existed still reads, unchanged, with no migration.
//!
//! ## Favourites still mean one thing
//!
//! A movement becomes a favourite when the operator confirms it is correct, and by
//! no other route. An agent posting a movement gets `favourite: false`
//! whatever it asked for. That rule is carried over verbatim: a star that
//! anything could set is a star that means nothing, and the library's whole
//! value is that the starred ones are known good.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One entry. `ticks` is deliberately `Value`: what a tick *is* depends on
/// `model`, and validating it is [`check`]'s job, done against the real type
/// on both sides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Movement {
    pub name: String,
    #[serde(default)]
    pub favourite: bool,
    /// The schema id these rows fit, for a model movement. `None` is a rig
    /// movement — see the module note on why absence rather than a tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `R22.8` point 4: the mask this sequence is cut by while it runs, an id
    /// in the target output's own `masks`. Absent (the default, and every
    /// movement saved before this) is no cut — `#[serde(default)]` keeps the
    /// older library file loading unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    pub ticks: Vec<Value>,
}

impl Movement {
    pub fn is_rig(&self) -> bool {
        self.model.is_none()
    }
}

/// What a caller must tell us to check a model movement: how wide that
/// model's row is. Passed in rather than looked up so this module does not
/// need the renderer, and so the check is the same in a test as in the server.
pub type Widths<'a> = &'a dyn Fn(&str) -> Option<usize>;

/// Reads the library. A missing or unreadable file is an empty library, not
/// an error: the first save creates it, and a studio that refused to start
/// because nobody had saved a movement yet would be absurd.
pub fn load(path: &str) -> Vec<Movement> {
    metal_vfs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Movement>>(&t).ok())
        .unwrap_or_default()
}

/// The listing the page shows: names, stars, lengths, and which mode each
/// belongs to — never the ticks themselves, which are large and which the
/// page only needs when it inserts one.
pub fn listing(path: &str) -> Value {
    let all = load(path);
    let rows: Vec<Value> = all
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "favourite": m.favourite,
                "ticks": m.ticks.len(),
                "kind": if m.is_rig() { "rig" } else { "model" },
                "model": m.model,
            })
        })
        .collect();
    json!({ "movements": rows, "path": path })
}

/// One movement by name, ticks and all.
pub fn get(path: &str, name: &str) -> Option<Movement> {
    load(path).into_iter().find(|m| m.name == name)
}

/// Checks a movement is made of real ticks, against whichever type it claims
/// to be.
///
/// The same discipline the rig library always applied, and for the same
/// reason: a library entry is pasted straight into what you are authoring, so
/// a movement the workspace cannot read is a track the workspace cannot read,
/// one insert later. Extended rather than relaxed for the model side — a float
/// row of the wrong width would be scrambled on the way into a renderer, with
/// camera floats landing in a colour slot and nothing anywhere reporting it.
pub fn check(m: &Movement, widths: Widths) -> Result<(), String> {
    if m.name.trim().is_empty() {
        return Err("a movement needs a name".into());
    }
    if m.ticks.is_empty() {
        return Err(format!("{} has no ticks in it", m.name));
    }
    match &m.model {
        None => {
            // Through the real type, one tick at a time.
            let frames: Vec<metal_rig::frame::Frame> =
                serde_json::from_value(Value::Array(m.ticks.clone()))
                    .map_err(|e| format!("{}: {e}", m.name))?;
            if frames.iter().any(|f| f.turns.contains_key(&0)) {
                return Err(format!("{} names node 0", m.name));
            }
        }
        Some(id) => {
            let Some(width) = widths(id) else {
                return Err(format!("{}: no model called {id:?}", m.name));
            };
            for (i, tick) in m.ticks.iter().enumerate() {
                let row = tick
                    .as_array()
                    .ok_or_else(|| format!("{}: tick {i} is not a row of floats", m.name))?;
                if row.len() != width {
                    return Err(format!(
                        "{}: tick {i} is {} floats, {id} takes {width}",
                        m.name,
                        row.len()
                    ));
                }
                if row.iter().any(|v| !v.is_f64() && !v.is_i64()) {
                    return Err(format!(
                        "{}: tick {i} has something that is not a number",
                        m.name
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Writes the library, having checked every movement in it.
///
/// Two shapes, and the difference is who is asking. The page posts the whole
/// library, stars and all, because the stars are the operator's and it is holding
/// them. Anything else posts **one movement**, and it lands pending whatever
/// it asked for.
pub fn save(path: &str, posted: &Value, widths: Widths) -> Result<usize, String> {
    let all: Vec<Movement> = match posted {
        Value::Array(_) => serde_json::from_value(posted.clone()).map_err(|e| e.to_string())?,
        Value::Object(_) => {
            let mut entry: Movement =
                serde_json::from_value(posted.clone()).map_err(|e| e.to_string())?;
            // A star is the operator's confirmation and arrives no other way.
            entry.favourite = false;
            let mut all = load(path);
            // A second save under one name replaces rather than doubles: a
            // proposal re-applied is a correction, not a new phrase.
            all.retain(|m| m.name != entry.name);
            all.push(entry);
            all
        }
        _ => return Err("the library is a list, or one movement".into()),
    };

    for m in &all {
        check(m, widths)?;
    }

    let p = Path::new(path);
    if let Some(dir) = p.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    crate::atomic::write(p, text).map_err(|e| e.to_string())?;
    Ok(all.len())
}

/// Lifts a run of ticks out of something already authored, so a good passage
/// of an existing example becomes a movement.
///
/// This is the half the rig library never had and the reason the user asked
/// for it: reuse was previously limited to what you had thought to save *as*
/// a movement while authoring it. Being able to take a slice of a finished
/// example after the fact is what makes a library of a corpus.
///
/// `ticks` is the authored, possibly sparse, source. `from`/`to` are tick
/// numbers, inclusive of `from` and exclusive of `to`. The result is
/// re-based to start at 0, because a movement is a shape, not a position —
/// where it goes is decided when it is inserted.
pub fn slice(ticks: &[(usize, Value)], from: usize, to: usize) -> Vec<Value> {
    let _ = from;
    ticks
        .iter()
        .filter(|(t, _)| *t >= from && *t < to)
        .map(|(_, v)| v.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths() -> impl Fn(&str) -> Option<usize> {
        |id: &str| match id {
            "turing" => Some(48),
            "geoterrain" => Some(18),
            _ => None,
        }
    }

    fn scratch(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("fw-library-{}-{name}", std::process::id()));
        metal_vfs::create_dir_all(&dir).expect("scratch");
        dir.join("movements.json").to_string_lossy().into_owned()
    }

    /// The compatibility gate. A library written before model movements
    /// existed must read back identically — no `model` key appearing, no
    /// migration, nothing to re-star.
    #[test]
    fn a_rig_movement_round_trips_in_the_old_shape_exactly() {
        let path = scratch("compat");
        let original = json!([{
            "name": "reach",
            "favourite": true,
            "ticks": [{ "turns": { "3": [0.0, 0.0, 0.0] } }]
        }]);
        metal_vfs::write(&path, original.to_string()).expect("seed");

        let loaded = load(&path);
        assert_eq!(loaded.len(), 1);
        assert!(
            loaded[0].is_rig(),
            "a movement with no model key is a rig movement"
        );
        assert!(loaded[0].favourite, "the star survived the read");

        save(&path, &json!(loaded), &widths()).expect("write back");
        let text = metal_vfs::read_to_string(&path).expect("read back");
        assert!(
            !text.contains("\"model\""),
            "a rig movement must not grow a model key"
        );
        let again = load(&path);
        assert_eq!(again.len(), 1);
        assert!(again[0].favourite, "the star survived the write");
    }

    #[test]
    fn a_model_movement_is_checked_against_that_models_row_width() {
        let good = Movement {
            name: "drop".into(),
            favourite: false,
            model: Some("geoterrain".into()),
            mask: None,
            ticks: vec![json!(vec![0.5f32; 18]), json!(vec![0.4f32; 18])],
        };
        assert!(check(&good, &widths()).is_ok());

        // The exact failure the manifest reconciliation was about: a row of
        // the wrong width is not rejected by anything downstream, it is
        // silently scrambled into the renderer.
        let wrong = Movement {
            mask: None,
            ticks: vec![json!(vec![0.5f32; 31])],
            ..good.clone()
        };
        let err = check(&wrong, &widths()).expect_err("a short row must be refused");
        assert!(
            err.contains("18"),
            "the error must say what the width should be: {err}"
        );

        let unknown = Movement {
            model: Some("nope".into()),
            ..good.clone()
        };
        assert!(
            check(&unknown, &widths()).is_err(),
            "an unknown model must be refused"
        );
    }

    #[test]
    fn a_rig_movement_still_cannot_name_node_zero() {
        // Carried over verbatim: 0 is not a node, and serde takes "0" as a map
        // key happily.
        let m = Movement {
            name: "bad".into(),
            favourite: false,
            model: None,
            mask: None,
            ticks: vec![json!({ "turns": { "0": [0.0, 0.0, 0.0] } })],
        };
        assert!(check(&m, &widths())
            .expect_err("must refuse")
            .contains("node 0"));
    }

    #[test]
    fn an_empty_or_nameless_movement_is_refused_in_both_kinds() {
        for model in [None, Some("turing".to_string())] {
            let m = Movement {
                name: "x".into(),
                favourite: false,
                model,
                mask: None,
            ticks: vec![],
            };
            assert!(
                check(&m, &widths()).is_err(),
                "an empty movement is not a movement"
            );
        }
        let m = Movement {
            name: "  ".into(),
            favourite: false,
            model: None,
            mask: None,
            ticks: vec![json!({ "turns": {} })],
        };
        assert!(check(&m, &widths()).is_err());
    }

    /// The star rule, which is the library's only real invariant.
    #[test]
    fn posting_one_movement_lands_it_pending_however_it_asked() {
        let path = scratch("star");
        let posted = json!({
            "name": "sneaky",
            "favourite": true,
            "model": "turing",
            "ticks": [vec![0.1f32; 48]]
        });
        save(&path, &posted, &widths()).expect("save");
        let all = load(&path);
        assert_eq!(all.len(), 1);
        assert!(
            !all[0].favourite,
            "a star must come from confirmation, not from the poster"
        );
    }

    #[test]
    fn saving_the_whole_library_keeps_the_stars_it_was_given() {
        // The page holds the stars, so a whole-library post is authoritative
        // about them — otherwise starring anything would be impossible.
        let path = scratch("wholelib");
        let posted = json!([{ "name": "good", "favourite": true,
                              "ticks": [{ "turns": { "5": [0.1, 0.0, 0.0] } }] }]);
        save(&path, &posted, &widths()).expect("save");
        assert!(load(&path)[0].favourite);
    }

    #[test]
    fn a_second_save_under_one_name_replaces_rather_than_doubles() {
        let path = scratch("replace");
        let one = json!({ "name": "same", "model": "geoterrain", "ticks": [vec![0.1f32; 18]] });
        let two = json!({ "name": "same", "model": "geoterrain",
                          "ticks": [vec![0.2f32; 18], vec![0.3f32; 18]] });
        save(&path, &one, &widths()).expect("first");
        save(&path, &two, &widths()).expect("second");
        let all = load(&path);
        assert_eq!(all.len(), 1, "a re-save is a correction, not a new phrase");
        assert_eq!(all[0].ticks.len(), 2, "the newer one won");
    }

    #[test]
    fn a_bad_movement_does_not_reach_disk() {
        // The whole point of checking before writing: a library with one
        // broken entry breaks every insert, not just that one.
        let path = scratch("atomic");
        save(
            &path,
            &json!({ "name": "fine", "ticks": [{ "turns": { "2": [0.0,0.0,0.0] } }] }),
            &widths(),
        )
        .expect("seed");
        let before = metal_vfs::read_to_string(&path).expect("read");
        let bad = json!({ "name": "broken", "model": "turing", "ticks": [vec![0.0f32; 3]] });
        assert!(save(&path, &bad, &widths()).is_err());
        assert_eq!(
            metal_vfs::read_to_string(&path).expect("read"),
            before,
            "the file changed anyway"
        );
    }

    /// Lifting a passage out of a finished example — the half that did not
    /// exist before, and the reason a library of a corpus is possible.
    #[test]
    fn a_slice_of_an_example_is_rebased_to_start_at_zero() {
        let authored: Vec<(usize, Value)> = (0..20).map(|t| (t, json!({ "tick": t }))).collect();
        let cut = slice(&authored, 5, 9);
        assert_eq!(cut.len(), 4, "from is inclusive, to is exclusive");
        assert_eq!(cut[0]["tick"], 5, "the content is the source's");
        // A movement is a shape, not a position: nothing in the result says
        // where it came from, so inserting it anywhere is the same operation.
        assert!(cut.iter().all(|v| v.get("offset").is_none()));
    }

    #[test]
    fn slicing_a_sparse_example_keeps_only_what_is_in_range() {
        // Authored examples are sparse — most ticks have nothing on them.
        let authored: Vec<(usize, Value)> = vec![
            (0, json!("a")),
            (7, json!("b")),
            (8, json!("c")),
            (30, json!("d")),
        ];
        assert_eq!(slice(&authored, 7, 9), vec![json!("b"), json!("c")]);
        assert!(
            slice(&authored, 9, 30).is_empty(),
            "an empty range is empty, not an error"
        );
    }
}
