//! Reading `GET https://api.github.com/repos/{owner}/{repo}/releases`.
//!
//! Only the fields this crate acts on are typed. Everything else in a release
//! object (author, reactions, upload URLs, ...) is ignored, so a field GitHub
//! adds tomorrow cannot break the parse.

use serde::Deserialize;

use crate::tags;
use crate::version::Version;

/// One downloadable file on a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// `metalvj-0.2.0-windows-x86_64.exe`
    pub name: String,
    /// `https://github.com/.../releases/download/v0.2.0/...`
    pub url: String,
    pub size: u64,
    /// `sha256:<hex>` when GitHub published one. It is frequently `null` on
    /// older releases and on assets uploaded by hand, so this is an
    /// optimisation, never a requirement.
    pub digest: Option<String>,
}

/// A published release, reduced to what the picker and the downloader need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The exact tag (`v0.2.0`, `dev-v0.2.1`).
    pub tag: String,
    /// The browser download page for the release (`html_url`).
    pub url: String,
    /// The binary for *this* platform, or `None` when the release shipped none
    /// — a real state (a tags-only release, or a platform not built yet) and
    /// one `apply` refuses by name rather than downloading the wrong file.
    pub asset: Option<Asset>,
    pub notes: String,
    /// The RFC 3339 string GitHub published (`published_at`). Kept as a string
    /// rather than parsed into a date: it is displayed and only ever a
    /// tie-break, and both are lexical on this format.
    pub published: String,
    /// `prerelease` on the release object (a tag's own `-rc.1` suffix says it
    /// too; either is enough — see [`Release::is_prerelease`]).
    pub prerelease: bool,
    /// `draft` — GitHub only returns drafts to a token that may see them; the
    /// picker skips them either way, because nobody is waiting on a draft.
    pub draft: bool,
}

impl Release {
    pub fn is_prerelease(&self) -> bool {
        self.prerelease || self.version.pre.is_some()
    }
}

/// What the picker decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection<'a> {
    /// The channel's newest release is not newer than the running build.
    UpToDate { current: Version, latest: Option<&'a Release> },
    /// A newer release exists on this channel.
    Newer(&'a Release),
    /// The channel's newest release is *older* than the running build (a dev
    /// build ahead of its own tag) — reported, not treated as an error.
    Ahead { latest: Option<&'a Release> },
    /// The channel has no releases at all yet.
    Empty,
}

/// Choose the best release for `channel`, against the running build's
/// `current` version. Pure: no network, no clock, no disk — the whole decision
/// is `(channel, releases, current)` and every branch of it is unit tested.
///
/// Rules, in order:
///
/// 1. Keep releases whose tag is this channel's ([`tags::version_for`]).
/// 2. Drop drafts.
/// 3. Drop prereleases unless the channel allows them
///    ([`tags::accepts_prerelease`]: everything but `main`).
/// 4. Pick the **highest version** (not the most recent `published_at`: a
///    backport hotfix published last is not "the newer release").
/// 5. Compare it to `current`.
pub fn select<'a>(channel: &str, releases: &'a [Release], current: &Version) -> Selection<'a> {
    if channel.trim().eq_ignore_ascii_case(crate::config::OFF) {
        // Callers turn this into `Check::Off`; here it simply has no releases.
        return Selection::Empty;
    }
    let allow_pre = tags::accepts_prerelease(channel);
    let best = releases
        .iter()
        .filter(|r| !r.draft)
        .filter(|r| tags::version_for(channel, &r.tag).is_some())
        .filter(|r| allow_pre || !r.is_prerelease())
        .max_by(|a, b| a.version.cmp(&b.version).then_with(|| a.published.cmp(&b.published)));
    let Some(best) = best else {
        return Selection::Empty;
    };
    match best.version.cmp(current) {
        std::cmp::Ordering::Greater => Selection::Newer(best),
        std::cmp::Ordering::Equal => Selection::UpToDate { current: current.clone(), latest: Some(best) },
        std::cmp::Ordering::Less => Selection::Ahead { latest: Some(best) },
    }
}

/// How a release asset's name reads: lower-cased, every run of non-alphanumeric
/// characters collapsed into one hyphen, with the *extension* dot restored:
/// `MetalVJ 0.2.0 (Win64).EXE` and `metalvj-0.2.0-win64.exe` are the same
/// shape. The dot before the last alphabetic run is the one exception, because
/// an extension is the one place a `.` means something.
pub fn natural(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
            if c == '.' && !out.is_empty() {
                // Keep the extension's dot; every other separator is a hyphen
                // (added lazily, so `a..b` is still `a-b`).
                out.push('.');
                pending_dash = false;
            }
        }
    }
    out
}

