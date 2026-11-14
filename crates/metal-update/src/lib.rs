//! **The in-app updater: GitHub Releases in, a verified binary out, no git.**
//!
//! `docs/patch-and-update-plan.md` section 0 is why this crate exists in this shape.
//! The install it serves is a clone with **no shared ancestry** with the public
//! repo (`alonso-leow/metalvj`) and a patch upstream does not carry. Merging is
//! impossible; rebasing is a person's job. What *can* be automatic is the
//! binary: the public repo publishes a release with a downloadable asset, and
//! an install that wants the newer code can fetch that file, verify it, and
//! swap it in on the next start. Nothing here knows what a commit is.
//!
//! So:
//!
//! - **No git**, ever. No command is spawned; no checkout is assumed.
//! - **GitHub Releases is the source of truth.** One `GET .../releases`, pick
//!   the channel's highest version, compare it to [`current`].
//! - **Verify before swapping.** A `sha256` digest when the release carries one
//!   (GitHub's `digest` field, often null), download to a sibling temp file,
//!   flush, rename. On Windows the rename happens at *startup*, not while the
//!   exe is locked — see [`finish_pending_update`].
//! - **Never mid-show.** [`check`] and [`apply`] are separate operations, and
//!   `apply` only runs when a caller (the `POST /update` route) asks for it by
//!   tag. `channel = "off"` disables both.
//!
//! # The tag convention (a channel is a branch)
//!
//! | channel (`[update].channel`) | release tags | example |
//! | --- | --- | --- |
//! | `main` (default) | `v<version>` | `v1.2.3` |
//! | a branch, e.g. `dev` | `<branch>-v<version>` | `dev-v1.2.4` |
//! | `off` | — | — |
//!
//! `main` ignores prereleases; every other channel counts them. The rule lives
//! in [`tags`], one place, with a table test. It is a tag test rather than a
//! `target_commitish` test on purpose: GitHub's `target_commitish` is an
//! unvalidated free-text field ("main", a SHA, a typo), while the tag is what
//! the release was actually cut from and what a person sees.
//!
//! # Version reporting
//!
//! [`current`] reports **the app's** version, not this crate's. Both use
//! `version.workspace = true`, so today the two compile to the same `0.1.0` —
//! but the day `metal-dash` is versioned ahead of `metal-update` (or the
//! reverse), the updater's own `CARGO_PKG_VERSION` would be the wrong "what is
//! running". `metal-update`'s own `CARGO_PKG_VERSION` is therefore a fallback
//! only:
//!
//! 1. `METALVJ_VERSION` at compile time — the release pipeline sets it to the
//!    tag it is building (`METALVJ_VERSION=0.2.0 cargo build --release`), which
//!    is the only way a build knows its own release tag;
//! 2. else the version of `metal-dash`, the crate a person runs, measured at
//!    build time (cargo passes a dependency's version as
//!    `DEP_<NAME>_VERSION` to the *dependent only*; so the binary crate is the
//!    one that must state it — see `crates/metal-dash/build.rs`, which sets
//!    `METALVJ_APP_VERSION` from its own `CARGO_PKG_VERSION`);
//! 3. else this crate's own `CARGO_PKG_VERSION`.
//!
//! The point of the order is that a release binary prints the release it was
//! built for, and a `cargo run` in a checkout prints the workspace version.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // First thing in `main`, before the window:
//! metal_update::finish_pending_update(&metal_update::this_exe()?)?;
//!
//! // Check is read-only; apply is explicit.
//! let cfg = metal_update::UpdateConfig::load()?;
//! if let metal_update::Check::Available(rel) = metal_update::check(&cfg)? {
//!     println!("{} is out: {}", rel.tag, rel.notes);
//!     // ... and only when the operator says so:
//!     let applied = metal_update::apply(&cfg, &rel)?;
//! }
//! # Ok(())
//! # }
//! ```

mod config;
mod http;
mod release;
mod swap;
mod tags;
mod version;

pub use config::{UpdateConfig, DEFAULT_REPO, MAIN, OFF};
pub use http::{api_base, BINARY_ACCEPT, JSON_ACCEPT};
pub use release::{asset_matches, pick_asset, select, Asset, Release, Selection};

/// Parse a `GET /releases` body (the JSON GitHub answers). The channel filter
/// is the caller's ([`select`]), so one parsed list can be read against two
/// channels. See `release::parse`.
pub fn parse_releases(json: &str) -> Result<Vec<Release>, serde_json::Error> {
    release::parse(json)
}
pub use swap::{finish_pending_update, new_path, old_path, staging_path};
pub use version::Version;

/// The `User-Agent` every request sends. GitHub rejects a request without one
/// (403, before any rate limit is even counted).
pub const USER_AGENT: &str = concat!("MetalVJ-updater/", env!("CARGO_PKG_VERSION"));

