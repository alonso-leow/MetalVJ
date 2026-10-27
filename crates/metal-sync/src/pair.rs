//! `fw/pair/1`: how a new machine joins the club.
//!
//! # The flow
//!
//! A is in the club, B is new. B finds A (mDNS, `GET /discover`) and asks
//! (`POST /pair/request`), which dials A on this ALPN and says `hello` with
//! B's name. A refuses a name the club already uses for somebody else, and
//! otherwise generates a six-digit code — valid [`CODE_TTL`], [`ATTEMPTS`]
//! tries — that it shows on its own screen (`GET /pair/pending`) and **never
//! sends**. The operator reads it off A and types it on B
//! (`POST /pair/confirm`), which dials A again with the code. Only a matching
//! code makes A mint a `DocTicket` (`ShareMode::Write`) and send it back;
//! B imports it (the engine swaps documents, see `EngineCmd::Join`) and both
//! sides write the other's name into the `name ↔ node_id` table in
//! `sync.toml`.
//!
//! Every message is one QUIC bi-stream: a JSON request, `finish`, a JSON
//! reply. The connection itself is the authentication — `remote_id` is the
//! peer's public key — so the code binds a *person* to a *key*, and the
//! ticket only ever leaves A towards the key that typed the right code.
//!
//! # What is pure and what is not
//!
//! [`PairBook`] holds every rule (the code, its expiry, the attempts, the one
//! outgoing request, the duplicate names) with the clock passed in, so the
//! rules are tested without a socket. [`decide`] is A's whole answer short of
//! minting the ticket. The async parts — dialling, minting, saving — are thin
//! on purpose.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, EndpointId,
};
use iroh_docs::{
    api::{
        protocol::{AddrInfoOptions, ShareMode},
        Doc,
    },
    DocTicket,
};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::state::SyncState;

pub const ALPN: &[u8] = b"fw/pair/1";

/// How long a code shown on A stays good.
pub const CODE_TTL: Duration = Duration::from_secs(60);

/// How many wrong codes A takes before it forgets the request.
pub const ATTEMPTS: u8 = 3;

/// Requests and replies are a few hundred bytes; a ticket is the largest.
const MAX_MESSAGE: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    /// The name is the club's for somebody else (or is this machine's own).
    DuplicateName(String),
    /// A confirm with no request behind it — never asked, rejected, or used up.
    NoRequest,
    Expired,
    WrongCode { attempts_left: u8 },
    /// B already has a request in flight; one at a time.
    Busy,
    EmptyName,
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairError::DuplicateName(n) => write!(f, "the name {n:?} is already taken in this club"),
            PairError::NoRequest => write!(f, "no pending request from this device"),
            PairError::Expired => write!(f, "the code expired"),
            PairError::WrongCode { attempts_left } => {
                write!(f, "wrong code, {attempts_left} attempts left")
            }
            PairError::Busy => write!(f, "a request is already in flight"),
            PairError::EmptyName => write!(f, "a device needs a name"),
        }
    }
}

/// A request waiting on A for its code to be typed on B.
#[derive(Debug, Clone)]
struct Incoming {
    name: String,
    code: String,
    created: Instant,
    attempts_left: u8,
}

/// B's side of a request it sent.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub addr: EndpointAddr,
    started: Instant,
}

/// Every pairing rule, with the clock passed in.
#[derive(Debug)]
pub struct PairBook {
    name: String,
    /// name → node id. The club as this machine knows it.
    peers: BTreeMap<String, String>,
    /// node id → its request.
    incoming: HashMap<String, Incoming>,
    outgoing: Option<Outgoing>,
}

