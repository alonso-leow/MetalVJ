//! The updater's settings and JSON replies, for the two routes in
//! `call::update`.
//!
//! **The crate is always linked; the behaviour is gated.** `metal-update`
//! itself knows nothing of Android or the browser (it is `std::fs` + `ureq` +
//! `sha2`, and `Cargo.lock` already had both), so `metal-dash` names it
//! unconditionally and only turns the *swap* off where it cannot work:
//!
//! - **desktop** (`desktop` feature): the whole feature — a writable install,
//!   a binary beside the exe, the restart dance.
//! - **Android**: no. The install is an APK the package manager owns; there is
//!   no file this process may rename, so every call answers `state: "off"` with
//!   the reason, and `finish_pending_update_at_startup` does nothing.
//! - **browser**: no binary on disk at all; same `off` answer.
//!
//! # The channel, and the `--updates <off|main|dev>` flag
//!
//! The setting lives in `[update]` of `~/.config/metalvj/config.toml`
//! (`metal-config`'s one file). The flag overrides it for this run only:
//! `crates/metal-dash/src/lib.rs`'s `Cli` stores it in [`CHANNEL_FLAG`], and
//! [`channel`] applies it over the file's own value. `off` disables the whole
//! feature for the run; `main` and any other branch name pick the tag
//! convention `metal-update`'s crate docs define (`v1.2.3` / `dev-v1.2.4`).
//!
//! # The reply shapes
//!
//! See `call::update`'s module docs for the full table. Both routes answer one
//! object with a `state` string; this module is the only place that builds it,
//! so the panel, a shell script and a test all parse the same thing.

use serde_json::{json, Value};

use crate::call::CallError;

/// `true` on a build that can actually swap a binary: the desktop, and only
/// the desktop. See the module docs.
const CAN_UPDATE: bool = cfg!(all(feature = "desktop", not(target_arch = "wasm32"), not(target_os = "android")));

/// The `--updates <off|main|dev>` the process was started with, set once from
/// `cli.updates` in `desktop_main`. `None` means "the config file's own
/// `[update].channel`".
static CHANNEL_FLAG: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Record `--updates`. First call wins (there is one `main`); a test that wants
/// a different value sets `METALVJ_UPDATE_CHANNEL` instead, which is read live.
pub fn set_channel_flag(value: Option<String>) {
    let _ = CHANNEL_FLAG.set(value);
}

/// The channel this run uses: `METALVJ_UPDATE_CHANNEL` (a test/deploy seam),
/// then the `--updates` flag, then the config file's `[update].channel`,
/// then `main`.
pub fn channel() -> String {
    if let Ok(v) = std::env::var("METALVJ_UPDATE_CHANNEL") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    if let Some(Some(v)) = CHANNEL_FLAG.get() {
        return v.clone();
    }
    config_value()
}

/// The channel from the config file alone (no `--updates`, no env): the
/// `[update].channel` table, or `main` when the file or table is absent.
fn config_value() -> String {
    metal_update::UpdateConfig::load()
        .map(|c| c.channel)
        .unwrap_or_else(|_| metal_update::MAIN.to_string())
}

/// Whether this run will never update: the build cannot, the channel is `off`,
/// or `[update].enabled = false`.
pub fn is_off() -> bool {
    if !CAN_UPDATE {
        return true;
    }
    channel().trim().eq_ignore_ascii_case(metal_update::OFF) || !config_enabled()
}

fn config_enabled() -> bool {
    metal_update::UpdateConfig::load().map(|c| c.enabled).unwrap_or(true)
}

/// The config this run's requests use: the file's `[update]` table with the
/// run's channel applied over it.
fn config() -> metal_update::UpdateConfig {
    let mut cfg = metal_update::UpdateConfig::load().unwrap_or_default();
    cfg.channel = channel();
    cfg
}

/// Why this build cannot update at all (the message an `off` answer carries).
fn unavailable_reason() -> &'static str {
    if target_arch_wasm() {
        "the browser build updates by reloading the page, not this route"
    } else if target_os_android() {
        "Android installs the APK the package manager shipped, not a file this process can swap"
    } else {
        "updates are off (`[update].channel = \"off\"`)"
    }
}

fn target_arch_wasm() -> bool {
    cfg!(target_arch = "wasm32")
}
fn target_os_android() -> bool {
    cfg!(target_os = "android")
}

