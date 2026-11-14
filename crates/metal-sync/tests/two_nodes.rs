//! Two `metal-sync` engines in one process, on localhost only: pairing end to
//! end (`fw/pair/1`), the document converging after the join, a frame
//! crossing machines byte for byte (`fw/frame/1`), and the two refusals —
//! a stranger gets no answer, an unknown name is a 404.
//!
//! No relay and no DNS (`EngineOpts::local_only`): each engine learns the
//! other's loopback address through a `MemoryLookup`, the way mDNS would
//! hand it over on a LAN.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metal_sync::engine::{EngineOpts, Join, SyncEngine};
use metal_sync::frame::FrameAsk;
use metal_sync::pair::Net;
use iroh_docs::{
    api::protocol::{AddrInfoOptions, ShareMode},
    store::Query,
    DocTicket,
};
use metal_sync::SyncState;
use iroh::address_lookup::memory::MemoryLookup;
use iroh::{endpoint::presets, Endpoint, EndpointAddr};
use serde_json::{json, Value};

/// A stand-in JPEG: the test only cares that the bytes arrive unchanged.
const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 1, 2, 3, 4, 5, 0, 255, 7, 0xff, 0xd9];

struct Node {
    dir: tempfile::TempDir,
    net: Net,
    addr: EndpointAddr,
    lookup: MemoryLookup,
    state: Arc<Mutex<SyncState>>,
    /// Kept so a restart can come back as the same node.
    secret: iroh::SecretKey,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Node {
    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn namespace(&self) -> iroh_docs::NamespaceId {
        self.net.club.doc.read().unwrap().as_ref().unwrap().id()
    }
}

/// A fake `metal-dash` on an ephemeral port: `/frame?device=wall` is [`JPEG`],
/// anything else a 404.
fn fake_dash() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            reader.read_line(&mut line).unwrap();
            // Drain the headers: closing on unread bytes resets the socket.
            let mut h = String::new();
            while reader.read_line(&mut h).is_ok_and(|n| n > 2) {
                h.clear();
            }
            let (status, body) =
                if line.starts_with("GET /frame?device=wall ") { ("200 OK", JPEG) } else { ("404 Not Found", &b"{}"[..]) };
            let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(body);
        }
    });
    port
}

fn loopback(endpoint: &Endpoint) -> EndpointAddr {
    let mut addr = EndpointAddr::from(endpoint.id());
    for s in endpoint.bound_sockets() {
        if s.is_ipv4() {
            addr = addr.with_ip_addr(SocketAddr::from(([127, 0, 0, 1], s.port())));
        }
    }
    addr
}

async fn node(name: &str, dash_port: u16, outputs: &str) -> Node {
    node_with(name, dash_port, outputs, &[], None, |_| {}).await
}

/// [`node`] with files already on disk before the engine starts, joining
/// `ticket` when one is given, and the options tweaked before it starts.
async fn node_with(
    name: &str,
    dash_port: u16,
    outputs: &str,
    files: &[(&str, &[u8])],
    ticket: Option<DocTicket>,
    tweak: impl FnOnce(&mut EngineOpts),
) -> Node {
    let dir = tempfile::TempDir::new().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("outputs.json"), outputs).unwrap();
    for (rel, bytes) in files {
        std::fs::write(data.join(rel), bytes).unwrap();
    }
    start(name, dir, iroh::SecretKey::generate(), dash_port, ticket.map_or(Join::Create, Join::Ticket), tweak).await
}

/// Start an engine on `dir`, which already holds `data/`.
async fn start(
    name: &str,
    dir: tempfile::TempDir,
    secret: iroh::SecretKey,
    dash_port: u16,
    join: Join,
    tweak: impl FnOnce(&mut EngineOpts),
) -> Node {
    let data = dir.path().join("data");
    let state = Arc::new(Mutex::new(SyncState::new(name.into(), data.clone())));
    let mut opts = EngineOpts {
        name: Some(name.into()),
        config_path: Some(dir.path().join("cfg/sync.toml")),
        dash_port,
        local_only: true,
        ..EngineOpts::default()
    };
    tweak(&mut opts);
    let (engine, net) = SyncEngine::spawn_with(
        secret.clone(),
        data,
        dir.path().join("store"),
        join,
        Arc::clone(&state),
        opts,
    )
    .await
    .unwrap();
    let lookup = MemoryLookup::new();
    engine.endpoint().address_lookup().unwrap().add(lookup.clone());
    let addr = loopback(engine.endpoint());
    let task = tokio::spawn(engine.run());
    Node { dir, net, addr, lookup, state, secret, task }
}

