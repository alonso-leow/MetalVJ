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
        // A device graph and its owner stamp, directly under `graphs/`.
        Some(("graphs", name)) => name.starts_with("device-") && (name.ends_with(".json") || name.ends_with(".owner")) && !name.contains('/'),
        // A model file, or one folder of them (`models/basic-pitch/x.onnx`).
        Some(("models", rest)) => {
            let parts: Vec<&str> = rest.split('/').collect();
            parts.len() <= 2 && parts.iter().all(|p| !p.is_empty())
        }
        _ => false,
    }
}

/// What an on-demand pull moves. The song bundle is the first; device graphs
/// and models follow the same listing, stream and hash check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Song,
    /// `graphs/device-<id>.json` (+ `.owner`), the name being `device-<id>`.
    Graph,
    /// `models/<name>` — one file, or every file of a folder of that name.
    Model,
}

impl Kind {
    /// The wire ask for `name`.
    pub fn ask(self, name: &str) -> Value {
        match self {
            Kind::Song => json!({ "song": name }),
            Kind::Graph => json!({ "graph": name }),
            Kind::Model => json!({ "model": name }),
        }
    }

    /// The ask on the wire, whichever kind it is.
    pub fn of_ask(req: &Value) -> Option<(Kind, &str)> {
        [("song", Kind::Song), ("graph", Kind::Graph), ("model", Kind::Model)]
            .into_iter()
            .find_map(|(k, kind)| req[k].as_str().map(|n| (kind, n)))
    }

    /// The key a pull's progress is filed under: a song by its name, the rest
    /// prefixed with their kind.
    pub fn key(self, name: &str) -> String {
        match self {
            Kind::Song => name.to_string(),
            Kind::Graph => format!("graph:{name}"),
            Kind::Model => format!("model:{name}"),
        }
    }

    /// The files of `name`'s bundle, relative to the data root; some may not
    /// exist on a given node.
    pub fn names(self, data_root: &Path, name: &str) -> Vec<String> {
        match self {
            Kind::Song => bundle_names(name),
            Kind::Graph => vec![format!("graphs/{name}.json"), format!("graphs/{name}.owner")],
            Kind::Model => {
                let dir = data_root.join("models").join(name);
                if dir.is_dir() {
                    let mut files: Vec<String> = std::fs::read_dir(&dir)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .filter(|e| e.path().is_file())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .map(|f| format!("models/{name}/{f}"))
                        .collect();
                    files.sort();
                    files
                } else {
                    vec![format!("models/{name}")]
                }
            }
        }
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
    local_files(data_root, Kind::Song, song)
}

/// The bundle this node holds for `name` of `kind`: only files a stream may serve.
pub fn local_files(data_root: &Path, kind: Kind, name: &str) -> Vec<Entry> {
    kind.names(data_root, name)
        .into_iter()
        .filter(|rel| servable(rel))
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
    answer_bundle(data_root, Kind::Song, song)
}

/// The answer to a `{"song"|"graph"|"model": …}` ask.
pub fn answer_bundle(data_root: &Path, kind: Kind, name: &str) -> Frame {
    if !is_song(name) {
        return Frame { status: 400, body: json!({ "error": "not a plain name" }).to_string().into_bytes() };
    }
    let files = local_files(data_root, kind, name);
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
        self.pull(Kind::Song, song).await
    }

    /// The same for a device graph (`device-<id>`) or a model (`models/<name>`).
    /// Progress is filed under [`Kind::key`].
    pub async fn pull(&self, kind: Kind, name: &str) {
        let key = kind.key(name);
        self.progress(&key, |p| *p = Pull::new(name, "searching"));
        match self.pull_inner(kind, name).await {
            Ok(Some(())) => self.progress(&key, |p| p.state = "done".into()),
            Ok(None) => self.progress(&key, |p| p.state = "missing".into()),
            Err(e) => {
                tracing::warn!(%name, error = %format!("{e:#}"), "pull failed");
                let msg = format!("{e:#}");
                self.progress(&key, |p| {
                    p.state = "failed".into();
                    p.error = Some(msg);
                });
            }
        }
    }

