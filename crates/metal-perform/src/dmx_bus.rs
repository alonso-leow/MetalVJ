//! D3 — the universe bus: one shared DMX buffer per universe that every
//! `out.*` and `venue.fixtures` source writes into, merged per source with a
//! **priority** before it goes back out through the existing [`Wire`]s.
//!
//! Today each sACN/Art-Net node sends its own universe packet and nothing
//! reconciles two sources on the same universe — the last writer's slots win,
//! silently zeroing the other's ([`crate::wire`]'s own note). The model is
//! grandMA3's per-universe slot map (report §6): a remote source maps onto a
//! local universe with a priority, the highest priority wins a slot, and a
//! sACN source that stops sending is dropped after a **3 s data-loss timeout**
//! (`sACN::OnDMX`, report §6). A tie between equal priorities merges HTP (the
//! maximum value per slot), and ShowXpress' per-input mode (report §3)
//! decides a tie when a source is an `Input` or `Software` rather than an
//! `Htp`.
//!
//! **The bus is data, not a second state.** Its inputs — priority and mode —
//! are a *reading of the graph* (rule 4): they live in each source node's own
//! config ([`bus_source`]), edited by the same `POST /graph/node {op:
//! "config"}` (or the `POST /outputs {op:"dmx-bus"}` helper) as anything
//! else. The bus itself is only the live merge a frame produces, like
//! [`crate::wire::SampledWire`], never saved.
//!
//! **One packet per universe per tick.** [`DmxBus::begin_tick`] starts a tick,
//! every [`DmxBus::write`] marks its universe touched, and [`DmxBus::flush`]
//! resolves each touched universe exactly once. A universe no source wrote
//! this tick is left alone (the caller's keep-alive covers it).

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use web_time::{Duration, Instant};

use crate::wire::DMX_SLOTS;

/// A sACN source that stops sending is dropped after this long — the
/// grandMA3 `sACN::OnDMX` data-loss window (report §6, `now - stored < 3000`).
pub const DATA_LOSS_TIMEOUT: Duration = Duration::from_secs(3);

/// sACN's own default priority (ANSI E1.31 §5.7: 100), the same number
/// [`crate::wire`] puts in every packet it builds.
pub const DEFAULT_PRIORITY: u8 = 100;

/// How a source's value takes a slot when several sources share the winning
/// priority. ShowXpress' per-input modes (report §3): `Htp` is a max-merge,
/// `Input` is "the physical/live input wins" and `Software` is "the
/// controller's value wins".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceMode {
    /// HTP: equal priorities merge per slot, keeping the maximum.
    #[default]
    Htp,
    /// The live/physical input wins the tie outright.
    Input,
    /// The controller's (software) value wins the tie outright.
    Software,
}

/// One source's placement on the bus before addresses are assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadPlacement {
    /// Stable id of the head (`bar.wash-1`, a node's head id): what a kept
    /// address is looked up by.
    pub id: String,
    /// The universe the head lives on.
    pub universe: u16,
    /// Its profile's own 1-based start address (or the fixture's override).
    pub address: u16,
    /// How many slots the head's profile spans.
    pub span: u16,
}

/// The one address allocator both [`crate::wire::venue_packets`] (a
/// `venue.fixtures` node) and `call::venue::write_fixtures` (a projector's
/// `out.sacn` node) route through, so the two agree on where each head lands.
///
/// A head a `kept` address already names keeps it (the operator edited it, or
/// an earlier projection assigned it); every other head packs after the
/// highest address in use on its own universe, never below its profile's own
/// start. Returns an address per placement, in the same order.
pub fn allocate_addresses(
    placements: &[HeadPlacement],
    kept: impl Fn(&str) -> Option<u16>,
) -> Vec<u16> {
    // First pass: every kept head reserves its span so a free head packs
    // after it, not on top of it.
    let mut next: HashMap<u16, u16> = HashMap::new();
    for p in placements {
        if let Some(a) = kept(&p.id) {
            let e = next.entry(p.universe).or_insert(0);
            *e = (*e).max(a.saturating_add(p.span));
        }
    }
    let mut out = Vec::with_capacity(placements.len());
    for p in placements {
        let a = match kept(&p.id) {
            Some(a) => a,
            None => {
                let e = next.entry(p.universe).or_insert(0);
                let a = (*e).max(p.address);
                *e = a.saturating_add(p.span);
                a
            }
        };
        out.push(a);
    }
    out
}

