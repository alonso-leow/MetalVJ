//! Palettes: the colour clipboard, given a name and a place on disk.
//!
//! `colorSlot()` in `fw-studio/page/suite.js` already drew four pickers per
//! colour block — twelve floats, four RGB triples — and already copied and
//! pasted a whole slot between models. Its own comment says why that paste is
//! safe where a keyframe paste would not be: *"a colour is fungible in a way a
//! keyframe is not: it means the same thing wherever it lands"*. Nothing about
//! twelve numbers is model-specific, so nothing has to match on the other side
//! for the paste to mean what it looked like it meant.
//!
//! What that clipboard did not have was a name or a tomorrow. It was one
//! variable in one closure; reloading the page lost it. This module is that
//! variable, written down. **Saving is the only new thing** — applying a
//! palette is the paste that already existed, called with numbers that came
//! off disk instead of numbers that came out of another slot.
//!
//! ## Why a sibling file and not a field on a movement
//!
//! `data/library/movements.json` ([`super::library`]) is next door and takes
//! the obvious-looking shape: it already stores named, reusable, pasteable
//! things, and its module note sets the rule this file follows too — a new
//! capability adds an **optional** field, absence means the old meaning, and
//! no migration ever runs.
//!
//! A palette was still put in `palettes.json` beside it rather than inside it,
//! and the reason is in `library::check`, not in taste. That function is the
//! movement library's whole point, and both of its arms are about **ticks**: a
//! movement with none is refused (`"has no ticks in it"`), and a movement with
//! some has every one of them checked — as a `metal_rig::frame::Frame`, or
//! against the row width of the model it names. A palette has no ticks. Not
//! zero ticks as a degenerate case: none, ever, by construction, because a
//! colour is not a thing that happens at a moment. Putting one in that file
//! means a third arm in `check` whose entire content is "this kind is exempt
//! from the check", which is the same as deleting the check for anything that
//! sets the new field.
//!
//! And it would not stay contained in `check`. Three places read that file and
//! two of them do not filter: `library::listing` (served at both
//! `/author/rig/library` and `/author/models/library`) and `rig::page_data`,
//! which dumps the raw array into the page for the rig's own library panel.
//! Both would start offering palettes as movements to paste into a timeline —
//! silent, in range, and wrong, which is the failure this project has now
//! named three times (§3.3, §11.7, and `library::check`'s own comment about
//! camera floats landing in a colour slot).
//!
//! So: same directory, same discipline, same route shapes, separate file. A
//! missing `palettes.json` is an empty set of palettes and not an error, for
//! the same reason a missing `movements.json` is an empty library — the first
//! save creates it, and there is nothing to migrate because there is nothing
//! there.
//!
//! ## No star, on purpose
//!
//! [`super::library`] carries one real invariant: a movement becomes a
//! favourite only when the operator confirms it, never because the poster asked. It
//! exists because a movement's listing (`{name, favourite, ticks: 12}`) cannot
//! show you whether the movement is any good — the star is the only carrier of
//! that judgement.
//!
//! A palette's listing carries the palette. Twelve floats are small enough to
//! ship whole (a movement's ticks deliberately are not), so the page draws the
//! actual gradient next to the name and you can see whether it is good. A star
//! there would add a field to maintain and a rule to enforce for something
//! already on screen, so there is no `favourite` here at all — not one that is
//! ignored, which would be the worthless star that note warns about.
//!
//! ## By value, never by name
//!
//! A [`Palette`] is a name and twelve numbers, and that is the whole type. It
//! does not record the model, the slot or the song it was lifted from, and
//! applying one writes the numbers into the row and stores nothing anywhere
//! saying where they came from. That is deliberate and it is the property
//! `colorSlot`'s comment is protecting: if a palette were a live reference,
//! renaming or editing one would silently repaint every model that had ever
//! used it, and an operator would have no way to see which those were. Twelve
//! numbers that travelled once cannot do that.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Floats in one colour slot: four RGB triples. The same twelve `colorSlot()`
/// draws four pickers for, and the same twelve a `color` block occupies in a
/// model's row (`metal-schema`'s `color_slot`).
pub const SLOT: usize = 12;

/// One palette. A name and twelve floats — see the module note on why there is
/// nothing else in here, in particular no provenance and no star.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    pub colors: Vec<f32>,
}