    async fn pull_inner(&self, kind: Kind, name: &str) -> Result<Option<()>> {
        if !is_song(name) {
            bail!("not a plain name: {name:?}");
        }
        let key = kind.key(name);
        let root = self.club.data_root.clone();
        let mine = {
            let (root, name) = (root.clone(), name.to_string());
            tokio::task::spawn_blocking(move || local_files(&root, kind, &name)).await?
        };
        let peers: Vec<(String, String)> = {
            let book = self.club.book.lock().expect("pair book poisoned");
            book.peers().iter().map(|(n, id)| (n.clone(), id.clone())).collect()
        };
        let mut chosen = None;
        for (peer, id) in peers {
            let Ok(eid) = id.parse::<iroh::EndpointId>() else { continue };
            let addr = addr_for(&self.club, eid);
            let req = kind.ask(name).to_string();
            let Ok(bytes) = exchange(&self.endpoint, addr.clone(), ALPN, req.as_bytes()).await else { continue };
            let Some(frame) = Frame::decode(&bytes) else { continue };
            if frame.status != 200 {
                continue;
            }
            let listed: Value = serde_json::from_slice(&frame.body).unwrap_or(Value::Null);
            let files: Vec<Entry> = serde_json::from_value(listed["files"].clone()).unwrap_or_default();
            if !files.is_empty() {
                chosen = Some((peer, addr, files));
                break;
            }
        }
        let Some((peer, addr, files)) = chosen else { return Ok(None) };

        let need: Vec<Entry> = files
            .into_iter()
            .filter(|f| !mine.iter().any(|m| m.rel == f.rel && m.blake3 == f.blake3))
            .collect();
        let total: u64 = need.iter().map(|f| f.size).sum();
        let count = need.len();
        self.progress(&key, |p| {
            p.state = "pulling".into();
            p.from = Some(peer.clone());
            p.files = count;
            p.done_files = 0;
            p.total_bytes = total;
            p.got_bytes = 0;
            p.error = None;
        });
        for f in &need {
            self.fetch(&addr, &root, &key, f).await?;
            self.progress(&key, |p| p.done_files += 1);
        }
        self.club.log("song_pull", json!({ "song": name, "kind": format!("{kind:?}"), "from": peer, "files": count, "bytes": total }));
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

    #[test]
    fn device_graphs_and_models_are_servable_and_nothing_near_them() {
        assert!(servable("graphs/device-ab12.json"));
        assert!(servable("graphs/device-ab12.owner"));
        assert!(!servable("graphs/main.json"), "only device graphs are pulled on demand");
        assert!(!servable("graphs/nested/device-x.json"));
        assert!(servable("models/basic-pitch/model.onnx"));
        assert!(servable("models/x.onnx"));
        assert!(!servable("models/a/b/c.onnx"));
        assert!(!servable("models/../settings.json"));
        assert!(!servable("models/"));
    }

    #[test]
    fn a_model_folder_lists_its_files_and_a_graph_its_json_and_owner() {
        let dir = std::env::temp_dir().join(format!("fw-pull-kinds-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("models/bp")).unwrap();
        std::fs::write(dir.join("models/bp/m.onnx"), b"m").unwrap();
        std::fs::write(dir.join("models/bp/n.txt"), b"n").unwrap();
        let mut got: Vec<String> = local_files(&dir, Kind::Model, "bp").into_iter().map(|e| e.rel).collect();
        got.sort();
        assert_eq!(got, ["models/bp/m.onnx", "models/bp/n.txt"]);
        assert_eq!(Kind::Graph.names(&dir, "device-1"), ["graphs/device-1.json", "graphs/device-1.owner"]);
        assert_eq!(Kind::of_ask(&json!({ "graph": "device-1" })), Some((Kind::Graph, "device-1")));
        assert_eq!(Kind::Model.key("bp"), "model:bp");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
