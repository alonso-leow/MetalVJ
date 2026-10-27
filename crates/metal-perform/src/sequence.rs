//! Sequences: which `Collection` a device shows, scheduled instead of chosen
//! by hand.
//!
//! `the design notes PARTE IV` §17.5, second step. The first step
//! (`layers::DeviceMode::Collection`, see that type) made picking a
//! collection from a device's own dropdown a *live* read of
//! `Collections::find` rather than a copy — a device left on one keeps
//! tracking edits made to the collection, with no reselect needed. That is
//! the "vivo" half of the ask: *"tocar la celda"* and the picture changes
//! now. This module is the "secuencial" half: a named, ordered timeline of
//! the same kind of statement — *this* collection, from *this* second to
//! *that* one — evaluated the same way `overlay::Overlays` evaluates a
//! movement: a pure function of `State::time()`, consulted at the point
//! `compile_device_plan` already resolves what a device is showing, never
//! written back to the device's own saved `mode`.
//!
//! # Reusing §16.7's mechanism rather than rebuilding it
//!
//! `overlay::Play`/`Overlays` already solved "run something scheduled
//! without a scheduler": record when it started, answer `at(time)` by
//! subtraction, do nothing when nothing is running. [`Run`]/[`Runs`] here are
//! that shape again — one device instead of one device *and* one layer (a
//! `Collection` replaces a whole stack; there is no per-layer target to
//! name), a span of entries instead of a per-tick row (a `Collection` has no
//! per-instant payload the way a movement's floats do, so there is nothing to
//! sample — only a start and an end to be inside of or not). The doc's own
//! words for why this has to be one mechanism: *"hay que reusarlo entero."*
//!
//! # Two triggers, one mechanism — the symmetry the ask names
//!
//! *"Vivo: tocar la celda de Overlays lo activa ahora. Secuencial: la pista
//! lo activa en su tick."* Both land on [`Runs::start`]:
//!
//! - **Live** ([`Run::live`]): one collection, one device, from now until
//!   stopped by hand. A single entry with no scheduled end (`end` is
//!   `f32::INFINITY`), so [`Run::is_over`] is never true and the operator's
//!   own "stop" is the only way off. Nothing here touches [`Sequences`] — a
//!   live cue is not saved, the same way tapping an overlay trigger does not
//!   write to the movement library.
//! - **Sequential** (the `"run"` op in `main.rs`): a saved [`Sequence`]'s
//!   entries, copied onto a device at the moment it is loaded — `started` is
//!   that instant, and every entry's `start`/`end` counts seconds from
//!   there, the same way a movement's ticks count from `Play::started`.
//!   Editing or re-saving the `Sequence` afterwards cannot change what is
//!   already scheduled, for the same reason renaming a movement mid-play
//!   cannot: the entries are a copy, taken once, at arm time.
//!
//! Neither trigger is a second scheduling system. Both produce exactly one
//! [`Run`], and every reader — `compile_device_plan`, `render_device`, the
//! `/state` report — asks [`Runs`] one question, "what does this device show
//! at this instant," without caring which trigger put the answer there. A
//! `Collection` remains a resource with no destination of its own; only a
//! `Run` (live or scheduled) says which device.
//!
//! # Two events, not a state — why a range does not linger
//!
//! [`Run::at`] answers `None` for any instant not covered by one of its
//! entries: before the first, in a gap between two, or after the last. A
//! `Collection` left selected past its scheduled range with nothing on
//! screen saying so is the silent-wrong-state bug class
//! `the design notes` §3.3 and §11.7 both name, applied to a whole stack
//! instead of one field. Entering and leaving a span are the two events the
//! doc asks for; nothing here ever holds a "currently showing" value between
//! them — it is recomputed from `started` and `entries` every time it is
//! asked, the same as `overlay::Play::at` recomputes a tick instead of
//! caching one.
//!
//! # The guard: rejected at load, named, not discovered on air
//!
//! [`check`] runs when a [`Sequence`] is saved *and* again when one is
//! loaded onto a device (`main.rs`'s `"run"` op) — the second check mirrors
//! `overlay.rs`'s own `"play"` op re-checking a movement's row width even
//! though `library::check` already ran at save time: a file can be
//! hand-edited, or a `Collection` it names can be deleted, in between. Both
//! paths reject by naming the missing collection, not by rendering black on
//! whichever frame first tries to read it.
//!
//! Deleting a `Collection` out from under an already-armed `Run` is *not*
//! guarded here — see the comment on the `"run"`/`"cue"` ops in `main.rs`
//! for why that is a deliberate, narrower decision than `State::orphan`'s.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::layers::Collections;