/// Stop `old`'s engine and start it again on the same directory, store and
/// identity, the way a process restart does: `Join::Existing` on the document
/// it already holds, with fresh in-memory state.
async fn restart(old: Node, name: &str, hold_timeout: Duration) -> Node {
    let ns = old.namespace();
    let Node { dir, net, secret, task, .. } = old;
    task.abort();
    let _ = task.await;
    drop(net);
    // Give the aborted engine's store time to release its files.
    tokio::time::sleep(Duration::from_secs(1)).await;
    start(name, dir, secret, 1, Join::Existing(ns), |o| o.hold_timeout = hold_timeout).await
}

/// `b` asks `a` to pair, reads the code off `a`, and confirms it. Returns once
/// `b` has switched to `a`'s document.
async fn pair(a: &Node, b: &Node) {
    a.lookup.add_endpoint_info(b.addr.clone());
    b.lookup.add_endpoint_info(a.addr.clone());
    let (status, v) = b.net.request(a.addr.clone()).await;
    assert_eq!(status, 200, "{v}");
    let pending = a.net.club.book.lock().unwrap().pending(Instant::now());
    let code = pending[0]["code"].as_str().unwrap().to_string();
    let (status, v) = b.net.confirm(&code).await;
    assert_eq!(status, 200, "{v}");
    let a_ns = a.namespace();
    wait_for("B to switch documents", Duration::from_secs(30), || b.namespace() == a_ns).await;
}

fn landed_paths(n: &Node) -> Vec<Value> {
    n.state
        .lock()
        .unwrap()
        .log_entries()
        .into_iter()
        .filter(|e| e["kind"] == "landed")
        .map(|e| e["detail"].clone())
        .collect()
}


async fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn read(p: &Path) -> Option<Vec<u8>> {
    std::fs::read(p).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_nodes_pair_converge_and_pass_a_frame() {
    // `wall` is shown by "bar" (A); "nobody" is a name no one carries.
    let outputs = r#"[
        {"id":"wall","label":"Wall","width":64,"height":36,"masks":[],"node":"bar"},
        {"id":"ghost","label":"Ghost","width":64,"height":36,"masks":[],"node":"nobody"}
    ]"#;
    let a = node("bar", fake_dash(), outputs).await;
    let b = node("stage", 1, outputs).await;
    a.lookup.add_endpoint_info(b.addr.clone());
    b.lookup.add_endpoint_info(a.addr.clone());
    std::fs::write(a.data().join("trims.json"), br#"{"from":"bar"}"#).unwrap();

    // --- pairing ---------------------------------------------------------
    let (status, v) = b.net.request(a.addr.clone()).await;
    assert_eq!(status, 200, "{v}");
    // A second request while the first is in flight is refused.
    assert_eq!(b.net.request(a.addr.clone()).await.0, 409);

    let pending = a.net.club.book.lock().unwrap().pending(Instant::now());
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["name"], "stage");
    let code = pending[0]["code"].as_str().unwrap().to_string();
    let wrong = if code == "000000" { "111111" } else { "000000" };

    let (status, v) = b.net.confirm(wrong).await;
    assert_eq!(status, 403);
    assert_eq!(v["attempts_left"], 2);
    assert!(!a.net.club.book.lock().unwrap().is_member(&b.addr.id.to_string()));

    let (status, v) = b.net.confirm(&code).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["joined"], "bar");
    assert!(a.net.club.book.lock().unwrap().is_member(&b.addr.id.to_string()));
    assert_eq!(b.net.club.book.lock().unwrap().node_of("bar"), Some(a.addr.id.to_string().as_str()));

    // Both sides wrote the table; B also wrote A's namespace.
    let a_ns = a.net.club.doc.read().unwrap().as_ref().unwrap().id().to_string();
    let a_cfg = a.dir.path().join("cfg/sync.toml");
    assert!(std::fs::read_to_string(&a_cfg).unwrap().contains("stage"));
    let b_sync = b.dir.path().join("cfg/sync.toml");
    wait_for("B to switch documents", Duration::from_secs(20), || {
        std::fs::read_to_string(&b_sync).is_ok_and(|t| t.contains(&a_ns))
    })
    .await;

    // --- the document converges ----------------------------------------
    let want = br#"{"from":"bar"}"#.to_vec();
    let b_trims = b.data().join("trims.json");
    wait_for("A's trims.json to land on B", Duration::from_secs(60), || {
        read(&b_trims).as_ref() == Some(&want)
    })
    .await;

    // --- frames ----------------------------------------------------------
    let f = b.net.frame(FrameAsk::Device("wall".into())).await;
    assert_eq!(f.status, 200, "{}", String::from_utf8_lossy(&f.body));
    assert_eq!(f.body, JPEG, "the frame must cross byte for byte");

    // an_unknown_name_is_a_404_not_a_guess
    assert_eq!(b.net.frame(FrameAsk::Device("ghost".into())).await.status, 404);
    // A layer frame reaches A's metal-dash at `/render/l/1`, which the fake
    // does not know (404) — the ask was forwarded, not guessed at.
    let layer = FrameAsk::Layer { layer: "l".into(), frame: 1, on: Some("bar".into()), w: 64, h: 36, state: None, fmt: None, q: None };
    assert_eq!(b.net.frame(layer).await.status, 404);

    // a_frame_request_from_a_stranger_gets_no_answer
    let stranger = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let got = async {
        let conn = stranger.connect(a.addr.clone(), metal_sync::frame::ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(br#"{"device":"wall"}"#).await?;
        send.finish()?;
        anyhow::Ok(recv.read_to_end(1 << 20).await?)
    };
    match tokio::time::timeout(Duration::from_secs(10), got).await {
        Ok(Ok(bytes)) => assert!(bytes.is_empty(), "a stranger got {} bytes", bytes.len()),
        Ok(Err(_)) | Err(_) => {} // closed or silent: no answer either way
    }

    // --- leave_keeps_data_on_disk ---------------------------------------
    assert_eq!(b.net.leave().await.0, 200);
    wait_for("B to leave", Duration::from_secs(20), || {
        std::fs::read_to_string(&b_sync).is_ok_and(|t| !t.contains(&a_ns))
    })
    .await;
    assert!(b.net.club.book.lock().unwrap().peers().is_empty());
    assert_eq!(read(&b_trims), Some(want), "leaving never touches data/");
}

/// A paused node holds every remote write, lands none of them, and on resume
/// lands the latest entry per key once, in the order the writes were made. A
/// superseded write is counted under `lww_dropped`, not silently lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_node_lands_nothing_and_resume_applies_in_timestamp_order() {
    let a = node("bar", 1, "[]").await;
    let b = node("stage", 1, "[]").await;
    pair(&a, &b).await;

    // B writes trims first; A lands it. That entry is the one LWW will drop.
    let b_v0 = br#"{"v":0}"#;
    std::fs::write(b.data().join("trims.json"), b_v0).unwrap();
    wait_for("A to land B's trims", Duration::from_secs(30), || {
        read(&a.data().join("trims.json")).as_deref() == Some(b_v0)
    })
    .await;

    // Park B. Both writes below are made by A, later than B's.
    b.state.lock().unwrap().set_paused(true);
    let before = landed_paths(&b).len();

    let a_v2 = br#"{"v":2}"#;
    std::fs::write(a.data().join("trims.json"), a_v2).unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await; // A's tick publishes it
    std::fs::write(a.data().join("collections.json"), br#"{"c":1}"#).unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;

    assert_eq!(read(&b.data().join("trims.json")).as_deref(), Some(&b_v0[..]), "paused: nothing lands");
    assert!(!b.data().join("collections.json").exists(), "paused: nothing lands");
    assert_eq!(landed_paths(&b).len(), before, "paused: no landed events");

    b.state.lock().unwrap().set_paused(false);
    wait_for("B to catch up after resume", Duration::from_secs(60), || {
        read(&b.data().join("trims.json")).as_deref() == Some(a_v2)
            && b.data().join("collections.json").exists()
    })
    .await;

    // Timestamp order: trims was written before collections.
    let after: Vec<Value> = landed_paths(&b).into_iter().skip(before).collect();
    let paths: Vec<&str> = after.iter().map(|d| d["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["trims.json", "collections.json"]);
    assert!(after[0]["timestamp"].as_u64() < after[1]["timestamp"].as_u64());

    // The superseded B entry is counted, with its path.
    let report = b.state.lock().unwrap().lww_report();
    assert!(report["count"].as_u64().unwrap() >= 1, "{report}");
    assert!(
        report["entries"].as_array().unwrap().iter().any(|e| e["detail"]["path"] == "trims.json"),
        "{report}"
    );
}

/// The first accept is when a lone node's document goes into the club: the
/// ticket names that document and `sync.toml` records it at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lone_node_creates_its_document_on_first_accept() {
    let a = node("bar", 1, "[]").await;
    let b = node("stage", 1, "[]").await;
    let a_ns = a.namespace().to_string();
    pair(&a, &b).await;
    let cfg = std::fs::read_to_string(a.dir.path().join("cfg/sync.toml")).unwrap();
    assert!(cfg.contains(&a_ns), "the first accept must write the namespace: {cfg}");
    assert_eq!(b.namespace().to_string(), a_ns);
}

const CLUB: &[u8] = br#"{"from":"bar"}"#;
const STAGE: &[u8] = br#"{"from":"stage"}"#;

/// The content hash the document currently holds for `rel`, if any.
async fn latest_hash(n: &Node, rel: &str) -> Option<[u8; 32]> {
    let doc = n.net.club.doc.read().unwrap().as_ref().unwrap().clone();
    let entry = doc.get_one(Query::single_latest_per_key().key_exact(rel)).await.unwrap()?;
    Some(*entry.content_hash().as_bytes())
}

/// The club (A) publishes `trims.json`. A joiner (B) starts from A's ticket
/// with its own, different `trims.json` already on disk: the file a node
/// carries from before it joined. Returns once A's entry exists in A's
/// document, so the club's entry is always the older of the two.
async fn club_and_joiner(hold: bool) -> (Node, Node) {
    let a = node("bar", 1, "[]").await;
    std::fs::write(a.data().join("trims.json"), CLUB).unwrap();
    wait_for("A to publish its trims", Duration::from_secs(30), || {
        futures_lite_block(latest_hash(&a, "trims.json")) == Some(*blake3::hash(CLUB).as_bytes())
    })
    .await;
    let ticket = {
        let doc = a.net.club.doc.read().unwrap().as_ref().unwrap().clone();
        let share = doc.share(ShareMode::Write, AddrInfoOptions::RelayAndAddresses);
        futures_lite_block(share).unwrap()
    };
    let b = node_with("stage", 1, "[]", &[("trims.json", STAGE)], Some(ticket), |o| o.hold_joiner = hold).await;
    (a, b)
}

/// Blocks on a future from inside a `wait_for` poll. The poll runs on a
/// multi-thread runtime worker, so `block_in_place` lets the store's tasks
/// keep running while the lookup completes.
fn futures_lite_block<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// A joiner's own files do not replace the club's. The joiner holds its
/// publishes until the club's first sync lands, so its newer `trims.json`
/// never reaches the document ahead of the club's entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn republishing_a_joiners_files_does_not_clobber_the_clubs() {
    let (a, b) = club_and_joiner(true).await;

    let b_trims = b.data().join("trims.json");
    wait_for("B to take the club's trims", Duration::from_secs(60), || read(&b_trims).as_deref() == Some(CLUB)).await;
    assert_eq!(latest_hash(&b, "trims.json").await, Some(*blake3::hash(CLUB).as_bytes()), "B's document must keep the club's entry as the winner");
    assert_eq!(read(&a.data().join("trims.json")).as_deref(), Some(CLUB), "A's copy was clobbered");
    assert_eq!(read(&b_trims).as_deref(), Some(CLUB), "B's copy was republished");
}

/// The same ordering with the hold switched off: B's newer entry is the
/// document's winner under LWW, so the club's copy is gone. This is the race
/// the hold exists to close; if it stops reproducing, the test above proves
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_hold_the_same_ordering_clobbers_the_club() {
    let (_a, b) = club_and_joiner(false).await;
    wait_for("B to publish its own trims", Duration::from_secs(30), || {
        futures_lite_block(latest_hash(&b, "trims.json")) == Some(*blake3::hash(STAGE).as_bytes())
    })
    .await;
}

/// A ticket for the club's document that names an address nobody answers on.
fn unreachable_ticket(secret: iroh_docs::Capability) -> DocTicket {
    let nobody = EndpointAddr::from(iroh::SecretKey::from_bytes(&[9; 32]).public());
    DocTicket::new(secret, vec![nobody])
}

/// A club that never answers must not hold a joiner's new writes forever: once
/// the hold lapses, a file written after the join publishes. The files that
/// were on disk at join time do not: their timestamps are older than the
/// club's real entries, and publishing them now would win LWW over those.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_club_releases_new_writes_but_not_pre_existing_files() {
    let secret = iroh_docs::NamespaceSecret::from_bytes(&[7; 32]);
    let ticket = unreachable_ticket(iroh_docs::Capability::Write(secret));
    let b = node_with(
        "stage",
        1,
        "[]",
        &[("trims.json", STAGE), ("collections.json", b"{\"x\":1}")],
        Some(ticket),
        |o| o.hold_timeout = Duration::from_secs(2),
    )
    .await;
    std::fs::write(b.data().join("switches.json"), b"{\"new\":true}").unwrap();

    wait_for("B to publish its new write", Duration::from_secs(30), || {
        futures_lite_block(latest_hash(&b, "switches.json")).is_some()
    })
    .await;
    assert_eq!(latest_hash(&b, "trims.json").await, None, "a pre-existing file was published without the club");
    assert_eq!(latest_hash(&b, "collections.json").await, None, "a pre-existing file was published without the club");
    assert_eq!(read(&b.data().join("trims.json")).as_deref(), Some(STAGE), "the local copy must be left alone");
}

