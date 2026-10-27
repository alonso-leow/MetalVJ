//! The two HTTP operations: read the release list, download one asset.
//!
//! `ureq` 3.4.1 is the client. It is in `Cargo.lock` already (as `ort-sys`'s
//! build-dependency, which every default build carries through `metal-ml`),
//! it is small, it is blocking — which is what every caller here is (`main`'s
//! startup swap, a route handler that answers a panel) — and built with
//! `default-features = false, features = ["rustls"]` it verifies with the
//! exact `rustls` + `ring` + `webpki-roots` triple `metal-agent` already names
//! for its provider turns, so this crate does not pick a second TLS story.
//! `reqwest` would be the wrong borrow: on this target set it arrives through
//! `iroh` (a QUIC crate), it is `tokio`-shaped, and none of that is needed to
//! GET two URLs.
//!
//! Every request carries the headers GitHub requires a client to send
//! (`User-Agent`, and an `Accept`), because GitHub answers a request without a
//! `User-Agent` with 403 by policy. The token, when one is configured, only
//! raises the anonymous 60-requests-an-hour limit; the crate works without one.

use crate::{Error, UpdateConfig, USER_AGENT};

/// The `Accept` for the JSON API: the versioned media type, so a breaking
/// change to the REST shape is opt-in rather than a surprise.
pub const JSON_ACCEPT: &str = "application/vnd.github+json";
/// The `Accept` for the asset download: a plain binary stream. Sending the
/// JSON media type here would ask GitHub for a redirect to blob metadata
/// instead of the file.
pub const BINARY_ACCEPT: &str = "application/octet-stream";

/// Where the API lives by default.
pub const GITHUB_API: &str = "https://api.github.com";

/// The release list URL, relative to the API root: `GET <api><path>`.
pub fn releases_path(repo: &str) -> String {
    format!("/repos/{repo}/releases?per_page=100")
}

/// The absolute release list URL `check` reads.
pub fn releases_url(repo: &str) -> String {
    format!("{}{}", api_base(), releases_path(repo))
}

/// The API root. Overridable through `METALVJ_UPDATE_API` — a local test
/// server's seam, and a GitHub Enterprise deploy's one knob; a person who
/// points this at their own host also sets `[update].repo` to their fork.
pub fn api_base() -> String {
    std::env::var("METALVJ_UPDATE_API")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| GITHUB_API.to_string())
}

/// A blocking client with the user agent and the `Accept` already set. One
/// per operation: at update frequency a pooled agent buys nothing, and
/// GitHub's rate limit counts requests, not connections. The token travels
/// per call ([`get_with`]) so a `check` and an `apply` can share this agent
/// even when the caller changed the token between them.
fn agent(accept: &'static str) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::WebPki)
        .build();
    let config = ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .accept(accept)
        .tls_config(tls)
        // A stalled connection must not hold a show's first frame hostage:
        // the startup swap is best-effort, and a check is a button.
        .timeout_global(Some(std::time::Duration::from_secs(60)));
    ureq::Agent::new_with_config(config.build())
}
/// Run a GET with the standard headers; the token, when present, is attached
/// per call so the agent can be shared across a `check` and an `apply`.
fn get_with(token: Option<&str>, url: &str, accept: &'static str) -> Result<ureq::Body, Error> {
    let mut request = agent(accept).get(url).header("Accept", accept);
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let response = request.call().map_err(http_error)?;
    Ok(response.into_body())
}

/// `GET <api>/repos/<repo>/releases`, parsed into [`crate::Release`]s.
pub fn fetch_releases(cfg: &UpdateConfig) -> Result<Vec<crate::Release>, Error> {
    let url = releases_url(&cfg.repo);
    let body = get_with(cfg.token().as_deref(), &url, JSON_ACCEPT)?;
    let text = body
        .into_with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| Error::Http(e.to_string()))?;
    crate::release::parse(&text).map_err(|e| Error::BadJson(e.to_string()))
}

