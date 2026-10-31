//! The render farm over `fw/farm/1`: the conductor that answers workers, and
//! the worker that claims, renders, beats and delivers. The state machine they
//! share is `farm.rs`.
//!
//! # Roles
//!
//! `[farm].conductor` in the peer config names the paired peer that conducts.
//! Unset, or this node's own name, this node is the conductor: it takes
//! submissions (`POST /farm/submit` on the control surface), splits them,
//! answers claims, expires silent workers and, when every shard of a job is in
//! `reels/<reel>/verified/`, joins them. Set to another peer, this node is a
//! worker of that peer.
//!
//! # The wire
//!
//! One bi-stream per request. The request is one JSON line (`\n`-ended); a
//! `put` is followed by the shard file's bytes until the stream ends. The
//! reply is JSON.
//!
//! - `{"op":"claim"}` -> `{"shard": Assignment}` or `{"none":true}`
//! - `{"op":"beat","job","index"}` -> `{"ok":bool}` (`false`: stop rendering)
//! - `{"op":"fail","job","index"}` -> `{"ok":bool}`
//! - `{"op":"put","job","index","size","blake3"}` + bytes -> `{"ok":bool}`
//!
//! A worker is identified by its node id (the connection's), never by a field
//! it sends, and only paired members are answered at all.
//!
//! A worker renders by posting its shard to the local `metal-dash`'s
//! `/export/video` (range and path replaced) and polling `/export`: the farm
//! adds no renderer, it drives the one the export queue already runs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::Endpoint;
use serde_json::{json, Value};

use crate::farm::{self, Assignment, Farm, FarmError};
use crate::pair::{addr_for, exchange, Club};

pub const ALPN: &[u8] = b"fw/farm/1";

/// The most a request line may be, and the largest shard file taken.
const MAX_LINE: usize = 64 * 1024;
const MAX_SHARD_BYTES: u64 = 16 << 30;

/// What `[farm]` says, with defaults filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct FarmCfg {
    pub conductor: Option<String>,
    pub shard_frames: u64,
    pub missed_beats: u32,
}

impl Default for FarmCfg {
    fn default() -> FarmCfg {
        FarmCfg { conductor: None, shard_frames: farm::DEFAULT_SHARD_FRAMES, missed_beats: farm::DEFAULT_MISSED_BEATS }
    }
}

impl From<&metal_config::Farm> for FarmCfg {
    fn from(f: &metal_config::Farm) -> FarmCfg {
        FarmCfg {
            conductor: f.conductor.clone().filter(|c| !c.trim().is_empty()),
            shard_frames: f.shard_frames.unwrap_or(farm::DEFAULT_SHARD_FRAMES).max(1),
            missed_beats: f.missed_beats.unwrap_or(farm::DEFAULT_MISSED_BEATS).max(1),
        }
    }
}

/// The farm's slot in [`Club`].
pub struct FarmHandle {
    pub cfg: FarmCfg,
    pub queue: Mutex<Farm>,
    epoch: Instant,
    /// Jobs whose join failed, so the 1 s tick does not retry them forever.
    join_failed: Mutex<Vec<String>>,
}

impl FarmHandle {
    /// Loads `data_root/render/queue.json` (claims requeued).
    pub fn load(cfg: FarmCfg, data_root: &Path) -> FarmHandle {
        FarmHandle {
            queue: Mutex::new(farm::load(data_root, cfg.missed_beats)),
            cfg,
            epoch: Instant::now(),
            join_failed: Mutex::new(Vec::new()),
        }
    }

    pub fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }
}

impl Club {
    /// This node splits and conducts (`[farm].conductor` unset or its own name).
    pub fn is_conductor(&self) -> bool {
        let own = self.book.lock().expect("pair book poisoned").name().to_string();
        self.farm.cfg.conductor.as_deref().is_none_or(|c| c == own)
    }

    fn farm_persist(&self, queue: &Farm) {
        if let Err(e) = farm::save(&self.data_root, queue) {
            tracing::warn!(error = %e, "could not write render/queue.json");
        }
    }

