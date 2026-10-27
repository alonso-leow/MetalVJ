//! 2026-10-09: a song's audio and digest, pulled on demand from a paired peer.
//!
//! The document never carries `audio/` or `digest/` (the allowlist excludes
//! them, `manifest.rs` tests it), so a phone that has the song's metadata but
//! not its sound has nothing to play. This module fills that gap without
//! widening the document: when a node opens a song it lacks, it asks each
//! paired peer for the song's **bundle** over `fw/frame/1`, takes the first
//! peer that holds one, and streams the files down.
//!
//! Two asks, both on the frame ALPN, both answered only to members (the
//! accept gate in `frame.rs` closes anything else before a byte is read):
//!
//! - `{"song": "<stem>"}` answers a normal [`Frame`] whose body is
//!   `{"files": [{rel, size, blake3}, ...]}`: the bundle the peer holds.
//! - `{"asset": "audio/x.mp3", "from": n}` answers a **streamed** reply: a
//!   2-byte status, an 8-byte big-endian total length, then the file's bytes
//!   from `n` on, in [`CHUNK`]-sized writes. Nothing is buffered whole on
//!   either side, so a long track does not cost its size in memory. Only files
//!   directly under `audio/` or `digest/` are served.
//!
//! The client writes `<name>.part`, resumes from it after a dropped trip,
//! checks the blake3 the listing promised, and renames into place. A file the
//! node already holds with the same hash is not fetched again.
//!
//! Progress lives in [`crate::pair::Club::pulls`] and is what the control
//! surface reports (`GET /song/pull`).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use metal_schema::{digest_file_name, TickRate};
use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt, SeekFrom};

use crate::frame::{Frame, ALPN};
use crate::manifest::is_safe;
use crate::pair::{addr_for, exchange, Net};

/// The extensions a song may be ingested as: `metal-dash`'s `author::audio::SOUND`.
pub const SOUND: [&str; 7] = [".mp3", ".wav", ".flac", ".m4a", ".ogg", ".opus", ".aiff"];

/// One write to the wire or the disk.
pub const CHUNK: usize = 64 * 1024;

/// Status (2 bytes) and total length (8 bytes) of a streamed asset reply.
pub const HEADER: usize = 10;

/// One file of a bundle, as the peer lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Relative to the data root, `audio/…` or `digest/…`.
    pub rel: String,
    pub size: u64,
    pub blake3: String,
}

/// A song name is a plain stem: no path separators, no dot-prefixed or
/// parent-relative forms.
pub fn is_song(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
}

/// The files a song's bundle may hold, relative to the data root. Some of
/// them may not exist on a given node.
pub fn bundle_names(song: &str) -> Vec<String> {
    let mut out: Vec<String> = SOUND.iter().map(|ext| format!("audio/{song}{ext}")).collect();
    out.push(format!("audio/{song}.tempo.json"));
    for rate in [TickRate::Base, TickRate::Fine] {
        let name = digest_file_name(song, rate);
        out.push(format!("audio/{name}"));
        out.push(format!("digest/{name}"));
    }
    out
}

/// Is `rel` a file the stream may serve: directly under `audio/` or `digest/`,
/// and safe under the data root.
pub fn servable(rel: &str) -> bool {
    if !is_safe(Path::new(rel)) {
        return false;
    }
    match rel.split_once('/') {
        Some(("audio" | "digest", name)) => !name.is_empty() && !name.contains('/'),
        _ => false,
    }
}

/// blake3 of a file, streamed.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut h = blake3::Hasher::new();
    h.update_reader(std::fs::File::open(path)?)?;
    Ok(h.finalize().to_hex().to_string())
}

/// The bundle this node holds for `song`.
pub fn local_bundle(data_root: &Path, song: &str) -> Vec<Entry> {
    bundle_names(song)
        .into_iter()
        .filter_map(|rel| {
            let path = data_root.join(&rel);
            let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file())?;
            let blake3 = hash_file(&path).ok()?;
            Some(Entry { rel, size: meta.len(), blake3 })
        })
        .collect()
}

/// The answer to a `{"song": …}` ask: this node's bundle for the song.
pub fn answer_listing(data_root: &Path, song: &str) -> Frame {
    if !is_song(song) {
        return Frame { status: 400, body: json!({ "error": "not a song name" }).to_string().into_bytes() };
    }
    let files = local_bundle(data_root, song);
    Frame { status: 200, body: json!({ "files": files }).to_string().into_bytes() }
}