/// The club comes back after the hold released. Its first sync lands its
/// entries, and only then does B publish: the pre-existing `trims.json` loses
/// to the club's entry and stays the club's, while the joiner-only
/// `extra.json` the club never had goes up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_the_club_returns_its_entry_wins_and_joiner_only_files_publish() {
    let (a, b) = club_and_joiner_unreachable().await;
    let b_trims = b.data().join("trims.json");
    // The hold lapses on its own; nothing from B reaches the document yet.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(latest_hash(&b, "trims.json").await, None);

    // The club is reachable again: A dials B and the two sync.
    a.lookup.add_endpoint_info(b.addr.clone());
    b.lookup.add_endpoint_info(a.addr.clone());
    let a_doc = a.net.club.doc.read().unwrap().as_ref().unwrap().clone();
    a_doc.start_sync(vec![b.addr.clone()]).await.unwrap();

    wait_for("B to take the club's trims", Duration::from_secs(60), || read(&b_trims).as_deref() == Some(CLUB)).await;
    wait_for("the joiner-only file to publish", Duration::from_secs(60), || {
        futures_lite_block(latest_hash(&a, "collections.json")) == Some(*blake3::hash(b"{\"x\":1}").as_bytes())
    })
    .await;
    assert_eq!(latest_hash(&b, "trims.json").await, Some(*blake3::hash(CLUB).as_bytes()), "the club's entry must win");
    assert_eq!(read(&a.data().join("trims.json")).as_deref(), Some(CLUB), "A's copy was clobbered");
}

