//! `metal-update`'s `current()` reports **the app's** version, and this is how
//! it learns it.
//!
//! Cargo passes a dependency's version as `DEP_<NAME>_VERSION` only to the
//! crate that *depends on* it, so `metal-dash` is the one place that can say
//! "this is the app". A release pipeline that stamps a tag sets
//! `METALVJ_VERSION` (`METALVJ_VERSION=1.4.1 cargo build --release`); a plain
//! checkout has none, and the workspace version is the honest answer.
//!
//! This writes `METALVJ_APP_VERSION`, which `metal-update`'s `APP_VERSION`
//! reads. Without this file that constant would fall back to `metal-update`'s
//! own `CARGO_PKG_VERSION` — the same value today, but the wrong one the day the
//! two crates are versioned apart.

fn main() {
    println!("cargo:rerun-if-env-changed=METALVJ_VERSION");
    let version = std::env::var("METALVJ_VERSION")
        .ok()
        .map(|v| v.trim().trim_start_matches(['v', 'V']).to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=METALVJ_APP_VERSION={version}");
}