/// The asset for this platform, preferring the exact `-<os>-<arch>.<ext>`
/// spelling, then the platform's own extension, then the shortest name (the
/// most specific — a `.sha256` sidecar is longer), then the largest file.
pub fn pick_asset(assets: &[Asset]) -> Option<&Asset> {
    let (os, arch) = this_platform();
    pick_for(assets, &os, &arch)
}

/// [`pick_asset`] for an explicit platform, owned: the shape the crate's own
/// tests use so the answer does not depend on the machine running them.
#[cfg(test)]
pub fn pick_asset_for_platform(assets: &[Asset], os: &[&str], arch: &[&str]) -> Option<Asset> {
    pick_for(assets, os, arch).cloned()
}

/// True when `name` looks like a binary for this platform. See [`pick_for`].
pub fn asset_matches(name: &str) -> bool {
    let (os, arch) = this_platform();
    matches_platform(&natural(name), &os, &arch) != Match::None
}

/// How well a name matches a platform: nothing, a loose match, or the
/// convention's own exact `-<os>-<arch>.<ext>` spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Match {
    None,
    Loose,
    Exact,
}

/// The OS/arch aliases this build responds to, in preference order (the first
/// is what the convention's own naming scheme uses). Longer aliases come
/// first, because `x8664` must win over `x64` in the exact-spelling check.
pub fn this_platform() -> (Vec<&'static str>, Vec<&'static str>) {
    let os = match std::env::consts::OS {
        "windows" => vec!["windows", "win"],
        "linux" => vec!["linux"],
        "macos" => vec!["macos", "darwin", "osx", "mac"],
        other => vec![other],
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => vec!["x8664", "amd64", "x64"],
        "aarch64" => vec!["aarch64", "arm64"],
        "x86" => vec!["i686", "x86", "386"],
        "arm" => vec!["armv7", "arm"],
        other => vec![other],
    };
    (os, arch)
}

/// Every binary extension the matcher accepts.
const BINARY_EXTENSIONS: &[&str] = &[".appimage", ".exe", ".zip"];

/// The platform's own extension, for the tie-break only.
fn preferred_extension() -> &'static str {
    match std::env::consts::OS {
        "windows" => ".exe",
        _ => ".appimage",
    }
}

/// How `name` (naturalised) matches `(os, arch)`.
///
/// A name carries an OS token and an arch token as **whole hyphen-delimited
/// parts** of the naturalised name: `metalvj-1.2.3-windows-x86_64.exe` splits
/// into `[...] windows x86 64 exe`, so a host asking for `x64` has to see both
/// `x86` and `64` (or the fused `x8664`). A token is never a bare substring of
/// another token: `arm` does not match through `arm64`, `x86` does not match
/// through `x8664`.
fn matches_platform(name: &str, os: &[&str], arch: &[&str]) -> Match {
    let parts = parts_of(name);
    let has_os = os.iter().any(|t| token_present(t, &parts));
    let has_arch = arch_present(arch, &parts);
    if !has_os || !has_arch {
        return Match::None;
    }
    // The exact spelling: the convention's own OS alias is followed by an arch
    // alias *as adjacent parts* (`...-windows-x86_64.exe`). A hand upload
    // (`win64`) and a version-geometry name still match, but loosely.
    let exact = (0..parts.len().saturating_sub(1)).any(|i| {
        os.iter().any(|o| parts[i] == *o) && arch.iter().any(|a| parts[i + 1] == *a)
    });
    if exact {
        Match::Exact
    } else {
        Match::Loose
    }
}

/// The naturalised name's hyphen-delimited parts, extension dot split off.
fn parts_of(name: &str) -> Vec<&str> {
    name.split(['-', '.']).filter(|p| !p.is_empty()).collect()
}

/// Real-world spellings that are one hyphenated word but two tokens. A hand
/// upload (`Win64`) and the version-geometry style (`linux_x86_64`) both read
/// as these, and an arch token must see them.
const FUSED: &[(&str, &[&str])] = &[
    // (fused, the tokens it carries)
    ("win64", &["win", "64", "x64", "x8664", "amd64"]),
    ("win32", &["win", "32", "x86", "i686", "386"]),
    ("linux64", &["linux", "64", "x64", "x8664", "amd64"]),
    ("mac64", &["mac", "64", "x64", "x8664"]),
    ("darwin64", &["darwin", "64", "x64", "x8664"]),
    ("x8664", &["x86", "64", "x64", "x8664", "amd64"]),
    ("arm64", &["arm", "64", "arm64", "aarch64"]),
];