/// Reads the palettes. A missing or unreadable file is an empty set, not an
/// error — same reasoning as [`super::library::load`]: the first save creates
/// the file, and a studio that refused to start because nobody had saved a
/// palette yet would be absurd.
pub fn load(path: &str) -> Vec<Palette> {
    metal_vfs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Palette>>(&t).ok())
        .unwrap_or_default()
}

/// The listing the page shows — names **and colours**, which is where this
/// deliberately differs from [`super::library::listing`].
///
/// That one ships lengths and withholds the ticks because a movement's ticks
/// are large and only wanted at the moment of an insert. A palette is twelve
/// floats; withholding them would mean a second round trip to draw a swatch,
/// and the swatch is the only way to tell two palettes apart. So the listing
/// *is* the content, and applying one needs no fetch at all.
pub fn listing(path: &str) -> Value {
    let all = load(path);
    json!({ "palettes": all, "path": path })
}

/// Checks a palette is a palette.
///
/// Written for one failure in particular, and it is the colour version of the
/// one [`super::library::check`] names: a row sliced at the wrong offset. The
/// page reads a slot as `row.slice(at, at + 12)`, and `at` is accumulated
/// across a model's blocks — an off-by-one there yields twelve perfectly
/// ordinary-looking floats that are not a colour at all, and nothing
/// downstream would object, because a renderer takes whatever twelve floats it
/// is handed. Length and range are the two things that make the difference
/// visible: every colour slot in `data/schema.json` (107 of them, across 34
/// models) defaults inside 0..=1, and the page's only writer is `unhex()`,
/// which cannot leave it. A float outside that came from somewhere else in the
/// row.
pub fn check(p: &Palette) -> Result<(), String> {
    if p.name.trim().is_empty() {
        return Err("a palette needs a name".into());
    }
    if p.colors.len() != SLOT {
        return Err(format!(
            "{}: a palette is {SLOT} floats (four RGB), this has {}",
            p.name,
            p.colors.len()
        ));
    }
    for (i, v) in p.colors.iter().enumerate() {
        if !v.is_finite() || *v < 0.0 || *v > 1.0 {
            return Err(format!(
                "{}: float {i} is {v}, and a colour component is between 0 and 1",
                p.name
            ));
        }
    }
    Ok(())
}

/// Writes the palettes, having checked every one of them.
///
/// The two shapes are [`super::library::save`]'s two shapes, for the same
/// reason: the page holds the whole set and posts it back when it is
/// rearranging, and anything saving a single palette posts just that one. A
/// second save under one name replaces rather than doubles — re-saving a
/// palette you have just tweaked is a correction, not a second palette.
///
/// Nothing reaches disk unless every entry passed [`check`]. One broken entry
/// in the file breaks every listing, not just its own row.
pub fn save(path: &str, posted: &Value) -> Result<usize, String> {
    let all: Vec<Palette> = match posted {
        Value::Array(_) => serde_json::from_value(posted.clone()).map_err(|e| e.to_string())?,
        Value::Object(_) => {
            let entry: Palette =
                serde_json::from_value(posted.clone()).map_err(|e| e.to_string())?;
            let mut all = load(path);
            all.retain(|p| p.name != entry.name);
            all.push(entry);
            all
        }
        _ => return Err("the palettes are a list, or one palette".into()),
    };

    for p in &all {
        check(p)?;
    }

    write(path, &all)
}

/// Removes one palette by name. `Ok(false)` means there was no such palette —
/// not an error, because the only way to ask this question is to have been
/// looking at a listing, and a listing can be a moment out of date.
///
/// A named delete rather than "post the whole set without it", which is how
/// the movement library removes things: that works, and it is also how two
/// pages open at once lose a palette neither of them meant to touch. Deleting
/// one by name cannot take anything else with it.
pub fn remove(path: &str, name: &str) -> Result<bool, String> {
    let mut all = load(path);
    let before = all.len();
    all.retain(|p| p.name != name);
    if all.len() == before {
        return Ok(false);
    }
    write(path, &all)?;
    Ok(true)
}

fn write(path: &str, all: &[Palette]) -> Result<usize, String> {
    let p = Path::new(path);
    if let Some(dir) = p.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(all).map_err(|e| e.to_string())?;
    crate::atomic::write(p, text).map_err(|e| e.to_string())?;
    Ok(all.len())
}