    /// Splits the export `body` (a `/export/video` request) into shards and
    /// queues it. `range` is `[start, end)` in frames. Returns the job id.
    pub fn farm_submit(&self, body: &Value) -> Result<String, String> {
        if !self.is_conductor() {
            return Err("this node is not the conductor".into());
        }
        let range = body["range"].as_array().ok_or("expected range [start,end]")?;
        let (from, to) = match (range.first().and_then(Value::as_u64), range.get(1).and_then(Value::as_u64)) {
            (Some(a), Some(b)) if b > a => (a, b),
            _ => return Err("range must be [start,end] with end > start".into()),
        };
        let reel = body["reel"]
            .as_str()
            .map(str::to_string)
            .or_else(|| body["device"].as_str().map(|d| d.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()))
            .ok_or("expected reel or device")?;
        let id = format!("f{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()));
        let mut q = self.farm.queue.lock().expect("farm lock");
        q.submit(&id, &reel, body.clone(), from, to, self.farm.cfg.shard_frames).map_err(|e| e.to_string())?;
        self.farm_persist(&q);
        Ok(id)
    }

    /// `GET /farm`: every job with its shard counts.
    pub fn farm_report(&self) -> Value {
        let q = self.farm.queue.lock().expect("farm lock");
        let jobs: Vec<Value> = q
            .jobs
            .iter()
            .map(|j| {
                let (queued, claimed, done) = q.progress(&j.id).unwrap_or_default();
                json!({ "id": j.id, "reel": j.reel, "shards": j.shards.len(), "queued": queued, "claimed": claimed, "done": done })
            })
            .collect();
        json!({ "conductor": self.is_conductor(), "jobs": jobs })
    }
}

fn reply(v: Value) -> Vec<u8> {
    v.to_string().into_bytes()
}

/// Reads the request line off `recv`; returns it and any bytes read past it.
async fn read_line(recv: &mut iroh::endpoint::RecvStream) -> Result<(Value, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        if let Some(at) = buf.iter().position(|b| *b == b'\n') {
            let line: Value = serde_json::from_slice(&buf[..at]).context("the request line is not JSON")?;
            return Ok((line, buf[at + 1..].to_vec()));
        }
        anyhow::ensure!(buf.len() <= MAX_LINE, "the request line is too long");
        match recv.read(&mut chunk).await? {
            Some(n) => buf.extend_from_slice(&chunk[..n]),
            None => anyhow::bail!("the stream ended before the request line"),
        }
    }
}

/// The peer side: `fw/farm/1` on the router.
#[derive(Debug, Clone)]
pub struct FarmProtocol {
    pub club: Arc<Club>,
}

impl ProtocolHandler for FarmProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let from = conn.remote_id().to_string();
        if !self.club.book.lock().expect("pair book poisoned").is_member(&from) {
            conn.close(1u32.into(), b"");
            return Ok(());
        }
        let (mut send, mut recv) = conn.accept_bi().await.map_err(|e| AcceptError::from_boxed(e.into()))?;
        let answer = match read_line(&mut recv).await {
            Ok((req, rest)) => self.serve(&from, &req, rest, &mut recv).await.unwrap_or_else(|e| json!({ "ok": false, "error": format!("{e:#}") })),
            Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
        };
        send.write_all(&reply(answer)).await.map_err(|e| AcceptError::from_boxed(e.into()))?;
        send.finish().map_err(|e| AcceptError::from_boxed(e.into()))?;
        conn.closed().await;
        Ok(())
    }
}

impl FarmProtocol {
    async fn serve(&self, worker: &str, req: &Value, rest: Vec<u8>, recv: &mut iroh::endpoint::RecvStream) -> Result<Value> {
        let club = &self.club;
        if !club.is_conductor() {
            return Ok(json!({ "ok": false, "error": "this node is not the conductor" }));
        }
        let job = req["job"].as_str().unwrap_or_default();
        let index = req["index"].as_u64().unwrap_or(u64::MAX) as usize;
        let now = club.farm.now_ms();
        match req["op"].as_str().unwrap_or_default() {
            "claim" => {
                let mut q = club.farm.queue.lock().expect("farm lock");
                Ok(match q.claim(worker, now) {
                    Some(a) => {
                        club.farm_persist(&q);
                        json!({ "shard": a })
                    }
                    None => json!({ "none": true }),
                })
            }
            "beat" => {
                let ok = club.farm.queue.lock().expect("farm lock").heartbeat(worker, job, index, now).is_ok();
                Ok(json!({ "ok": ok }))
            }
            "fail" => {
                let mut q = club.farm.queue.lock().expect("farm lock");
                let ok = q.fail(worker, job, index).is_ok();
                club.farm_persist(&q);
                Ok(json!({ "ok": ok }))
            }
            "put" => self.take_shard(worker, req, rest, recv).await,
            other => Ok(json!({ "ok": false, "error": format!("no such op {other:?}") })),
        }
    }