impl PairBook {
    pub fn new(name: String, peers: BTreeMap<String, String>) -> PairBook {
        PairBook { name, peers, incoming: HashMap::new(), outgoing: None }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn peers(&self) -> &BTreeMap<String, String> {
        &self.peers
    }

    /// Is this node id a member of the document — someone we paired with?
    /// The one question `fw/frame/1` asks before answering anything.
    pub fn is_member(&self, node_id: &str) -> bool {
        self.peers.values().any(|id| id == node_id)
    }

    /// The node id a machine name stands for. `None` is "no such peer",
    /// never a guess.
    pub fn node_of(&self, name: &str) -> Option<&str> {
        self.peers.get(name).map(String::as_str)
    }

    /// Put a peer in the table, dropping any older name that id had.
    pub fn admit(&mut self, name: String, node_id: String) {
        self.peers.retain(|_, id| *id != node_id);
        self.peers.insert(name, node_id);
    }

    /// `POST /leave`: the club is gone, the table with it.
    pub fn forget_all(&mut self) {
        self.peers.clear();
        self.incoming.clear();
        self.outgoing = None;
    }

    /// A's side of `hello`: record the request under `code`, or refuse the
    /// name. A repeated hello from the same id replaces its old code.
    pub fn hello(&mut self, from: &str, name: &str, code: String, now: Instant) -> Result<(), PairError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(PairError::EmptyName);
        }
        let taken = name == self.name
            || self.peers.get(name).is_some_and(|id| id != from)
            || self.incoming.iter().any(|(id, r)| id != from && r.name == name && !expired(r, now));
        if taken {
            return Err(PairError::DuplicateName(name.to_string()));
        }
        self.incoming.insert(
            from.to_string(),
            Incoming { name: name.to_string(), code, created: now, attempts_left: ATTEMPTS },
        );
        Ok(())
    }

    /// A's side of `confirm`. A match admits the peer and returns its name;
    /// a miss burns one attempt, and the last miss forgets the request.
    pub fn confirm(&mut self, from: &str, code: &str, now: Instant) -> Result<String, PairError> {
        let Some(req) = self.incoming.get_mut(from) else {
            return Err(PairError::NoRequest);
        };
        if expired(req, now) {
            self.incoming.remove(from);
            return Err(PairError::Expired);
        }
        if req.code != code.trim() {
            req.attempts_left -= 1;
            let attempts_left = req.attempts_left;
            if attempts_left == 0 {
                self.incoming.remove(from);
            }
            return Err(PairError::WrongCode { attempts_left });
        }
        let name = self.incoming.remove(from).map(|r| r.name).unwrap_or_default();
        self.admit(name.clone(), from.to_string());
        Ok(name)
    }

    /// A taps **Rechazar**.
    pub fn reject(&mut self, node_id: &str) -> bool {
        self.incoming.remove(node_id).is_some()
    }

    /// What A shows: every live request, code included. This is loopback
    /// only (`:7901` binds 127.0.0.1) — the code reaches a screen, never a
    /// wire.
    pub fn pending(&mut self, now: Instant) -> Vec<Value> {
        self.incoming.retain(|_, r| !expired(r, now));
        self.incoming
            .iter()
            .map(|(id, r)| {
                json!({
                    "node_id": id,
                    "name": r.name,
                    "code": r.code,
                    "attempts_left": r.attempts_left,
                    "expires_in_s": CODE_TTL.saturating_sub(now - r.created).as_secs(),
                })
            })
            .collect()
    }

    /// B starts a request. Only one at a time: two codes on two screens is
    /// a way to type the right code into the wrong box.
    pub fn begin_outgoing(&mut self, addr: EndpointAddr, now: Instant) -> Result<(), PairError> {
        if self.outgoing(now).is_some() {
            return Err(PairError::Busy);
        }
        self.outgoing = Some(Outgoing { addr, started: now });
        Ok(())
    }

    /// B's request in flight, if it has not outlived A's code.
    pub fn outgoing(&mut self, now: Instant) -> Option<&Outgoing> {
        if self.outgoing.as_ref().is_some_and(|o| now - o.started >= CODE_TTL) {
            self.outgoing = None;
        }
        self.outgoing.as_ref()
    }

    pub fn end_outgoing(&mut self) {
        self.outgoing = None;
    }
}

