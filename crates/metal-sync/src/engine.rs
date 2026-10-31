//! The iroh half: an endpoint, a document, and the loop that keeps `data/` and
//! the document agreeing with each other.
//!
//! # The shape
//!
//! One `iroh-docs` document holds one entry per synced path. The key is the
//! path relative to `data/`; the value is the BLAKE3 hash of the bytes, which
//! live in the blob store. That indirection is what makes two machines holding
//! the same graph transfer nothing at all — same bytes, same hash, nothing to
//! send.
//!
//! # Two directions, two triggers
//!
//! **Outbound is polled.** There is no file-watching anywhere in this
//! workspace and no `notify` dependency, so a tick re-hashes the syncable
//! files and publishes what changed. Polling is also the honest choice given
//! how `metal-dash` writes: in-place, non-atomically, with no notification of any
//! kind. A watcher would fire mid-write and read a torn file; a hash on a tick
//! either sees the old bytes or the new ones.
//!
//! **Inbound is evented.** `Doc::subscribe` reports a remote insert the moment
//! it arrives, so a peer's save lands here in seconds without waiting for our
//! own tick.
//!
//! # Reconfiguration under a live engine
//!
//! `[sync].allowseed` in `config.toml` is the allowlist, and it is read
//! fresh on every scan tick (`SyncState::allowlist`) rather than once at
//! start — so the answer to "what happens when a hand edit narrows the list
//! while this engine is watching a path" is decided here, not left to
//! whichever code path happens to run first:
//!
//! - **A path that just left the list** stops being published on the next
//!   tick (the scan no longer lists it) and stops being landed the next time
//!   a peer inserts it (`land_entry` refuses it). It is **never deleted**,
//!   locally or in the document: the file stays on disk, the document entry
//!   stays for peers that still allow it. Excluding a path means "this node
//!   stops carrying it", never "this node removes it".
//! - **A path that just joined the list** is picked up like any file that
//!   appeared on disk: the next scan publishes it. For the inbound side the
//!   engine notices the list changed between two ticks and runs
//!   [`SyncEngine::land_existing`] once more, so entries peers already hold
//!   for the new pattern land now rather than the next time somebody edits
//!   one. Comparing this tick's patterns to last tick's is change detection,
//!   not a cache — the list itself is still read from the file each time.
//! - **A `config.toml` that does not parse** (an editor mid-save) makes the
//!   list empty for that tick: nothing out, nothing in, nothing deleted,
//!   and a `warn!` saying why. It resumes the tick after the file parses.
//!
//! Polling is the natural mechanism because the outbound side already ticks;
//! a watch on `config.toml` would be the only watcher in the crate and would
//! fire on the same torn-write problem the module doc above describes.
//!
//! # Why content arrives twice
//!
//! `InsertRemote` says an entry exists; it does not mean the bytes are here.
//! When `content_status` is not `Complete` the blob is still downloading, and
//! reading it then would land a truncated file. Those entries are parked until
//! `ContentReady` names their hash. This is the one piece of the protocol that
//! cannot be simplified away without reintroducing torn writes over the wire.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use futures_lite::StreamExt;
use iroh::{address_lookup::UserData, endpoint::presets, protocol::Router, Endpoint};
use iroh_blobs::{store::fs::FsStore, BlobsProtocol, ALPN as BLOBS_ALPN};
use iroh_docs::{
    api::{
        protocol::{AddrInfoOptions, ShareMode},
        Doc,
    },
    engine::LiveEvent,
    protocol::Docs,
    store::Query,
    AuthorId, ContentStatus, DocTicket, NamespaceId, ALPN as DOCS_ALPN,
};
use iroh_gossip::{net::Gossip, ALPN as GOSSIP_ALPN};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use tokio::sync::mpsc;

use crate::frame::FrameProtocol;
use crate::land::{Landing, Sender};
use crate::pair::{Club, EngineCmd, Net, PairBook, PairProtocol, Seen};
use crate::state::SyncState;

/// How often the outbound scan re-hashes `data/`.
///
/// Seconds rather than milliseconds because the payload is a few megabytes of
/// authored JSON that a human changes by hand. A tighter loop would burn CPU
/// hashing files nobody touched; a looser one would make "I saved it, why
/// hasn't it arrived" a real question.
const SCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