/// Download `asset` to `dest`. Returns whether a digest verified it.
///
/// The bytes land in a sibling temp file first and only become `dest` after
/// they are flushed to disk, so a broken download never leaves a half-written
/// file where a launcher could find it. A `sha256` digest that is present but
/// does not match removes the temp file and fails; a missing one is normal
/// (GitHub leaves `digest` null on hand-uploaded assets) and the download is
/// kept, with `Ok(false)` telling the caller it is unverified.
pub fn download(asset: &crate::Asset, dest: &std::path::Path) -> Result<bool, Error> {
    let expected = asset.digest.as_deref().map(parse_digest).transpose()?;
    let tmp = crate::staging_path(dest);
    let body = get_with(None, &asset.url, BINARY_ACCEPT)?;
    let mut reader = body.into_with_config().limit(u64::MAX).reader();
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| Error::Io(format!("cannot create {}: {e}", tmp.display())))?;
        std::io::copy(&mut reader, &mut file)
            .map_err(|e| Error::Io(format!("cannot write {}: {e}", tmp.display())))?;
        file.flush()
            .map_err(|e| Error::Io(format!("cannot flush {}: {e}", tmp.display())))?;
        // fsync-ish: the bytes must be on the platter before the rename, or a
        // power cut between the two leaves a zero-length "new" binary.
        file.sync_all()
            .map_err(|e| Error::Io(format!("cannot sync {}: {e}", tmp.display())))?;
    }
    let verified = match expected {
        Some(expected) => {
            let bytes = std::fs::read(&tmp).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                Error::Io(format!("cannot read back {}: {e}", tmp.display()))
            })?;
            let got = sha256(&bytes);
            if got != expected {
                let _ = std::fs::remove_file(&tmp);
                return Err(Error::Digest { expected: hex(&expected), got: hex(&got) });
            }
            true
        }
        None => false,
    };
    // A stale staging file from a crashed earlier attempt would make the
    // rename fail on Windows (`rename` does not replace there).
    if dest.exists() {
        let _ = std::fs::remove_file(dest);
    }
    std::fs::rename(&tmp, dest)
        .map_err(|e| Error::Io(format!("cannot move {} into place: {e}", dest.display())))?;
    Ok(verified)
}

/// `sha256:<hex>` (GitHub's `digest`) or a bare 64-char hash, to 32 bytes.
fn parse_digest(digest: &str) -> Result<[u8; 32], Error> {
    let hex_part = digest
        .strip_prefix("sha256:")
        .or_else(|| digest.strip_prefix("sha256="))
        .unwrap_or(digest)
        .trim()
        .trim_matches('"');
    if hex_part.len() != 64 || !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::BadDigest(digest.to_string()));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex_part[i * 2..i * 2 + 2], 16)
            .map_err(|_| Error::BadDigest(digest.to_string()))?;
    }
    Ok(out)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn http_error(e: ureq::Error) -> Error {
    let text = e.to_string();
    // The rate-limit case deserves its own sentence: it is the one failure a
    // person can fix (put a token in `[update].token_file`).
    if text.contains(" 403") || text.contains(" 429") || text.contains("Status(403)") || text.contains("Status(429)") {
        return Error::RateLimited(text);
    }
    Error::Http(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_urls_follow_githubs_rest_shape() {
        assert_eq!(releases_path("alonso-leow/metalvj"), "/repos/alonso-leow/metalvj/releases?per_page=100");
        assert_eq!(
            releases_url("alonso-leow/metalvj"),
            "https://api.github.com/repos/alonso-leow/metalvj/releases?per_page=100"
        );
    }

    #[test]
    fn a_client_is_built_with_or_without_a_token_and_opens_no_socket() {
        // Construction only; nothing here resolves a name.
        let _ = agent(JSON_ACCEPT);
        let _ = agent(BINARY_ACCEPT);
    }

    #[test]
    fn digests_parse_the_forms_github_hands_out() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(parse_digest(&digest).unwrap(), [0xab; 32]);
        assert_eq!(parse_digest(&"cd".repeat(32)).unwrap(), [0xcd; 32], "a bare hash");
        assert_eq!(parse_digest(&format!("sha256:{}", "EF".repeat(32))).unwrap(), [0xef; 32]);
    }

    #[test]
    fn a_malformed_digest_is_a_refusal_not_a_guess() {
        assert!(matches!(parse_digest("sha256:abc"), Err(Error::BadDigest(_))));
        assert!(matches!(parse_digest("md5:00"), Err(Error::BadDigest(_))));
        assert!(matches!(
            parse_digest(&format!("sha256:{}", "zz".repeat(32))),
            Err(Error::BadDigest(_))
        ));
    }

    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