/// The priority and mode a source node writes with — a reading of its config
/// (rule 4), defaulting to sACN's own priority and HTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusSource {
    pub priority: u8,
    pub mode: SourceMode,
}

/// Reads `priority`/`mode` off a source node's own config, the same
/// "anything that does not parse is the default" reading
/// [`crate::wire::SacnConfig::from_node`] takes. `priority` is clamped to a
/// byte (a sACN priority field is one byte); `mode` is `"htp"`/`"input"`/
/// `"software"` (case-insensitive), anything else HTP.
pub fn bus_source(node: &metal_graph::Node) -> BusSource {
    let priority = node
        .config
        .get("priority")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(u8::MAX as u64) as u8)
        .unwrap_or(DEFAULT_PRIORITY);
    let mode = match node
        .config
        .get("mode")
        .and_then(|v| v.as_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("input") => SourceMode::Input,
        Some("software") => SourceMode::Software,
        _ => SourceMode::Htp,
    };
    BusSource { priority, mode }
}

/// One source's contribution to a universe: its full 512-slot shadow copy,
/// the priority and mode it wrote with, and when it last wrote.
struct Contribution {
    priority: u8,
    mode: SourceMode,
    slots: [u8; DMX_SLOTS],
    last_write: Instant,
}

/// One universe's sources.
#[derive(Default)]
struct Universe {
    sources: HashMap<String, Contribution>,
}

/// The shared DMX bus. One instance lives in `State` for the life of the
/// process; [`crate::wire::drive_sampled`] writes into it and flushes it.
#[derive(Default)]
pub struct DmxBus {
    universes: HashMap<u16, Universe>,
    /// Universes written since the last [`begin_tick`](Self::begin_tick), in
    /// insertion order — the set the next [`flush`](Self::flush) resolves.
    touched: BTreeSet<u16>,
    timeout: Duration,
}

impl DmxBus {
    pub fn new() -> DmxBus {
        DmxBus { universes: HashMap::new(), touched: BTreeSet::new(), timeout: DATA_LOSS_TIMEOUT }
    }

    /// The data-loss window sources are dropped after. Tests shrink it;
    /// production keeps the sACN 3 s.
    pub fn with_timeout(timeout: Duration) -> DmxBus {
        DmxBus { timeout, ..DmxBus::new() }
    }

    /// Starts a tick: no universe is touched until a [`write`](Self::write)
    /// marks one.
    pub fn begin_tick(&mut self) {
        self.touched.clear();
    }

    /// A source writes its slots into `universe`. The shadow copy is a full
    /// 512 slots, so a slot a source stops naming goes to zero for that
    /// source (and HTP-merges away if another source keeps it lit). Marks the
    /// universe touched for the next flush. A source already past the
    /// timeout is not "refreshed" — this call is what makes it live again.
    pub fn write(
        &mut self,
        universe: u16,
        source: &str,
        priority: u8,
        mode: SourceMode,
        slots: &[(u16, u8)],
        now: Instant,
    ) {
        let mut buf = [0u8; DMX_SLOTS];
        for &(address, byte) in slots {
            if (1..=DMX_SLOTS as u16).contains(&address) {
                buf[(address - 1) as usize] = byte;
            }
        }
        let u = self.universes.entry(universe).or_default();
        match u.sources.get_mut(source) {
            Some(c) => {
                c.priority = priority;
                c.mode = mode;
                c.slots = buf;
                c.last_write = now;
            }
            None => {
                u.sources.insert(
                    source.to_string(),
                    Contribution { priority, mode, slots: buf, last_write: now },
                );
            }
        }
        self.touched.insert(universe);
    }

    /// The universes a source wrote since [`begin_tick`](Self::begin_tick),
    /// sorted.
    pub fn touched(&self) -> Vec<u16> {
        self.touched.iter().copied().collect()
    }