/// `GET /update`: the full, read-only answer.
///
/// A test build answers `off` without a request: a unit test must not depend on
/// what GitHub says today, and the route table test walks every row.
pub fn check_reply() -> Value {
    if cfg!(test) {
        return json!({
            "op": "check",
            "state": "off",
            "channel": channel(),
            "repo": metal_update::DEFAULT_REPO,
            "current": metal_update::current().to_string(),
            "latest": Value::Null,
            "message": "off in a test build (no network)",
        });
    }
    if !CAN_UPDATE {
        return json!({
            "op": "check",
            "state": "off",
            "channel": channel(),
            "repo": metal_update::DEFAULT_REPO,
            "current": metal_update::current().to_string(),
            "latest": Value::Null,
            "message": unavailable_reason(),
        });
    }
    let cfg = config();
    let base = base_of(&cfg, "check");
    if cfg.is_off() {
        return with(base, "off", Value::Null, unavailable_reason());
    }
    match metal_update::check(&cfg) {
        Ok(metal_update::Check::Off) => with(base, "off", Value::Null, unavailable_reason()),
        Ok(metal_update::Check::UpToDate { current: _, latest }) => {
            let message = match &latest {
                Some(r) => format!("up to date (newest on {} is {})", cfg.channel, r.tag),
                None => format!("up to date ({} has no releases yet)", cfg.channel),
            };
            with(base, "up_to_date", latest.as_ref().map(release_json).unwrap_or(Value::Null), &message)
        }
        Ok(metal_update::Check::Available(rel)) => {
            let message = match &rel.asset {
                Some(_) => format!("{} is available", rel.tag),
                None => format!("{} is available (no build for this machine)", rel.tag),
            };
            with(base, "available", release_json(&rel), &message)
        }
        Err(e) => error_reply(base, "check", &e.to_string()),
    }
}

/// `POST /update` with `op: "apply"`: download the release whose tag is `tag`
/// and stage it. Refuses when the channel is off, when `tag` is not the one
/// the check reports, or when the release has no asset for this platform.
pub fn apply_reply(tag: &str) -> Result<Value, CallError> {
    if !CAN_UPDATE {
        return Ok(refused_unavailable(tag));
    }
    let cfg = config();
    if cfg.is_off() {
        return Ok(refused_unavailable(tag));
    }
    // The tag must be a release this channel actually offers — a client cannot
    // name an arbitrary asset from another channel (a `dev-` tag while on
    // `main`, say) and have it installed.
    let rel = match find_release(&cfg, tag) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Ok(refused(
                &cfg,
                tag,
                &format!("no release {tag:?} on channel {:?}", cfg.channel),
            ))
        }
        Err(e) => return Ok(error_reply(base_of(&cfg, "apply"), "apply", &e.to_string())),
    };
    match metal_update::apply(&cfg, &rel) {
        Ok(metal_update::Applied::Installed { path, verified }) => Ok(json!({
            "op": "apply",
            "state": "applied",
            "channel": cfg.channel,
            "repo": cfg.repo,
            "current": metal_update::current().to_string(),
            "latest": release_json(&rel),
            "restart": false,
            "verified": verified,
            "path": path.display().to_string(),
            "message": format!("{} installed", rel.tag),
        })),
        Ok(metal_update::Applied::Restart { path, pending, verified }) => Ok(json!({
            "op": "apply",
            "state": "applied",
            "channel": cfg.channel,
            "repo": cfg.repo,
            "current": metal_update::current().to_string(),
            "latest": release_json(&rel),
            "restart": true,
            "verified": verified,
            "path": path.display().to_string(),
            "pending": pending.display().to_string(),
            "message": format!("{} downloaded; restart MetalVJ to finish", rel.tag),
        })),
        Err(e) => Ok(json!({
            "op": "apply",
            "state": "error",
            "channel": cfg.channel,
            "repo": cfg.repo,
            "current": metal_update::current().to_string(),
            "latest": release_json(&rel),
            "restart": false,
            "verified": false,
            "message": e.to_string(),
        })),
    }
}

fn find_release(
    cfg: &metal_update::UpdateConfig,
    tag: &str,
) -> Result<Option<metal_update::Release>, metal_update::Error> {
    // The same one request `check` makes; `apply` needs the whole release
    // object (its asset URL and digest) and not just the tag.
    Ok(metal_update::releases(cfg)?.into_iter().find(|r| r.tag == tag))
}

fn base_of(cfg: &metal_update::UpdateConfig, op: &str) -> Value {
    json!({
        "op": op,
        "channel": cfg.channel.clone(),
        "repo": cfg.repo.clone(),
        "current": metal_update::current().to_string(),
    })
}

fn with(mut base: Value, state: &str, latest: Value, message: &str) -> Value {
    let obj = base.as_object_mut().expect("an object");
    obj.insert("state".into(), json!(state));
    obj.insert("latest".into(), latest);
    obj.insert("message".into(), json!(message));
    base
}

