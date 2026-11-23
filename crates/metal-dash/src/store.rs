//! Storage behind one trait, so the same code reads graphs and collections
//! from a directory on a native host and from a bundled snapshot in the
//! browser.
//!
//! Paths are relative and use `/`: `graphs/demo-preview.json`,
//! `collections.json`. The browser has no file system: [`LocalStore`] keeps
//! the bundled snapshot in memory and mirrors every write to `localStorage`,
//! so an edit survives a reload. Both stores compile on every target; only
//! [`FsStore`] ever touches a real disk, and on wasm its calls fail softly
//! (`std::fs` returns an error there, it does not panic).

use std::collections::BTreeMap;

/// Read, write and list files by relative path.
pub trait Store {
    /// The file's bytes, or `None` if it does not exist or cannot be read.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// Create or replace a file, making parent directories as needed.
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String>;
    /// Names (not paths) of the files directly under `dir`, sorted. `""` is
    /// the root. Subdirectories are not listed.
    fn list(&self, dir: &str) -> Vec<String>;

    /// [`Store::read`] as UTF-8 text.
    fn read_text(&self, path: &str) -> Option<String> {
        self.read(path).and_then(|b| String::from_utf8(b).ok())
    }
}

/// Files under a directory on disk.
pub struct FsStore {
    root: std::path::PathBuf,
}

impl FsStore {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        FsStore { root: root.into() }
    }
}

impl Store for FsStore {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        metal_vfs::read(self.root.join(path)).ok()
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let full = self.root.join(path);
        if let Some(parent) = full.parent() {
            metal_vfs::create_dir_all(parent).map_err(|e| format!("{path}: {e}"))?;
        }
        metal_vfs::write(&full, bytes).map_err(|e| format!("{path}: {e}"))
    }

    fn list(&self, dir: &str) -> Vec<String> {
        let Ok(entries) = metal_vfs::read_dir(self.root.join(dir)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| metal_vfs::is_file(&e.path()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        names
    }
}

/// An in-memory store. Seeded from a snapshot of files known at build time,
/// and written to only in memory.
#[derive(Default, Clone)]
pub struct MemStore {
    files: BTreeMap<String, Vec<u8>>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store holding `files`, each a `(path, bytes)` pair.
    pub fn from_files(files: &[(&str, &[u8])]) -> Self {
        MemStore {
            files: files.iter().map(|(p, b)| (p.to_string(), b.to_vec())).collect(),
        }
    }
}

impl Store for MemStore {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).cloned()
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.files.insert(path.to_string(), bytes.to_vec());
        Ok(())
    }

    fn list(&self, dir: &str) -> Vec<String> {
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        // BTreeMap iterates in key order, so the names come out sorted.
        self.files
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix))
            .filter(|rest| !rest.is_empty() && !rest.contains('/'))
            .map(str::to_string)
            .collect()
    }
}

/// The demo snapshot in `web/wasm/data`, compiled in. Graphs are copied from
/// the repo's `data/graphs` fixtures; `schema.json` is the node manifest the
/// node catalog is derived from. The browser build ships all of it. Add a line
/// here to bundle a new file.
pub const BUNDLED: &[(&str, &[u8])] = &[
    (
        "graphs/demo-cyberspace.json",
        include_bytes!("../../../web/wasm/data/graphs/demo-cyberspace.json"),
    ),
    (
        "graphs/demo-preview.json",
        include_bytes!("../../../web/wasm/data/graphs/demo-preview.json"),
    ),
    (
        "graphs/device-preview.json",
        include_bytes!("../../../web/wasm/data/graphs/device-preview.json"),
    ),
    (
        "graphs/g.json",
        include_bytes!("../../../web/wasm/data/graphs/g.json"),
    ),
    (
        "graphs/device-sync-mixer-737857.json",
        include_bytes!("../../../web/wasm/data/graphs/device-sync-mixer-737857.json"),
    ),
    (
        "graphs/device-stroke-material-800647.json",
        include_bytes!("../../../web/wasm/data/graphs/device-stroke-material-800647.json"),
    ),
    (
        "collections.json",
        include_bytes!("../../../web/wasm/data/collections.json"),
    ),
    (
        "schema.json",
        include_bytes!("../../../web/wasm/data/schema.json"),
    ),
];

/// A [`MemStore`] holding [`BUNDLED`].
pub fn bundled() -> MemStore {
    MemStore::from_files(BUNDLED)
}

/// Where the browser keeps the data tree, inside `metal_vfs`'s in-memory tree.
/// `output::set_data_root` points the engine here, so every `data_root()`
/// path is `data/…` and every read and write lands in this tree.
pub const WEB_ROOT: &str = "data";

/// The `localStorage` key prefix. A file `data/graphs/x.json` is stored under
/// `fw:data/graphs/x.json`, value base64.
#[cfg(target_arch = "wasm32")]
const LOCAL_PREFIX: &str = "fw:";