// the cleanup: the raw-socket `serve` dispatcher that used to live here (its own
// note explained why palettes are dispatched once, unlike the movement
// library's two doors — see `crate::call::author::palette`, which carries
// that note forward) moved to `crate::call::author::palette` as typed
// handler functions over `listing`/`save`/`remove` above, unchanged.

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("fw-palette-{}-{name}", std::process::id()));
        metal_vfs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("palettes.json");
        let _ = metal_vfs::remove_file(&path);
        path.to_string_lossy().into_owned()
    }

    fn twelve(v: f32) -> Vec<f32> {
        vec![v; SLOT]
    }

    /// The claim the module note makes about `movements.json`, asserted rather
    /// than trusted: a palette put through the movement library's own check
    /// would be refused. This is the test that would fail first if someone
    /// later decided to merge the two files, and it is meant to — merging them
    /// means answering this, not deleting it.
    #[test]
    fn a_palette_is_not_a_movement_and_the_movement_check_says_so() {
        let widths = |id: &str| if id == "geoterrain" { Some(18) } else { None };

        // As a rig movement: twelve floats are not a `Frame`.
        let as_rig = crate::author::library::Movement {
            name: "atardecer".into(),
            favourite: false,
            model: None,
            mask: None,
            ticks: vec![json!(twelve(0.5))],
        };
        assert!(
            crate::author::library::check(&as_rig, &widths).is_err(),
            "twelve floats are not a rig frame"
        );

        // As a model movement: twelve floats are not any model's row width.
        let as_model = crate::author::library::Movement {
            model: Some("geoterrain".into()),
            ..as_rig.clone()
        };
        assert!(
            crate::author::library::check(&as_model, &widths).is_err(),
            "a colour slot is not a model row"
        );

        // And with no ticks at all — which is what a palette actually is —
        // both arms refuse it before they get as far as looking at anything.
        let empty = crate::author::library::Movement {
            mask: None,
            ticks: vec![],
            ..as_rig
        };
        assert!(crate::author::library::check(&empty, &widths)
            .expect_err("must refuse")
            .contains("no ticks"));
    }

    /// The compatibility gate, the same one the movement library has. Nothing
    /// migrates: a file written by an older build reads back identically, and
    /// a missing file is an empty set rather than a failure to start.
    #[test]
    fn a_missing_file_is_an_empty_set_and_the_first_save_creates_it() {
        let path = scratch("empty");
        assert!(load(&path).is_empty(), "no file is no palettes");
        let listed = listing(&path);
        assert_eq!(listed["palettes"].as_array().map(|a| a.len()), Some(0));

        save(
            &path,
            &json!({ "name": "atardecer", "colors": twelve(0.4) }),
        )
        .expect("first save");
        assert!(metal_vfs::exists(&Path::new(&path)), "the first save created the file");
        assert_eq!(load(&path).len(), 1);
    }

    /// The listing ships the colours, unlike the movement library's. A swatch
    /// with no colours in it is the reason this route exists at all.
    #[test]
    fn the_listing_carries_the_colours_themselves() {
        let path = scratch("listing");
        save(&path, &json!({ "name": "ember", "colors": twelve(0.75) })).expect("save");
        let listed = listing(&path);
        let row = &listed["palettes"][0];
        assert_eq!(row["name"], "ember");
        assert_eq!(
            row["colors"]
                .as_array()
                .expect("colours in the listing")
                .len(),
            SLOT,
            "the page must be able to draw the gradient without a second request"
        );
    }

    /// A wrongly-sliced row is the failure this check was written for: twelve
    /// floats read from the wrong offset look like a palette and are not one.
    #[test]
    fn a_slot_that_is_not_twelve_floats_in_range_is_refused() {
        let short = Palette {
            name: "corto".into(),
            colors: vec![0.5; 9],
        };
        let err = check(&short).expect_err("nine floats are not a colour slot");
        assert!(
            err.contains("12"),
            "the error must say what the width should be: {err}"
        );

        // Out of range: `density` is 180.0 and `twist` is 9.0 in the real
        // manifest, so a slot read one block early is full of numbers like
        // these — the exact shape of an off-by-one in the block walk.
        let stray = Palette {
            name: "corrido".into(),
            colors: {
                let mut c = twelve(0.5);
                c[0] = 180.0;
                c
            },
        };
        assert!(check(&stray).is_err(), "180.0 is not a colour component");

        let nan = Palette {
            name: "nan".into(),
            colors: {
                let mut c = twelve(0.5);
                c[7] = f32::NAN;
                c
            },
        };
        assert!(
            check(&nan).is_err(),
            "a NaN in a colour is a black hole in a render"
        );

        let nameless = Palette {
            name: "  ".into(),
            colors: twelve(0.5),
        };
        assert!(
            check(&nameless).is_err(),
            "a palette with no name cannot be found again"
        );
    }

    #[test]
    fn a_second_save_under_one_name_replaces_rather_than_doubles() {
        let path = scratch("replace");
        save(&path, &json!({ "name": "misma", "colors": twelve(0.1) })).expect("first");
        save(&path, &json!({ "name": "misma", "colors": twelve(0.2) })).expect("second");
        let all = load(&path);
        assert_eq!(all.len(), 1, "a re-save is a correction, not a new palette");
        assert_eq!(all[0].colors[0], 0.2, "the newer one won");
    }

    /// The whole-set shape, which is what a page rearranging its palettes
    /// posts. Same two shapes as the movement library's `save`.
    #[test]
    fn the_whole_set_can_be_posted_at_once() {
        let path = scratch("wholeset");
        save(&path, &json!({ "name": "vieja", "colors": twelve(0.9) })).expect("seed");
        let n = save(
            &path,
            &json!([
                { "name": "una", "colors": twelve(0.1) },
                { "name": "otra", "colors": twelve(0.2) },
            ]),
        )
        .expect("whole set");
        assert_eq!(n, 2);
        let all = load(&path);
        assert_eq!(
            all.len(),
            2,
            "the posted set is the set, not an addition to it"
        );
        assert!(all.iter().all(|p| p.name != "vieja"));
    }

    #[test]
    fn a_bad_palette_does_not_reach_disk() {
        // The same atomicity the movement library has, and for the same
        // reason: one unreadable entry breaks the whole listing, not its row.
        let path = scratch("atomic");
        save(&path, &json!({ "name": "buena", "colors": twelve(0.3) })).expect("seed");
        let before = metal_vfs::read_to_string(&path).expect("read");
        assert!(save(
            &path,
            &json!([
                { "name": "buena", "colors": twelve(0.3) },
                { "name": "rota", "colors": vec![0.1f32; 5] },
            ])
        )
        .is_err());
        assert_eq!(
            metal_vfs::read_to_string(&path).expect("read"),
            before,
            "the file changed anyway"
        );
    }

    #[test]
    fn a_delete_takes_exactly_one_and_a_missing_one_is_not_an_error() {
        let path = scratch("delete");
        save(
            &path,
            &json!([
                { "name": "una", "colors": twelve(0.1) },
                { "name": "otra", "colors": twelve(0.2) },
            ]),
        )
        .expect("seed");
        assert!(remove(&path, "una").expect("delete"), "it was there");
        let left = load(&path);
        assert_eq!(left.len(), 1);
        assert_eq!(
            left[0].name, "otra",
            "the delete took exactly the one named"
        );
        assert!(
            !remove(&path, "una").expect("second delete"),
            "deleting what is already gone reports that, it does not fail"
        );
    }

    /// The property the module note is really about. A palette carries no
    /// reference to where it was applied, so editing it afterwards cannot
    /// reach back into anything.
    #[test]
    fn a_palette_is_twelve_numbers_and_carries_no_link_to_where_it_went() {
        let path = scratch("byvalue");
        save(&path, &json!({ "name": "fija", "colors": twelve(0.6) })).expect("save");
        let text = metal_vfs::read_to_string(&path).expect("read");
        let raw: Value = serde_json::from_str(&text).expect("parse");
        // Sorted, because `serde_json`'s map is a `BTreeMap` here and reading
        // this in declaration order would be asserting a fact about serde
        // rather than about the palette.
        let mut keys: Vec<&str> = raw[0]
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["colors", "name"],
            "a palette is a name and twelve numbers — anything else is a link waiting to happen"
        );

        // And editing it does not change what an earlier apply wrote: an apply
        // is a copy of these numbers into a row, and the row is somewhere else
        // entirely.
        let applied_earlier = load(&path)[0].colors.clone();
        save(&path, &json!({ "name": "fija", "colors": twelve(0.05) })).expect("edit");
        assert_eq!(
            applied_earlier[0], 0.6,
            "the copy that already travelled is untouched"
        );
    }
}