/// An OS/arch alias is either one whole part (`windows`, `x8664`, `x64`) or one
/// half of a fused part (`win64`, `arm64`) — never a bare substring, which is
/// what keeps `arm` off an `arm64` build and `x86` off an `x8664` one, and what
/// a `64`-part alone (`x86-64`) is completed by in [`token_present_pair`].
fn token_present(token: &str, parts: &[&str]) -> bool {
    if parts.iter().any(|p| *p == token) {
        return true;
    }
    FUSED.iter().any(|(fused, tokens)| {
        parts.iter().any(|p| p == fused) && tokens.contains(&token)
    })
}

/// The one completion a split arch needs: `x86` and `64` as adjacent parts is
/// the 64-bit x86 build, which is what the native release name writes.
fn split_x86_64(parts: &[&str]) -> bool {
    parts.windows(2).any(|w| w[0] == "x86" && w[1] == "64")
}

fn arch_present(arch: &[&str], parts: &[&str]) -> bool {
    if arch.iter().any(|t| token_present(t, parts)) {
        return true;
    }
    // `x64` may be answered by the `x86` + `64` pair; nothing else.
    split_x86_64(parts) && arch.iter().any(|t| *t == "x64" || *t == "x8664" || *t == "amd64")
}

/// The asset for an explicit platform, preferring the exact spelling, then the
/// platform's own extension, then the shortest name, then the largest file.
/// `None` when the release has no binary for it.
fn pick_for<'a>(assets: &'a [Asset], os: &[&str], arch: &[&str]) -> Option<&'a Asset> {
    let (mut best, mut best_key): (Option<&Asset>, Option<(Match, u8, usize, u64)>) = (None, None);
    for asset in assets {
        let n = natural(&asset.name);
        if !BINARY_EXTENSIONS.iter().any(|e| n.ends_with(e)) {
            continue;
        }
        let m = matches_platform(&n, os, arch);
        if m == Match::None {
            continue;
        }
        let key = (m, u8::from(!n.ends_with(preferred_extension())), usize::MAX - n.len(), asset.size);
        if best_key.is_none_or(|k| key > k) {
            best = Some(asset);
            best_key = Some(key);
        }
    }
    best
}

/// The version half of a channel-agnostic tag: everything from the last `v`
/// that starts a number, so `v1.2.3`, `dev-v1.2.4` and `1.0.0` all read.
fn tag_version(tag: &str) -> Option<Version> {
    if let Some(v) = Version::parse(tag) {
        return Some(v);
    }
    let at = tag.rfind("-v")?;
    Version::parse(&tag[at + 2..])
}

/// The GitHub wire shape of one release, flattened from the JSON.
#[derive(Deserialize)]
struct WireRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<WireAsset>,
}

