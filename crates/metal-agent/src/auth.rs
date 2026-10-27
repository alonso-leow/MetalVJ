//! The per-install bearer token that guards metal-dash's `POST /agent/tool`.
//!
//! metal-dash creates the token on first start and writes it 0600; metal-mcp reads
//! it back so it can forward calls without the operator copying a secret.
//! Both sides resolve the same path: `$FW_AGENT_TOKEN_FILE` if set, else
//! `$XDG_CONFIG_HOME/metalvj/agent-token` (`~/.config/...` by default).
//! A token in the environment (`FW_DASH_TOKEN` for metal-mcp) wins over the file.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Overrides the token file's location for both metal-dash and metal-mcp.
pub const TOKEN_FILE_ENV: &str = "FW_AGENT_TOKEN_FILE";

/// Where the token lives on this machine.
pub fn token_path() -> PathBuf {
    if let Some(p) = std::env::var_os(TOKEN_FILE_ENV).filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    metal_config::app_dir_in(&config).join("agent-token")
}

/// The token stored at `path`, if there is one that is not blank.
pub fn read_token(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let token = text.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// The token at `path`, created first if the file does not exist. A new file
/// is 0600 from its first byte (no window where it is world-readable), and an
/// existing one is reused as is, so restarts keep the same token.
pub fn ensure_token(path: &Path) -> io::Result<String> {
    if let Some(token) = read_token(path) {
        return Ok(token);
    }
    let token = random_token()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{token}")?;
    Ok(token)
}

/// 32 bytes from the kernel's CSPRNG, hex-encoded.
fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// `true` when `presented` is exactly `expected`. The comparison touches every
/// byte of the longer input, so a wrong guess does not reveal a matching prefix.
pub fn tokens_match(expected: &str, presented: &str) -> bool {
    let (a, b) = (expected.as_bytes(), presented.as_bytes());
    let mut diff = (a.len() != b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_token_creates_once_then_reuses_0600() {
        let dir = std::env::temp_dir().join(format!("fw-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("agent-token");
        let first = ensure_token(&path).expect("create");
        assert_eq!(first.len(), 64);
        assert_eq!(ensure_token(&path).expect("reuse"), first);
        assert_eq!(read_token(&path).as_deref(), Some(first.as_str()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tokens_match_is_exact() {
        assert!(tokens_match("abc", "abc"));
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("abc", "ab"));
        assert!(!tokens_match("abc", ""));
    }
}
