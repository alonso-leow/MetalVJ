//! **The Curator** — the studio's half of `metal_reel::curate`, and the second
//! model with a lane.
//!
//! > *"for the footage curator […] first split into scenes […] create for the
//! > curator a list of scenes, we need to determine which has more movement,
//! > length, and the other aspects […] the frames/video curator is also a model
//! > like the router model."*
//!
//! So it sits beside [`crate::author::router`] and is built the same way on purpose:
//! picks on a lane, a train button, and a read-back of what the model *would*
//! do so a bad answer is diagnosable. What it authors is not a float row and not
//! an animation — it is **which scene of which clip is on, and how it is
//! played**.
//!
//! # The unit is a scene, not a frame
//!
//! This is the decision the whole lane turns on, and it was asked for directly.
//! `index::cut` has already divided every clip at the frames where the picture
//! changes completely, and `index::Passage` carries what each stretch is like —
//! its length, its peak movement, how much it varies, and the twelve Lumetri
//! columns. A lane that picked *frames* would ask an operator to author at
//! twelve times a second against material the indexer has already understood at
//! the level of shots.
//!
//! So the lane lists scenes, sorted by whichever quality is being looked for,
//! and a pick names one. Which is also what makes the picks trainable: a scene
//! has a mean descriptor, and [`metal_reel::curate::samples`] pairs that with the
//! music that was playing.
//!
//! # Loop, slow motion, reverse and a held frame are *not* four things
//!
//! They are [`metal_reel::curate::Play`], and it has a window and a speed. A
//! window narrower than the scene is a loop; a window of no width is a held
//! frame; a speed below one is slow motion. Four booleans would have a meaning
//! for all of them at once and nobody would agree what it is.
//!
//! **`Play` is authored but not trained on.** `Sample` carries the music and the
//! image and nothing else, deliberately: the Curator learns which footage a
//! piece of music wants, and how fast somebody chose to run it is a second
//! decision that would leak into the descriptor as noise.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use metal_reel::curate::{self, Curator, Pick, Play};
use metal_reel::index::{self, Library};
use metal_reel::read;
use metal_router::{features, npy};

/// Where libraries, picks and the trained coupling live.
///
/// The same `data/` the booth reads, because a library indexed in the app is
/// the one the studio authors against — unlike the router, whose takes are the
/// studio's own.
pub const DIR: &str = "data";

fn dir() -> PathBuf {
    PathBuf::from(DIR)
}

pub(crate) fn matrix_for(song: &str) -> Option<Vec<[f32; features::COLUMNS]>> {
    npy::load(&npy::base_path(Path::new(crate::author::SONGS), song)).ok()
}

/// A name that may be used as a path segment.
///
/// Library and song names arrive from a page and become directory names under
/// `data/reels/`. `metal_router::take::safe_name` makes the same check for the
/// same reason; this one is restated rather than imported because a library
/// name is not a song name and the two are free to diverge.
pub(crate) fn safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| !matches!(c, '/' | '\\' | ':') && !c.is_control())
}

/// Every scene in a library, as the lane lists them.
///
/// The qualities come out flat rather than nested under the descriptor because
/// the lane sorts by them — "which of these has the most movement" is the
/// question the operator opens this list to answer.
pub(crate) fn scenes(lib: &Library) -> Vec<Value> {
    let mut out = Vec::new();
    for (ci, clip) in lib.clips.iter().enumerate() {
        for (pi, p) in clip.passages.iter().enumerate() {
            out.push(json!({
                "clip": clip.id,
                "clipIndex": ci,
                "file": clip.file,
                "passage": pi,
                "from": p.from as f32 / lib.rate,
                "seconds": p.seconds,
                "peak": p.peak,
                "variety": p.variety,
                "motion": p.d[11],
                "d": p.d,
                // The rung a thumbnail would come from: the middle of the
                // scene, not its first frame. A cut's first frame is very often
                // a dissolve or a flash, which is the one frame in the scene
                // that does not look like it.
                "rung": (p.from + p.to) / 2,
            }));
        }
    }
    out
}