    /// Streams a worker's shard into `reels/<reel>/verified/`, checks it, and
    /// marks the shard done. Refused up front when the shard is not the
    /// worker's any more.
    async fn take_shard(&self, worker: &str, req: &Value, mut rest: Vec<u8>, recv: &mut iroh::endpoint::RecvStream) -> Result<Value> {
        let club = &self.club;
        let job = req["job"].as_str().unwrap_or_default().to_string();
        let index = req["index"].as_u64().unwrap_or(u64::MAX) as usize;
        let size = req["size"].as_u64().unwrap_or(u64::MAX);
        let want = req["blake3"].as_str().unwrap_or_default().to_string();
        let reel = {
            let q = club.farm.queue.lock().expect("farm lock");
            if !q.may_deliver(worker, &job, index) {
                return Ok(json!({ "ok": false, "error": "that shard is not yours" }));
            }
            q.jobs.iter().find(|j| j.id == job).map(|j| j.reel.clone()).unwrap_or_default()
        };
        anyhow::ensure!(size <= MAX_SHARD_BYTES, "a shard that large is refused");
        let rel = farm::verified_rel(&reel, &job, index);
        let dest = club.data_root.join(&rel);
        let part = PathBuf::from(format!("{}.part", dest.display()));
        if let Some(dir) = dest.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        use tokio::io::AsyncWriteExt;
        let mut out = tokio::fs::File::create(&part).await?;
        let mut hasher = blake3::Hasher::new();
        let mut got = 0u64;
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            if !rest.is_empty() {
                got += rest.len() as u64;
                anyhow::ensure!(got <= size, "more bytes than the declared size");
                hasher.update(&rest);
                out.write_all(&rest).await?;
                rest.clear();
            }
            match recv.read(&mut chunk).await? {
                Some(n) => rest.extend_from_slice(&chunk[..n]),
                None => break,
            }
        }
        out.flush().await?;
        drop(out);
        if got != size || hasher.finalize().to_hex().to_string() != want {
            let _ = tokio::fs::remove_file(&part).await;
            return Ok(json!({ "ok": false, "error": "the shard does not match its size or hash" }));
        }
        tokio::fs::rename(&part, &dest).await?;
        let mut q = club.farm.queue.lock().expect("farm lock");
        match q.deliver(worker, &job, index, &rel) {
            Ok(()) => {
                club.farm_persist(&q);
                Ok(json!({ "ok": true }))
            }
            Err(FarmError::NotYours) => {
                let _ = std::fs::remove_file(&dest);
                Ok(json!({ "ok": false, "error": "that shard is not yours" }))
            }
            Err(e) => Ok(json!({ "ok": false, "error": e.to_string() })),
        }
    }
}

// -- the conductor's tick ------------------------------------------------------------

/// Once a second: silent workers' shards go back in the queue, and a job whose
/// shards are all in is joined and leaves the queue.
pub async fn conduct_once(club: &Arc<Club>) {
    if !club.is_conductor() {
        return;
    }
    let now = club.farm.now_ms();
    let finished = {
        let mut q = club.farm.queue.lock().expect("farm lock");
        let back = q.expire(now);
        if !back.is_empty() {
            for (job, index, worker) in &back {
                tracing::warn!(%job, index, %worker, "worker missed its heartbeats; the shard is back in the queue");
            }
            club.farm_persist(&q);
        }
        let failed = club.farm.join_failed.lock().expect("join lock").clone();
        q.finished().into_iter().filter(|j| !failed.contains(j)).collect::<Vec<_>>()
    };
    for id in finished {
        let (files, out) = {
            let q = club.farm.queue.lock().expect("farm lock");
            let Some(rels) = q.join_order(&id) else { continue };
            let job = q.jobs.iter().find(|j| j.id == id).expect("a finished job exists");
            let out = match job.body["path"].as_str().filter(|p| !p.is_empty()) {
                Some(p) => PathBuf::from(p),
                None => club.data_root.join("reels").join(&job.reel).join(format!("{id}.mp4")),
            };
            (rels.into_iter().map(|r| club.data_root.join(r)).collect::<Vec<_>>(), out)
        };
        let joined = tokio::task::spawn_blocking(move || metal_ingest::export::join_shards(&files, &out)).await;
        let result = match joined {
            Ok(r) => r,
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok(()) => {
                let mut q = club.farm.queue.lock().expect("farm lock");
                q.remove(&id);
                club.farm_persist(&q);
                tracing::info!(job = %id, "farm job joined");
            }
            Err(why) => {
                tracing::warn!(job = %id, error = %why, "joining the shards failed");
                club.farm.join_failed.lock().expect("join lock").push(id);
            }
        }
    }
}

pub fn spawn_conductor(club: Arc<Club>) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            conduct_once(&club).await;
        }
    })
    .abort_handle()
}

// -- the worker ----------------------------------------------------------------------

/// Where a worker keeps a shard while it renders.
fn shard_path(data_root: &Path, a: &Assignment) -> PathBuf {
    data_root.join("render").join("shards").join(format!("{}-{}.mp4", a.job, a.index))
}

/// The export request a shard runs: the job's, with the shard's range and file.
pub fn shard_body(a: &Assignment, path: &Path) -> Value {
    let mut body = a.body.clone();
    body["range"] = json!([a.from, a.to]);
    body["path"] = json!(path.display().to_string());
    if let Some(obj) = body.as_object_mut() {
        obj.remove("farm");
        obj.remove("op");
    }
    body
}

