//! `reindex_is_idempotent` (S.AH point 8): `scripts/reindex.py`'s whole job
//! is to be re-run every time sync sees a diff under `data/`, so running it
//! twice against the same tree has to produce byte-identical output — a
//! script that drifts between runs (dict ordering, a timestamp, whatever)
//! would make every re-run look like a config change to review.

use std::path::PathBuf;
use std::process::Command;

fn reindex_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/reindex.py")
}

fn run_reindex(data_root: &std::path::Path) -> String {
    let output = Command::new("python3")
        .arg(reindex_script())
        .arg(data_root)
        .output()
        .expect("python3 must be on PATH to run scripts/reindex.py");
    assert!(
        output.status.success(),
        "reindex.py exited with {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("reindex.py prints UTF-8")
}

#[test]
fn reindex_is_idempotent() {
    let dir = tempfile::TempDir::new().unwrap();
    let data = dir.path();
    std::fs::create_dir_all(data.join("graphs")).unwrap();
    std::fs::create_dir_all(data.join("timelines")).unwrap();
    std::fs::write(data.join("graphs").join("entrance-set.json"), "{}").unwrap();
    std::fs::write(data.join("collections.json"), "[]").unwrap();

    let palettes = metal_config::materialize_palettes(&{
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            "night".to_string(),
            metal_config::PaletteDef {
                base: Some("#0b0f1a".to_string()),
                accent: Some("#ff5c8a".to_string()),
                background: None,
                flash: None,
                extra: vec![],
            },
        );
        m
    });
    std::fs::write(
        data.join("palettes.json"),
        serde_json::to_string_pretty(&palettes).unwrap(),
    )
    .unwrap();

    let first = run_reindex(data);
    let second = run_reindex(data);
    assert_eq!(first, second, "reindex.py must print the same thing twice in a row");
    assert!(first.contains("[studio]"));
    assert!(first.contains("night"));
}

#[test]
fn reindex_on_an_empty_tree_still_exits_cleanly() {
    let dir = tempfile::TempDir::new().unwrap();
    let out = run_reindex(dir.path());
    assert!(out.contains("[studio]"));
}