/// How long a joiner holds its publishes waiting for the club's first sync,
/// when `sync.toml` does not say otherwise (`join_hold_secs`).
pub const DEFAULT_HOLD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The sidecar, beside `sync.toml`, that holds a joiner's withheld files
/// and whether the club has synced since the join.
pub const HOLD_FILE: &str = "join_hold.json";

/// Which document this node should end up in.
pub enum Join {
    /// Reopen the document named in `sync.toml`.
    Existing(NamespaceId),
    /// Join the document a ticket points at. This is pairing.
    Ticket(DocTicket),
    /// Start a new document and be its first member.
    Create,
}

/// Everything about the network surface beyond the document itself: who
/// this machine is to the club, where it saves that, and how it finds peers.
pub struct EngineOpts {
    /// This machine's name in the club. `None` derives one
    /// (`pair::default_name`).
    pub name: Option<String>,
    /// The `name ↔ node_id` table as `sync.toml` left it.
    pub peers: BTreeMap<String, String>,
    /// `sync.toml`, rewritten on pair/join/leave. `None` keeps it in memory.
    pub config_path: Option<PathBuf>,
    /// The local `metal-dash` that frames for devices on this machine come from.
    pub dash_port: u16,
    /// Advertise and listen on the LAN with `iroh-mdns-address-lookup`.
    pub mdns: bool,
    /// No relay, no DNS: direct addresses only. Tests on localhost.
    pub local_only: bool,
    /// Whether a node started by a ticket (or joining one) holds its publishes
    /// until the club's entries have landed. On in production; the regression
    /// test turns it off to reproduce the race it closes.
    pub hold_joiner: bool,
    /// How long the hold waits for the club before releasing. A club that
    /// stays unreachable must not stop a node from publishing its own files.
    pub hold_timeout: std::time::Duration,
}

impl Default for EngineOpts {
    fn default() -> Self {
        EngineOpts {
            name: None,
            peers: BTreeMap::new(),
            config_path: None,
            dash_port: 7900,
            mdns: false,
            local_only: false,
            hold_joiner: true,
            hold_timeout: DEFAULT_HOLD_TIMEOUT,
        }
    }
}

/// The engine's background tasks (the 5 s stats tick, mDNS). They hold the
/// endpoint and the club, so they must end with the engine: left running after
/// the `run` future is dropped they keep the document store open, and a restart
/// on the same store (a process restart done in-process) waits on it forever.
#[derive(Default)]
struct Tasks(Vec<tokio::task::AbortHandle>);

impl Drop for Tasks {
    fn drop(&mut self) {
        for t in &self.0 {
            t.abort();
        }
    }
}

pub struct SyncEngine {
    /// Dropped (and so aborted) with the engine.
    _tasks: Tasks,
    doc: Doc,
    author: AuthorId,
    blobs: FsStore,
    state: Arc<Mutex<SyncState>>,
    data_root: PathBuf,
    /// Where a local `metal-dash` is listening, so landed files it caches in
    /// memory can be re-read. `None` means never ask.
    dash: Option<(String, u16)>,
    club: Arc<Club>,
    cmds: mpsc::Receiver<EngineCmd>,
    router: Router,
    docs: Docs,
    /// Started by a ticket: publishing waits for the club's entries to land.
    hold_publish: bool,
    /// Whether a join (pairing while running) also holds publishes.
    hold_joiner: bool,
    hold_timeout: std::time::Duration,
}

impl SyncEngine {
    /// Bind an endpoint, open the blob and doc stores, and join or create the
    /// document.
    ///
    /// `store_root` is where iroh's own databases live — deliberately *not*
    /// inside `data/`, because `data/` is the thing being synced and a store
    /// that synced itself would be a loop.
    pub async fn spawn(
        secret: iroh::SecretKey,
        data_root: PathBuf,
        store_root: PathBuf,
        join: Join,
        state: Arc<Mutex<SyncState>>,
    ) -> Result<SyncEngine> {
        let (engine, _net) =
            Self::spawn_with(secret, data_root, store_root, join, state, EngineOpts::default()).await?;
        Ok(engine)
    }