#[derive(Deserialize)]
struct WireAsset {
    name: String,
    /// `browser_download_url` is the URL `apply` downloads (`url` is the API
    /// URL, which needs a token and a different `Accept`).
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

/// Parse a `GET /releases` body into [`Release`]s, tagged with each one's
/// platform asset. A tag whose version half does not parse is skipped —
/// publishing a release named `nightly` must not fail the update.
///
/// The channel filter is [`select`]'s job, not this one's, so a caller can hand
/// one parsed list to two different channels (and a test can read a single
/// fixture against both). The only channel-shaped decision here is the very
/// loose one: a tag must carry a `<...>v?<version>` shape the picker could
/// later recognise.
pub fn parse(json: &str) -> Result<Vec<Release>, serde_json::Error> {
    let wire: Vec<WireRelease> = serde_json::from_str(json)?;
    let mut out = Vec::new();
    for w in wire {
        let Some(version) = tag_version(&w.tag_name) else {
            continue;
        };
        let assets: Vec<Asset> = w
            .assets
            .into_iter()
            .filter(|a| !a.browser_download_url.is_empty())
            .map(|a| Asset { name: a.name, url: a.browser_download_url, size: a.size, digest: a.digest })
            .collect();
        let asset = pick_asset(&assets).cloned();
        out.push(Release {
            version,
            tag: w.tag_name,
            url: w.html_url,
            asset,
            notes: w.body.unwrap_or_default(),
            published: w.published_at.unwrap_or_default(),
            prerelease: w.prerelease,
            draft: w.draft,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset { name: name.to_string(), url: format!("https://example.invalid/{name}"), size: 1, digest: None }
    }

    /// A release built by hand: the tag's own version half, no network, and an
    /// asset picked for a *fixed* windows x86_64 platform so the assertion is
    /// the same on every host.
    fn release(tag: &str, assets: &[&str]) -> Release {
        let owned: Vec<Asset> = assets.iter().map(|a| asset(a)).collect();
        Release {
            version: tag_version(tag).expect("a version in the tag"),
            tag: tag.to_string(),
            url: String::new(),
            asset: pick_for(&owned, &["windows", "win"], &["x8664", "amd64", "x64"]).cloned(),
            notes: String::new(),
            published: String::new(),
            prerelease: false,
            draft: false,
        }
    }

    #[test]
    fn natural_folds_case_and_keeps_the_extension_dot() {
        assert_eq!(natural("MetalVJ 0.2.0 Win64.EXE"), "metalvj-0.2.0-win64.exe");
        assert_eq!(natural("metalvj-0.2.0-win64.exe"), "metalvj-0.2.0-win64.exe");
        assert_eq!(natural("  a__b--c..d  "), "a-b-c..d", "every separator run is one hyphen; a dot stays a dot");
    }

    #[test]
    fn this_platform_has_the_convention_aliases_first() {
        let (os, arch) = this_platform();
        assert!(!os.is_empty() && !arch.is_empty());
        match std::env::consts::OS {
            "windows" => assert_eq!(os[0], "windows"),
            "linux" => assert_eq!(os[0], "linux"),
            _ => {}
        }
        match std::env::consts::ARCH {
            "x86_64" => assert_eq!(arch[0], "x8664"),
            "aarch64" => assert_eq!(arch[0], "aarch64"),
            _ => {}
        }
    }

    /// The asset matcher is tolerant but not blind: right OS + right arch, and
    /// a wrong-arch or wrong-OS asset is never picked.
    #[test]
    fn only_this_platforms_binary_is_picked() {
        let assets = vec![
            asset("metalvj-0.2.0-windows-x86_64.exe"),
            asset("metalvj-0.2.0-windows-aarch64.exe"),
            asset("metalvj-0.2.0-linux-x86_64.AppImage"),
            asset("metalvj-0.2.0-macos-aarch64.AppImage"),
            asset("checksums.txt"),
        ];
        let picked = pick_for(&assets, &["windows", "win"], &["x8664", "amd64", "x64"]).unwrap();
        assert_eq!(picked.name, "metalvj-0.2.0-windows-x86_64.exe");
        let picked = pick_for(&assets, &["linux"], &["x8664", "amd64", "x64"]).unwrap();
        assert_eq!(picked.name, "metalvj-0.2.0-linux-x86_64.AppImage");
        let picked = pick_for(&assets, &["macos", "darwin", "mac"], &["aarch64", "arm64"]).unwrap();
        assert_eq!(picked.name, "metalvj-0.2.0-macos-aarch64.AppImage");
        assert_eq!(pick_for(&assets, &["linux"], &["aarch64", "arm64"]), None, "no linux arm build");
        assert_eq!(pick_for(&assets, &["freebsd"], &["x8664"]), None, "no freebsd build");
        assert_eq!(
            pick_for(&assets, &["windows", "win"], &["aarch64", "arm64"]).unwrap().name,
            "metalvj-0.2.0-windows-aarch64.exe",
            "the arm build is picked for an arm host"
        );
    }

    #[test]
    fn a_loose_hand_uploaded_name_still_matches() {
        let assets = vec![asset("MetalVJ 0.3.0 Win64.EXE"), asset("MetalVJ 0.3.0 Linux.AppImage")];
        let picked = pick_for(&assets, &["windows", "win"], &["x8664", "x64"]).unwrap();
        assert_eq!(picked.name, "MetalVJ 0.3.0 Win64.EXE");
    }

    #[test]
    fn an_x86_64_asset_is_not_read_as_32_bit() {
        let assets = vec![asset("metalvj-0.2.0-windows-x86_64.exe"), asset("metalvj-0.2.0-windows-i686.exe")];
        assert_eq!(
            pick_for(&assets, &["windows", "win"], &["i686", "x86", "386"]).unwrap().name,
            "metalvj-0.2.0-windows-i686.exe"
        );
        assert_eq!(
            pick_for(&assets, &["windows", "win"], &["x8664", "amd64", "x64"]).unwrap().name,
            "metalvj-0.2.0-windows-x86_64.exe"
        );
    }

    #[test]
    fn main_ignores_prereleases_and_dev_takes_them() {
        let mut pre = release("v2.0.0-rc.1", &[]);
        pre.prerelease = true;
        let stable = release("v1.5.0", &[]);
        let releases = vec![pre, stable];
        let current = Version::new(1, 0, 0);
        match select("main", &releases, &current) {
            Selection::Newer(r) => assert_eq!(r.tag, "v1.5.0"),
            other => panic!("{other:?}"),
        }
        match select("dev", &[release("dev-v2.0.0-rc.1", &[])], &current) {
            Selection::Newer(r) => assert_eq!(r.tag, "dev-v2.0.0-rc.1"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn draft_is_never_picked() {
        let mut draft = release("v9.9.9", &[]);
        draft.draft = true;
        match select("main", &[draft, release("v1.0.1", &[])], &Version::new(1, 0, 0)) {
            Selection::Newer(r) => assert_eq!(r.tag, "v1.0.1"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn equal_is_up_to_date_older_is_ahead_and_empty_is_empty() {
        let current = Version::new(1, 2, 3);
        match select("main", &[release("v1.2.3", &[])], &current) {
            Selection::UpToDate { latest: Some(r), .. } => assert_eq!(r.tag, "v1.2.3"),
            other => panic!("{other:?}"),
        }
        match select("main", &[release("v1.0.0", &[])], &current) {
            Selection::Ahead { latest: Some(r) } => assert_eq!(r.tag, "v1.0.0"),
            other => panic!("{other:?}"),
        }
        assert_eq!(select("main", &[], &current), Selection::Empty);
    }

    #[test]
    fn the_highest_version_wins_not_the_latest_upload() {
        let mut newer_upload = release("v1.4.1", &[]);
        newer_upload.published = "2026-10-01T00:00:00Z".into();
        let mut older_upload = release("v1.4.2", &[]);
        older_upload.published = "2026-09-01T00:00:00Z".into();
        match select("main", &[newer_upload, older_upload], &Version::new(1, 0, 0)) {
            Selection::Newer(r) => assert_eq!(r.tag, "v1.4.2"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parsing_keeps_every_tag_but_refuses_one_without_a_version() {
        let json = r#"
        [
          { "tag_name": "v1.2.3", "html_url": "u1", "body": "notes", "published_at": "2026-01-01T00:00:00Z",
            "assets": [
              { "name": "metalvj-1.2.3-linux-x86_64.AppImage", "browser_download_url": "a1", "size": 10, "digest": "sha256:aa" },
              { "name": "metalvj-1.2.3-windows-x86_64.exe", "browser_download_url": "a2", "size": 20, "digest": null },
              { "name": "metalvj-1.2.3-windows-aarch64.exe", "browser_download_url": "a3", "size": 20, "digest": null }
            ] },
          { "tag_name": "dev-v1.3.0", "html_url": "u2", "prerelease": true, "assets": [] },
          { "tag_name": "nightly", "assets": [] }
        ]"#;
        let releases = parse(json).unwrap();
        assert_eq!(releases.len(), 2, "the version-less tag is skipped");
        assert_eq!(releases[0].version, Version::new(1, 2, 3));
        assert_eq!(releases[0].notes, "notes");
        assert_eq!(releases[0].published, "2026-01-01T00:00:00Z");
        assert!(releases[1].is_prerelease());
        // The windows x86_64 binary on an x86_64 windows host, never the arm
        // one, and the linux `.AppImage` on linux.
        if std::env::consts::OS == "windows" && std::env::consts::ARCH == "x86_64" {
            let asset = releases[0].asset.as_ref().expect("a windows asset");
            assert_eq!(asset.name, "metalvj-1.2.3-windows-x86_64.exe");
            assert_eq!(asset.url, "a2");
            assert_eq!(asset.digest, None, "a null digest is tolerated");
        }
        if std::env::consts::OS == "linux" && std::env::consts::ARCH == "x86_64" {
            let asset = releases[0].asset.as_ref().expect("a linux asset");
            assert_eq!(asset.name, "metalvj-1.2.3-linux-x86_64.AppImage");
            assert_eq!(asset.digest.as_deref(), Some("sha256:aa"));
        }
    }
}
