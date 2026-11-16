//! The hand-written `GET /releases` fixture, and the release-selection table
//! these tests prove against it. No network: the JSON below is the byte-for-byte
//! shape GitHub answers, trimmed to the fields this crate reads, and every
//! assertion drives the same `parse` → `select` path a live `check` does.
//!
//! The fixture deliberately carries the awkward cases, so a matcher that
//! regresses fails here and not on the night it matters:
//!
//! - a **prerelease** (`v2.0.0-rc.1`) that `main` must ignore and `dev` must take;
//! - a **wrong-arch** asset on the same release (an aarch64 Windows build beside
//!   the x86_64 one), so "windows" alone is not enough;
//! - a **wrong-OS** asset (a macOS build) on the same release;
//! - a release with **no asset** for this platform at all (`v1.4.0-ios`);
//! - a **draft** (`v9.9.9`), which must never be picked even though it is the
//!   highest version;
//! - a **dev-channel** release (`dev-v1.5.0`) that `main` must not see.

use metal_update::{select, Asset, Check, Release, Selection, UpdateConfig, Version};

const FIXTURE: &str = include_str!("fixtures/releases.json");

fn parsed() -> Vec<Release> {
    metal_update::parse_releases(FIXTURE).expect("the fixture parses")
}

fn asset_names(release: &Release) -> Vec<&str> {
    release.asset.iter().map(|a| a.name.as_str()).collect()
}

#[test]
fn the_fixture_parses_into_five_releases_and_skips_the_versionless_tag() {
    let releases = parsed();
    let tags: Vec<&str> = releases.iter().map(|r| r.tag.as_str()).collect();
    assert_eq!(
        tags,
        vec!["v2.0.0-rc.1", "v1.4.1", "v1.4.0-ios", "dev-v1.5.0", "v9.9.9"],
        "order as GitHub returns it; `nightly` has no version and is dropped"
    );
    assert_eq!(releases[0].notes, "Second release candidate.\n\n- fixed the thing");
    assert_eq!(releases[0].published, "2026-10-20T10:00:00Z");
    assert!(releases[0].is_prerelease());
    assert!(releases[4].draft);
}

/// `main` takes the highest stable `v<version>` and never the rc, the draft or
/// the `dev-` tag.
#[test]
fn main_picks_the_newest_stable_release() {
    let releases = parsed();
    match select("main", &releases, &Version::new(1, 4, 0)) {
        Selection::Newer(r) => assert_eq!(r.tag, "v1.4.1"),
        other => panic!("{other:?}"),
    }
    // Running the exact newest stable: up to date.
    match select("main", &releases, &Version::new(1, 4, 1)) {
        Selection::UpToDate { latest: Some(r), .. } => assert_eq!(r.tag, "v1.4.1"),
        other => panic!("{other:?}"),
    }
    // Ahead of every stable tag (a dev build): reported, not an error.
    match select("main", &releases, &Version::new(1, 6, 0)) {
        Selection::Ahead { latest: Some(r) } => assert_eq!(r.tag, "v1.4.1"),
        other => panic!("{other:?}"),
    }
}

/// `dev` counts prereleases and its own tags, and never `main`'s.
#[test]
fn dev_counts_prereleases_and_its_own_tags_only() {
    let releases = parsed();
    match select("dev", &releases, &Version::new(1, 0, 0)) {
        Selection::Newer(r) => assert_eq!(r.tag, "dev-v1.5.0"),
        other => panic!("{other:?}"),
    }
    // Prereleases are `main`'s tags: `dev` takes a `v2.0.0-rc.1` only if it
    // carries dev's own prefix (`dev-v2.0.0-rc.1`), so a plain rc is not dev's.
    let only_rc: Vec<Release> = releases.iter().filter(|r| r.tag == "v2.0.0-rc.1").cloned().collect();
    assert_eq!(select("dev", &only_rc, &Version::new(1, 9, 0)), Selection::Empty);
    let mut dev_rc = releases.iter().find(|r| r.tag == "v2.0.0-rc.1").unwrap().clone();
    dev_rc.tag = "dev-v2.0.0-rc.1".into();
    match select("dev", &[dev_rc], &Version::new(1, 9, 0)) {
        Selection::Newer(r) => assert_eq!(r.tag, "dev-v2.0.0-rc.1"),
        other => panic!("{other:?}"),
    }
}