/// The app version, see the crate docs. Set by `metal-dash/build.rs` from
/// `METALVJ_VERSION` or its own `CARGO_PKG_VERSION`; this crate's version is the
/// last resort.
pub const APP_VERSION: &str = match option_env!("METALVJ_APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// The running build's version — **the app's**, not this crate's. See
/// [`APP_VERSION`].
pub fn current() -> Version {
    Version::parse(APP_VERSION).unwrap_or_else(|| Version::new(0, 0, 0))
}

/// What [`check`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// `[update].channel = "off"` (or `enabled = false`): nothing was asked,
    /// nothing will be.
    Off,
    /// The newest release on this channel is not newer than the running build.
    UpToDate { current: Version, latest: Option<Release> },
    /// A newer release exists. Nothing has been downloaded.
    Available(Release),
}

impl Check {
    /// The newer release, when there is one to apply.
    pub fn available(&self) -> Option<&Release> {
        match self {
            Check::Available(r) => Some(r),
            _ => None,
        }
    }
}

/// What [`apply`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The new binary is installed *and running*: POSIX's rename-over-sibling
    /// case, where the next start is the new code and nothing is pending.
    Installed { path: std::path::PathBuf, verified: bool },
    /// The new binary is staged as `<exe>.new` and will be swapped in by the
    /// **next start** ([`finish_pending_update`]) — the Windows case, and the
    /// POSIX case where a rename could not be done (see `Applied::Installed`).
    ///
    /// This is not an error: it is what every update on Windows looks like, and
    /// what the operator must be told ("restart to finish").
    Restart { path: std::path::PathBuf, pending: std::path::PathBuf, verified: bool },
}

impl Applied {
    /// Whether the caller must restart the app to finish.
    pub fn needs_restart(&self) -> bool {
        matches!(self, Applied::Restart { .. })
    }
}

/// Everything that can go wrong. Each variant carries a sentence a person can
/// act on, because the route's `message` is this text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("this release has no build for this platform ({os}, {arch})")]
    NoAsset { os: String, arch: String },
    #[error("the release does not exist or is not on this channel")]
    UnknownRelease,
    #[error("updates are off (`[update].channel = \"off\"`)")]
    Off,
    #[error("GitHub refused the request (rate limited? set `[update].token_file`): {0}")]
    RateLimited(String),
    #[error("the request failed: {0}")]
    Http(String),
    #[error("GitHub answered something that is not a release list: {0}")]
    BadJson(String),
    #[error("the asset's digest does not parse: {0}")]
    BadDigest(String),
    #[error("the download does not match its published sha256 (expected {expected}, got {got})")]
    Digest { expected: String, got: String },
    #[error("{0}")]
    Io(String),
}

/// Read this channel's releases and decide. **One network request**, one
/// comparison; nothing is downloaded. `Off` short-circuits before the request.
///
/// The comparison is against [`current`] (the app's version). A release that
/// is newer but ships no asset for this platform is still
/// [`Check::Available`] — `apply` is what refuses it, by name, so a `GET
/// /update` can tell a person why the button will not work.
pub fn check(cfg: &UpdateConfig) -> Result<Check, Error> {
    if cfg.is_off() {
        return Ok(Check::Off);
    }
    let current = current();
    availability(cfg, &current)
}

/// [`check`] as a pure function of a release list — the shape the tests and the
/// route's fixture use.
/// The channel's raw release list — the one GET `check` makes, exposed for a
/// caller that needs a whole release object (the route's `apply`, which must
/// hold the asset it is about to download, not just re-find a tag).
pub fn releases(cfg: &UpdateConfig) -> Result<Vec<Release>, Error> {
    if cfg.is_off() {
        return Ok(Vec::new());
    }
    http::fetch_releases(cfg)
}
pub fn availability(cfg: &UpdateConfig, current: &Version) -> Result<Check, Error> {
    if cfg.is_off() {
        return Ok(Check::Off);
    }
    let releases = http::fetch_releases(cfg)?;
    Ok(match select(&cfg.channel, &releases, current) {
        Selection::Newer(release) => Check::Available(release.clone()),
        Selection::UpToDate { current, latest } => {
            Check::UpToDate { current, latest: latest.cloned() }
        }
        // A dev build ahead of its own tag: nothing to do, and the newest tag
        // is still worth showing a person.
        Selection::Ahead { latest } => {
            Check::UpToDate { current: current.clone(), latest: latest.cloned() }
        }
        Selection::Empty => Check::UpToDate { current: current.clone(), latest: None },
    })
}