async fn ask(endpoint: &Endpoint, club: &Club, conductor: &str, req: Value) -> Result<Value> {
    let id = club
        .book
        .lock()
        .expect("pair book poisoned")
        .peers()
        .get(conductor)
        .cloned()
        .with_context(|| format!("no paired peer called {conductor:?}"))?;
    let addr = addr_for(club, id.parse().context("the conductor's node id")?);
    let bytes = exchange(endpoint, addr, ALPN, format!("{req}\n").as_bytes()).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Sends a shard file: the request line, then the bytes, then waits for the verdict.
async fn put(endpoint: &Endpoint, club: &Club, conductor: &str, a: &Assignment, file: &Path) -> Result<bool> {
    let id = club.book.lock().expect("pair book poisoned").peers().get(conductor).cloned().context("the conductor is not paired")?;
    let addr = addr_for(club, id.parse().context("the conductor's node id")?);
    let (size, hash) = {
        let f = file.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<(u64, String)> {
            Ok((std::fs::metadata(&f)?.len(), crate::pull::hash_file(&f)?))
        })
        .await??
    };
    let conn = endpoint.connect(addr, ALPN).await.context("dialling the conductor")?;
    let (mut send, mut recv) = conn.open_bi().await?;
    let head = json!({ "op": "put", "job": a.job, "index": a.index, "size": size, "blake3": hash });
    send.write_all(format!("{head}\n").as_bytes()).await?;
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(file).await?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        send.write_all(&buf[..n]).await?;
    }
    send.finish()?;
    let verdict = recv.read_to_end(crate::frame::MAX_FRAME).await?;
    conn.close(0u32.into(), b"done");
    let v: Value = serde_json::from_slice(&verdict)?;
    Ok(v["ok"] == json!(true))
}

/// One claim-render-deliver round. Returns whether a shard was worked (so the
/// caller asks again at once instead of waiting).
pub async fn work_once(endpoint: &Endpoint, club: &Arc<Club>) -> bool {
    let Some(conductor) = club.farm.cfg.conductor.clone().filter(|_| !club.is_conductor()) else { return false };
    let claimed = match ask(endpoint, club, &conductor, json!({ "op": "claim" })).await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %format!("{e:#}"), "farm claim failed");
            return false;
        }
    };
    let Some(a) = claimed.get("shard").and_then(|s| serde_json::from_value::<Assignment>(s.clone()).ok()) else { return false };
    let file = shard_path(&club.data_root, &a);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::remove_file(&file);
    let dash = club.dash_port;
    let body = shard_body(&a, &file);
    let started = tokio::task::spawn_blocking(move || crate::frame::loopback_post(dash, "/export/video", &body)).await;
    let ok_start = matches!(&started, Ok(Ok(f)) if f.status == 200);
    let finish = |ok: bool| ask(endpoint, club, &conductor, json!({ "op": if ok { "beat" } else { "fail" }, "job": a.job, "index": a.index }));
    if !ok_start {
        let _ = finish(false).await;
        return true;
    }
    // Render: beat every BEAT_MS; stop when told the shard is not ours.
    let mut tick = tokio::time::interval(Duration::from_millis(farm::BEAT_MS));
    let rendered = loop {
        tick.tick().await;
        let alive = match finish(true).await {
            Ok(v) => v["ok"] == json!(true),
            Err(_) => true, // the conductor is unreachable: keep rendering, the next beat decides
        };
        if !alive {
            let _ = tokio::task::spawn_blocking(move || crate::frame::loopback_post(dash, "/export", &json!({ "op": "cancel" }))).await;
            let _ = std::fs::remove_file(&file);
            return true;
        }
        let status = tokio::task::spawn_blocking(move || crate::frame::loopback_get(dash, "/export")).await;
        let state = match status {
            Ok(Ok(f)) => serde_json::from_slice::<Value>(&f.body).ok().and_then(|v| v["state"].as_str().map(str::to_string)),
            _ => None,
        };
        match state.as_deref() {
            Some("done") => break true,
            Some("error") | Some("cancelled") => break false,
            _ => {}
        }
    };
    if !rendered {
        let _ = finish(false).await;
        return true;
    }
    match put(endpoint, club, &conductor, &a, &file).await {
        Ok(true) => {
            let _ = std::fs::remove_file(&file);
        }
        Ok(false) => tracing::warn!(job = %a.job, index = a.index, "the conductor refused the shard"),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "delivering the shard failed");
            let _ = finish(false).await;
        }
    }
    true
}

pub fn spawn_worker(endpoint: Endpoint, club: Arc<Club>) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        loop {
            if !work_once(&endpoint, &club).await {
                tokio::time::sleep(Duration::from_millis(farm::BEAT_MS)).await;
            }
        }
    })
    .abort_handle()
}