// ---------------------------------------------------------------------------
// the resource: a named, ordered timeline of collections
// ---------------------------------------------------------------------------

/// One span: `collection` is live on the device from `start` to `end`,
/// seconds on the sequence's own clock — zero is whenever the sequence is
/// loaded, not any wall-clock instant (see [`Run::started`]). `end` is
/// exclusive, so two entries may touch (`a.end == b.start`) without
/// overlapping.
///
/// Deliberately not "ticks" in `overlay::Play`'s sense: a movement's ticks
/// are eighths of a second because the payload underneath them (a model's
/// float row) was authored at that rate. A `Collection` has no per-instant
/// payload — it is the same subgraph for the whole span — so there is
/// nothing here for a sample rate to be *of*. Plain seconds,
/// `State::time()`'s own unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequenceEntry {
    pub collection: String,
    pub start: f32,
    pub end: f32,
}

/// A saved timeline — *"el lane como timeline de recursos"* — of
/// [`SequenceEntry`] spans. The "resource" the ask describes is the
/// `Collection` each entry names; this is the part the design notes PARTE IV §17.5
/// says is genuinely new: not a subgraph, a placement of subgraphs over
/// time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    pub id: String,
    pub name: String,
    pub entries: Vec<SequenceEntry>,
}

impl Sequence {
    pub fn describe(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "entries": self.entries.iter().map(|e| json!({
                "collection": e.collection,
                "start": e.start,
                "end": e.end,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Every saved sequence, and the file they live in.
///
/// A separate file from `data/collections.json`, for the same reason
/// `Collections` gives for being separate from `data/outputs.json`: a
/// sequence is authored content (which folder plays when, over a set) and a
/// `Collection` is a resource it points at by id. Keeping the two apart is
/// what lets a sequence travel between venues without dragging every
/// collection it names along, and what lets one collection be scheduled by
/// three different sequences without three copies of it.
#[derive(Default)]
pub struct Sequences {
    pub list: Vec<Sequence>,
}

fn sequences_path() -> std::path::PathBuf {
    crate::output::data_root().join("sequences.json")
}

impl Sequences {
    /// A missing or unreadable file is an empty library, not an error — the
    /// first save creates it, same discipline as `Collections::load` and
    /// `author::library::load`.
    pub fn load() -> Sequences {
        let Ok(text) = metal_vfs::read_to_string(sequences_path()) else {
            return Sequences::default();
        };
        Sequences {
            list: serde_json::from_str(&text).unwrap_or_default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = sequences_path();
        if let Some(dir) = path.parent() {
            metal_vfs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&self.list)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::atomic::write(path, format!("{text}\n"))
    }

    pub fn find(&self, id: &str) -> Option<&Sequence> {
        self.list.iter().find(|s| s.id == id)
    }

    pub fn describe(&self) -> Value {
        json!(self.list.iter().map(Sequence::describe).collect::<Vec<_>>())
    }
}

/// Checks a sequence is playable: real spans, naming collections that exist.
///
/// The guard the design notes PARTE IV §17.5 asks for, restated from `Collection` (a
/// subgraph naming a node that vanished) to `Sequence` (a timeline naming a
/// collection that vanished) — same shape as `author::library::check`: run
/// at save time and again at the door before a run is armed, named after the
/// exact thing that is wrong rather than "invalid sequence".
pub fn check(seq: &Sequence, collections: &Collections) -> Result<(), String> {
    if seq.name.trim().is_empty() {
        return Err("a sequence needs a name".into());
    }
    if seq.entries.is_empty() {
        return Err(format!("{}: has no entries in it", seq.name));
    }
    for (i, e) in seq.entries.iter().enumerate() {
        if collections.find(&e.collection).is_none() {
            return Err(format!(
                "{}: entry {i} names collection {:?}, which does not exist",
                seq.name, e.collection
            ));
        }
        if !e.start.is_finite() || !e.end.is_finite() {
            return Err(format!(
                "{}: entry {i} has a non-finite start or end",
                seq.name
            ));
        }
        if e.start < 0.0 {
            return Err(format!(
                "{}: entry {i} starts before zero ({})",
                seq.name, e.start
            ));
        }
        if e.end <= e.start {
            return Err(format!(
                "{}: entry {i} ends at {} at or before its own start {}",
                seq.name, e.end, e.start
            ));
        }
    }
    // Overlaps: a device shows one collection at a time, so two entries
    // covering the same instant would be a tie broken however `Run::at`'s
    // `find` happens to order them — exactly the "silent, in range, wrong"
    // shape this project refuses to ship. Sorted first so only neighbours
    // are compared; if two non-neighbours overlapped, a neighbour of each
    // would too, so this still catches it.
    let mut order: Vec<&SequenceEntry> = seq.entries.iter().collect();
    order.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for w in order.windows(2) {
        if w[1].start < w[0].end {
            return Err(format!(
                "{}: {:?} ({}..{}) overlaps {:?} ({}..{})",
                seq.name,
                w[0].collection,
                w[0].start,
                w[0].end,
                w[1].collection,
                w[1].start,
                w[1].end
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// the live registry: which device is running which sequence, right now
// ---------------------------------------------------------------------------

/// One sequence — or one live cue — running on one device.
pub struct Run {
    pub device: String,
    /// The sequence this was loaded from, by id — carried for the report
    /// only, same discipline as `overlay::Play::movement`. `None` for a live
    /// cue: one collection, armed by hand, with no library entry behind it.
    pub sequence: Option<String>,
    /// A copy, taken at arm time. Re-saving or deleting the `Sequence` this
    /// came from cannot change what is already scheduled — the same
    /// discipline `overlay::Play::ticks` applies to a movement.
    pub entries: Vec<SequenceEntry>,
    /// `State::time()` when this was armed, in the app's seconds.
    pub started: f32,
}

impl Run {
    /// A live cue: one collection, on one device, from now until stopped by
    /// hand — the "vivo" trigger. See the module doc for why this needs
    /// nothing from [`Sequences`].
    pub fn live(device: String, collection: String, started: f32) -> Run {
        Run {
            device,
            sequence: None,
            entries: vec![SequenceEntry {
                collection,
                start: 0.0,
                end: f32::INFINITY,
            }],
            started,
        }
    }

    /// The collection live at `time`, or `None` before the first entry, in a
    /// gap between two, or after the last — the two-events-not-a-state rule
    /// the module doc opens with.
    ///
    /// A clock that has gone backwards under a running sequence reads as
    /// nothing scheduled yet rather than as an error, the same call
    /// `overlay::Play::at` makes and for the same reason: `State::time` is
    /// monotonic across pause and resume, but a render loop is not where to
    /// find out if that ever stops being true.
    pub fn at(&self, time: f32) -> Option<&str> {
        let elapsed = time - self.started;
        if elapsed < 0.0 {
            return None;
        }
        self.entries
            .iter()
            .find(|e| elapsed >= e.start && elapsed < e.end)
            .map(|e| e.collection.as_str())
    }

    /// True once every entry is behind us. A live cue's one entry never ends
    /// (`end` is infinite), so this is never true for one — only the
    /// operator's own "stop" takes it off.
    fn is_over(&self, time: f32) -> bool {
        let elapsed = time - self.started;
        self.entries.iter().all(|e| elapsed >= e.end)
    }

    fn describe(&self, time: f32) -> Value {
        json!({
            "device": self.device,
            "sequence": self.sequence,
            "collection": self.at(time),
            "entries": self.entries.len(),
        })
    }
}

/// Every sequence in the air, at most one per device — a device shows one
/// collection at a time, so a second load or cue replaces rather than
/// stacks, the same rule `overlay::Overlays::start` applies per layer.
///
/// In the air only, never on disk — the same choice `engine.rs` documents for
/// `State::overlays` and for the same reason: a run armed during a set is a
/// gesture, and an app that came back up still forcing a device onto a
/// collection an hour later would be restoring a mistake, not a set.
#[derive(Default)]
pub struct Runs {
    running: Vec<Run>,
}

impl Runs {
    /// Arms a run, replacing whatever that device had.
    pub fn start(&mut self, run: Run) {
        self.running.retain(|r| r.device != run.device);
        self.running.push(run);
    }

    /// Takes one device's run off. True if there was one.
    pub fn stop(&mut self, device: &str) -> bool {
        let before = self.running.len();
        self.running.retain(|r| r.device != device);
        before != self.running.len()
    }

    /// The off switch. Everything, everywhere, at once — the same reason
    /// `overlay::Overlays::stop_all` exists: one gesture back to the stacks
    /// the operator knows, rather than remembering which devices were armed.
    pub fn stop_all(&mut self) -> usize {
        let n = self.running.len();
        self.running.clear();
        n
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    /// The collection `device` should show at `time`, if a run is on it and
    /// one of its entries covers this instant. `compile_device_plan`'s only
    /// question, and `render_device`'s composition lookup asks the
    /// identical one, so the two cannot resolve to different collections.
    pub fn active(&self, device: &str, time: f32) -> Option<&str> {
        self.running.iter().find(|r| r.device == device)?.at(time)
    }

    /// Drops runs that are over, and ones whose device is gone. See
    /// `overlay::Overlays::prune` — same bookkeeping, called from the same
    /// place in `main.rs`, for the same reason: `active`/`at` already answer
    /// `None` for a finished or orphaned run, so nothing is ever drawn from
    /// something this has not got to yet.
    pub fn prune(&mut self, time: f32, alive: impl Fn(&str) -> bool) {
        self.running
            .retain(|r| !r.is_over(time) && alive(&r.device));
    }

    /// What is in the air, for `/state`.
    pub fn report(&self, time: f32) -> Value {
        json!(self
            .running
            .iter()
            .map(|r| r.describe(time))
            .collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::{Collection, Composition};

    fn collections(ids: &[&str]) -> Collections {
        Collections {
            list: ids
                .iter()
                .map(|id| Collection {
                    id: id.to_string(),
                    name: id.to_string(),
                    layers: Vec::new(),
                    composition: Composition::default(),
                })
                .collect(),
        }
    }

    fn seq(entries: Vec<(&str, f32, f32)>) -> Sequence {
        Sequence {
            id: "s1".into(),
            name: "s1".into(),
            entries: entries
                .into_iter()
                .map(|(c, start, end)| SequenceEntry {
                    collection: c.into(),
                    start,
                    end,
                })
                .collect(),
        }
    }

    // -- check ----------------------------------------------------------

    #[test]
    fn check_rejects_a_missing_collection_by_name() {
        let cols = collections(&["a"]);
        let s = seq(vec![("a", 0.0, 4.0), ("ghost", 4.0, 8.0)]);
        let err = check(&s, &cols).unwrap_err();
        assert!(
            err.contains("ghost"),
            "the error must name the missing collection: {err}"
        );
    }

    #[test]
    fn check_accepts_a_sequence_whose_entries_all_name_real_collections() {
        let cols = collections(&["a", "b"]);
        let s = seq(vec![("a", 0.0, 4.0), ("b", 4.0, 8.0)]);
        assert!(check(&s, &cols).is_ok());
    }

    #[test]
    fn check_rejects_an_empty_sequence() {
        let cols = collections(&["a"]);
        let s = Sequence {
            id: "s1".into(),
            name: "s1".into(),
            entries: Vec::new(),
        };
        assert!(check(&s, &cols).is_err());
    }

    #[test]
    fn check_rejects_an_entry_that_ends_at_or_before_it_starts() {
        let cols = collections(&["a"]);
        let s = seq(vec![("a", 4.0, 4.0)]);
        assert!(
            check(&s, &cols).is_err(),
            "end equal to start is not a real span"
        );
    }

    #[test]
    fn check_rejects_two_entries_that_overlap() {
        let cols = collections(&["a", "b"]);
        let s = seq(vec![("a", 0.0, 5.0), ("b", 4.0, 8.0)]);
        let err = check(&s, &cols).unwrap_err();
        assert!(
            err.contains("\"a\"") && err.contains("\"b\""),
            "must name both overlapping entries: {err}"
        );
    }

    #[test]
    fn check_accepts_entries_authored_out_of_order_as_long_as_they_do_not_overlap() {
        let cols = collections(&["a", "b"]);
        // "b" authored first in the list but scheduled second in time.
        let s = seq(vec![("b", 4.0, 8.0), ("a", 0.0, 4.0)]);
        assert!(check(&s, &cols).is_ok());
    }

    #[test]
    fn check_accepts_two_entries_that_only_touch_at_the_boundary() {
        let cols = collections(&["a", "b"]);
        let s = seq(vec![("a", 0.0, 4.0), ("b", 4.0, 8.0)]);
        assert!(
            check(&s, &cols).is_ok(),
            "a.end == b.start is a handoff, not an overlap"
        );
    }

    // -- Run::at: entering and exiting a span ----------------------------

    /// The acceptance shape the design notes PARTE IV §17.5 asks for: a range of ticks
    /// is two events. Before the first entry, inside each, in the gap
    /// between two, and after the last — five instants, four different
    /// answers.
    #[test]
    fn a_run_enters_and_exits_each_entry_and_answers_nothing_in_a_gap() {
        let run = Run {
            device: "preview".into(),
            sequence: Some("s1".into()),
            entries: vec![
                SequenceEntry {
                    collection: "a".into(),
                    start: 0.0,
                    end: 4.0,
                },
                SequenceEntry {
                    collection: "b".into(),
                    start: 6.0,
                    end: 10.0,
                },
            ],
            started: 100.0,
        };
        assert_eq!(run.at(99.0), None, "before the run was even armed");
        assert_eq!(
            run.at(100.0),
            Some("a"),
            "the instant it enters the first entry"
        );
        assert_eq!(run.at(103.999), Some("a"), "still inside the first entry");
        assert_eq!(
            run.at(104.0),
            None,
            "the instant it exits — exclusive of end"
        );
        assert_eq!(run.at(105.0), None, "the gap between entries shows nothing");
        assert_eq!(
            run.at(106.0),
            Some("b"),
            "the instant it enters the second entry"
        );
        assert_eq!(run.at(109.999), Some("b"));
        assert_eq!(run.at(110.0), None, "past the last entry, for good");
        assert_eq!(
            run.at(999.0),
            None,
            "and it stays gone — a one-shot, not a hold"
        );
    }

    #[test]
    fn a_live_cue_holds_from_the_instant_it_is_armed_until_stopped() {
        let run = Run::live("preview".into(), "night".into(), 10.0);
        assert_eq!(run.at(9.999), None, "not armed yet");
        assert_eq!(run.at(10.0), Some("night"));
        assert_eq!(
            run.at(1e6),
            Some("night"),
            "no scheduled end — only a stop takes it off"
        );
    }

    #[test]
    fn a_clock_that_went_backwards_reads_as_nothing_scheduled_yet() {
        let run = Run {
            device: "preview".into(),
            sequence: None,
            entries: vec![SequenceEntry {
                collection: "a".into(),
                start: 0.0,
                end: 4.0,
            }],
            started: 100.0,
        };
        assert_eq!(run.at(0.0), None);
        assert_eq!(run.at(-50.0), None);
    }

    // -- Runs: start/stop/replace/prune/report ---------------------------

    #[test]
    fn one_device_holds_one_run_and_a_second_start_replaces_the_first() {
        let mut runs = Runs::default();
        runs.start(Run::live("preview".into(), "a".into(), 0.0));
        runs.start(Run::live("preview".into(), "b".into(), 0.0));
        assert_eq!(
            runs.active("preview", 0.0),
            Some("b"),
            "the second start won"
        );
        assert_eq!(
            runs.report(0.0).as_array().unwrap().len(),
            1,
            "one run, not two"
        );
    }

    #[test]
    fn two_devices_run_independently() {
        let mut runs = Runs::default();
        runs.start(Run::live("preview".into(), "a".into(), 0.0));
        runs.start(Run::live("wall".into(), "b".into(), 0.0));
        assert_eq!(runs.active("preview", 0.0), Some("a"));
        assert_eq!(runs.active("wall", 0.0), Some("b"));
    }

    #[test]
    fn stopping_takes_off_one_device_and_the_off_switch_takes_off_all() {
        let mut runs = Runs::default();
        runs.start(Run::live("preview".into(), "a".into(), 0.0));
        runs.start(Run::live("wall".into(), "b".into(), 0.0));

        assert!(runs.stop("preview"));
        assert!(
            !runs.stop("preview"),
            "stopping what is not running is false, not an error"
        );
        assert_eq!(runs.active("preview", 0.0), None);
        assert_eq!(
            runs.active("wall", 0.0),
            Some("b"),
            "the other device is untouched"
        );

        assert_eq!(runs.stop_all(), 1);
        assert!(runs.is_empty());
    }

    #[test]
    fn a_finished_run_answers_nothing_before_anything_prunes_it() {
        let mut runs = Runs::default();
        runs.start(Run {
            device: "preview".into(),
            sequence: Some("s1".into()),
            entries: vec![SequenceEntry {
                collection: "a".into(),
                start: 0.0,
                end: 4.0,
            }],
            started: 0.0,
        });
        assert_eq!(runs.active("preview", 99.0), None, "over is over, unpruned");
        assert_eq!(
            runs.report(99.0).as_array().unwrap().len(),
            1,
            "still listed until pruned"
        );
        runs.prune(99.0, |_| true);
        assert!(runs.is_empty());
    }

    #[test]
    fn a_run_on_a_device_that_no_longer_exists_is_pruned_even_mid_run() {
        let mut runs = Runs::default();
        runs.start(Run::live("preview".into(), "a".into(), 0.0));
        runs.prune(1.0, |_| true);
        assert!(!runs.is_empty(), "a running cue on a live device stays");
        runs.prune(1.0, |_| false);
        assert!(
            runs.is_empty(),
            "a running cue on a device that is gone does not"
        );
    }

    #[test]
    fn a_live_cue_is_never_pruned_for_being_over() {
        let mut runs = Runs::default();
        runs.start(Run::live("preview".into(), "a".into(), 0.0));
        runs.prune(1e9, |_| true);
        assert!(
            !runs.is_empty(),
            "no scheduled end means prune must never call this one over"
        );
    }

    #[test]
    fn report_names_the_sequence_for_a_scheduled_run_and_nothing_for_a_live_cue() {
        let mut runs = Runs::default();
        runs.start(Run {
            device: "preview".into(),
            sequence: Some("s1".into()),
            entries: vec![SequenceEntry {
                collection: "a".into(),
                start: 0.0,
                end: 4.0,
            }],
            started: 0.0,
        });
        runs.start(Run::live("wall".into(), "b".into(), 0.0));
        let report = runs.report(1.0);
        let by_device = |d: &str| {
            report
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["device"] == d)
                .unwrap()
                .clone()
        };
        assert_eq!(by_device("preview")["sequence"], json!("s1"));
        assert_eq!(by_device("preview")["collection"], json!("a"));
        assert_eq!(by_device("wall")["sequence"], json!(null));
        assert_eq!(by_device("wall")["collection"], json!("b"));
    }
}
