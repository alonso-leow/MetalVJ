//! 2026-10-01: a video's scenes, and the operator's book of the ones worth
//! keeping.
//!
//! Two files, two lifetimes:
//!
//! - **The cut list** (`<slug>.scenes.json` beside a HAP proxy, plus its
//!   posters in `<slug>.scenes/`) is *derived*: [`cuts`] over the rows the
//!   ingest pass already decoded, the same [`index::cut`] a library uses.
//!   Re-ingesting a clip rewrites it, and nothing else points into it.
//! - **The book** (`data/scenes/favorites.json`, [`BOOK`]) is *authored*: the
//!   scenes the operator saved or favourited, under a name, and the named
//!   groups they were filed into. It is synced (metal-sync's allowlist), and it
//!   is what `reel.batch` reads when it is handed a group.
//!
//! The two meet at the scene id, `<hap slug>#<start_ms>-<end_ms>` ([`id`]):
//! the cutter is deterministic over the same footage at the same rate, so a
//! re-ingest gives the same ids and a favourite finds its scene again. A
//! scene the cutter no longer produces (the clip was re-encoded) keeps its
//! row in the book — its clip, start and end are all a player needs.
//!
//! **The book's shape is a contract** shared with `reel.batch`; it is
//! written exactly as [`SceneBook`] serialises and read leniently (every
//! field defaults), so a hand-edited or older file still loads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::index::{self, Passage};
use crate::read::{self, Rgb, COLUMNS};

/// The book, relative to the data root.
pub const BOOK: &str = "scenes/favorites.json";

// ---------------------------------------------------------------------------
// the book
// ---------------------------------------------------------------------------

/// `data/scenes/favorites.json`: every kept scene, and the groups.
///
/// "The global favourites" are the scenes with `favourite` set — there is no
/// separate list to drift from the flags. Groups are optional named subsets
/// (an artist's two installations, each its own focus); a scene may sit in
/// several, and deleting a group never touches a scene's flags.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneBook {
    #[serde(default)]
    pub scenes: BTreeMap<String, Scene>,
    #[serde(default)]
    pub groups: BTreeMap<String, Vec<String>>,
}

/// One kept scene: a stretch of one video.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// The video's path, as the editor opened it.
    pub clip: String,
    /// Seconds into the clip, half open: `[start, end)`.
    pub start: f64,
    pub end: f64,
    /// What the operator calls it; empty until they name it.
    #[serde(default)]
    pub name: String,
    /// The poster JPEG, when the ingest made one. Derived — a missing file
    /// only costs the thumbnail.
    #[serde(default)]
    pub poster: Option<String>,
    /// Kept under its name, favourite or not.
    #[serde(default)]
    pub saved: bool,
    /// In the global favourites.
    #[serde(default)]
    pub favourite: bool,
}

impl SceneBook {
    /// The book at `path`; an empty one when there is none or it does not
    /// parse (a corrupt book is reported by [`SceneBook::try_load`]).
    pub fn load(path: &Path) -> SceneBook {
        SceneBook::try_load(path).unwrap_or_default()
    }

    /// The book at `path`; `Ok(empty)` when the file does not exist.
    pub fn try_load(path: &Path) -> Result<SceneBook, String> {
        match metal_vfs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SceneBook::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes the book whole, through a temporary file and a rename, so a
    /// sync peer never reads half of it.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        metal_vfs::write(&tmp, text).map_err(|e| e.to_string())?;
        metal_vfs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// The global favourites, by id.
    pub fn favourites(&self) -> impl Iterator<Item = (&String, &Scene)> {
        self.scenes.iter().filter(|(_, s)| s.favourite)
    }

    /// A group's scenes, in the group's own order; ids the book no longer
    /// holds are skipped.
    pub fn group(&self, name: &str) -> Vec<(&String, &Scene)> {
        self.groups
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|id| self.scenes.get_key_value(id))
            .collect()
    }

    /// The groups `id` is filed in.
    pub fn groups_of(&self, id: &str) -> Vec<String> {
        self.groups.iter().filter(|(_, ids)| ids.iter().any(|i| i == id)).map(|(g, _)| g.clone()).collect()
    }