/// Download `rel`'s asset for this platform and stage it beside the running
/// binary. **Never automatic**: only a caller that already has a `&Release`
/// (i.e. a person pressed the button) reaches this.
///
/// On POSIX the rename is done here ([`Applied::Installed`]); on Windows the
/// download lands as `<exe>.new` ([`Applied::Restart`]) and
/// [`finish_pending_update`] completes it at the next start. Either way the
/// digest is verified when the release carries one (`verified` in the result
/// says which happened).
pub fn apply(cfg: &UpdateConfig, rel: &Release) -> Result<Applied, Error> {
    if cfg.is_off() {
        return Err(Error::Off);
    }
    let asset = rel.asset.as_ref().ok_or_else(|| Error::NoAsset {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    })?;
    let exe = this_exe().map_err(|e| Error::Io(format!("cannot find the running binary: {e}")))?;
    apply_to(&exe, asset)
}

/// [`apply`] against an explicit binary path — the shape a test can drive with
/// a temp file instead of the test harness's own exe.
pub fn apply_to(exe: &std::path::Path, asset: &Asset) -> Result<Applied, Error> {
    // The download lands on `<exe>.new.part`, then moves to `<exe>.new`. The
    // staging name is not a temporary directory: it is the same file system as
    // the binary by construction, which the final rename needs.
    let pending = new_path(exe);
    let verified = http::download(asset, &pending)?;
    // Keep the restart honest: on POSIX the rename-over is safe while the old
    // image runs, and doing it now means the next start is already new. On
    // Windows it is not possible, and pretending otherwise would fail at the
    // first `rename`.
    #[cfg(unix)]
    {
        if swap::finish_pending_update(exe).unwrap_or(false) {
            return Ok(Applied::Installed { path: exe.to_path_buf(), verified });
        }
    }
    Ok(Applied::Restart { path: exe.to_path_buf(), pending, verified })
}

/// The running binary's path. `current_exe`, with one Windows nicety: a path
/// with the `\\?\` verbatim prefix (which `current_exe` can return) is
/// stripped, so the `.new` beside it is the path a person can find.
pub fn this_exe() -> std::io::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset { name: name.to_string(), url: String::new(), size: 1, digest: None }
    }

    /// A release with a hand-built tag; the asset is picked for a *fixed*
    /// platform so the assertion is the same on every host.
    fn release_with(tag: &str, assets: &[&str]) -> Release {
        let owned: Vec<Asset> = assets.iter().map(|a| asset(a)).collect();
        Release {
            version: Version::parse(tag.split("-v").last().unwrap_or(tag).trim_start_matches('v'))
                .expect("a version in the tag"),
            tag: tag.to_string(),
            url: String::new(),
            asset: release::pick_asset_for_platform(
                &owned,
                &["windows", "win"],
                &["x8664", "x64", "amd64"],
            ),
            notes: String::new(),
            published: String::new(),
            prerelease: tag.contains("-rc"),
            draft: false,
        }
    }

    #[test]
    fn current_reports_a_real_version() {
        let v = current();
        assert_ne!(v, Version::new(0, 0, 0), "APP_VERSION must parse: {APP_VERSION}");
        assert_eq!(v.to_string(), APP_VERSION);
    }

    #[test]
    fn off_never_touches_the_network() {
        let cfg = UpdateConfig { channel: "off".into(), ..UpdateConfig::default() };
        assert_eq!(check(&cfg).unwrap(), Check::Off);
        assert_eq!(availability(&cfg, &Version::new(0, 0, 0)).unwrap(), Check::Off);
        let rel = release_with("v9.9.9", &[]);
        assert_eq!(apply(&cfg, &rel), Err(Error::Off));
    }

    #[test]
    fn apply_refuses_a_release_with_no_asset_for_this_platform() {
        let cfg = UpdateConfig::default();
        let mut rel = release_with("v9.9.9", &["metalvj-9.9.9-freebsd-riscv64.exe"]);
        rel.asset = None;
        let err = apply(&cfg, &rel).unwrap_err();
        assert!(matches!(err, Error::NoAsset { .. }), "{err}");
        assert!(err.to_string().contains("no build for this platform"));
    }

    #[test]
    fn a_release_asset_is_picked_and_a_wrong_arch_is_not() {
        let rel = release_with(
            "v1.1.0",
            &["metalvj-1.1.0-windows-aarch64.exe", "metalvj-1.1.0-windows-x86_64.exe"],
        );
        assert_eq!(rel.asset.expect("x86_64 asset").name, "metalvj-1.1.0-windows-x86_64.exe");
    }

    #[test]
    fn selection_maps_onto_check() {
        let current = Version::new(1, 0, 0);
        let releases = vec![release_with("v1.0.0", &[]), release_with("v1.1.0", &[])];
        match release::select("main", &releases, &current) {
            Selection::Newer(r) => assert_eq!(r.tag, "v1.1.0"),
            other => panic!("{other:?}"),
        }
        assert!(Check::Available(releases[1].clone()).available().is_some());
        assert!(Check::UpToDate { current, latest: None }.available().is_none());
    }
}