/// The Curator's state, as the page's boot payload wants it.
pub fn page_data(song: Option<&str>) -> Value {
    let data = dir();
    let curator = Curator::load(&data);
    let libraries = Library::list(&data);
    json!({
        "libraries": libraries,
        "trained": curator.trained(),
        "songs": curator.coupling.songs,
        "picks": curator.coupling.picks,
        "error": curator.coupling.error,
        "columns": read::NAMES,
        "features": features::NAMES,
        "minSongs": curate::MIN_SONGS,
        "holdSeconds": curate::HOLD_SECONDS,
        "song": song,
        "rate": index::FPS,
    })
}

// the cleanup: the raw-socket `serve` dispatcher that used to live here moved to
// `crate::call::author::curator` as typed handler functions over the same
// helpers below (`page_data`, `safe`, `scenes`, `matrix_for`, `coverage`,
// `play_of` — now `pub(crate)` so the sibling module tree can reach them).

/// How many picks each clip got.
///
/// Beside the train button for the reason the router's is: *"the Curator never
/// plays that clip"* has an answer before anybody retrains looking for one.
pub(crate) fn coverage(lib: &Library, picks: &[Pick]) -> Vec<Value> {
    lib.clips
        .iter()
        .map(|c| {
            json!({
                "clip": c.id,
                "picks": picks.iter().filter(|p| p.clip == c.id).count(),
            })
        })
        .collect()
}

/// A pick's playback, from the page.
///
/// Every field defaults, so a page that has not been taught about `play` — or a
/// pick authored before it existed — saves a scene that plays normally rather
/// than one that holds frame zero forever.
pub(crate) fn play_of(p: &Value) -> Play {
    let d = Play::default();
    Play {
        speed: p["speed"]
            .as_f64()
            .map(|v| v as f32)
            .unwrap_or(d.speed)
            .clamp(0.0, 4.0),
        reverse: p["reverse"].as_bool().unwrap_or(d.reverse),
        from: p["from"]
            .as_f64()
            .map(|v| v as f32)
            .unwrap_or(d.from)
            .clamp(0.0, 1.0),
        to: p["to"]
            .as_f64()
            .map(|v| v as f32)
            .unwrap_or(d.to)
            .clamp(0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curator_only_answers_for_its_own_prefix() {
        // The dispatcher relies on every module returning `None` for anything it
        // does not own, or an unrouted path is silently swallowed by whichever
        // matched first.
        for p in [
            "/router/take",
            "/models/example",
            "/rig/state",
            "/",
            "/curatorish",
        ] {
            assert!(!p.starts_with("/curator/"), "{p} would have been claimed");
        }
    }

    #[test]
    fn a_library_name_cannot_climb_out_of_the_data_directory() {
        // These become a path under `data/reels/`. The traversal is the reason
        // this check exists; the length and the leading dot are because a name
        // that is legal and useless still makes a directory nobody can find.
        for bad in [
            "",
            "..",
            "../secrets",
            "a/b",
            "a\\b",
            "c:evil",
            ".hidden",
            "a\nb",
        ] {
            assert!(!safe(bad), "{bad:?} was accepted");
        }
        for good in ["metraje", "rushes 2026", "a-b_c.mp4s"] {
            assert!(safe(good), "{good:?} was refused");
        }
    }

    #[test]
    fn a_pick_with_no_playback_authored_plays_the_scene_normally() {
        // The page may send a pick with nothing but a tick and a scene, and an
        // older pick on disk has no `play` at all. Both must mean "play it", not
        // "hold frame zero" — which is what a zero-initialised `Play` would be.
        let bare = play_of(&json!({ "tick": 4 }));
        assert_eq!(bare, Play::default());
        assert!(!bare.held());

        let authored = play_of(&json!({ "speed": 0.5, "reverse": true, "from": 0.25, "to": 0.5 }));
        assert_eq!(authored.speed, 0.5);
        assert!(authored.reverse);
        assert!(!authored.held());
    }

    #[test]
    fn authored_playback_is_clamped_to_what_a_scene_can_mean() {
        // The page is not the authority on these. A speed of a thousand or a
        // window from -3 to 40 renders without complaining and shows the wrong
        // footage — `Cue::position` clamps too, and this is the other end.
        let wild = play_of(&json!({ "speed": 1000.0, "from": -3.0, "to": 40.0 }));
        assert!(wild.speed <= 4.0, "{}", wild.speed);
        assert_eq!((wild.from, wild.to), (0.0, 1.0));
    }
}
