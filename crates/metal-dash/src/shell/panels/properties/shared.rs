//! Small shared helpers for the properties browser: the door `GET`/`POST`
//! wrapper every section already used, and a few value readers. Kept in one
//! place so the per-section files never each grow their own copy.

use std::collections::HashMap;

use serde_json::Value;

use crate::call::{Call, Params};

/// A `GET` with a query string, mirroring `panels::nodes::get`'s own contract:
/// `Some(body)` only for a `200`, `None` for anything else. Every section that
/// reads the graph, the catalog or the device state goes through here, so a
/// route the door refuses comes back `None` rather than a panic.
pub(crate) fn get(door: &dyn Call, method: &str, path: &str, query: &[(&str, String)]) -> Option<Value> {
    // `GET /catalog` is the big static one: the panel reads it several times a
    // frame (header label, surfaces folder, the effects picker), and each read
    // serialises the whole catalogue under the shared lock (~26 ms here). The
    // catalogue only changes when a model is added, so a short memo removes the
    // per-frame cost without a stale picker: a re-read lands within a second.
    if method == "GET" && path == "/catalog" {
        let stamp = catalog_stamp(query);
        if let Some(v) = catalog_memo(&stamp) {
            return Some(v);
        }
        let v = get_uncached(door, method, path, query)?;
        store_catalog_memo(stamp, &v);
        return Some(v);
    }
    get_uncached(door, method, path, query)
}

/// How long a `GET /catalog` reply is reused before re-reading.
const CATALOG_TTL: f64 = 1.0;

thread_local! {
    static CATALOG: std::cell::RefCell<Option<(String, f64, Value)>> =
        const { std::cell::RefCell::new(None) };
}

/// A stable key for a `/catalog` read: the path plus its sorted query.
fn catalog_stamp(query: &[(&str, String)]) -> String {
    let mut pairs: Vec<(String, String)> =
        query.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
    pairs.sort();
    let mut out = String::from("/catalog");
    for (k, v) in pairs {
        out.push('\u{1}');
        out.push_str(&k);
        out.push('=');
        out.push_str(&v);
    }
    out
}

fn catalog_memo(stamp: &str) -> Option<Value> {
    CATALOG.with(|c| {
        let c = c.borrow();
        let (s, at, v) = c.as_ref()?;
        (s == stamp && clock() - at < CATALOG_TTL).then(|| v.clone())
    })
}

fn store_catalog_memo(stamp: String, value: &Value) {
    CATALOG.with(|c| *c.borrow_mut() = Some((stamp, clock(), value.clone())));
}

/// The panel's own clock (egui's `input.time` basis is not available in this
/// free helper, so a monotonic process instant is close enough for a 1 s TTL).
fn clock() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<web_time::Instant> = OnceLock::new();
    START.get_or_init(web_time::Instant::now).elapsed().as_secs_f64()
}

fn get_uncached(door: &dyn Call, method: &str, path: &str, query: &[(&str, String)]) -> Option<Value> {
    let mut q = HashMap::new();
    for (k, v) in query {
        q.insert((*k).to_string(), v.clone());
    }
    let params = Params {
        query: q,
        body: Vec::new(),
        tail: None,
    };
    match door.call(method, path, params) {
        Some(reply) if reply.status == 200 => {
            let mut v = crate::json_memo::parse(&reply.body).ok()?;
            if path == "/graph" {
                overlay_pending(&mut v);
            }
            Some(v)
        }
        _ => None,
    }
}

/// How long an edit is shown over the graph the read cache still serves
/// (the write lands behind the frame, and a stale read can come back first).
const PENDING_FOR: std::time::Duration = std::time::Duration::from_millis(1500);

thread_local! {
    /// Config edits posted but not yet seen in a `GET /graph`: (graph, node,
    /// the fields written, when). Painted over every graph read until the read
    /// agrees or [`PENDING_FOR`] passes, so a picker never snaps back and
    /// every panel reading the graph sees the edit at once.
    static PENDING: std::cell::RefCell<Vec<(String, String, serde_json::Map<String, Value>, web_time::Instant)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The one way a panel writes a node's `config`: queued behind the frame,
/// shown at once over the cached graph, and announced to the canvas (the
/// generation bump) — the cascade every properties edit shares.
pub(crate) fn post_node_config(door: &dyn Call, gid: &str, node_id: &str, config: Value) {
    if let Some(fields) = config.as_object() {
        PENDING.with(|p| {
            let mut p = p.borrow_mut();
            p.push((gid.to_string(), node_id.to_string(), fields.clone(), web_time::Instant::now()));
        });
    }
    WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    door.post_async(
        "/graph/node",
        serde_json::json!({ "graph": gid, "node": node_id, "op": "config", "config": config }),
    );
}

/// Paints the pending edits over a `GET /graph` reply (in place). An edit the
/// reply already agrees with, or that is older than [`PENDING_FOR`], is dropped.
pub(crate) fn overlay_pending(graph: &mut Value) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if p.is_empty() {
            return;
        }
        let gid = graph["graph"]["id"].as_str().map(str::to_string);
        let Some(nodes) = graph["graph"]["nodes"].as_array_mut() else { return };
        p.retain(|(g, id, fields, at)| {
            if at.elapsed() > PENDING_FOR {
                return false;
            }
            if gid.as_deref().is_some_and(|x| x != g) {
                return true;
            }
            let Some(node) = nodes.iter_mut().find(|n| n["id"].as_str() == Some(id)) else {
                return true;
            };
            let agrees = fields.iter().all(|(k, v)| node["config"].get(k) == Some(v));
            if agrees {
                return false;
            }
            if !node["config"].is_object() {
                node["config"] = Value::Object(Default::default());
            }
            for (k, v) in fields {
                node["config"][k] = v.clone();
            }
            true
        });
    });
}

/// Keeps the frames coming while an edit is still pending, so the write's
/// read-back (which carries no repaint of its own) is picked up and painted.
pub(crate) fn repaint_while_pending(ctx: &egui::Context) {
    if PENDING.with(|p| !p.borrow().is_empty()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// A direct `POST /graph/node {op:"config"}` is shown at once over the cached
/// graph exactly like [`post_node_config`], so no caller's edit snaps back.
fn note_pending_config(path: &str, body: &Value) {
    if path != "/graph/node" || body["op"] != "config" {
        return;
    }
    let (Some(g), Some(n), Some(fields)) =
        (body["graph"].as_str(), body["node"].as_str(), body["config"].as_object())
    else {
        return;
    };
    PENDING.with(|p| {
        p.borrow_mut().push((g.to_string(), n.to_string(), fields.clone(), web_time::Instant::now()))
    });
}

/// A `POST` of one JSON body, discarding the reply — the shape most section
/// writes take, where the next frame's fresh read is the feedback.
pub(crate) fn post(door: &dyn Call, path: &str, body: Value) {
    note_pending_config(path, &body);
    let _ = door.json("POST", path, Some(body));
    WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// [`post`] that hands back the door's answer, for the writes whose reply
/// names what was made (`insert`, `add`). Bumps the same generation.
pub(crate) fn post_reply(door: &dyn Call, path: &str, body: Value) -> Result<Value, (u16, String)> {
    note_pending_config(path, &body);
    WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    door.json("POST", path, Some(body))
}

/// Bumped by every [`post`]: a cached read older than the last write is
/// stale (the browser's `GET /graph` cache keys on it).
static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many writes this panel has posted — the cache generation.
pub(crate) fn writes() -> u64 {
    WRITES.load(std::sync::atomic::Ordering::Relaxed)
}
