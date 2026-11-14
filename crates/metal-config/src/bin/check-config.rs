//! `scripts/check-config`'s Rust half (S.AH point 5).
//!
//! Round-trip: parse the file, rewrite it pretty, compare the two. Anything
//! that differs means a person's hand-edit and this crate's writer disagree
//! about the canonical form — a shape `check-config` should catch before it
//! ships, not a fight `metal-sync --doctor` should be having every boot.
//! Idempotent by construction: rewriting the *rewritten* text has to produce
//! the same bytes again, or the writer itself is unstable.
//!
//! **Same lints as `--doctor`, same code path (rule 6).** Both call
//! [`metal_config::problems`] — this is what makes `key_file`'s existence/
//! permission checks (S.AL.2) show up here too, rather than only in
//! `metal-sync --doctor`; there is exactly one place those checks are written.
//! Like `--doctor`, a lint here never aborts the run and the key itself is
//! never printed — `problems`'s messages only ever carry the path and an OS
//! error, both safe.

use std::path::PathBuf;

fn main() {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| metal_config::default_path(&metal_config::config_dir()));

    let original = match metal_vfs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("check-config: {} does not exist — nothing to check", path.display());
            return;
        }
        Err(e) => {
            eprintln!("check-config: cannot read {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    let cfg = match metal_config::Config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("check-config: {e}");
            std::process::exit(1);
        }
    };

    // Same validator `metal-sync --doctor` calls — never a duplicate check
    // (rule 6). Lints (unknown keys, a broken/loose `key_file`, …) are
    // printed but never abort; only a `Severity::Error` problem does.
    let problems = metal_config::problems(&cfg);
    for p in &problems {
        println!("check-config: {p}");
    }
    if problems.iter().any(|p| p.severity == metal_config::Severity::Error) {
        eprintln!("check-config: config.toml has hard errors — fix before relying on it");
        std::process::exit(1);
    }

    let rewritten = match toml::to_string_pretty(&cfg) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("check-config: cannot serialize: {e}");
            std::process::exit(1);
        }
    };

    // Idempotence: parsing the rewritten text and rewriting *that* has to
    // land on the same bytes, or "pretty" is not actually a fixed point.
    let reparsed: metal_config::Config = match toml::from_str(&rewritten) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("check-config: rewritten output does not parse back: {e}");
            std::process::exit(1);
        }
    };
    let rewritten_again = toml::to_string_pretty(&reparsed).expect("already serialized once");
    if rewritten != rewritten_again {
        eprintln!("check-config: rewriting is not idempotent");
        std::process::exit(1);
    }

    if original.trim() != rewritten.trim() {
        println!("check-config: {} is not in canonical form; canonical form:\n", path.display());
        print!("{rewritten}");
        // Not an error by itself — a hand-authored file with comments and a
        // slightly different key order is still valid TOML. This just tells
        // the operator what `Config::save` would have written.
    } else {
        println!("check-config: {} is already canonical", path.display());
    }
}