/// Sets up the browser's data tree, once, before the engine reads anything:
///
/// 1. [`BUNDLED`] is mounted under [`WEB_ROOT`] (seeded, not reported).
/// 2. Every file the user wrote on an earlier visit is laid over it: the
///    `fw:` entries in `localStorage`, mounted the same way.
/// 3. The `metal_vfs` write hook is installed, so every later write or removal
///    under the tree is mirrored to `localStorage`.
///
/// A missing or refused `localStorage` (private window, blocked site data)
/// degrades to the memory-only behaviour, never an error.
#[cfg(target_arch = "wasm32")]
pub fn install_web_tree() {
    for (rel, bytes) in BUNDLED {
        metal_vfs::mount(WEB_ROOT, [(*rel, *bytes)]);
    }
    let mut restored: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(storage) = local_storage() {
        let len = storage.length().unwrap_or(0);
        for i in 0..len {
            let Some(key) = storage.key(i).ok().flatten() else { continue };
            let Some(path) = key.strip_prefix(LOCAL_PREFIX) else { continue };
            let Some(value) = storage.get_item(&key).ok().flatten() else { continue };
            if let Ok(bytes) = base64_decode(&value) {
                restored.push((path.to_string(), bytes));
            }
        }
    }
    for (path, bytes) in &restored {
        metal_vfs::mount("", [(path.as_str(), bytes.as_slice())]);
    }
    metal_vfs::set_hook(|key, bytes| {
        let Some(storage) = local_storage() else { return };
        let stored = format!("{LOCAL_PREFIX}{key}");
        // A full quota or a refused write is not the caller's error: the bytes
        // are in the tree for this session either way.
        match bytes {
            Some(bytes) => {
                let _ = storage.set_item(&stored, &base64_encode(bytes));
            }
            None => {
                let _ = storage.remove_item(&stored);
            }
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// The browser's store: the tree under [`WEB_ROOT`], after [`install_web_tree`].
#[cfg(target_arch = "wasm32")]
pub struct LocalStore(FsStore);

#[cfg(target_arch = "wasm32")]
impl LocalStore {
    /// The store over the installed tree. Call [`install_web_tree`] first.
    pub fn open() -> Self {
        LocalStore(FsStore::new(WEB_ROOT))
    }
}

#[cfg(target_arch = "wasm32")]
impl Store for LocalStore {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path)
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.0.write(path, bytes)
    }

    fn list(&self, dir: &str) -> Vec<String> {
        self.0.list(dir)
    }
}

/// Standard base64 with padding. Hand-rolled to keep the browser build free of
/// a crate for twenty lines of bit-shifting.
#[cfg(target_arch = "wasm32")]
fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(target_arch = "wasm32")]
fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let val = |c: u8| -> Result<u32, String> {
        Ok(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(format!("bad base64 byte {c:#x}")),
        } as u32)
    };
    let bytes = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        let produced = chunk.len() - 1;
        for i in 0..produced {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The preview device's own graph draws the two layers `web/wasm/scene.json`
    /// names: two generators wired to the output through their effect chains.
    #[test]
    fn bundled_preview_graph_draws_two_layers() {
        let text = bundled().read_text("graphs/device-preview.json").expect("bundled");
        let g = metal_graph::Graph::from_json(&text).expect("parses");
        let gens = metal_perform::engine::authored_generators(&g);
        assert_eq!(gens.len(), 2, "one layer per generator wired to the output");
    }

    #[test]
    fn bundled_graphs_parse() {
        let store = bundled();
        let graphs = store.list("graphs");
        assert_eq!(graphs.len(), 6);
        for name in graphs {
            let text = store.read_text(&format!("graphs/{name}")).expect("bundled graph");
            metal_graph::Graph::from_json(&text).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }

    #[test]
    fn mem_store_round_trips_and_lists_one_level() {
        let mut store = bundled();
        store.write("graphs/new.json", b"{}").unwrap();
        assert_eq!(store.read("graphs/new.json").as_deref(), Some(&b"{}"[..]));
        assert_eq!(
            store.list("graphs"),
            [
                "demo-cyberspace.json",
                "demo-preview.json",
                "device-preview.json",
                "device-stroke-material-800647.json",
                "device-sync-mixer-737857.json",
                "g.json",
                "new.json",
            ]
        );
        assert_eq!(store.list(""), ["collections.json", "schema.json"]);
        assert!(store.list("missing").is_empty());
    }

    #[test]
    fn fs_store_round_trips() {
        let root = std::env::temp_dir().join(format!("fw-store-test-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&root);
        let mut store = FsStore::new(&root);
        store.write("graphs/a.json", b"{\"x\":1}").unwrap();
        assert_eq!(store.read_text("graphs/a.json").as_deref(), Some("{\"x\":1}"));
        assert_eq!(store.list("graphs"), ["a.json"]);
        assert!(store.read("graphs/none.json").is_none());
        let _ = metal_vfs::remove_dir_all(&root);
    }
}