    /// [`SyncEngine::spawn`], plus the pairing and frame protocols on the
    /// router and the [`Net`] handle the control surface drives them with.
    pub async fn spawn_with(
        secret: iroh::SecretKey,
        data_root: PathBuf,
        store_root: PathBuf,
        join: Join,
        state: Arc<Mutex<SyncState>>,
        opts: EngineOpts,
    ) -> Result<(SyncEngine, Net)> {
        // Both stores want their directory to already exist; neither creates it.
        let blobs_root = store_root.join("blobs");
        let docs_root = store_root.join("docs");
        for dir in [&blobs_root, &docs_root] {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }

        // The N0 preset brings the standard discovery and relay setup, which is
        // what lets a venue Pi behind NAT be reached at all.
        let node_id = secret.public().to_string();
        let config_dir = opts.config_path.as_deref().and_then(Path::parent);
        let name =
            opts.name.clone().unwrap_or_else(|| crate::pair::default_name(&node_id, config_dir));
        let builder = if opts.local_only {
            Endpoint::builder(presets::Minimal)
        } else {
            Endpoint::builder(presets::N0)
        };
        // The name rides along with our addresses, so `GET /discover` can
        // show "bar" rather than a public key.
        let builder = match UserData::try_from(name.clone()) {
            Ok(data) => builder.user_data_for_address_lookup(data),
            Err(_) => builder,
        };
        let endpoint = builder.secret_key(secret).bind().await.context("binding the iroh endpoint")?;

        let blobs = FsStore::load(&blobs_root).await.context("opening blob store")?;
        let gossip = Gossip::builder().spawn(endpoint.clone());
        let docs = Docs::persistent(docs_root)
            .spawn(endpoint.clone(), (*blobs).clone(), gossip.clone())
            .await
            .context("opening doc store")?;

        let club = Arc::new(Club {
            book: Mutex::new(PairBook::new(name, opts.peers.clone())),
            doc: std::sync::RwLock::new(None),
            state: Arc::clone(&state),
            config_path: opts.config_path.clone(),
            data_root: data_root.clone(),
            dash_port: opts.dash_port,
            discovered: Mutex::new(BTreeMap::new()),
            pulls: Mutex::new(BTreeMap::new()),
            stats: Mutex::new(crate::stats::StatsBook::default()),
        });

        let router = Router::builder(endpoint.clone())
            .accept(BLOBS_ALPN, BlobsProtocol::new(&blobs, None))
            .accept(GOSSIP_ALPN, gossip)
            .accept(DOCS_ALPN, docs.clone())
            .accept(crate::pair::ALPN, PairProtocol { club: Arc::clone(&club) })
            .accept(crate::frame::ALPN, FrameProtocol { club: Arc::clone(&club) })
            .accept(crate::stats::ALPN, crate::stats::StatsProtocol { club: Arc::clone(&club) })
            .spawn();
        let mut tasks = Tasks::default();
        tasks.0.push(spawn_stats_tick(endpoint.clone(), Arc::clone(&club)));

        if opts.mdns {
            tasks.0.extend(spawn_mdns(&endpoint, Arc::clone(&club)));
        }

        let api = docs.api();
        let author = api.author_default().await.context("resolving the default author")?;

        // Rejoin the document we already belong to, join one from a ticket, or
        // become the first member of a new one. Reopening by id is what keeps a
        // restart from orphaning this node.
        // A ticket start withholds what is on disk now. A restart on the known
        // document keeps the hold an earlier run left in `join_hold.json` until
        // the club's first sync; without that, its pre-existing files publish.
        let hold_file = opts.config_path.as_deref().and_then(Path::parent).map(|d| d.join(HOLD_FILE));
        let restored = {
            let mut st = state.lock().expect("sync state mutex poisoned");
            st.set_hold_file(hold_file);
            matches!(join, Join::Existing(_)) && opts.hold_joiner && st.load_hold()
        };
        let hold_publish = (opts.hold_joiner && matches!(join, Join::Ticket(_))) || restored;
        if opts.hold_joiner && matches!(join, Join::Ticket(_)) {
            state.lock().expect("sync state mutex poisoned").withhold_existing();
        }
        let doc = match join {
            Join::Ticket(ticket) => {
                api.import(ticket).await.context("joining the document from a ticket")?
            }
            Join::Existing(id) => match api.open(id).await.context("opening the known document")? {
                Some(doc) => doc,
                // The config named a document this store has never seen. Making
                // a fresh one would silently strand the node; say so instead.
                None => anyhow::bail!(
                    "sync.toml names document {id} but the local store does not have it — \
                     re-pair with `metal-sync join <ticket>`, or clear `namespace` in sync.toml"
                ),
            },
            Join::Create => api.create().await.context("creating a document")?,
        };

        // Rejoin the live swarm for this document.
        //
        // `import` does this for us on the run that consumes a ticket, but
        // `open` does not — and `open` is what every run after the first one
        // takes. Without this the node holds the right document, reports the
        // right namespace, and silently never syncs: the failure looks exactly
        // like a working node with nothing to say. Passing no peers means
        // "rejoin using the peers this document already knows".
        doc.start_sync(Vec::new()).await.context("joining the document's swarm")?;
        *club.doc.write().expect("doc lock poisoned") = Some(doc.clone());

        let (tx, rx) = mpsc::channel(4);
        let net = Net { endpoint, club: Arc::clone(&club), cmds: tx };
        Ok((
            SyncEngine {
                _tasks: tasks,
                doc,
                author,
                blobs,
                state,
                data_root,
                dash: None,
                club,
                cmds: rx,
                router,
                docs,
                hold_publish,
                hold_joiner: opts.hold_joiner,
                hold_timeout: opts.hold_timeout,
            },
            net,
        ))
    }