/// Writes a streamed asset reply: the file's bytes from `from` on, or a 404
/// header for anything not servable.
pub async fn stream_asset<W: AsyncWrite + Unpin>(out: &mut W, data_root: &Path, rel: &str, from: u64) -> Result<()> {
    let file = if servable(rel) { tokio::fs::File::open(data_root.join(rel)).await.ok() } else { None };
    let Some(mut file) = file else {
        out.write_all(&header(404, 0)).await?;
        return Ok(());
    };
    let meta = file.metadata().await?;
    if !meta.is_file() {
        out.write_all(&header(404, 0)).await?;
        return Ok(());
    }
    let total = meta.len();
    out.write_all(&header(200, total)).await?;
    file.seek(SeekFrom::Start(from.min(total))).await?;
    copy_chunks(&mut file, out).await?;
    Ok(())
}

async fn copy_chunks<R, W>(src: &mut R, dst: &mut W) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 {
            return Ok(total);
        }
        dst.write_all(&buf[..n]).await?;
        total += n as u64;
    }
}

fn header(status: u16, total: u64) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[..2].copy_from_slice(&status.to_be_bytes());
    h[2..].copy_from_slice(&total.to_be_bytes());
    h
}

/// Where a pull stands, for the shell: `GET /song/pull?song=`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Pull {
    pub song: String,
    /// `idle`, `searching`, `pulling`, `done`, `missing` (no peer holds it),
    /// or `failed`.
    pub state: String,
    /// The peer the bytes come from.
    pub from: Option<String>,
    pub files: usize,
    pub done_files: usize,
    pub total_bytes: u64,
    pub got_bytes: u64,
    pub error: Option<String>,
}

impl Pull {
    fn new(song: &str, state: &str) -> Pull {
        Pull { song: song.to_string(), state: state.to_string(), ..Pull::default() }
    }
}

impl Net {
    /// The current pull of `song`, or an idle record.
    pub fn pull_status(&self, song: &str) -> Value {
        let pulls = self.club.pulls.lock().expect("pulls lock");
        match pulls.get(song) {
            Some(p) => json!(p),
            None => json!(Pull::new(song, "idle")),
        }
    }

    fn progress(&self, song: &str, f: impl FnOnce(&mut Pull)) {
        let mut pulls = self.club.pulls.lock().expect("pulls lock");
        f(pulls.entry(song.to_string()).or_insert_with(|| Pull::new(song, "idle")));
    }

    /// Pull `song`'s bundle from the first paired peer that holds it. Records
    /// its progress; never panics on a bad peer.
    pub async fn pull_song(&self, song: &str) {
        self.progress(song, |p| *p = Pull::new(song, "searching"));
        match self.pull_inner(song).await {
            Ok(Some(())) => self.progress(song, |p| p.state = "done".into()),
            Ok(None) => self.progress(song, |p| p.state = "missing".into()),
            Err(e) => {
                tracing::warn!(%song, error = %format!("{e:#}"), "song pull failed");
                let msg = format!("{e:#}");
                self.progress(song, |p| {
                    p.state = "failed".into();
                    p.error = Some(msg);
                });
            }
        }
    }

    async fn pull_inner(&self, song: &str) -> Result<Option<()>> {
        if !is_song(song) {
            bail!("not a song name: {song:?}");
        }
        let root = self.club.data_root.clone();
        let mine = {
            let (root, song) = (root.clone(), song.to_string());
            tokio::task::spawn_blocking(move || local_bundle(&root, &song)).await?
        };
        let peers: Vec<(String, String)> = {
            let book = self.club.book.lock().expect("pair book poisoned");
            book.peers().iter().map(|(n, id)| (n.clone(), id.clone())).collect()
        };
        let mut chosen = None;
        for (name, id) in peers {
            let Ok(eid) = id.parse::<iroh::EndpointId>() else { continue };
            let addr = addr_for(&self.club, eid);
            let req = json!({ "song": song }).to_string();
            let Ok(bytes) = exchange(&self.endpoint, addr.clone(), ALPN, req.as_bytes()).await else { continue };
            let Some(frame) = Frame::decode(&bytes) else { continue };
            if frame.status != 200 {
                continue;
            }
            let listed: Value = serde_json::from_slice(&frame.body).unwrap_or(Value::Null);
            let files: Vec<Entry> = serde_json::from_value(listed["files"].clone()).unwrap_or_default();
            if !files.is_empty() {
                chosen = Some((name, addr, files));
                break;
            }
        }
        let Some((name, addr, files)) = chosen else { return Ok(None) };

        let need: Vec<Entry> = files
            .into_iter()
            .filter(|f| !mine.iter().any(|m| m.rel == f.rel && m.blake3 == f.blake3))
            .collect();
        let total: u64 = need.iter().map(|f| f.size).sum();
        let count = need.len();
        self.progress(song, |p| {
            p.state = "pulling".into();
            p.from = Some(name.clone());
            p.files = count;
            p.done_files = 0;
            p.total_bytes = total;
            p.got_bytes = 0;
            p.error = None;
        });
        for f in &need {
            self.fetch(&addr, &root, song, f).await?;
            self.progress(song, |p| p.done_files += 1);
        }
        self.club.log("song_pull", json!({ "song": song, "from": name, "files": count, "bytes": total }));
        Ok(Some(()))
    }

