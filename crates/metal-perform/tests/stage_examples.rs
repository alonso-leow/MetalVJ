//! The stage-fixture examples (`data/dataset/stage/examples`, cut by
//! `stage_scaffold`, authored by `scripts/stage_author.py`) against the stage venue
//! patch they were cut for.
//!
//! Every example must parse as `metal_stage::dataset::Example` and pass
//! `Example::validate` on `data/agent/venues/stage.toml`: same patch hash and matrix
//! columns, every tick's music and stage floats in range, ticks increasing.

use metal_perform::{stage, venue::Venue};
use metal_stage::dataset::Example;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn stage_examples_validate_against_the_stage_venue() {
    let root = repo();
    let venue = Venue::parse(&std::fs::read_to_string(root.join("data/agent/venues/stage.toml")).unwrap()).unwrap();
    let patch = stage::patch_of(&venue, &root.join("data"));
    // `STAGE_EXAMPLES=<dir>` checks another set (a scratch run) instead of the shipped one.
    let dir = std::env::var_os("STAGE_EXAMPLES").map(PathBuf::from).unwrap_or_else(|| root.join("data/dataset/stage/examples"));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    files.sort();
    if files.is_empty() {
        eprintln!("no stage examples in {} (none authored yet)", dir.display());
        return;
    }

    let mut ticks = 0usize;
    let mut problems = Vec::new();
    for f in &files {
        let ex: Example = serde_json::from_str(&std::fs::read_to_string(f).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        ticks += ex.ticks.len();
        assert!(ex.ticks.iter().all(|t| t.stage.is_some()), "{}: an unauthored tick", f.display());
        for p in ex.validate(&patch) {
            problems.push(format!("{}: tick {:?}: {}", f.file_name().unwrap().to_string_lossy(), p.tick, p.what));
        }
    }
    assert!(problems.is_empty(), "{} problem(s):\n{}", problems.len(), problems.join("\n"));
    eprintln!("{} stage examples, {} ticks, patch `{}` ({} floats)", files.len(), ticks, patch.name, patch.matrix().columns.len());
}