    /// The bound endpoint — tests read its address to dial it directly.
    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    /// Swap the document under a running engine (pairing, leaving), and
    /// write the new namespace to `sync.toml`.
    async fn apply(&mut self, cmd: EngineCmd) -> Result<()> {
        let api = self.docs.api();
        let doc = match cmd {
            EngineCmd::Join(ticket) => {
                api.import(ticket).await.context("joining the document from a ticket")?
            }
            EngineCmd::Leave => {
                // `leave` stops syncing; the entries and `data/` stay where
                // they are. A fresh document makes this node a club of one.
                self.doc.leave().await.context("leaving the document")?;
                self.club.book.lock().expect("pair book poisoned").forget_all();
                // The hold was for the club we just left. Its withheld files
                // are no longer waiting on anyone, so they publish into the
                // fresh document on the next scan.
                let released = self.state.lock().expect("sync state mutex poisoned").release_withheld();
                if released > 0 {
                    tracing::info!(released, "left the club — withheld files publish to the new document");
                }
                let doc = api.create().await.context("creating a document")?;
                doc.start_sync(Vec::new()).await?;
                doc
            }
        };
        self.doc = doc.clone();
        *self.club.doc.write().expect("doc lock poisoned") = Some(doc);
        if let Err(e) = self.club.save() {
            tracing::warn!(%e, "could not write sync.toml");
        }
        match self.ticket().await {
            Ok(t) => self.state.lock().expect("poisoned").set_ticket(t.to_string()),
            Err(e) => tracing::warn!(%e, "could not mint a ticket for the new document"),
        }
        let namespace = self.doc.id().to_string();
        self.state.lock().expect("poisoned").log("document", serde_json::json!({ "namespace": namespace }));
        Ok(())
    }

    /// Point this engine at a local `metal-dash` to notify after landing.
    pub fn with_dash(mut self, host: String, port: u16) -> Self {
        self.dash = Some((host, port));
        self
    }

    pub fn namespace(&self) -> NamespaceId {
        self.doc.id()
    }

    /// A ticket another device can use to join this document. This is the
    /// pairing credential: it carries the namespace and how to reach us.
    pub async fn ticket(&self) -> Result<DocTicket> {
        // `RelayAndAddresses` rather than the default `Id`: a ticket that
        // carries the relay URL and direct addresses can be dialled without a
        // DNS lookup succeeding first, which is the difference between pairing
        // working and not working on a venue network.
        self.doc
            .share(ShareMode::Write, AddrInfoOptions::RelayAndAddresses)
            .await
            .context("creating a share ticket")
    }