    /// This universe's live contributions: everything written within the
    /// data-loss window, stale sources dropped. Empty when none are live.
    fn live<'a>(&'a self, universe: u16, now: Instant) -> Vec<&'a Contribution> {
        self.universes
            .get(&universe)
            .map(|u| {
                u.sources
                    .values()
                    .filter(|c| now.duration_since(c.last_write) < self.timeout)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The merged 512 slots of one universe at `now`, or `None` when no source
    /// is live. Highest priority wins; a tie between HTP sources merges per
    /// slot (max); a tie with an `Input` (or `Software`) source resolves to
    /// that source's value outright (the most recent one of the winning mode).
    pub fn resolve(&self, universe: u16, now: Instant) -> Option<[u8; DMX_SLOTS]> {
        let live = self.live(universe, now);
        if live.is_empty() {
            return None;
        }
        let top = live.iter().map(|c| c.priority).max().unwrap();
        let winners: Vec<&Contribution> = live.into_iter().filter(|c| c.priority == top).collect();
        if winners.len() == 1 {
            return Some(winners[0].slots);
        }
        // A mode decides a tie before HTP does: the latest Input wins, else
        // the latest Software, else the per-slot maximum.
        for wanted in [SourceMode::Input, SourceMode::Software] {
            if let Some(c) = winners
                .iter()
                .filter(|c| c.mode == wanted)
                .max_by_key(|c| c.last_write)
            {
                return Some(c.slots);
            }
        }
        let mut out = [0u8; DMX_SLOTS];
        for c in &winners {
            for (o, b) in out.iter_mut().zip(c.slots.iter()) {
                *o = (*o).max(*b);
            }
        }
        Some(out)
    }

    /// The sparse `(1-based address, byte)` pairs of a resolved universe —
    /// what a [`Wire`](crate::wire::Wire) takes. Zero slots are omitted: the
    /// packet builders always send all 512 and zero what is not named.
    pub fn sparse(bytes: &[u8; DMX_SLOTS]) -> Vec<(u16, u8)> {
        bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b != 0)
            .map(|(i, b)| ((i + 1) as u16, *b))
            .collect()
    }

    /// Resolves every touched universe and hands each to `send` **once**, in
    /// universe order. Returns the number of failed sends (the same
    /// count-a-drop-never-panic contract [`crate::wire::Wire::send`] keeps).
    /// Clears the touched set, so a second call in the same tick sends
    /// nothing.
    pub fn flush(
        &mut self,
        now: Instant,
        mut send: impl FnMut(u16, &[(u16, u8)]) -> bool,
    ) -> u64 {
        let touched = std::mem::take(&mut self.touched);
        let mut errors = 0u64;
        for universe in touched {
            let Some(bytes) = self.resolve(universe, now) else { continue };
            let slots = DmxBus::sparse(&bytes);
            if !send(universe, &slots) {
                errors += 1;
            }
        }
        // Drop sources that are long gone so the map does not grow forever.
        self.universes.retain(|_, u| {
            u.sources.retain(|_, c| now.duration_since(c.last_write) < self.timeout);
            !u.sources.is_empty()
        });
        errors
    }

    /// The live merged state of every universe, for `GET /venue` — a reading
    /// built fresh on every call, never a copy kept (rule 4).
    pub fn snapshot(&self, now: Instant, limit: usize) -> Vec<serde_json::Value> {
        let mut out: Vec<serde_json::Value> = self
            .universes
            .iter()
            .filter_map(|(&universe, u)| {
                let live: Vec<&Contribution> = u
                    .sources
                    .values()
                    .filter(|c| now.duration_since(c.last_write) < self.timeout)
                    .collect();
                if live.is_empty() {
                    return None;
                }
                let merged = self.resolve(universe, now)?;
                let top = live.iter().map(|c| c.priority).max().unwrap();
                let sources: Vec<serde_json::Value> = {
                    let mut names: Vec<(&String, &Contribution)> = u.sources.iter().collect();
                    names.sort_by_key(|(n, _)| n.as_str());
                    names
                        .iter()
                        .filter(|(_, c)| now.duration_since(c.last_write) < self.timeout)
                        .map(|(n, c)| {
                            serde_json::json!({
                                "source": n,
                                "priority": c.priority,
                                "mode": c.mode,
                                "live_ms_ago": now.duration_since(c.last_write).as_millis() as u64,
                            })
                        })
                        .collect()
                };
                Some(serde_json::json!({
                    "universe": universe,
                    "priority": top,
                    "sources": sources,
                    "slots": DmxBus::sparse(&merged),
                }))
            })
            .collect();
        out.sort_by_key(|v| v["universe"].as_u64().unwrap_or(0));
        out.truncate(limit);
        out
    }

    /// [`snapshot`](Self::snapshot) at the current clock — for a caller
    /// (`GET /venue`) that has no `Instant` of its own.
    pub fn snapshot_now(&self, limit: usize) -> Vec<serde_json::Value> {
        self.snapshot(Instant::now(), limit)
    }

    /// The number of universes currently carrying at least one live source.
    pub fn live_universes(&self, now: Instant) -> usize {
        self.universes
            .keys()
            .filter(|u| !self.live(**u, now).is_empty())
            .count()
    }

    /// The last packet **one source** wrote, as its own 512-slot shadow copy:
    /// `(universe, slots)`, or `None` when the source has not written or has
    /// gone stale. Unlike [`resolve`](Self::resolve) this is not the merged
    /// universe — it is what that source alone last put on the bus, which is
    /// what a fixture node's live swatch reads (F10/F11/F14), never a
    /// recomputation of the colour.
    pub fn source_packet(&self, universe: u16, source: &str, now: Instant) -> Option<(u16, [u8; DMX_SLOTS])> {
        let c = self.universes.get(&universe)?.sources.get(source)?;
        if now.duration_since(c.last_write) >= self.timeout {
            return None;
        }
        Some((universe, c.slots))
    }

    /// [`source_packet`](Self::source_packet) for a source whose key starts
    /// with `prefix`, searched across every universe: a venue fixture node's
    /// key is `venue/<graph>/<node>/<universe>`, so the caller knows the node
    /// but not which universe its profile put it on. The first live match in
    /// universe order wins — a node writes one universe.
    pub fn source_packet_for(&self, prefix: &str, now: Instant) -> Option<(u16, [u8; DMX_SLOTS])> {
        let mut universes: Vec<u16> = self
            .universes
            .iter()
            .filter(|(_, u)| u.sources.keys().any(|k| k.starts_with(prefix)))
            .map(|(u, _)| *u)
            .collect();
        universes.sort_unstable();
        for universe in universes {
            let u = &self.universes[&universe];
            for (source, c) in &u.sources {
                if source.starts_with(prefix) && now.duration_since(c.last_write) < self.timeout {
                    return Some((universe, c.slots));
                }
            }
        }
        None
    }

    /// [`source_packet_for`](Self::source_packet_for) at the current clock —
    /// for a caller (`GET /venue/fixture-packet`) that has no `Instant`.
    pub fn source_packet_for_now(&self, prefix: &str) -> Option<(u16, [u8; DMX_SLOTS])> {
        self.source_packet_for(prefix, Instant::now())
    }
}