    /// `scene` under `id`, keeping the flags and name it already has.
    pub fn upsert(&mut self, id: &str, scene: Scene) -> &mut Scene {
        let slot = self.scenes.entry(id.to_string()).or_insert_with(|| scene.clone());
        slot.clip = scene.clip;
        slot.start = scene.start;
        slot.end = scene.end;
        if scene.poster.is_some() {
            slot.poster = scene.poster;
        }
        slot
    }

    /// Drops `id` once it is neither saved, favourite nor filed anywhere —
    /// the only way a scene leaves the book.
    pub fn prune(&mut self, id: &str) {
        let keep = self.scenes.get(id).is_some_and(|s| s.saved || s.favourite)
            || self.groups.values().any(|ids| ids.iter().any(|i| i == id));
        if !keep {
            self.scenes.remove(id);
        }
    }

    pub fn create_group(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a group needs a name".into());
        }
        self.groups.entry(name.to_string()).or_default();
        Ok(())
    }

    pub fn rename_group(&mut self, from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("a group needs a name".into());
        }
        if from == to {
            return Ok(());
        }
        if self.groups.contains_key(to) {
            return Err(format!("there is already a group {to:?}"));
        }
        let ids = self.groups.remove(from).ok_or_else(|| format!("no group {from:?}"))?;
        self.groups.insert(to.to_string(), ids);
        Ok(())
    }

    /// Removes the group. Its scenes keep their flags; one that was only
    /// kept for being filed there leaves the book with it.
    pub fn delete_group(&mut self, name: &str) -> Result<(), String> {
        let ids = self.groups.remove(name).ok_or_else(|| format!("no group {name:?}"))?;
        for id in ids {
            self.prune(&id);
        }
        Ok(())
    }

    /// Files `id` into `group` (made if missing), or takes it out.
    pub fn file(&mut self, id: &str, group: &str, on: bool) -> Result<(), String> {
        if on {
            if !self.scenes.contains_key(id) {
                return Err(format!("no scene {id:?}"));
            }
            self.create_group(group)?;
            let ids = self.groups.get_mut(group.trim()).expect("just made");
            if !ids.iter().any(|i| i == id) {
                ids.push(id.to_string());
            }
        } else if let Some(ids) = self.groups.get_mut(group) {
            ids.retain(|i| i != id);
            self.prune(id);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// the cut list
// ---------------------------------------------------------------------------

/// One scene as the ingest found it — a row of `<slug>.scenes.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cut {
    pub id: String,
    pub clip: String,
    pub start: f64,
    pub end: f64,
    /// The mid-scene frame, a JPEG beside the sidecar.
    #[serde(default)]
    pub poster: Option<String>,
    /// The proxy frame the poster is: the middle of the scene, not its first
    /// frame — a cut's first frame is very often a flash or a dissolve
    /// (`curator::scenes` reads the same rung).
    pub rung: usize,
}

/// A clip's HAP file stem: its own stem plus a hash of its full path, so two
/// `intro.mp4` in different folders never share one. The first half of every
/// scene id.
pub fn slug(path: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    let stem = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    format!("{stem}-{:016x}", h.finish())
}

/// `<slug>#<start_ms>-<end_ms>`.
pub fn id(slug: &str, start: f64, end: f64) -> String {
    let ms = |s: f64| (s * 1000.0).round().max(0.0) as u64;
    format!("{slug}#{}-{}", ms(start), ms(end))
}

/// `<root>/<slug>.scenes.json`, beside the HAP proxy's own sidecar.
pub fn sidecar(root: &Path, slug: &str) -> PathBuf {
    root.join(format!("{slug}.scenes.json"))
}

/// `<root>/<slug>.scenes/<k>.jpg`.
pub fn poster(root: &Path, slug: &str, k: usize) -> PathBuf {
    root.join(format!("{slug}.scenes")).join(format!("{k}.jpg"))
}

/// The clip's scenes, cut from its descriptor rows at `rate` rows a second.
/// Posters are the caller's (it holds the frames); `poster` is left `None`.
pub fn cuts(clip: &str, rows: &[[f32; COLUMNS]], rate: f32) -> Vec<Cut> {
    cuts_from_passages(clip, &index::cut(rows, rate), rate)
}

/// The same scenes from passages already cut (a library's `index.json`):
/// there is one cut pass per clip, and every consumer reads its result.
pub fn cuts_from_passages(clip: &str, passages: &[Passage], rate: f32) -> Vec<Cut> {
    let slug = slug(clip);
    passages
        .iter()
        .map(|p: &Passage| {
            let start = p.from as f64 / f64::from(rate);
            let end = p.to as f64 / f64::from(rate);
            Cut { id: id(&slug, start, end), clip: clip.to_string(), start, end, poster: None, rung: (p.from + p.to) / 2 }
        })
        .collect()
}

/// The scenes of `clip` when an indexed library under `data` holds it (its
/// source folder plus the clip's file name, at the library's own rate), so the
/// scenes sidecar reads the library's cut instead of cutting the clip again.
pub fn library_cuts(data: &Path, clip: &str) -> Option<Vec<Cut>> {
    let clip_path = Path::new(clip);
    let (dir, file) = (clip_path.parent()?, clip_path.file_name()?.to_str()?);
    for name in index::Library::list(data) {
        let Ok(lib) = index::Library::load(data, &name) else { continue };
        if Path::new(&lib.source) != dir {
            continue;
        }
        if let Some(c) = lib.clips.iter().find(|c| c.file == file).filter(|c| !c.passages.is_empty()) {
            return Some(cuts_from_passages(clip, &c.passages, lib.rate));
        }
    }
    None
}

/// The descriptor rows of a JPEG ladder (`dir/%06d.jpg`, one-based), every
/// core at once — the rows [`index::cut`] reads. An unreadable rung repeats
/// its neighbour, as in a library build.
pub fn ladder_rows(dir: &Path) -> Vec<[f32; COLUMNS]> {
    let mut files: Vec<PathBuf> = metal_vfs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jpg"))
        .collect();
    files.sort();
    let frames = index::across(&files, |p| index::decode(p).ok());
    rows_of(&frames)
}

/// The descriptor rows of a HAP proxy — for a proxy made before scenes
/// existed, so its cut list is one pass over the proxy, not a re-ingest.
pub fn hap_rows(hap: &mut crate::hap::Hap) -> Vec<[f32; COLUMNS]> {
    let frames: Vec<Option<Rgb>> = (0..hap.frames())
        .map(|k| {
            hap.frame(k).ok().map(|f| Rgb {
                width: f.width,
                height: f.height,
                px: f.rgba().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
            })
        })
        .collect();
    rows_of(&frames)
}

fn rows_of(frames: &[Option<Rgb>]) -> Vec<[f32; COLUMNS]> {
    let pairs: Vec<(Option<&Rgb>, Option<&Rgb>)> = (0..frames.len())
        .map(|k| (frames[k].as_ref(), if k == 0 { None } else { frames[k - 1].as_ref() }))
        .collect();
    let mut rows: Vec<[f32; COLUMNS]> = Vec::with_capacity(frames.len());
    for row in index::across(&pairs, |(f, prev)| f.map(|f| read::describe(f, *prev))) {
        rows.push(row.unwrap_or_else(|| *rows.last().unwrap_or(&[0.0; COLUMNS])));
    }
    rows
}

pub fn load_cuts(root: &Path, slug: &str) -> Option<Vec<Cut>> {
    serde_json::from_str(&metal_vfs::read_to_string(sidecar(root, slug)).ok()?).ok()
}

pub fn save_cuts(root: &Path, slug: &str, cuts: &[Cut]) -> Result<(), String> {
    metal_vfs::create_dir_all(root).map_err(|e| e.to_string())?;
    let text = serde_json::to_string(cuts).map_err(|e| e.to_string())?;
    metal_vfs::write(sidecar(root, slug), text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(clip: &str, start: f64, end: f64) -> Scene {
        Scene { clip: clip.into(), start, end, ..Scene::default() }
    }

    /// The file is exactly the shared shape `reel.batch` reads.
    #[test]
    fn the_book_writes_the_shared_shape() {
        let mut book = SceneBook::default();
        book.upsert("a-1#0-2000", scene("/v/a.mp4", 0.0, 2.0)).favourite = true;
        book.file("a-1#0-2000", "hall", true).unwrap();
        let v = serde_json::to_value(&book).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "scenes": { "a-1#0-2000": {
                    "clip": "/v/a.mp4", "start": 0.0, "end": 2.0, "name": "",
                    "poster": null, "saved": false, "favourite": true,
                } },
                "groups": { "hall": ["a-1#0-2000"] },
            })
        );
        assert_eq!(serde_json::from_value::<SceneBook>(v).unwrap(), book);
        assert_eq!(serde_json::from_str::<SceneBook>("{}").unwrap(), SceneBook::default());
    }

    /// Deleting a group never unfavourites; a scene kept only by the group
    /// goes with it.
    #[test]
    fn deleting_a_group_keeps_its_favourites() {
        let mut book = SceneBook::default();
        book.upsert("a#0-1", scene("/a", 0.0, 1.0)).favourite = true;
        book.upsert("b#0-1", scene("/b", 0.0, 1.0));
        book.file("a#0-1", "x", true).unwrap();
        book.file("b#0-1", "x", true).unwrap();
        book.file("a#0-1", "y", true).unwrap();
        book.rename_group("x", "z").unwrap();
        assert_eq!(book.group("z").len(), 2);
        book.delete_group("z").unwrap();
        assert!(book.scenes["a#0-1"].favourite);
        assert_eq!(book.groups_of("a#0-1"), vec!["y".to_string()]);
        assert!(!book.scenes.contains_key("b#0-1"));
        assert_eq!(book.favourites().count(), 1);
    }

    #[test]
    fn the_book_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!("fw-scenes-book-{}", std::process::id()));
        let path = dir.join(BOOK);
        assert_eq!(SceneBook::load(&path), SceneBook::default());
        let mut book = SceneBook::default();
        book.upsert("a#0-1", scene("/a", 0.0, 1.0)).saved = true;
        book.save(&path).unwrap();
        assert_eq!(SceneBook::load(&path), book);
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// Two cuts in the rows are three scenes, and the ids are the slug and
    /// the milliseconds.
    #[test]
    fn rows_with_two_cuts_are_three_scenes() {
        let mut rows = vec![[0.0f32; COLUMNS]; 72];
        rows[24][11] = 1.0;
        rows[48][11] = 1.0;
        let cuts = cuts("/v/a b.mp4", &rows, 8.0);
        assert_eq!(cuts.len(), 3);
        assert_eq!((cuts[1].start, cuts[1].end, cuts[1].rung), (3.0, 6.0, 36));
        assert!(cuts[1].id.starts_with("a_b-") && cuts[1].id.ends_with("#3000-6000"));
    }
}

#[cfg(test)]
mod library_cut_tests {
    use super::*;

    /// A clip a library holds is cut once: its scenes are the library's
    /// passages, with the same ids a cut over the rows would give.
    #[test]
    fn a_library_clip_takes_the_librarys_passages_not_a_second_cut() {
        let rows: Vec<[f32; COLUMNS]> = (0..48).map(|k| { let mut r = [0.1; COLUMNS]; r[11] = if k == 24 { 1.0 } else { 0.0 }; r }).collect();
        let passages = index::cut(&rows, 12.0);
        let from_rows = cuts("/v/a.mp4", &rows, 12.0);
        let from_passages = cuts_from_passages("/v/a.mp4", &passages, 12.0);
        assert_eq!(from_rows, from_passages);
        assert!(!from_passages.is_empty());
        let data = std::env::temp_dir().join(format!("fw-libcut-{}", std::process::id()));
        assert!(library_cuts(&data, "/v/a.mp4").is_none(), "no library, no library cut");
    }
}