fn expired(r: &Incoming, now: Instant) -> bool {
    now - r.created >= CODE_TTL
}

/// Six digits, uniformly, leading zeros kept.
pub fn new_code() -> String {
    format!("{:06}", rand::random_range(0..1_000_000u32))
}

/// What A does with one request, short of minting the ticket.
#[derive(Debug, PartialEq)]
pub enum Decision {
    Reply(Value),
    /// The code matched: mint a ticket for this peer and send it.
    Admit { name: String },
}

/// A's whole answer to one `fw/pair/1` message. Never carries a ticket — the
/// ticket is minted by the caller, and only on [`Decision::Admit`].
pub fn decide(book: &mut PairBook, from: &str, req: &Value, code: String, now: Instant) -> Decision {
    match req["op"].as_str() {
        Some("hello") => {
            let name = req["name"].as_str().unwrap_or("");
            match book.hello(from, name, code, now) {
                Ok(()) => Decision::Reply(json!({ "waiting": true })),
                Err(e) => Decision::Reply(json!({ "refused": e.to_string() })),
            }
        }
        Some("confirm") => {
            let code = req["code"].as_str().unwrap_or("");
            match book.confirm(from, code, now) {
                Ok(name) => Decision::Admit { name },
                Err(PairError::WrongCode { attempts_left }) => {
                    Decision::Reply(json!({ "wrong": true, "attempts_left": attempts_left }))
                }
                Err(e) => Decision::Reply(json!({ "refused": e.to_string() })),
            }
        }
        _ => Decision::Reply(json!({ "refused": "unknown op" })),
    }
}

/// What this machine calls itself when `sync.toml` does not say: the
/// configured hostname, then the system's, then a slice of the node id.
pub fn default_name(node_id: &str, config_dir: Option<&std::path::Path>) -> String {
    let configured = config_dir
        .map(metal_config::default_path)
        .filter(|p| p.exists())
        .and_then(|p| metal_config::Config::load(&p).ok())
        .and_then(|c| c.node.hostname);
    configured
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| format!("fw-{}", &node_id[..node_id.len().min(8)]))
}

/// A machine seen on the LAN by mDNS.
#[derive(Debug, Clone)]
pub struct Seen {
    pub name: Option<String>,
    pub addrs: Vec<String>,
    pub last_seen: u64,
}

/// What the engine, the two protocols and the control surface share.
pub struct Club {
    pub book: Mutex<PairBook>,
    /// The document this node is in right now. The engine swaps it on
    /// join/leave; A mints tickets from it.
    pub doc: RwLock<Option<Doc>>,
    pub state: Arc<Mutex<SyncState>>,
    /// `sync.toml`. `None` (tests) means the table lives in memory only.
    pub config_path: Option<PathBuf>,
    pub data_root: PathBuf,
    /// The local `metal-dash`, which serves frames for devices on this machine.
    pub dash_port: u16,
    /// node id → what mDNS last said about it.
    pub discovered: Mutex<BTreeMap<String, Seen>>,
    /// Song bundles pulled from peers, by song stem (`pull.rs`).
    pub pulls: Mutex<BTreeMap<String, crate::pull::Pull>>,
}

impl std::fmt::Debug for Club {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Club").field("book", &self.book).finish_non_exhaustive()
    }
}

impl Club {
    /// Persist name, table and namespace to `sync.toml`.
    pub fn save(&self) -> Result<(), String> {
        let Some(path) = &self.config_path else { return Ok(()) };
        let (name, peers) = {
            let b = self.book.lock().expect("pair book poisoned");
            (b.name().to_string(), b.peers().clone())
        };
        let namespace =
            self.doc.read().expect("doc lock poisoned").as_ref().map(|d| d.id().to_string());
        // Pairing rewrites this file; a hand-set `join_hold_secs` must survive it.
        let join_hold_secs = crate::config::load(path).ok().and_then(|c| c.join_hold_secs);
        crate::config::save(
            path,
            &crate::config::SyncConfig { namespace, name: Some(name), peers, join_hold_secs },
        )
    }