/// A club (A) with `trims.json` on disk, and a joiner (B) started from a
/// ticket whose only address nobody answers. B's `trims.json` is on disk
/// before the join. The hold lapses (2 s) before this returns.
async fn club_and_joiner_unreachable() -> (Node, Node) {
    let a = node("bar", 1, "[]").await;
    std::fs::write(a.data().join("trims.json"), CLUB).unwrap();
    wait_for("A to publish its trims", Duration::from_secs(30), || {
        futures_lite_block(latest_hash(&a, "trims.json")) == Some(*blake3::hash(CLUB).as_bytes())
    })
    .await;
    let share = {
        let doc = a.net.club.doc.read().unwrap().as_ref().unwrap().clone();
        futures_lite_block(doc.share(ShareMode::Write, AddrInfoOptions::RelayAndAddresses)).unwrap()
    };
    let ticket = unreachable_ticket(share.capability);
    let b = node_with(
        "stage",
        1,
        "[]",
        &[("trims.json", STAGE), ("collections.json", b"{\"x\":1}")],
        Some(ticket),
        |o| o.hold_timeout = Duration::from_secs(2),
    )
    .await;
    (a, b)
}

/// A joiner whose club is unreachable restarts before the club's first sync.
/// The restart reopens the known document (`Join::Existing`), so the ticket
/// path does not run again: the pre-existing files must stay withheld from
/// the withheld set `join_hold.json` carried across the restart. When the club
/// returns, its entry wins and only the joiner-only file goes up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_before_the_clubs_first_sync_keeps_withholding() {
    let (a, b) = club_and_joiner_unreachable().await;
    // The hold lapses on its own before the restart.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let hold_file = b.dir.path().join("cfg/join_hold.json");
    assert!(hold_file.exists(), "the hold must be persisted before the club syncs");

    let b = restart(b, "stage", Duration::from_secs(2)).await;
    assert!(b.state.lock().unwrap().has_withheld(), "the restart forgot the withheld set");
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(latest_hash(&b, "trims.json").await, None, "a restart published a pre-existing file");
    assert_eq!(latest_hash(&b, "collections.json").await, None, "a restart published a pre-existing file");

    // The club is reachable again: A dials B's new address and the two sync.
    a.lookup.add_endpoint_info(b.addr.clone());
    b.lookup.add_endpoint_info(a.addr.clone());
    let a_doc = a.net.club.doc.read().unwrap().as_ref().unwrap().clone();
    a_doc.start_sync(vec![b.addr.clone()]).await.unwrap();

    let b_trims = b.data().join("trims.json");
    wait_for("B to take the club's trims", Duration::from_secs(60), || read(&b_trims).as_deref() == Some(CLUB)).await;
    wait_for("the joiner-only file to publish", Duration::from_secs(60), || {
        futures_lite_block(latest_hash(&a, "collections.json")) == Some(*blake3::hash(b"{\"x\":1}").as_bytes())
    })
    .await;
    assert_eq!(latest_hash(&b, "trims.json").await, Some(*blake3::hash(CLUB).as_bytes()), "the club's entry must win");
    assert_eq!(read(&a.data().join("trims.json")).as_deref(), Some(CLUB), "A's copy was clobbered");
    assert!(!hold_file.exists(), "the hold must be cleared once the club has synced");
}

