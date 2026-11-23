//! The committed fixtures under `crates/metal-dash/tests/fixtures/`. Tests read
//! these, never the gitignored `data/` (`collections.json`, `graphs/`), so a
//! fresh clone passes without any private state. `schema.json` is the one
//! repo-data file still read: it is tracked and it is the manifest, not state.
//!
//! Test-only (`#[cfg(test)]` in `lib.rs`).

use std::path::PathBuf;

/// The fixtures directory. Panics (loudly, with the restore command) when the
/// committed files are missing, rather than falling back to the repo's `data/`.
pub fn dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for required in ["graphs/empty-1.json", "collections.json"] {
        assert!(
            dir.join(required).is_file(),
            "metal-dash test fixture missing: {} — the fixtures are committed under \
             crates/metal-dash/tests/fixtures/; restore them with `git checkout -- \
             crates/metal-dash/tests/fixtures` (tests must not fall back to data/)",
            dir.join(required).display()
        );
    }
    dir
}

/// Point the graphs scratch copy at the fixture graphs (see
/// `metal_perform::engine::set_test_graphs_source`).
pub fn use_graphs() {
    metal_perform::engine::set_test_graphs_source(&dir().join("graphs"));
}

/// The fixture collections, parsed the way `Collections::load` parses
/// `collections.json`.
pub fn collections() -> metal_perform::layers::Collections {
    let path = dir().join("collections.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let list = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} did not parse: {e}", path.display()));
    metal_perform::layers::Collections { list }
}
