//! Global favourites and recents for the galleries (`shell::gallery`): one
//! store for the whole install, shared by the nodes canvas's `new` gallery and
//! the sequence's `importar` gallery.
//!
//! An [`Entry`] is what a gallery card picks: its `key` is the kind it
//! creates (`fx.shake`, `gen.<model>`, `mat.*`…). The store lives in
//! `data/favorites.json` (`metal_perform::output::data_root`) and is only
//! written by the shell, never by a test: tests load and save a store at a
//! path inside a tempdir.
//!
//! Stars are toggled from two places — a gallery card's star and the
//! Properties card's star. Both cannot reach the shell's state directly, so
//! they queue a request on the egui context and the shell drains it once a
//! frame with [`take_toggles`], the same way the blank-click flag travels.

use std::path::Path;

use egui::{Context, Id};
use serde::{Deserialize, Serialize};

use super::gallery::{Item, Section};
use super::i18n::tr;

/// How many recently picked entries the store keeps.
pub const RECENTS_CAP: usize = 12;

/// The file under the data root.
pub const FILE: &str = "favorites.json";

/// Where the shell keeps the store. Test builds never touch the operator's
/// `data/`: they get a per-test-thread scratch file (the same guard
/// `metal_perform::engine::graphs_dir` gives the graphs).
pub fn path() -> std::path::PathBuf {
    if cfg!(test) {
        let thread = format!("{:?}", std::thread::current().id()).replace(|c: char| !c.is_ascii_digit(), "");
        return std::env::temp_dir().join(format!("metal-dash-favorites-shell-{}-{thread}.json", std::process::id()));
    }
    metal_perform::output::data_root().join(FILE)
}

/// One favourite or recent: what a card creates, and how it reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub label: String,
    pub family: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
}

/// The global favourites (kept in the order they were starred) and the
/// recents (most recent first, at most [`RECENTS_CAP`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub favourites: Vec<Entry>,
    #[serde(default)]
    pub recents: Vec<Entry>,
}

impl Store {
    /// The store at `path`, or an empty one when the file is missing or does
    /// not parse (a damaged file must not stop the shell from starting).
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    /// Writes the store to `path` through a temporary file, so a crash
    /// mid-write leaves the previous file intact.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn is_favourite(&self, key: &str) -> bool {
        self.favourites.iter().any(|e| e.key == key)
    }

    /// Stars `entry` when it is not a favourite, unstars it when it is.
    /// Returns whether it is a favourite afterwards.
    pub fn toggle(&mut self, entry: Entry) -> bool {
        match self.favourites.iter().position(|e| e.key == entry.key) {
            Some(i) => {
                self.favourites.remove(i);
                false
            }
            None => {
                self.favourites.push(entry);
                true
            }
        }
    }

    /// Moves `entry` to the front of the recents, dropping an older copy of
    /// the same key and anything past [`RECENTS_CAP`].
    pub fn note_recent(&mut self, entry: Entry) {
        self.recents.retain(|e| e.key != entry.key);
        self.recents.insert(0, entry);
        self.recents.truncate(RECENTS_CAP);
    }
}

/// The key prefix of a library file or folder: `file:<kind>:<path>`, where
/// `kind` is what the file is (`image`, `video`, `audio`, `folder`).
pub const FILE_PREFIX: &str = "file:";

/// The store key of a library file (or folder, `kind` `folder`) at `path`.
pub fn file_key(kind: &str, path: &str) -> String {
    format!("{FILE_PREFIX}{kind}:{path}")
}

/// `(kind, path)` of a file key, or `None` for a kind (`fx.*`, `gen.*`…).
pub fn file_of_key(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix(FILE_PREFIX)?.split_once(':')
}

/// The favourite-or-recent entry a library file or folder stands for: the
/// card's label is its name, its family its kind.
pub fn file_entry(kind: &str, name: &str, path: &str) -> Entry {
    Entry {
        key: file_key(kind, path),
        label: name.to_string(),
        family: kind.to_string(),
        inputs: Vec::new(),
        outputs: Vec::new(),
    }
}

/// The entry a gallery card stands for.
pub fn entry_of(item: &Item) -> Entry {
    Entry {
        key: item.kind.clone(),
        label: item.label.clone(),
        family: item.family.clone(),
        inputs: item.inputs.clone(),
        outputs: item.outputs.clone(),
    }
}

/// The gallery's two leading sections, `Recent files` and `Favorites`, built
/// from the store. `tile` answers a kind's model picture, as the nodes canvas
/// has it. Both sections are always present; an empty one shows "nothing here".
pub fn sections(store: &Store, tile: &dyn Fn(&str) -> crate::shell::gallery::Tiles) -> Vec<Section> {
    let items = |entries: &[Entry]| -> Vec<Item> {
        entries
            .iter()
            .map(|e| Item {
                kind: e.key.clone(),
                label: e.label.clone(),
                family: e.family.clone(),
                inputs: e.inputs.clone(),
                outputs: e.outputs.clone(),
                tiles: tile(&e.key),
                starred: false,
                favourite: true,
            })
            .collect()
    };
    vec![
        Section { icon: crate::shell::components::Icon::File, label: tr("recent files").to_string(), items: items(&store.recents) },
        Section { icon: crate::shell::components::Icon::Other, label: tr("favorites").to_string(), items: items(&store.favourites) },
    ]
}