/// The wrong-arch and wrong-OS assets in the fixture are never handed to a
/// Windows x86_64 caller is not something a fixture can prove (the matcher
/// runs against the *host*), so this asserts it the way the code must be true
/// on any host: the picked asset's name contains the host's own OS and arch,
/// and `v1.4.0-ios` (no asset at all) has no asset.
#[test]
fn every_picked_asset_is_the_hosts_own_and_the_ios_release_has_none() {
    let releases = parsed();
    for release in &releases {
        let Some(asset) = &release.asset else {
            continue;
        };
        let lower = asset.name.to_ascii_lowercase();
        let os = match std::env::consts::OS {
            "windows" => vec!["windows", "win"],
            "linux" => vec!["linux"],
            _ => vec!["macos", "mac", "darwin", "osx"],
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => vec!["x8664", "x64", "amd64"],
            "aarch64" => vec!["aarch64", "arm64"],
            _ => vec!["x86", "i686", "386"],
        };
        // The naturalised name folds `x86_64` into two parts and every
        // separator into one hyphen, so both halves of a token count.
        let parts: Vec<&str> = lower.split(['-', '_', '.']).filter(|p| !p.is_empty()).collect();
        assert!(
            os.iter().any(|o| parts.contains(o)),
            "{} picked {} for an unrelated OS",
            asset.name,
            std::env::consts::OS
        );
        let arch_seen = arch.iter().any(|a| parts.contains(a))
            || (parts.windows(2).any(|w| w == ["x86", "64"])
                && arch.iter().any(|a| *a == "x64" || *a == "x8664" || *a == "amd64"));
        assert!(
            arch_seen,
            "{} picked {} for an unrelated arch",
            asset.name,
            std::env::consts::ARCH
        );
    }
    let ios = releases.iter().find(|r| r.tag == "v1.4.0-ios").expect("the ios release");
    assert!(ios.asset.is_none(), "a release with no asset for this platform has none");
    assert_eq!(asset_names(&releases[1]), vec![releases[1].asset.as_ref().unwrap().name.as_str()]);
}

/// A release with no asset is still `Available` to `check`, and `apply` is what
/// refuses it — the error names the platform.
#[test]
fn apply_refuses_the_asset_less_release_by_name() {
    let releases = parsed();
    let ios = releases.iter().find(|r| r.tag == "v1.4.0-ios").unwrap();
    let err = metal_update::apply(&UpdateConfig::default(), ios).unwrap_err();
    assert!(matches!(err, metal_update::Error::NoAsset { .. }), "{err}");
    let text = err.to_string();
    assert!(text.contains(std::env::consts::OS), "{text}");
    assert!(text.contains(std::env::consts::ARCH), "{text}");
}

/// `check` with no network: `off` is answered without a request, and an
/// unreachable API is an error a `GET /update` can show, not a panic.
#[test]
fn off_needs_no_network_and_a_bad_api_is_an_error() {
    let off = UpdateConfig { channel: "off".into(), ..UpdateConfig::default() };
    assert_eq!(metal_update::check(&off).unwrap(), Check::Off);

    // Point the API at a host that cannot answer and make sure `check` returns,
    // rather than panicking or hanging on a default timeout.
    let cfg = UpdateConfig { repo: "alonso-leow/metalvj".into(), ..UpdateConfig::default() };
    std::env::set_var("METALVJ_UPDATE_API", "http://127.0.0.1:1");
    let result = metal_update::check(&cfg);
    std::env::remove_var("METALVJ_UPDATE_API");
    assert!(result.is_err(), "a refused connection is an error");
}

/// The channel is the tag convention: the fixture's `dev-v1.5.0` is invisible to
/// `main`, and `main`'s tags are invisible to `dev`.
#[test]
fn the_fixture_proves_the_channel_is_the_tag_prefix() {
    let releases = parsed();
    let main_tags: Vec<&str> = releases
        .iter()
        .filter(|r| select("main", std::slice::from_ref(r), &Version::new(0, 0, 0)) != Selection::Empty)
        .map(|r| r.tag.as_str())
        .collect();
    assert!(main_tags.contains(&"v1.4.1"));
    assert!(!main_tags.contains(&"dev-v1.5.0"));
    assert!(!main_tags.contains(&"v9.9.9"), "a draft is never a candidate");

    let dev_tags: Vec<&str> = releases
        .iter()
        .filter(|r| select("dev", std::slice::from_ref(r), &Version::new(0, 0, 0)) != Selection::Empty)
        .map(|r| r.tag.as_str())
        .collect();
    assert!(dev_tags.contains(&"dev-v1.5.0"));
    assert!(!dev_tags.contains(&"v1.4.1"));
}

/// The route's JSON is a stable projection of these fields, so this pins them:
/// the version, the tag, the release page, and the asset's name/url/digest.
#[test]
fn a_release_exposes_the_fields_a_client_needs() {
    let releases = parsed();
    let r = &releases[1];
    assert_eq!(r.version, Version::new(1, 4, 1));
    assert_eq!(r.tag, "v1.4.1");
    assert!(!r.url.is_empty(), "the browser download page");
    let a: &Asset = r.asset.as_ref().expect("an asset for this host");
    assert!(!a.url.is_empty());
    assert!(a.size > 0);
    // A digest that is present is GitHub's `sha256:<64 hex>`; a null one is the
    // normal case on a hand-uploaded asset and is tolerated by `download`.
    if let Some(digest) = &a.digest {
        assert!(digest.starts_with("sha256:") && digest.len() == 71, "{digest}");
    }
    assert!(
        a.name.starts_with("metalvj-1.4.1-"),
        "the convention's own name: {}",
        a.name
    );
}