/// Leaving while the hold is still up: the withheld pre-existing files have no
/// club to wait for any more, so they publish into the node's fresh one-member
/// document, and the sidecar is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leaving_releases_the_hold_and_publishes_the_withheld_files() {
    let (_a, b) = club_and_joiner_unreachable().await;
    let hold_file = b.dir.path().join("cfg/join_hold.json");
    assert!(hold_file.exists(), "the hold must be persisted before the leave");
    assert!(b.state.lock().unwrap().has_withheld());

    let (status, v) = b.net.leave().await;
    assert_eq!(status, 200, "{v}");

    wait_for("the withheld trims to publish into the new document", Duration::from_secs(60), || {
        futures_lite_block(latest_hash(&b, "trims.json")) == Some(*blake3::hash(STAGE).as_bytes())
    })
    .await;
    wait_for("the withheld collections to publish", Duration::from_secs(60), || {
        futures_lite_block(latest_hash(&b, "collections.json")) == Some(*blake3::hash(b"{\"x\":1}").as_bytes())
    })
    .await;
    assert!(!b.state.lock().unwrap().has_withheld(), "the leave kept a withheld set");
    assert!(!hold_file.exists(), "the leave must delete join_hold.json");
    assert_eq!(read(&b.data().join("trims.json")).as_deref(), Some(STAGE), "the local copy must be left alone");
}

/// The hold lapses (2 s), then B restarts with a hold far longer than the test.
/// The lapse was persisted, so the restart does not hold new writes again:
/// a file written after the restart publishes promptly. The pre-existing
/// files stay withheld, as they do for a restart that has not lapsed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_after_the_hold_lapsed_publishes_new_writes_at_once() {
    let (_a, b) = club_and_joiner_unreachable().await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    let hold_file = b.dir.path().join("cfg/join_hold.json");
    let record: Value = serde_json::from_str(&std::fs::read_to_string(&hold_file).unwrap()).unwrap();
    assert_eq!(record["timed_out"], Value::Bool(true), "the lapse must be persisted: {record}");

    let b = restart(b, "stage", Duration::from_secs(120)).await;
    assert!(b.state.lock().unwrap().has_withheld(), "the restart forgot the withheld set");
    std::fs::write(b.data().join("switches.json"), b"{\"new\":true}").unwrap();
    wait_for("B to publish its new write without a second timeout", Duration::from_secs(30), || {
        futures_lite_block(latest_hash(&b, "switches.json")).is_some()
    })
    .await;
    assert_eq!(latest_hash(&b, "trims.json").await, None, "a restart published a pre-existing file");
    assert_eq!(latest_hash(&b, "collections.json").await, None, "a restart published a pre-existing file");
}