    /// One file: resume from `<name>.part` when there is one, verify, rename.
    async fn fetch(&self, addr: &EndpointAddr, root: &Path, song: &str, f: &Entry) -> Result<()> {
        if !servable(&f.rel) {
            bail!("the listing offered {} which is not a bundle file", f.rel);
        }
        let dest: PathBuf = root.join(&f.rel);
        let part = PathBuf::from(format!("{}.part", dest.display()));
        if let Some(dir) = dest.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0).min(f.size);
        self.progress(song, |p| p.got_bytes += have);

        let conn = self.endpoint.connect(addr.clone(), ALPN).await.context("dialling the peer")?;
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(json!({ "asset": f.rel, "from": have }).to_string().as_bytes()).await?;
        send.finish()?;
        let mut head = [0u8; HEADER];
        recv.read_exact(&mut head).await.context("reading the stream header")?;
        let status = u16::from_be_bytes([head[0], head[1]]);
        let total = u64::from_be_bytes(head[2..].try_into().expect("8 bytes"));
        if status != 200 || total != f.size {
            bail!("{} answered {status} at {total} bytes, the listing said {}", f.rel, f.size);
        }

        let mut out = if have > 0 {
            tokio::fs::OpenOptions::new().append(true).open(&part).await?
        } else {
            tokio::fs::File::create(&part).await?
        };
        let mut buf = vec![0u8; CHUNK];
        let mut got = have;
        while got < f.size {
            let n = recv.read(&mut buf).await?.unwrap_or(0);
            if n == 0 {
                break;
            }
            let take = n.min((f.size - got) as usize);
            out.write_all(&buf[..take]).await?;
            got += take as u64;
            self.progress(song, |p| p.got_bytes += take as u64);
        }
        out.flush().await?;
        drop(out);
        conn.close(0u32.into(), b"done");
        if got != f.size {
            bail!("{} ended at {got} of {} bytes", f.rel, f.size);
        }
        let check = part.clone();
        let hash = tokio::task::spawn_blocking(move || hash_file(&check)).await??;
        if hash != f.blake3 {
            let _ = tokio::fs::remove_file(&part).await;
            bail!("{} does not match its listed hash", f.rel);
        }
        tokio::fs::rename(&part, &dest).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn song_names_are_plain_stems() {
        assert!(is_song("track 01"));
        assert!(is_song("my.song"));
        assert!(!is_song(""));
        assert!(!is_song(".."));
        assert!(!is_song("../outputs"));
        assert!(!is_song("a/b"));
        assert!(!is_song(".hidden"));
    }

    #[test]
    fn only_audio_and_digest_files_are_servable() {
        assert!(servable("audio/track.mp3"));
        assert!(servable("digest/track.8hz.npy"));
        assert!(!servable("outputs.json"));
        assert!(!servable("audio/sub/x.mp3"));
        assert!(!servable("audio/../outputs.json"));
        assert!(!servable("/etc/passwd"));
        assert!(!servable("ai.json"));
    }

    #[test]
    fn a_bundle_names_its_sound_its_tempo_and_both_digest_rates() {
        let names = bundle_names("song");
        assert!(names.contains(&"audio/song.mp3".to_string()));
        assert!(names.contains(&"audio/song.tempo.json".to_string()));
        assert!(names.contains(&"audio/song.8hz.npy".to_string()));
        assert!(names.contains(&"digest/song.32hz.npy".to_string()));
        assert!(names.iter().all(|n| servable(n)));
    }
}
