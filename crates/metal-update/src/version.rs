//! A ~40-line semver subset: `major.minor.patch` with an optional prerelease,
//! and the ordering the release picker needs.
//!
//! **Why not the `semver` crate.** It is in `Cargo.lock` (via `ort`), so adding
//! it would not have been a *new* dependency — but it is a build-dependency of
//! an unrelated crate, its `Version` is not `serde`-friendly for the shape this
//! crate hands a JSON route, and the whole job here is one table of comparisons
//! (`1.2.3 < 1.2.4 < 1.3.0 < 2.0.0`, a prerelease below its release, numeric
//! identifiers below alphanumeric ones). That is small enough to own and to
//! test visibly, which matters more than using a crate for it.
//!
//! The subset deliberately ignores build metadata (`+sha`): it never affects
//! ordering, and a release tag that carries one is still one release.

use std::cmp::Ordering;
use std::fmt;

/// `major.minor.patch`, plus the `-pre` suffix of a prerelease tag.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// `Some("rc.1")` for `1.2.3-rc.1`; `None` for a release.
    pub pre: Option<String>,
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version { major, minor, patch, pre: None }
    }

    /// A prerelease of [`Version::new`]: `Version::pre(1, 2, 3, "rc.1")`.
    pub fn pre(major: u64, minor: u64, patch: u64, pre: impl Into<String>) -> Version {
        Version { major, minor, patch, pre: Some(pre.into()) }
    }

    /// Parses `1.2.3`, `1.2.3-rc.1`, `v1.2.3`, `v1.2.3+build.7`, and a
    /// two- or one-number tag (`1.2` is `1.2.0`) because release tags are hand
    /// written. Build metadata after `+` is dropped: it never orders anything.
    ///
    /// `None` for anything else — the caller decides whether that is "ignore
    /// this tag" or an error.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let text = text.split_once('+').map_or(text, |(v, _)| v);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        if let Some(pre) = pre {
            if pre.is_empty()
                || !pre.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            {
                return None;
            }
        }
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = match parts.next() {
            Some(n) => n.parse().ok()?,
            None => 0,
        };
        let patch = match parts.next() {
            Some(n) => n.parse().ok()?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch, pre: pre.map(str::to_string) })
    }

    /// The exact string a tag or asset name uses for this version: `1.2.3` or
    /// `1.2.3-rc.1`. Never carries a `v` prefix (the tag code adds it).
    pub fn to_string_no_prefix(&self) -> String {
        match &self.pre {
            Some(pre) => format!("{}.{}.{}-{pre}", self.major, self.minor, self.patch),
            None => format!("{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

/// Semver precedence: numeric core first, then "a prerelease is lower than the
/// release it precedes", then identifier-by-identifier (numeric below
/// alphanumeric, shorter below longer when every shared identifier is equal).
impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => {
                    let mut left = a.split('.');
                    let mut right = b.split('.');
                    loop {
                        match (left.next(), right.next()) {
                            (None, None) => return Ordering::Equal,
                            (None, Some(_)) => return Ordering::Less,
                            (Some(_), None) => return Ordering::Greater,
                            (Some(x), Some(y)) => {
                                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                                    (Ok(x), Ok(y)) => x.cmp(&y),
                                    (Ok(_), Err(_)) => Ordering::Less,
                                    (Err(_), Ok(_)) => Ordering::Greater,
                                    (Err(_), Err(_)) => x.cmp(y),
                                };
                                if ord != Ordering::Equal {
                                    return ord;
                                }
                            }
                        }
                    }
                }
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_no_prefix())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s} parses"))
    }

    #[test]
    fn parse_reads_the_forms_a_release_tag_actually_uses() {
        assert_eq!(v("1.2.3"), Version::new(1, 2, 3));
        assert_eq!(v("v1.2.3"), Version::new(1, 2, 3));
        assert_eq!(v("V0.1.0-rc.2"), Version::pre(0, 1, 0, "rc.2"));
        assert_eq!(v("1.2.3+build.7"), Version::new(1, 2, 3));
        assert_eq!(v("1.2"), Version::new(1, 2, 0));
        assert_eq!(v("2"), Version::new(2, 0, 0));
        for bad in ["", "v", "1.2.3.4", "one.two.three", "1.2.3-", "1.2.3-!", "x1.2.3"] {
            assert_eq!(Version::parse(bad), None, "{bad:?} is not a version");
        }
    }

    #[test]
    fn display_round_trips_and_never_carries_a_v() {
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
        assert_eq!(v("1.2.3-rc.1").to_string(), "1.2.3-rc.1");
        assert_eq!(Version::parse("1.2.3-rc.1").unwrap(), v("1.2.3-rc.1"));
    }

    #[test]
    fn ordering_follows_semver_precedence() {
        let table = [
            // ascending, every adjacent pair strictly less
            "0.0.1", "0.1.0", "1.0.0", "1.0.1", "1.2.3", "1.10.0", "2.0.0",
            "2.0.1-alpha", "2.0.1-alpha.1", "2.0.1-alpha.beta", "2.0.1-beta",
            "2.0.1-beta.2", "2.0.1-beta.11", "2.0.1-rc.1", "2.0.1",
        ];
        for pair in table.windows(2) {
            let (a, b) = (v(pair[0]), v(pair[1]));
            assert!(a < b, "{} < {}", pair[0], pair[1]);
            assert!(b > a, "{} > {}", pair[1], pair[0]);
            assert_eq!(a.cmp(&b), a.partial_cmp(&b).unwrap());
        }
        assert_eq!(v("1.2.3+sha"), v("1.2.3"), "build metadata never orders");
        assert_eq!(v("1.2.3").cmp(&v("v1.2.3")), Ordering::Equal);
    }
}