    /// Hash `data/`, publish what changed. Returns how many entries moved.
    pub async fn publish_changes(&self) -> Result<usize> {
        let changed = {
            let mut s = self.state.lock().expect("sync state mutex poisoned");
            s.changed_since_last_scan()
        };

        let mut published = 0;
        for rel in changed {
            let bytes = match std::fs::read(self.data_root.join(&rel)) {
                Ok(b) => b,
                // A file that vanished between the scan and the read is not an
                // error worth stopping for; the next tick settles it.
                Err(e) => {
                    tracing::debug!(path = %rel.display(), %e, "skipped a file that went away");
                    continue;
                }
            };
            let key = key_of(&rel);
            self.doc
                .set_bytes(self.author, key, bytes)
                .await
                .with_context(|| format!("publishing {}", rel.display()))?;
            tracing::info!(path = %rel.display(), "published");
            published += 1;
        }
        Ok(published)
    }

    /// Land everything the document already holds that we do not.
    ///
    /// This is what makes a freshly paired device catch up: `subscribe` only
    /// reports what happens next, so without this pass a new node would sit
    /// empty until someone edited something.
    ///
    /// Every author's entry is read, not just the winner per key, so that a
    /// superseded write is counted (`lww_dropped`) rather than invisible. The
    /// winners land in timestamp order, so the log reads as the writes
    /// happened.
    pub async fn land_existing(&self) -> Result<usize> {
        let mut entries = Box::pin(self.doc.get_many(Query::all()).await?);
        let mut winners: HashMap<Vec<u8>, (u64, iroh_blobs::Hash, AuthorId)> = HashMap::new();
        let mut dropped: Vec<(Vec<u8>, u64)> = Vec::new();
        while let Some(entry) = entries.next().await {
            let entry = entry?;
            let key = entry.key().to_vec();
            let (ts, hash, author) = (entry.timestamp(), entry.content_hash(), entry.author());
            match winners.get(&key).cloned() {
                Some((won, _, _)) if won >= ts => dropped.push((key, ts)),
                Some((won, _, _)) => {
                    dropped.push((key.clone(), won));
                    winners.insert(key, (ts, hash, author));
                }
                None => {
                    winners.insert(key, (ts, hash, author));
                }
            }
        }
        for (key, ts) in dropped {
            let path = path_of(&key).map(|p| p.display().to_string()).unwrap_or_default();
            self.state
                .lock()
                .expect("sync state mutex poisoned")
                .lww_dropped(serde_json::json!({ "path": path, "timestamp": ts }));
        }

        let mut order: Vec<(u64, Vec<u8>, iroh_blobs::Hash, AuthorId)> =
            winners.into_iter().map(|(key, (ts, hash, author))| (ts, key, hash, author)).collect();
        order.sort_by_key(|(ts, _, _, _)| *ts);
        let mut landed = 0;
        for (ts, key, hash, author) in order {
            let sender = self.sender_of_author(author);
            match self.land_entry(&key, hash, ts, sender).await {
                Ok(true) => landed += 1,
                Ok(false) => {}
                Err(e) => tracing::warn!(%e, "landing an existing entry failed"),
            }
        }
        Ok(landed)
    }