#[cfg(test)]
mod bus_tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    /// Two sources on one universe: the higher priority owns a slot; a lower
    /// priority writing after it cannot overwrite it.
    #[test]
    fn bus_highest_priority_wins_over_a_later_lower_source() {
        let mut bus = DmxBus::new();
        let t = at(0);
        bus.begin_tick();
        bus.write(1, "a", 200, SourceMode::Htp, &[(1, 100), (2, 40)], t);
        bus.write(1, "b", 100, SourceMode::Htp, &[(1, 255), (3, 200)], at(1));
        let out = bus.resolve(1, at(2)).expect("both sources are live");
        assert_eq!(out[0], 100, "the higher-priority source owns slot 1");
        assert_eq!(out[1], 40, "its own slot 2");
        assert_eq!(out[2], 0, "the lower-priority source's slot 3 is not in the winning priority");
    }

    /// A tie between two HTP sources merges per slot, keeping the maximum.
    #[test]
    fn bus_tie_merges_htp_max_per_slot() {
        let mut bus = DmxBus::new();
        let t = at(0);
        bus.begin_tick();
        bus.write(2, "a", 100, SourceMode::Htp, &[(1, 100), (2, 30)], t);
        bus.write(2, "b", 100, SourceMode::Htp, &[(1, 60), (3, 200)], at(1));
        let out = bus.resolve(2, at(2)).unwrap();
        assert_eq!((out[0], out[1], out[2]), (100, 30, 200), "max per slot");
    }

    /// A tie with an `Input` source resolves to that source's value outright,
    /// not the HTP maximum (ShowXpress' `Input` mode).
    #[test]
    fn bus_input_mode_wins_a_tie() {
        let mut bus = DmxBus::new();
        let t = at(0);
        bus.begin_tick();
        bus.write(3, "desk", 100, SourceMode::Htp, &[(1, 255)], t);
        bus.write(3, "live", 100, SourceMode::Input, &[(1, 8)], at(1));
        let out = bus.resolve(3, at(2)).unwrap();
        assert_eq!(out[0], 8, "the live input wins the tie, not the max");
    }

    /// A source past the data-loss timeout is dropped: its slots go away even
    /// though nothing else was written.
    #[test]
    fn bus_drops_a_source_after_the_timeout() {
        let mut bus = DmxBus::with_timeout(Duration::from_secs(3));
        bus.begin_tick();
        bus.write(4, "a", 100, SourceMode::Htp, &[(1, 100)], at(0));
        assert!(bus.resolve(4, at(2_999)).is_some(), "still inside the window");
        assert_eq!(bus.resolve(4, at(3_000)), None, "3 s of silence drops it");
    }

    /// A timeout while another source is live leaves only the live source's
    /// contribution — the stale slots go dark.
    #[test]
    fn bus_a_stale_source_leaves_the_live_one() {
        let mut bus = DmxBus::with_timeout(Duration::from_millis(50));
        bus.begin_tick();
        bus.write(5, "stale", 100, SourceMode::Htp, &[(1, 200)], at(0));
        bus.write(5, "live", 100, SourceMode::Htp, &[(2, 30)], at(60));
        let out = bus.resolve(5, at(70)).unwrap();
        assert_eq!(out[0], 0, "the stale source's slot 1 went dark");
        assert_eq!(out[1], 30, "the live source's slot 2 stays");
    }

    /// The last packet one source wrote is its own shadow, not the merged
    /// universe, and a stale source answers nothing.
    #[test]
    fn bus_reads_one_sources_own_last_packet() {
        let mut bus = DmxBus::with_timeout(Duration::from_millis(50));
        bus.begin_tick();
        bus.write(1, "venue/g/f-0/1", 100, SourceMode::Htp, &[(1, 40)], at(0));
        bus.write(1, "venue/g/f-1/1", 100, SourceMode::Htp, &[(1, 90)], at(0));
        assert_eq!(
            bus.source_packet(1, "venue/g/f-0/1", at(1)).map(|(u, s)| (u, s[0])),
            Some((1, 40)),
            "f-0's own packet, not the merged max of 90"
        );
        assert_eq!(
            bus.source_packet_for("venue/g/f-1/", at(1)).map(|(u, s)| (u, s[0])),
            Some((1, 90)),
            "prefix finds the universe and the source's own slots"
        );
        assert!(bus.source_packet(1, "venue/g/f-0/1", at(60)).is_none(), "stale reads nothing");
    }

    /// The flush hands each touched universe to `send` exactly once per tick,
    /// however many sources wrote it.
    #[test]
    fn bus_flushes_one_packet_per_universe_per_tick() {
        let mut bus = DmxBus::new();
        bus.begin_tick();
        bus.write(9, "a", 100, SourceMode::Htp, &[(1, 5)], at(0));
        bus.write(9, "b", 100, SourceMode::Htp, &[(1, 9)], at(0));
        bus.write(10, "c", 100, SourceMode::Htp, &[(1, 1)], at(0));

        let mut sent: Vec<(u16, Vec<(u16, u8)>)> = Vec::new();
        let errors = bus.flush(at(1), |u, slots| {
            sent.push((u, slots.to_vec()));
            true
        });
        assert_eq!(errors, 0);
        assert_eq!(sent.len(), 2, "one packet for universe 9, one for 10");
        assert_eq!(sent[0].0, 9);
        assert_eq!(sent[0].1, vec![(1, 9)], "the merged max");
        assert_eq!(sent[1].0, 10);
        assert!(sent.iter().all(|(u, _)| *u != 9 || sent.iter().filter(|(x, _)| *x == 9).count() == 1));

        let errors = bus.flush(at(2), |_, _| panic!("a second flush in the tick sends nothing"));
        assert_eq!(errors, 0);
    }
}