/// A song's bundle reaches a paired node on demand, streamed and resumable;
/// before pairing there is no peer to ask, and a path outside the audio and
/// digest folders is refused on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_song_pulls_from_a_paired_peer_and_nothing_else() {
    let outputs = "[]";
    let a = node("bar", fake_dash(), outputs).await;
    let b = node("stage", 1, outputs).await;
    a.lookup.add_endpoint_info(b.addr.clone());
    b.lookup.add_endpoint_info(a.addr.clone());

    // A holds the song: 3 MiB of audio (several chunks), its digest, a file
    // that is not part of any bundle, and an unrelated document.
    let audio: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    std::fs::create_dir_all(a.data().join("audio")).unwrap();
    std::fs::write(a.data().join("audio/track.mp3"), &audio).unwrap();
    std::fs::write(a.data().join("audio/track.8hz.npy"), b"npy-bytes").unwrap();
    std::fs::write(a.data().join("audio/other.mp3"), b"other").unwrap();
    std::fs::write(a.data().join("trims.json"), b"{}").unwrap();

    // Before pairing B has no peer: the pull ends "missing" and writes nothing.
    b.net.pull_song("track").await;
    assert_eq!(b.net.pull_status("track")["state"], "missing");
    assert!(!b.data().join("audio/track.mp3").exists());

    // --- pair B with A ---------------------------------------------------
    let (status, v) = b.net.request(a.addr.clone()).await;
    assert_eq!(status, 200, "{v}");
    let pending = a.net.club.book.lock().unwrap().pending(Instant::now());
    let code = pending[0]["code"].as_str().unwrap().to_string();
    let (status, v) = b.net.confirm(&code).await;
    assert_eq!(status, 200, "{v}");

    // --- the pull: streamed, hash-checked, not the other files ----------
    b.net.pull_song("track").await;
    let st = b.net.pull_status("track");
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["from"], "bar");
    assert_eq!(st["got_bytes"], st["total_bytes"], "{st}");
    assert_eq!(read(&b.data().join("audio/track.mp3")).as_deref(), Some(&audio[..]));
    assert_eq!(read(&b.data().join("audio/track.8hz.npy")).as_deref(), Some(&b"npy-bytes"[..]));
    assert!(!b.data().join("audio/other.mp3").exists(), "a file outside the bundle must not come");
    assert!(!b.data().join("trims.json").exists(), "the document is not what this path moves");

    // --- resume: a dropped trip left the first MiB in `.part` ------------
    std::fs::remove_file(b.data().join("audio/track.mp3")).unwrap();
    std::fs::write(b.data().join("audio/track.mp3.part"), &audio[..1024 * 1024]).unwrap();
    b.net.pull_song("track").await;
    let st = b.net.pull_status("track");
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(read(&b.data().join("audio/track.mp3")).as_deref(), Some(&audio[..]));
    assert!(!b.data().join("audio/track.mp3.part").exists());

    // --- the stream refuses anything outside audio/ and digest/ ----------
    for rel in ["trims.json", "audio/../trims.json", "/etc/passwd"] {
        let conn = b.net.endpoint.connect(a.addr.clone(), metal_sync::frame::ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        send.write_all(json!({ "asset": rel, "from": 0 }).to_string().as_bytes()).await.unwrap();
        send.finish().unwrap();
        let bytes = recv.read_to_end(1 << 20).await.unwrap();
        assert_eq!(&bytes[..2], &404u16.to_be_bytes(), "{rel} must be refused, got {} bytes", bytes.len());
        assert_eq!(bytes.len(), 10, "{rel}: a refusal carries no body");
    }
}

/// A device graph and a model folder pull on demand the way a song does:
/// only the named bundle comes, and a graph that is not a device graph is not
/// served.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_graph_and_a_model_pull_from_a_paired_peer() {
    use metal_sync::pull::Kind;
    let a = node("bar", fake_dash(), "[]").await;
    let b = node("stage", 1, "[]").await;
    a.lookup.add_endpoint_info(b.addr.clone());
    b.lookup.add_endpoint_info(a.addr.clone());
    std::fs::create_dir_all(a.data().join("graphs")).unwrap();
    std::fs::create_dir_all(a.data().join("models/bp")).unwrap();
    std::fs::write(a.data().join("graphs/device-ph.json"), b"{\"nodes\":[]}").unwrap();
    std::fs::write(a.data().join("graphs/main.json"), b"{}").unwrap();
    std::fs::write(a.data().join("models/bp/m.onnx"), vec![7u8; 200_000]).unwrap();

    let (status, v) = b.net.request(a.addr.clone()).await;
    assert_eq!(status, 200, "{v}");
    let pending = a.net.club.book.lock().unwrap().pending(Instant::now());
    let code = pending[0]["code"].as_str().unwrap().to_string();
    assert_eq!(b.net.confirm(&code).await.0, 200);

    b.net.pull(Kind::Graph, "device-ph").await;
    let st = b.net.pull_status(&Kind::Graph.key("device-ph"));
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(read(&b.data().join("graphs/device-ph.json")).as_deref(), Some(&b"{\"nodes\":[]}"[..]));
    b.net.pull(Kind::Graph, "main").await;
    assert_eq!(b.net.pull_status(&Kind::Graph.key("main"))["state"], "missing", "a non-device graph is not served");
    assert!(!b.data().join("graphs/main.json").exists());

    b.net.pull(Kind::Model, "bp").await;
    let st = b.net.pull_status(&Kind::Model.key("bp"));
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["got_bytes"], st["total_bytes"], "{st}");
    assert_eq!(read(&b.data().join("models/bp/m.onnx")).map(|v| v.len()), Some(200_000));
}