    pub fn log(&self, kind: &str, detail: Value) {
        self.state.lock().expect("sync state mutex poisoned").log(kind, detail);
    }
}

/// Requests the control surface hands the engine's loop: the document
/// changes under a running engine, so only the loop may swap it.
pub enum EngineCmd {
    Join(DocTicket),
    Leave,
}

/// A's half: the `fw/pair/1` handler on the router.
#[derive(Debug, Clone)]
pub struct PairProtocol {
    pub club: Arc<Club>,
}

impl ProtocolHandler for PairProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        self.serve(conn).await.map_err(|e| AcceptError::from_boxed(e.into()))
    }
}

impl PairProtocol {
    async fn serve(&self, conn: Connection) -> Result<()> {
        let from = conn.remote_id().to_string();
        let (mut send, mut recv) = conn.accept_bi().await?;
        let req: Value = serde_json::from_slice(&recv.read_to_end(MAX_MESSAGE).await?)?;

        let decision = {
            let mut book = self.club.book.lock().expect("pair book poisoned");
            decide(&mut book, &from, &req, new_code(), Instant::now())
        };
        let reply = match decision {
            Decision::Reply(v) => {
                self.club.log("pair", json!({ "from": from, "op": req["op"], "reply": v }));
                v
            }
            Decision::Admit { name } => {
                let doc = self.club.doc.read().expect("doc lock poisoned").clone();
                let doc = doc.ok_or_else(|| anyhow!("no document to share"))?;
                let ticket = doc.share(ShareMode::Write, AddrInfoOptions::RelayAndAddresses).await?;
                if let Err(e) = self.club.save() {
                    tracing::warn!(%e, "could not save the name table");
                }
                self.club.log("paired", json!({ "name": name, "node_id": from }));
                let me = self.club.book.lock().expect("pair book poisoned").name().to_string();
                json!({ "ticket": ticket.to_string(), "name": me })
            }
        };
        send.write_all(reply.to_string().as_bytes()).await?;
        send.finish()?;
        // The dialler closes once it has read the reply.
        conn.closed().await;
        Ok(())
    }
}

/// One request/reply exchange on a fresh connection.
pub(crate) async fn exchange(endpoint: &Endpoint, to: EndpointAddr, alpn: &[u8], req: &[u8]) -> Result<Vec<u8>> {
    let conn = endpoint.connect(to, alpn).await.context("dialling the peer")?;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(req).await?;
    send.finish()?;
    let bytes = recv.read_to_end(crate::frame::MAX_FRAME).await?;
    conn.close(0u32.into(), b"done");
    Ok(bytes)
}

/// The handle the control surface holds: B's half of pairing, plus frames.
#[derive(Clone)]
pub struct Net {
    pub endpoint: Endpoint,
    pub club: Arc<Club>,
    pub cmds: mpsc::Sender<EngineCmd>,
}

/// An HTTP-shaped outcome for the control surface: status and JSON body.
pub type Answer = (u16, Value);

