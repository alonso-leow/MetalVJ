//! Telling a running `metal-dash` that a file underneath it changed.
//!
//! Only some of `data/` needs this. `metal-dash` reads graphs off disk on every
//! request, so a landed graph is live with nothing to say. But `outputs.json`,
//! `collections.json`, `sequences.json` and the reel indexes are read once at
//! startup and held in memory, so landing one of those is invisible until the
//! process re-reads it. `POST /reload` is that re-read.
//!
//! # Why this is fifteen lines and not a dependency
//!
//! `metal-agent` already has a hand-rolled blocking HTTP client, and reusing it
//! was the first instinct. But `metal-agent` is the AI harness — tools, provider
//! config, the turn orchestrator — and pulling all of that into the sync
//! process to send one POST to localhost costs far more than the request does.
//! `metal-agent` wrote its own client for the same reason rather than take
//! `reqwest`; this follows the precedent instead of the code.
//!
//! A failure here is logged and dropped on purpose. `metal-dash` may simply not
//! be running — the two processes are independent by design — and sync must
//! not stall because nobody is listening.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

/// Does landing this path require `metal-dash` to re-read anything?
///
/// Graphs deliberately return `false`: `load_graph` reads per request, so a
/// reload for them would be a round trip that changes nothing.
pub fn needs_reload(rel: &Path) -> bool {
    let Some(text) = rel.to_str() else { return false };
    matches!(text, "outputs.json" | "collections.json" | "sequences.json" | "trims.json")
        || (text.starts_with("reels/") && text.ends_with(".json"))
        || text.starts_with("library/")
}

/// `POST /reload` to a local `metal-dash`. Best effort.
pub fn reload(host: &str, port: u16) {
    if let Err(e) = post_reload(host, port) {
        tracing::debug!(%host, port, %e, "could not reach metal-dash to reload; it may not be running");
    } else {
        tracing::info!("asked metal-dash to reload");
    }
}

fn post_reload(host: &str, port: u16) -> std::io::Result<()> {
    let addr = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no address"))?;

    // Short timeouts throughout: this is a localhost call, and the sync loop
    // must not be held up by a dashboard that is wedged.
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    write!(
        stream,
        "POST /reload HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;

    let mut response = String::new();
    stream.take(512).read_to_string(&mut response)?;
    if response.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "metal-dash answered {:?}",
            response.lines().next().unwrap_or("nothing")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cached_state_needs_a_reload() {
        for p in ["outputs.json", "collections.json", "sequences.json", "trims.json"] {
            assert!(needs_reload(&PathBuf::from(p)), "{p} is held in memory by metal-dash");
        }
        assert!(needs_reload(&PathBuf::from("reels/footage/index.json")));
        assert!(needs_reload(&PathBuf::from("library/movements.json")));
    }

    /// `load_graph` reads per request, so a reload here would be a round trip
    /// that changes nothing.
    #[test]
    fn graphs_do_not_need_a_reload() {
        assert!(!needs_reload(&PathBuf::from("graphs/entrance-set.json")));
        assert!(!needs_reload(&PathBuf::from("animations/entrance/song.npy")));
    }

    #[test]
    fn a_dashboard_that_is_not_running_is_not_an_error() {
        // Nothing listens on this port; the call must return rather than panic
        // or block, because the two processes are independent by design.
        reload("127.0.0.1", 1);
    }
}