fn error_reply(mut base: Value, op: &str, message: &str) -> Value {
    let obj = base.as_object_mut().expect("an object");
    *obj.get_mut("op").expect("an op") = json!(op);
    obj.insert("state".into(), json!("error"));
    obj.insert("latest".into(), Value::Null);
    obj.insert("message".into(), json!(message));
    base
}

fn refused(cfg: &metal_update::UpdateConfig, tag: &str, message: &str) -> Value {
    json!({
        "op": "apply",
        "state": "refused",
        "channel": cfg.channel,
        "repo": cfg.repo,
        "current": metal_update::current().to_string(),
        "tag": tag,
        "restart": false,
        "verified": false,
        "message": message,
    })
}

fn refused_unavailable(tag: &str) -> Value {
    json!({
        "op": "apply",
        "state": "refused",
        "channel": channel(),
        "repo": metal_update::DEFAULT_REPO,
        "current": metal_update::current().to_string(),
        "tag": tag,
        "restart": false,
        "verified": false,
        "message": unavailable_reason(),
    })
}

fn release_json(rel: &metal_update::Release) -> Value {
    let asset = rel.asset.as_ref().map(|a| {
        json!({
            "name": a.name,
            "url": a.url,
            "size": a.size,
            "digest": a.digest,
        })
    });
    json!({
        "tag": rel.tag,
        "version": rel.version.to_string(),
        "published": rel.published,
        "url": rel.url,
        "notes": rel.notes,
        "prerelease": rel.is_prerelease(),
        "asset": asset.unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The env seam is process-wide, so tests that set it take turns.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_channel<T>(channel: &str, f: impl FnOnce() -> T) -> T {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("METALVJ_UPDATE_CHANNEL", channel);
        let out = f();
        std::env::remove_var("METALVJ_UPDATE_CHANNEL");
        out
    }

    #[test]
    fn the_env_seam_is_read_live_and_trimmed() {
        with_channel("dev", || assert_eq!(channel(), "dev"));
        with_channel(" OFF ", || {
            assert!(channel().trim().eq_ignore_ascii_case(metal_update::OFF))
        });
        // A locked guard cannot be held across `with_channel`; the value is
        // read live on each call, so nothing leaks once it is removed.
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("METALVJ_UPDATE_CHANNEL");
        assert_eq!(channel(), config_value());
    }

    #[test]
    fn a_release_projects_to_the_documented_keys() {
        let rel = metal_update::Release {
            version: metal_update::Version::new(1, 4, 1),
            tag: "v1.4.1".into(),
            url: "https://example.invalid/tag".into(),
            asset: Some(metal_update::Asset {
                name: "metalvj-1.4.1-windows-x86_64.exe".into(),
                url: "https://example.invalid/file".into(),
                size: 7,
                digest: Some("sha256:aa".into()),
            }),
            notes: "notes".into(),
            published: "2026-09-01T08:30:00Z".into(),
            prerelease: false,
            draft: false,
        };
        let v = release_json(&rel);
        assert_eq!(v["tag"], "v1.4.1");
        assert_eq!(v["version"], "1.4.1");
        assert_eq!(v["asset"]["size"], 7);
        assert_eq!(v["asset"]["digest"], "sha256:aa");
        assert_eq!(v["prerelease"], false);
    }

    #[test]
    fn a_refusal_carries_the_tag_and_a_message() {
        let cfg = metal_update::UpdateConfig::default();
        let v = refused(&cfg, "dev-v9.9.9", "no such release");
        assert_eq!(v["state"], "refused");
        assert_eq!(v["tag"], "dev-v9.9.9");
        assert_eq!(v["message"], "no such release");
        assert_eq!(v["restart"], false);
    }

    /// `GET` answers the documented keys on every path, and an `off` channel
    /// answers `off` without a request.
    #[test]
    fn the_check_reply_has_the_stable_shape() {
        let v = with_channel("off", check_reply);
        assert_eq!(v["state"], "off", "off needs no network: {v}");
        for key in ["op", "state", "channel", "repo", "current", "latest", "message"] {
            assert!(v.get(key).is_some(), "{key} missing: {v}");
        }
        assert_eq!(v["op"], "check");
    }

    /// `apply` on an `off` channel refuses without a request.
    #[test]
    fn an_off_channel_refuses_apply_before_any_download() {
        let v = with_channel("off", || apply_reply("v1.0.0").expect("a refusal is JSON"));
        assert_eq!(v["state"], "refused");
        assert_eq!(v["restart"], false);
        assert!(v["message"].as_str().is_some_and(|m| !m.is_empty()));
    }

    /// The `current` a reply reports is the app's version, which the
    /// `metal-dash/build.rs` script set to this package's own version.
    #[test]
    fn the_reported_version_is_the_apps() {
        assert_eq!(metal_update::current().to_string(), env!("CARGO_PKG_VERSION"));
    }
}