impl Net {
    /// `POST /pair/request`: say hello to A and wait for its code to be typed.
    pub async fn request(&self, to: EndpointAddr) -> Answer {
        let now = Instant::now();
        if let Err(e) = self.club.book.lock().expect("pair book poisoned").begin_outgoing(to.clone(), now) {
            return (409, json!({ "error": e.to_string() }));
        }
        let name = self.club.book.lock().expect("pair book poisoned").name().to_string();
        let msg = json!({ "op": "hello", "name": name }).to_string();
        let reply = match exchange(&self.endpoint, to.clone(), ALPN, msg.as_bytes()).await {
            Ok(b) => serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null),
            Err(e) => {
                self.club.book.lock().expect("pair book poisoned").end_outgoing();
                return (502, json!({ "error": format!("{e:#}") }));
            }
        };
        if reply["waiting"] == true {
            self.club.log("pair_request", json!({ "to": to.id.to_string() }));
            (200, json!({ "state": "awaiting_code", "node_id": to.id.to_string() }))
        } else {
            self.club.book.lock().expect("pair book poisoned").end_outgoing();
            (409, json!({ "error": reply["refused"].as_str().unwrap_or("refused") }))
        }
    }

    /// `POST /pair/confirm`: send the typed code; on a match, join.
    pub async fn confirm(&self, code: &str) -> Answer {
        let Some(out) = self.club.book.lock().expect("pair book poisoned").outgoing(Instant::now()).cloned()
        else {
            return (409, json!({ "error": "no request in flight" }));
        };
        let msg = json!({ "op": "confirm", "code": code }).to_string();
        let reply = match exchange(&self.endpoint, out.addr.clone(), ALPN, msg.as_bytes()).await {
            Ok(b) => serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null),
            Err(e) => return (502, json!({ "error": format!("{e:#}") })),
        };
        if let Some(t) = reply["ticket"].as_str() {
            let ticket: DocTicket = match t.parse() {
                Ok(t) => t,
                Err(e) => return (502, json!({ "error": format!("the ticket did not parse: {e}") })),
            };
            let peer_name = reply["name"].as_str().unwrap_or("").to_string();
            {
                let mut book = self.club.book.lock().expect("pair book poisoned");
                book.end_outgoing();
                book.admit(peer_name.clone(), out.addr.id.to_string());
            }
            self.club.log("paired", json!({ "name": peer_name, "node_id": out.addr.id.to_string() }));
            if self.cmds.send(EngineCmd::Join(ticket)).await.is_err() {
                return (503, json!({ "error": "the engine is not running" }));
            }
            (200, json!({ "joined": peer_name }))
        } else if reply["wrong"] == true {
            let left = reply["attempts_left"].as_u64().unwrap_or(0);
            if left == 0 {
                self.club.book.lock().expect("pair book poisoned").end_outgoing();
            }
            (403, json!({ "error": "wrong code", "attempts_left": left }))
        } else {
            self.club.book.lock().expect("pair book poisoned").end_outgoing();
            (409, json!({ "error": reply["refused"].as_str().unwrap_or("refused") }))
        }
    }

    /// `POST /leave`: a fresh document of our own; `data/` stays as it is.
    pub async fn leave(&self) -> Answer {
        match self.cmds.send(EngineCmd::Leave).await {
            Ok(()) => (200, json!({ "left": true })),
            Err(_) => (503, json!({ "error": "the engine is not running" })),
        }
    }
}

