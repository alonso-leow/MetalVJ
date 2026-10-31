//! metal-sync: P2P convergence of `data/` across MetalVJ installations.
//!
//! # Why this is its own process
//!
//! O.7 asked where the async runtime lives, and the answer was: not in the
//! core. `metal-dash` is a blocking HTTP server with a `Mutex<State>` and no
//! runtime, and iroh is async from end to end. Putting tokio behind a feature
//! flag in `metal-dash` would have been cheap once and expensive forever — the
//! rule would stop being true and the next async dependency would not have to
//! argue for itself.
//!
//! So the async is quarantined here, in a separate binary, and the two
//! processes meet at two places only:
//!
//! - **The filesystem.** `metal-sync` writes `data/`; `metal-dash` reads it. Graphs
//!   are read by `metal-dash` per request with no cache, so a landed graph is
//!   live immediately with nothing to notify.
//! - **`POST /reload` on :7900.** For the state `metal-dash` holds in memory from
//!   startup — outputs, collections, sequences, reels — a landed file is
//!   invisible until it re-reads. That one route is the whole coupling.
//!
//! Either process runs without the other.
//!
//! # What converges
//!
//! Authored state only, and by allowlist — see [`manifest`]. Derived files
//! (digests, reel frames, the router model, `schema.json`) are cheaper to
//! recompute than to ship, and `ai.json` holds credentials that must never
//! traverse the namespace.
//!
//! # Conflicts
//!
//! Last write wins, by entry timestamp, which is what `iroh-docs` orders by
//! and what `the build reference` §7.1 specifies. Two peers editing the same graph
//! while partitioned means the later one survives and the earlier is dropped —
//! logged with both peer ids and both timestamps, so a lost edit is at least
//! recoverable from the log. O.8's per-graph owner is the real fix and layers
//! on top of this without touching the transport.

pub mod config;
pub mod doctor;
pub mod engine;
pub mod frame;
pub mod http;
pub mod identity;
pub mod land;
pub mod manifest;
pub mod node;
pub mod notify;
pub mod pair;
pub mod farm;
pub mod pull;
pub mod reader;
pub mod state;
pub mod stats;

pub mod session_merge;
pub use land::{Lander, Landing, Sender};
pub use reader::DataReader;
pub use state::SyncState;