/// A stand-in `metal-dash` that renders: `POST /export/video` writes a real
/// mp4 of `range` (one frame per index, 10 fps) at the request's `path`, and
/// `GET /export` answers `done` once it has.
fn fake_render_dash() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let state = Arc::new(Mutex::new("idle"));
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                continue;
            }
            let mut len = 0usize;
            let mut h = String::new();
            while reader.read_line(&mut h).is_ok_and(|n| n > 2) {
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
                h.clear();
            }
            let mut body = vec![0u8; len];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
            let out = if line.starts_with("POST /export/video ") {
                let v: Value = serde_json::from_slice(&body).unwrap();
                let (a, b) = (v["range"][0].as_u64().unwrap(), v["range"][1].as_u64().unwrap());
                let path = v["path"].as_str().unwrap().to_string();
                let made = std::process::Command::new("ffmpeg")
                    .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=64x48:r=10"])
                    .args(["-frames:v", &(b - a).to_string(), "-pix_fmt", "yuv420p", "-c:v", "libx264", "-threads", "1"])
                    .arg(&path)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                *state.lock().unwrap() = if made { "done" } else { "error" };
                json!({ "state": "running" })
            } else if line.starts_with("GET /export ") {
                json!({ "state": *state.lock().unwrap() })
            } else if line.starts_with("POST /export ") {
                *state.lock().unwrap() = "cancelled";
                json!({ "state": "cancelled" })
            } else {
                json!({})
            };
            let text = out.to_string();
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
        }
    });
    port
}

/// The render farm end to end: the conductor splits an export into three
/// shards, a ghost worker claims one and goes silent (3 missed beats put it
/// back), the real worker over `fw/farm/1` renders all three and delivers them
/// into `reels/<reel>/verified/`, and the conductor joins them into the file
/// the export asked for, in frame order, and clears the job.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_farm_splits_requeues_delivers_and_joins() {
    if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let a = node_with("bar", fake_dash(), "[]", &[], None, |o| {
        o.farm = metal_sync::farm_net::FarmCfg { shard_frames: 20, ..Default::default() };
    })
    .await;
    let b = node_with("stage", fake_render_dash(), "[]", &[], None, |o| {
        o.farm = metal_sync::farm_net::FarmCfg { conductor: Some("bar".into()), ..Default::default() };
    })
    .await;
    pair(&a, &b).await;

    let out = a.dir.path().join("joined.mp4");
    let job = a
        .net
        .club
        .farm_submit(&json!({ "device": "out-1", "range": [0, 60], "path": out.display().to_string() }))
        .expect("the conductor accepts the export");
    // The ghost claims one shard now and never beats.
    let ghost = a.net.club.farm.queue.lock().unwrap().claim("ghost", a.net.club.farm.now_ms()).expect("a shard for the ghost");
    assert_eq!(ghost.job, job);

    wait_for("the conductor to join the shards", Duration::from_secs(120), || out.is_file() && a.net.club.farm_report()["jobs"] == json!([])).await;
    let probed = metal_ingest::video::probe(&out).expect("the joined file probes");
    assert_eq!((probed.duration * 10.0).round() as u32, 60, "3 shards of 20 frames, joined");
    for i in 0..3 {
        let shard = a.data().join(format!("reels/out-1/verified/{job}-{i}.mp4"));
        assert!(shard.is_file(), "shard {i} was delivered to verified/: {}", shard.display());
    }
    let queue = std::fs::read_to_string(a.data().join("render/queue.json")).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&queue).unwrap()["jobs"], json!([]), "the queue file is cleared");
}