/// Sets every card's `favourite` flag from the store, so a card shows its star.
pub fn mark(sections: &mut [Section], store: &Store) {
    for item in sections.iter_mut().flat_map(|s| s.items.iter_mut()) {
        item.favourite = store.is_favourite(&item.kind);
    }
}

fn snapshot_id() -> Id {
    Id::new("shell/favorites/snapshot")
}

/// The store as the shell last saw it, for panels that only paint a star.
pub fn publish(ctx: &Context, store: &Store) {
    ctx.data_mut(|d| d.insert_temp(snapshot_id(), store.clone()));
}

/// Whether `key` is a favourite in the last published store.
pub fn is_favourite(ctx: &Context, key: &str) -> bool {
    ctx.data(|d| d.get_temp::<Store>(snapshot_id()).is_some_and(|s| s.is_favourite(key)))
}

fn queue_id() -> Id {
    Id::new("shell/favorites/toggles")
}

/// Asks the shell to star or unstar `entry` at the end of this frame.
pub fn request_toggle(ctx: &Context, entry: Entry) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<Entry>>(queue_id()).push(entry));
}

/// Drains the toggles queued this frame (the shell's one call per frame).
pub fn take_toggles(ctx: &Context) -> Vec<Entry> {
    ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Vec<Entry>>(queue_id())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str) -> Entry {
        Entry { key: key.into(), label: key.into(), family: "fx".into(), inputs: vec![], outputs: vec![] }
    }

    /// A fresh directory under the system temp dir: tests never touch `data/`.
    fn scratch(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("metal-dash-favorites-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join(name)
    }

    #[test]
    fn toggle_stars_then_unstars_and_keeps_insertion_order() {
        let mut s = Store::default();
        assert!(s.toggle(entry("fx.a")));
        assert!(s.toggle(entry("gen.b")));
        assert!(s.is_favourite("fx.a") && s.is_favourite("gen.b"));
        assert!(!s.toggle(entry("fx.a")), "a second toggle unstars");
        assert!(!s.is_favourite("fx.a"));
        let keys: Vec<&str> = s.favourites.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["gen.b"]);
    }

    #[test]
    fn favourites_round_trip_through_the_file() {
        let path = scratch(FILE);
        let mut s = Store::default();
        s.toggle(entry("mat.steel"));
        s.note_recent(entry("gen.turing"));
        s.save(&path).expect("save");
        assert_eq!(Store::load(&path), s, "what was saved is what loads");
    }

    #[test]
    fn a_missing_or_damaged_file_loads_empty() {
        let path = scratch(FILE);
        assert_eq!(Store::load(&path), Store::default(), "missing file");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Store::load(&path), Store::default(), "damaged file");
    }

    #[test]
    fn recents_put_the_latest_first_drop_repeats_and_cap() {
        let mut s = Store::default();
        for i in 0..(RECENTS_CAP + 5) {
            s.note_recent(entry(&format!("fx.{i}")));
        }
        assert_eq!(s.recents.len(), RECENTS_CAP);
        assert_eq!(s.recents[0].key, format!("fx.{}", RECENTS_CAP + 4), "newest first");
        s.note_recent(entry("fx.5"));
        assert_eq!(s.recents[0].key, "fx.5");
        assert_eq!(s.recents.iter().filter(|e| e.key == "fx.5").count(), 1, "no repeats");
        assert_eq!(s.recents.len(), RECENTS_CAP);
    }

    #[test]
    fn a_library_file_is_a_favourite_keyed_by_kind_and_path() {
        let entry = file_entry("video", "clip.mp4", "/media/clip.mp4");
        assert_eq!(entry.key, "file:video:/media/clip.mp4");
        assert_eq!(file_of_key(&entry.key), Some(("video", "/media/clip.mp4")));
        assert_eq!(file_of_key("fx.shake"), None, "a kind is not a file");
        let mut s = Store::default();
        assert!(s.toggle(entry.clone()), "starred");
        assert!(s.is_favourite(&file_key("video", "/media/clip.mp4")));
        assert!(!s.is_favourite(&file_key("image", "/media/clip.mp4")), "the kind is part of the key");
        assert!(!s.toggle(entry), "toggled off");
    }

    #[test]
    fn the_gallery_sections_list_recents_and_favourites_and_mark_stars() {
        let mut s = Store::default();
        s.toggle(entry("fx.a"));
        s.note_recent(entry("gen.b"));
        let mut secs = sections(&s, &|_| Default::default());
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].items.iter().map(|i| i.kind.as_str()).collect::<Vec<_>>(), vec!["gen.b"]);
        assert_eq!(secs[1].items.iter().map(|i| i.kind.as_str()).collect::<Vec<_>>(), vec!["fx.a"]);
        mark(&mut secs, &s);
        assert!(secs[1].items[0].favourite);
        // A plain catalogue card is marked only when it is starred.
        let mut other = vec![Section {
            icon: crate::shell::components::Icon::Other,
            label: String::new(),
            items: vec![Item { kind: "gen.b".into(), label: "b".into(), family: String::new(), inputs: vec![], outputs: vec![], tiles: Default::default(), starred: false, favourite: false }],
        }];
        mark(&mut other, &s);
        assert!(!other[0].items[0].favourite, "gen.b is only recent, not favourite");
    }
}
