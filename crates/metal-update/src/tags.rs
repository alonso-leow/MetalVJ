//! The tag convention — the one rule that makes "channel = branch" true
//! without asking GitHub for a commit.
//!
//! GitHub's `GET /releases` gives every release a `tag_name` and a
//! `target_commitish`, but `target_commitish` is whatever string the person
//! typed when publishing, is *not* resolved against the repository, and is the
//! branch name for only some releases (a release cut from a commit SHA says
//! that SHA). Filtering on it would silently miss releases. Filtering on the
//! **tag** is the one thing that is always there and always exact.
//!
//! # The convention
//!
//! | channel | tag shape | example | prereleases |
//! | --- | --- | --- | --- |
//! | `main` | `v<version>` | `v1.2.3` | ignored |
//! | `dev` (any other branch name) | `<branch>-v<version>` | `dev-v1.2.4` | counted |
//! | `off` | — | — | — |
//!
//! * An optional `v`/`V` is accepted on either side of the branch prefix, so
//!   `vdev-v1.2.4` also reads as `dev`.
//! * The version half goes through [`crate::Version::parse`], so a tag with
//!   build metadata (`dev-v1.2.4+build.9`) is still one release.
//! * `main` never takes a prerelease. Every other channel does — a `dev` line
//!   exists to test what `main` has not blessed yet.
//! * A tag that carries *another* channel's prefix (`dev-v1.3.0` while the
//!   channel is `main`) is not this channel's release, and is skipped, not
//!   guessed at.
//!
//! The matcher is deliberately a prefix test on the *whole* tag: `dev-v1.2.4`
//! must not be read as `main`'s `v1.2.4` (it is not: `main` wants the tag to
//! start with `v`, and `dev-v...` starts with `d`).

use crate::Version;

#[allow(dead_code)]
/// The tag prefix a channel's releases carry: `v` for `main`, `<branch>-v`
/// otherwise. Lower-cased, trimmed; the channel is already known not to be off.
pub fn prefix_for(channel: &str) -> String {
    let channel = channel.trim();
    if channel.eq_ignore_ascii_case(crate::config::MAIN) {
        "v".to_string()
    } else {
        format!("{}-v", channel.to_ascii_lowercase())
    }
}

/// The version `tag` announces *for `channel`*, or `None` when the tag is not
/// this channel's. See the module docs for the shapes.
pub fn version_for(channel: &str, tag: &str) -> Option<Version> {
    let channel = channel.trim();
    if channel.is_empty() || channel.eq_ignore_ascii_case(crate::config::OFF) {
        return None;
    }
    let tag = tag.trim();
    let is_main = channel.eq_ignore_ascii_case(crate::config::MAIN);
    let mut rest = tag;
    if is_main {
        // `v1.2.3` or `V1.2.3` — nothing else is main's.
        rest = rest.strip_prefix(['v', 'V'])?;
        // A `dev-v1.2.3` tag stripped of its leading `d` is not a version, so
        // `Version::parse` refuses it below; but be explicit about the other
        // channels' prefix so the error message is honest.
        if rest.contains("-v") {
            return None;
        }
    } else {
        // `<branch>-v<version>`, or `v<branch>-v<version>` since a person may
        // put a `v` on the front too.
        let lowered = rest.to_ascii_lowercase();
        let prefixed = format!("{}-v", channel.to_ascii_lowercase());
        let alt = format!("v{prefixed}");
        let at = if lowered.starts_with(&alt) {
            alt.len()
        } else if lowered.starts_with(&prefixed) {
            prefixed.len()
        } else {
            return None;
        };
        rest = &rest[at..];
    }
    Version::parse(rest)
}

/// Whether `channel` accepts a prerelease. Only `main` does not.
pub fn accepts_prerelease(channel: &str) -> bool {
    !channel.trim().eq_ignore_ascii_case(crate::config::MAIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        assert_eq!(prefix_for("main"), "v");
        assert_eq!(prefix_for(" MAIN "), "v");
        assert_eq!(prefix_for("dev"), "dev-v");
        assert_eq!(prefix_for("promotion"), "promotion-v");
    }

    #[test]
    fn main_takes_v_tags_and_not_another_channels() {
        assert_eq!(version_for("main", "v1.2.3"), Version::parse("1.2.3"));
        assert_eq!(version_for("main", "V1.2.3"), Version::parse("1.2.3"));
        assert_eq!(version_for("main", "v1.2.3+build.7"), Version::parse("1.2.3"));
        assert_eq!(version_for("main", "v2.0.0-rc.1"), Version::parse("2.0.0-rc.1"));
        assert_eq!(version_for("main", "dev-v1.2.4"), None);
        assert_eq!(version_for("main", "1.2.3"), None, "no v prefix");
        assert_eq!(version_for("main", "release-1.2.3"), None);
    }

    #[test]
    fn dev_takes_its_own_prefixed_tags() {
        assert_eq!(version_for("dev", "dev-v1.2.4"), Version::parse("1.2.4"));
        assert_eq!(version_for("dev", "DEV-V1.2.4"), Version::parse("1.2.4"));
        assert_eq!(version_for("dev", "vdev-v1.2.4"), Version::parse("1.2.4"));
        assert_eq!(version_for("dev", "dev-v2.0.0-rc.2"), Version::parse("2.0.0-rc.2"));
        assert_eq!(version_for("dev", "v1.2.3"), None, "main's tag is not dev's");
        assert_eq!(version_for("dev", "promotion-v1.2.3"), None);
    }

    #[test]
    fn off_and_blank_take_nothing() {
        assert_eq!(version_for("off", "v1.2.3"), None);
        assert_eq!(version_for("", "v1.2.3"), None);
        assert_eq!(version_for("main", "  "), None);
    }

    #[test]
    fn only_main_refuses_prereleases() {
        assert!(!accepts_prerelease("main"));
        assert!(!accepts_prerelease("MAIN"));
        assert!(accepts_prerelease("dev"));
        assert!(accepts_prerelease("promotion"));
    }
}