/// An address for a node id: whatever mDNS last saw, plus the id itself so
/// the endpoint's other lookups (DNS, relay) can fill in the rest.
pub fn addr_for(club: &Club, id: EndpointId) -> EndpointAddr {
    let mut addr = EndpointAddr::from(id);
    if let Some(seen) = club.discovered.lock().expect("discovered poisoned").get(&id.to_string()) {
        for a in &seen.addrs {
            if let Ok(sock) = a.parse::<std::net::SocketAddr>() {
                addr = addr.with_ip_addr(sock);
            }
        }
    }
    addr
}

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> PairBook {
        PairBook::new("bar".into(), BTreeMap::new())
    }

    #[test]
    fn a_code_is_six_digits() {
        for _ in 0..50 {
            let c = new_code();
            assert_eq!(c.len(), 6);
            assert!(c.chars().all(|d| d.is_ascii_digit()));
        }
    }

    #[test]
    fn the_ticket_never_leaves_before_the_code_matches() {
        let mut b = book();
        let t = Instant::now();
        let hello = decide(&mut b, "B", &json!({"op":"hello","name":"stage"}), "123456".into(), t);
        assert_eq!(hello, Decision::Reply(json!({ "waiting": true })));
        let wrong = decide(&mut b, "B", &json!({"op":"confirm","code":"000000"}), "x".into(), t);
        assert!(matches!(wrong, Decision::Reply(_)), "a wrong code gets no ticket");
        // A stranger who never said hello gets nothing either.
        let stranger = decide(&mut b, "C", &json!({"op":"confirm","code":"123456"}), "x".into(), t);
        assert!(matches!(stranger, Decision::Reply(_)));
        assert!(!b.is_member("B"));
        let right = decide(&mut b, "B", &json!({"op":"confirm","code":"123456"}), "x".into(), t);
        assert_eq!(right, Decision::Admit { name: "stage".into() });
        assert!(b.is_member("B"));
    }

    #[test]
    fn a_wrong_code_burns_one_attempt_and_the_third_forgets_the_request() {
        let mut b = book();
        let t = Instant::now();
        b.hello("B", "stage", "123456".into(), t).unwrap();
        assert_eq!(b.confirm("B", "1", t), Err(PairError::WrongCode { attempts_left: 2 }));
        assert_eq!(b.confirm("B", "2", t), Err(PairError::WrongCode { attempts_left: 1 }));
        assert_eq!(b.confirm("B", "3", t), Err(PairError::WrongCode { attempts_left: 0 }));
        // Even the right code is too late now.
        assert_eq!(b.confirm("B", "123456", t), Err(PairError::NoRequest));
    }

    #[test]
    fn a_pending_request_expires_at_sixty_seconds() {
        let mut b = book();
        let t = Instant::now();
        b.hello("B", "stage", "123456".into(), t).unwrap();
        assert_eq!(b.pending(t + Duration::from_secs(59)).len(), 1);
        assert_eq!(b.confirm("B", "123456", t + CODE_TTL), Err(PairError::Expired));
        assert!(b.pending(t + CODE_TTL).is_empty());
    }

    #[test]
    fn only_one_outgoing_request_at_a_time() {
        let mut b = book();
        let t = Instant::now();
        let a = EndpointAddr::from(iroh::SecretKey::generate().public());
        let c = EndpointAddr::from(iroh::SecretKey::generate().public());
        b.begin_outgoing(a, t).unwrap();
        assert_eq!(b.begin_outgoing(c.clone(), t), Err(PairError::Busy));
        // Once A's code could no longer be valid, B may ask someone else.
        assert!(b.begin_outgoing(c, t + CODE_TTL).is_ok());
    }

    #[test]
    fn a_duplicate_name_is_refused_at_hello() {
        let mut b = book();
        let t = Instant::now();
        b.admit("stage".into(), "S".into());
        assert_eq!(
            b.hello("B", "stage", "1".into(), t),
            Err(PairError::DuplicateName("stage".into()))
        );
        assert_eq!(b.hello("B", "bar", "1".into(), t), Err(PairError::DuplicateName("bar".into())));
        // Two newcomers asking for the same name at once: the second waits.
        b.hello("B", "booth", "1".into(), t).unwrap();
        assert!(b.hello("C", "booth", "2".into(), t).is_err());
        // The same machine re-pairing under its own name is not a duplicate.
        assert!(b.hello("S", "stage", "3".into(), t).is_ok());
    }

    #[test]
    fn a_rejected_request_cannot_be_confirmed() {
        let mut b = book();
        let t = Instant::now();
        b.hello("B", "stage", "123456".into(), t).unwrap();
        assert!(b.reject("B"));
        assert_eq!(b.confirm("B", "123456", t), Err(PairError::NoRequest));
    }

    #[test]
    fn readmitting_an_id_drops_its_old_name() {
        let mut b = book();
        b.admit("stage".into(), "S".into());
        b.admit("stage-left".into(), "S".into());
        assert_eq!(b.node_of("stage"), None);
        assert_eq!(b.node_of("stage-left"), Some("S"));
    }
}