    /// The long-running loop: react to remote inserts, and scan on a tick.
    pub async fn run(mut self) -> Result<()> {
        let mut events =
            Box::pin(self.doc.subscribe().await.context("subscribing to the document")?);
        let mut tick = tokio::time::interval(SCAN_INTERVAL);
        // Entries whose bytes have not arrived yet, keyed by content hash.
        let mut pending: HashMap<iroh_blobs::Hash, (Vec<u8>, u64, Sender)> = HashMap::new();
        let mut peers: Vec<String> = Vec::new();
        // What the allowlist looked like on the previous tick — only to notice
        // that it changed (see the module doc, "Reconfiguration under a live
        // engine"); every decision still reads the file itself.
        let mut last_patterns: Vec<String> =
            self.state.lock().expect("sync state mutex poisoned").allowlist().patterns().to_vec();
        // `POST /pause` parks everything; the tick that sees it lifted lands
        // the document's latest entry per key once, in timestamp order.
        let mut was_paused = false;
        // A joiner publishes nothing until the club's entries have landed:
        // its own files would otherwise overwrite the club's under LWW.
        let mut holding = self.hold_publish;
        // When the hold gives up on the club and publishes anyway.
        let mut hold_until = holding.then(|| tokio::time::Instant::now() + self.hold_timeout);
        // Set when the first sync after a join finishes; the next tick lands
        // the club's entries, then publishes.
        let mut catch_up = false;
        // Whether the club's first sync has landed. Set by `SyncFinished` and
        // by nothing else: a resume or a timeout does not prove the club
        // answered, so withheld files stay withheld past them.
        let mut club_synced = false;

        loop {
            tokio::select! {
                cmd = self.cmds.recv() => {
                    let Some(cmd) = cmd else { continue };
                    let joined = matches!(cmd, EngineCmd::Join(_));
                    match self.apply(cmd).await {
                        Ok(()) => {
                            events = Box::pin(self.doc.subscribe().await.context("subscribing to the document")?);
                            pending.clear();
                            holding = joined && self.hold_joiner;
                            hold_until = holding.then(|| tokio::time::Instant::now() + self.hold_timeout);
                            if holding {
                                self.state.lock().expect("sync state mutex poisoned").withhold_existing();
                            }
                            match self.land_existing().await {
                                Ok(n) => tracing::info!(landed = n, "switched documents"),
                                Err(e) => tracing::warn!(%e, "catch-up after switching documents failed"),
                            }
                        }
                        Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not switch documents"),
                    }
                }
                _ = tick.tick() => {
                    let paused = self.state.lock().expect("sync state mutex poisoned").paused();
                    if paused {
                        was_paused = true;
                        continue;
                    }
                    if was_paused || catch_up {
                        let why = if was_paused { "resumed" } else { "joined" };
                        let synced = std::mem::take(&mut club_synced);
                        was_paused = false;
                        catch_up = false;
                        match self.land_existing().await {
                            Ok(n) => {
                                tracing::info!(landed = n, why, "caught up");
                                if synced {
                                    // The club's entries are in; the files the
                                    // joiner held back may now publish, and
                                    // only the ones the club does not have will
                                    // move (the rest landed over them above).
                                    let released = self.state.lock().expect("sync state mutex poisoned").release_withheld();
                                    if released > 0 {
                                        tracing::info!(released, "the club is in — pre-existing files may publish");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(%e, why, "catch-up failed");
                                // Still owed: retry on the next tick.
                                catch_up = synced;
                                club_synced = synced;
                            }
                        }
                    }
                    let patterns = self
                        .state
                        .lock()
                        .expect("sync state mutex poisoned")
                        .allowlist()
                        .patterns()
                        .to_vec();
                    if patterns != last_patterns {
                        tracing::info!(
                            before = last_patterns.len(),
                            after = patterns.len(),
                            "allowseed changed in config.toml — re-scanning the document for newly allowed paths"
                        );
                        last_patterns = patterns;
                        // A widened list may now admit entries peers published
                        // long ago; a narrowed one refuses them inside
                        // `land_entry`, so one pass covers both directions.
                        match self.land_existing().await {
                            Ok(n) if n > 0 => tracing::info!(landed = n, "landed after allowseed change"),
                            Ok(_) => {}
                            Err(e) => tracing::warn!(%e, "re-scan after allowseed change failed"),
                        }
                    }
                    if holding && hold_until.is_some_and(|t| tokio::time::Instant::now() >= t) {
                        holding = false;
                        let deferred = {
                            let mut s = self.state.lock().expect("sync state mutex poisoned");
                            s.mark_hold_timed_out();
                            s.withheld()
                        };
                        tracing::warn!(
                            timeout_secs = self.hold_timeout.as_secs(),
                            ?deferred,
                            "the club did not finish its first sync before the join hold ran out — publishing new local writes only; files that were on disk at join time wait for the club"
                        );
                    }
                    if !holding {
                        if let Err(e) = self.publish_changes().await {
                            tracing::warn!(%e, "publish failed");
                        }
                    }
                }
                event = events.next() => {
                    let Some(event) = event else { break };
                    match event {
                        // Paused: land nothing; the resume pass picks it up.
                        Ok(LiveEvent::InsertRemote { .. } | LiveEvent::ContentReady { .. })
                            if self.state.lock().expect("poisoned").paused() => {}
                        Ok(LiveEvent::InsertRemote { entry, content_status, from }) => {
                            let key = entry.key().to_vec();
                            let hash = entry.content_hash();
                            let ts = entry.timestamp();
                            if content_status == ContentStatus::Complete {
                                let sender = self.sender_of(&from.to_string());
                                if let Err(e) = self.land_entry(&key, hash, ts, sender).await {
                                    tracing::warn!(%e, "landing failed");
                                }
                            } else {
                                // Bytes still in flight. Reading now would
                                // write a truncated file.
                                tracing::debug!(peer = %from, "entry ahead of its content");
                                pending.insert(hash, (key, ts, self.sender_of(&from.to_string())));
                            }
                        }
                        Ok(LiveEvent::ContentReady { hash }) => {
                            if let Some((key, ts, sender)) = pending.remove(&hash) {
                                if let Err(e) = self.land_entry(&key, hash, ts, sender).await {
                                    tracing::warn!(%e, "landing failed");
                                }
                            }
                        }
                        // The first sync with the club ends the hold, and
                        // also releases files withheld by a timed-out hold.
                        Ok(LiveEvent::SyncFinished(_))
                            if holding || self.state.lock().expect("sync state mutex poisoned").has_withheld() =>
                        {
                            holding = false;
                            catch_up = true;
                            club_synced = true;
                        }
                        Ok(LiveEvent::NeighborUp(peer)) => {
                            peers.push(peer.to_string());
                            self.state.lock().expect("poisoned").set_peers(peers.clone());
                            tracing::info!(%peer, "peer up");
                        }
                        Ok(LiveEvent::NeighborDown(peer)) => {
                            peers.retain(|p| p != &peer.to_string());
                            self.state.lock().expect("poisoned").set_peers(peers.clone());
                            tracing::info!(%peer, "peer down");
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!(%e, "document event stream error"),
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether the node an entry came from is a member of the club (someone
    /// we paired with). A node that is not is `Unpaired`, and its writes to a
    /// graph this machine owns are dropped.
    fn sender_of(&self, node_id: &str) -> Sender {
        if self.club.book.lock().expect("pair book poisoned").is_member(node_id) {
            Sender::Paired
        } else {
            Sender::Unpaired
        }
    }

    /// The sender of an entry already in the document, known only by its
    /// author id. An author id is not a node id, so the book cannot name it;
    /// the document is only shared with the club, so any author but this
    /// machine's own counts as paired while the club has a member.
    fn sender_of_author(&self, author: AuthorId) -> Sender {
        let paired = !self.club.book.lock().expect("pair book poisoned").peers().is_empty();
        if author != self.author && paired {
            Sender::Paired
        } else {
            Sender::Unpaired
        }
    }

    /// Fetch one entry's bytes and put them through the landing gate.
    ///
    /// Returns whether anything was written. A hash we already have on disk is
    /// not written again — that is what stops a landed file from being
    /// re-published on the next scan and echoing around the mesh forever.
    async fn land_entry(&self, key: &[u8], hash: iroh_blobs::Hash, timestamp: u64, from: Sender) -> Result<bool> {
        let Some(rel) = path_of(key) else {
            tracing::warn!("a peer sent an entry whose key is not a usable path");
            return Ok(false);
        };

        {
            let s = self.state.lock().expect("sync state mutex poisoned");
            if s.already_have(&rel, &hash.to_hex().to_string()) {
                return Ok(false);
            }
        }

        let bytes = self
            .blobs
            .blobs()
            .get_bytes(hash)
            .await
            .with_context(|| format!("fetching content for {}", rel.display()))?;

        let outcome = {
            let s = self.state.lock().expect("sync state mutex poisoned");
            // The allowlist as of *now* — a path excluded since the last tick
            // is refused here, whatever the peer thinks.
            let allow = s.allowlist();
            s.lander().land(&rel, &bytes, &allow, from)?
        };

        match outcome {
            Landing::Wrote => {
                // Record what we just wrote so the next outbound scan sees it
                // as already published rather than as a local edit.
                {
                    let mut s = self.state.lock().expect("sync state mutex poisoned");
                    s.mark_published(&rel, blake3::hash(&bytes).to_hex().to_string());
                }
                tracing::info!(path = %rel.display(), "landed");
                self.state
                    .lock()
                    .expect("sync state mutex poisoned")
                    .log(
                        "landed",
                        serde_json::json!({ "path": rel.display().to_string(), "timestamp": timestamp }),
                    );

                // Only for the files metal-dash holds in memory; a landed graph
                // is already live.
                if let Some((host, port)) = &self.dash {
                    if crate::notify::needs_reload(&rel) {
                        crate::notify::reload(host, *port);
                    }
                }
                Ok(true)
            }
            Landing::Refused(why) | Landing::Invalid(why) => {
                tracing::warn!(path = %rel.display(), %why, "did not land");
                Ok(false)
            }
        }
    }
}

/// Advertise on the LAN and keep `Club::discovered` current.
///
/// `iroh-mdns-address-lookup` 0.6 is built against iroh 1.x and resolves
/// against this crate's 1.2; added to the endpoint's lookups it also lets a
/// bare node id from `GET /discover` be dialled with no relay at all.
fn spawn_mdns(endpoint: &Endpoint, club: Arc<Club>) -> Option<tokio::task::AbortHandle> {
    let mdns = match MdnsAddressLookup::builder().build(endpoint.id()) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(%e, "mDNS unavailable on this network; discovery is off");
            return None;
        }
    };
    match endpoint.address_lookup() {
        Ok(lookups) => lookups.add(mdns.clone()),
        Err(e) => tracing::warn!(%e, "could not register mDNS with the endpoint"),
    }
    Some(tokio::spawn(async move {
        let mut events = mdns.subscribe().await;
        while let Some(event) = events.next().await {
            let mut seen = club.discovered.lock().expect("discovered poisoned");
            match event {
                DiscoveryEvent::Discovered { endpoint_info, .. } => {
                    let data = &endpoint_info.data;
                    seen.insert(
                        endpoint_info.endpoint_id.to_string(),
                        Seen {
                            name: data.user_data().map(|u| u.to_string()),
                            addrs: data.ip_addrs().map(|a| a.to_string()).collect(),
                            last_seen: crate::pair::unix_now(),
                        },
                    );
                }
                DiscoveryEvent::Expired { endpoint_id } => {
                    seen.remove(&endpoint_id.to_string());
                }
                _ => {}
            }
        }
    }).abort_handle())
}

/// A path relative to `data/` becomes the document key, verbatim.
fn key_of(rel: &Path) -> Vec<u8> {
    rel.to_string_lossy().into_owned().into_bytes()
}

/// And back. Returns `None` for anything that is not valid UTF-8, which a key
/// written by a well-behaved peer always is.
fn path_of(key: &[u8]) -> Option<PathBuf> {
    std::str::from_utf8(key).ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_survives_the_round_trip_through_a_document_key() {
        for p in ["trims.json", "graphs/entrance-set.json", "animations/a/b.npy"] {
            let rel = PathBuf::from(p);
            assert_eq!(path_of(&key_of(&rel)).unwrap(), rel);
        }
    }

    #[test]
    fn a_key_that_is_not_utf8_is_rejected_rather_than_lossily_converted() {
        // Lossy conversion would turn a malformed key into a plausible path and
        // write a file nobody asked for.
        assert!(path_of(&[0xff, 0xfe]).is_none());
    }
}

/// The 5 s self-report: this machine's readings (and the local `GET /perf`'s
/// paint backend and frame rate) pushed to every peer. The loop polls twice a
/// second; [`crate::stats::Cadence`] decides when a tick is due.
fn spawn_stats_tick(endpoint: iroh::Endpoint, club: Arc<Club>) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        let mut cadence = crate::stats::Cadence::default();
        let mut sampler = crate::stats::CpuSampler::default();
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            poll.tick().await;
            if !cadence.due(std::time::Instant::now()) {
                continue;
            }
            let dash = club.dash_port;
            let perf = tokio::task::spawn_blocking(move || crate::frame::loopback_get(dash, "/perf"))
                .await
                .ok()
                .and_then(Result::ok);
            let mut stats = crate::stats::read_self(&mut sampler);
            if let Some(frame) = perf {
                (stats.paint_backend, stats.fps_paint) = crate::stats::perf_fields(&frame.body);
            }
            crate::stats::push(&endpoint, &club, &stats).await;
        }
    })
    .abort_handle()
}
