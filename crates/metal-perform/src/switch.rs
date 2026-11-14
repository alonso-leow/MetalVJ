//! S.AK / O.28: a grid button that turns one or a batch of nodes on or off.
//!
//! `plan-opus.md § O.28`'s decision, restated: **a switch is a [`Trigger`]
//! whose target is a set of `(node, on|off)`, and what it writes is
//! `Node.bypass`** — the field that already exists, already saves to
//! `data/graphs/*.json`, and is already honoured by `compose`/`engine`.
//! Nothing new in the engine: a node turned off is a node with `bypass:
//! true`; a strobo "that does not exist" is a strobo bypassed until the
//! button lets it back in. Two shapes fall out of that one decision:
//!
//! - **`toggle`** (a mood): has a latch. Pressing it applies `set` when it
//!   was off, `unset` when it was on.
//! - **`hold`** (a strobo): applies `set` on press, `unset` on release —
//!   the button *is* the state, not a memory of one.
//!
//! # Regla 4: on/off is a read of the graph, never stored
//!
//! [`Switch::state`] computes whether a switch is "on" by comparing every
//! entry in `set` against the node's current `bypass` in the live graph. A
//! switch that flips a node by hand (the properties panel, another switch
//! that shares a node) is read correctly the very next time anyone asks —
//! there is no separate "is this switch on" bit to fall out of sync with
//! the graph it describes, the same discipline `sequence::Runs::active`
//! applies to a running sequence's own collection.
//!
//! # One save, not one per node
//!
//! [`apply`] writes every entry of a batch to one in-memory [`metal_graph::Graph`]
//! and the caller saves once (`engine::save_graph`) — never once per node.
//! A mood with six entries that saved node-by-node would let a reader (this
//! app's own render loop, `metal-sync` mid-sync) observe a half-flipped mood:
//! three models cut over, three still on the old side. `atomic::write`
//! already makes one save torn-proof; this is what keeps a *batch* of
//! writes from being torn against each other.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use metal_graph::Graph;

/// `toggle` latches; `hold` is only on while held. `fixture`/`rig` (G1) are
/// the grid's show-control buttons: `fixture` names one `venue.fixtures`
/// fixture, `rig` a `Fixture.group`; both fire the same `set`/`unset` batch
/// over the `venue.fixtures` node that drives them, so the whole switch
/// mechanism (state read off `bypass`, one save, `switch-<id>` triggers) is
/// untouched — only the label and the recorded target differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwitchKind {
    Toggle,
    Hold,
    Fixture,
    Rig,
}

impl SwitchKind {
    /// The tag `describe` emits and `parse_kind` accepts.
    pub fn word(self) -> &'static str {
        match self {
            SwitchKind::Toggle => "toggle",
            SwitchKind::Hold => "hold",
            SwitchKind::Fixture => "fixture",
            SwitchKind::Rig => "rig",
        }
    }
}

/// G3: how a fixture/rig button fires. `switch` latches — a click flips it
/// (`POST /switch {op:on|off}`), exactly the `SwitchKind::Toggle` gesture a
/// mood uses; `hold` applies the value only while the button is pressed (the
/// press edge posts `on`, the release `off`), the `SwitchKind::Hold` gesture a
/// strobo uses. The pair is a *mode* beside the target family, because a
/// fixture and a rig each need both — a `SwitchKind::Hold` cannot say "hold
/// this fixture" without losing which fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ButtonMode {
    /// A click latches the button on/off (toggle semantics).
    #[default]
    Switch,
    /// Applies only while pressed (hold semantics).
    Hold,
}

impl ButtonMode {
    /// The tag `describe` emits and the grid reads.
    pub fn word(self) -> &'static str {
        match self {
            ButtonMode::Switch => "switch",
            ButtonMode::Hold => "hold",
        }
    }
}

/// One node this switch touches, and which side of `bypass` it wants —
/// `on: true` means "not bypassed", the same sense `Node.bypass` itself
/// reads backwards from.
///
/// An entry may also carry a float-parameter assignment (`param`/`value`):
/// the grid saves it so it can mark the parameter as already assigned and
/// render a dial cell. It is purely recorded — [`apply`] still writes
/// `bypass` as it always has (see its own note) — so an on/off entry without
/// a parameter serializes exactly as before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SwitchEntry {
    pub node: String,
    pub on: bool,
    /// The float parameter this entry also holds, when it is a parameter
    /// assignment rather than a plain on/off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    /// The value [`SwitchEntry::param`] holds for `param`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f32>,
}

impl SwitchEntry {
    /// A plain on/off entry: `on: true` means "not bypassed".
    pub fn node(node: impl Into<String>, on: bool) -> SwitchEntry {
        SwitchEntry {
            node: node.into(),
            on,
            param: None,
            value: None,
        }
    }

    /// A float-parameter entry: the button holds `value` for `param` of
    /// `node`. `on: true` is what [`apply`]'s existing `bypass` write expects,
    /// so a parameter entry rides along in the same batch as every on/off one.
    pub fn param(node: impl Into<String>, param: impl Into<String>, value: f32) -> SwitchEntry {
        SwitchEntry {
            node: node.into(),
            on: true,
            param: Some(param.into()),
            value: Some(value),
        }
    }

    /// This entry as `describe` emits it — `node`/`on` always, `param`/`value`
    /// only when held, so a pre-param entry's JSON is byte-identical.
    fn json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({ "node": self.node, "on": self.on }))
    }
}

/// A saved switch — `data/switches.json`'s one entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Switch {
    pub id: String,
    pub name: String,
    pub kind: SwitchKind,
    pub graph: String,
    /// Applied on `on` (toggle) or press (hold).
    pub set: Vec<SwitchEntry>,
    /// Applied on `off` (toggle) or release (hold). `None` means "the
    /// inverse of `set`" — same nodes, `on` flipped — which is enough for
    /// every switch this lote's two shots need (a mood's B side, a strobo's
    /// absence) and is spelled out explicitly only when a switch needs a
    /// third state `set`'s inverse cannot reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unset: Option<Vec<SwitchEntry>>,
    /// G1: the `venue.fixtures` fixture a `kind = "fixture"` button names —
    /// the venue `Fixture.id` its label and options come from. `None` for
    /// every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture: Option<String>,
    /// G1: the `Fixture.group` a `kind = "rig"` button drives as one. `None`
    /// for every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// G3: the fixture/rig button's own mode — `None` for a node switch (whose
    /// `SwitchKind::Toggle`/`Hold` already is the mode) and for every button
    /// saved before G3. Omitted from JSON when absent, so a pre-G3 file
    /// round-trips byte-identically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ButtonMode>,
}

impl Switch {
    /// What `off` applies: `unset` verbatim, or `set` with every `on`
    /// flipped when there is none.
    pub fn off_entries(&self) -> Vec<SwitchEntry> {
        match &self.unset {
            Some(u) => u.clone(),
            None => self
                .set
                .iter()
                .map(|e| SwitchEntry {
                    node: e.node.clone(),
                    on: !e.on,
                    param: e.param.clone(),
                    value: e.value,
                })
                .collect(),
        }
    }

    /// Is this switch on, read from `g`'s own `bypass` fields — regla 4.
    /// A node named in `set` that no longer exists in the graph counts as
    /// "not matching `set`" (so the switch reads off), the same way a
    /// deleted node reads as absent everywhere else rather than panicking a
    /// reader. An empty `set` is never on — there is nothing for it to be
    /// on *of*.
    pub fn state(&self, g: &Graph) -> bool {
        if self.set.is_empty() {
            return false;
        }
        self.set
            .iter()
            .all(|e| g.node(&e.node).is_some_and(|n| n.bypass != e.on))
    }

    /// G3: whether this button fires on the press/release edge (hold
    /// semantics) rather than a click (toggle/latch semantics). A node switch
    /// keeps its own `SwitchKind`; a fixture/rig button reads its `mode`.
    /// `None` — pre-G3, or a node switch — latches, the safe default a saved
    /// grid button had before this field existed.
    pub fn holds(&self) -> bool {
        self.mode == Some(ButtonMode::Hold)
    }

    pub fn describe(&self, g: Option<&Graph>) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "kind": self.kind.word(),
            "graph": self.graph,
            "set": self.set.iter().map(SwitchEntry::json).collect::<Vec<_>>(),
            "unset": self.unset.as_ref().map(|u| u.iter().map(SwitchEntry::json).collect::<Vec<_>>()),
            "fixture": self.fixture,
            "group": self.group,
            "mode": self.mode.map(ButtonMode::word),
            "on": g.map(|g| self.state(g)).unwrap_or(false),
        })
    }
}

/// Writes every entry of a batch onto `g`'s own nodes' `bypass` field — the
/// single function `S.AK` point 2 asks `graph.rs`'s `"bypass"` body to be
/// extracted to, so `POST /switch {op: on|off}` and `POST /graph/node
/// {op:"bypass"}` share one mutator instead of two copies drifting apart.
/// A `SwitchEntry` naming a node the graph does not have is skipped, not an
/// error — the caller (a switch whose graph was edited since) still wants
/// every *other* entry applied in this one save rather than none of them.
///
/// A parameter entry (`param`/`value`) rides along for the record only:
/// writing a held float needs the node catalog and `metal_graph::params::
/// set_held`, neither of which this batch mutator is handed, so it stays the
/// one `bypass` writer it always was and does not guess at a parameter. The
/// entry's `on: true` still un-bypasses its node, which is safe and visible;
/// the assignment itself is what the grid reads back through `describe`.
pub fn apply(g: &mut Graph, entries: &[SwitchEntry]) {
    for e in entries {
        if let Some(n) = g.node_mut(&e.node) {
            n.bypass = !e.on;
        }
    }
}

/// Every saved switch, and the file they live in (`data/switches.json`,
/// synced by allowlist — see `metal-sync::manifest::Allowlist ([sync].allowseed)`).
#[derive(Default)]
pub struct Switches {
    pub list: Vec<Switch>,
}

fn switches_path() -> std::path::PathBuf {
    crate::output::data_root().join("switches.json")
}

impl Switches {
    /// A missing or unreadable file is an empty library, not an error —
    /// same discipline as `sequence::Sequences::load`.
    pub fn load() -> Switches {
        let Ok(text) = metal_vfs::read_to_string(switches_path()) else {
            return Switches::default();
        };
        Switches {
            list: serde_json::from_str(&text).unwrap_or_default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = switches_path();
        if let Some(dir) = path.parent() {
            metal_vfs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&self.list)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::atomic::write(path, format!("{text}\n"))
    }

    pub fn find(&self, id: &str) -> Option<&Switch> {
        self.list.iter().find(|s| s.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Switch> {
        self.list.iter_mut().find(|s| s.id == id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|s| s.id != id);
        before != self.list.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::{Domain, Graph};

    fn graph_with(nodes: &[(&str, bool)]) -> Graph {
        let mut g = Graph::new("g1", "Test", Domain::Live);
        for (id, bypass) in nodes {
            let nid = g.add_node("n", "gen.turing", [0.0, 0.0]);
            // add_node mints its own id; rename by direct field write so the
            // fixture can use the caller's chosen id.
            if let Some(n) = g.node_mut(&nid) {
                n.id = id.to_string();
                n.bypass = *bypass;
            }
        }
        g
    }

    fn entries(pairs: &[(&str, bool)]) -> Vec<SwitchEntry> {
        pairs
            .iter()
            .map(|(node, on)| SwitchEntry::node(*node, *on))
            .collect()
    }

    fn mood() -> Switch {
        Switch {
            id: "mood-1".into(),
            name: "mood".into(),
            kind: SwitchKind::Toggle,
            graph: "g1".into(),
            set: entries(&[("a", true), ("b", true), ("c", false)]),
            unset: Some(entries(&[("a", false), ("b", false), ("c", true)])),
            fixture: None,
            group: None,
            mode: None,
        }
    }

    fn strobo() -> Switch {
        Switch {
            id: "strobo-1".into(),
            name: "strobo".into(),
            kind: SwitchKind::Hold,
            graph: "g1".into(),
            set: entries(&[("s", true)]),
            unset: None,
            fixture: None,
            group: None,
            mode: None,
        }
    }

    /// G1: a `fixture` button over the `venue.fixtures` node that drives it —
    /// the target is still a plain node entry, so every switch test above
    /// covers its firing too.
    fn fixture_button() -> Switch {
        Switch {
            id: "fx-1".into(),
            name: "wash 1".into(),
            kind: SwitchKind::Fixture,
            graph: "g1".into(),
            set: entries(&[("venue-1", true)]),
            unset: None,
            fixture: Some("wash-1".into()),
            group: None,
            mode: None,
        }
    }

    /// G3: a `fixture` button in `hold` mode — the value applies only while
    /// pressed. The target and firing node are the same as [`fixture_button`];
    /// only when `on`/`off` are posted differs, and that is the grid's job.
    fn hold_fixture_button() -> Switch {
        Switch {
            id: "fx-hold-1".into(),
            name: "wash 1 (hold)".into(),
            kind: SwitchKind::Fixture,
            graph: "g1".into(),
            set: entries(&[("venue-1", true)]),
            unset: None,
            fixture: Some("wash-1".into()),
            group: None,
            mode: Some(ButtonMode::Hold),
        }
    }

    /// G1: a `rig` button driving a whole `Fixture.group`.
    fn rig_button() -> Switch {
        Switch {
            id: "rig-1".into(),
            name: "front wash".into(),
            kind: SwitchKind::Rig,
            graph: "g1".into(),
            set: entries(&[("venue-1", true)]),
            unset: None,
            fixture: None,
            group: Some("front-wash".into()),
            mode: None,
        }
    }

    #[test]
    fn a_toggle_switch_writes_every_bypass_in_one_save() {
        let mut g = graph_with(&[("a", true), ("b", true), ("c", false)]);
        let sw = mood();
        apply(&mut g, &sw.set);
        assert!(!g.node("a").unwrap().bypass);
        assert!(!g.node("b").unwrap().bypass);
        assert!(g.node("c").unwrap().bypass);
        assert!(sw.state(&g), "every `set` entry now matches — the switch reads on");
    }

    #[test]
    fn a_hold_switch_unsets_on_release() {
        let mut g = graph_with(&[("s", true)]);
        let sw = strobo();
        apply(&mut g, &sw.set);
        assert!(sw.state(&g), "pressed: the strobo exists");
        apply(&mut g, &sw.off_entries());
        assert!(!sw.state(&g), "released: bypassed again, costs nothing");
        assert!(g.node("s").unwrap().bypass);
    }

    #[test]
    fn switch_state_is_read_from_the_graph_not_stored() {
        let mut g = graph_with(&[("a", true), ("b", true), ("c", false)]);
        let sw = mood();
        assert!(!sw.state(&g), "graph starts on the B side, not the A side `set` names");
        apply(&mut g, &sw.set);
        assert!(sw.state(&g));
        // Flip one node by hand, outside the switch entirely (properties
        // panel, another switch sharing the node) — the very next read
        // must see it, because there is no cached "on" bit to be stale.
        g.node_mut("a").unwrap().bypass = true;
        assert!(!sw.state(&g), "one entry now disagrees with the live graph");
    }

    #[test]
    fn firing_a_switch_trigger_with_state_off_turns_it_off() {
        let mut g = graph_with(&[("a", true), ("b", true), ("c", false)]);
        let sw = mood();
        apply(&mut g, &sw.set);
        assert!(sw.state(&g));
        apply(&mut g, &sw.off_entries());
        assert!(!sw.state(&g));
        assert!(g.node("a").unwrap().bypass);
        assert!(g.node("b").unwrap().bypass);
        assert!(!g.node("c").unwrap().bypass);
    }

    #[test]
    fn a_bypassed_node_is_not_rendered() {
        // The scaling condition O.28 fixes: a strobo held off is a node
        // with `bypass: true`, and `Node::new` — every node's starting
        // point — already defaults to bypassed-off. `compile`/`compose`
        // reading `bypass` to skip rendering is `metal-graph`/`metal-perform`'s
        // own existing contract; this asserts the one new fact this lote
        // adds to it: a switch never leaves a node in an ambiguous state
        // between "on" and "off" — it is always exactly one bool.
        let mut g = graph_with(&[("s", true)]);
        let sw = strobo();
        assert!(g.node("s").unwrap().bypass, "starts off — free to compose");
        apply(&mut g, &sw.set);
        assert!(!g.node("s").unwrap().bypass);
        apply(&mut g, &sw.off_entries());
        assert!(g.node("s").unwrap().bypass, "released — bypassed, and cheap again");
    }

    #[test]
    fn off_entries_defaults_to_the_inverse_of_set_when_unset_is_absent() {
        let sw = strobo();
        assert_eq!(sw.off_entries(), entries(&[("s", false)]));
    }

    #[test]
    fn a_missing_node_in_set_is_skipped_not_a_panic() {
        let mut g = graph_with(&[("a", true)]);
        apply(&mut g, &entries(&[("a", true), ("ghost", true)]));
        assert!(!g.node("a").unwrap().bypass);
        assert!(g.node("ghost").is_none());
    }

    #[test]
    fn a_fixture_switch_fires_its_venue_node_and_keeps_its_fixture_name() {
        let mut g = graph_with(&[("venue-1", true)]);
        let sw = fixture_button();
        assert_eq!(sw.kind.word(), "fixture");
        assert_eq!(sw.fixture.as_deref(), Some("wash-1"));
        apply(&mut g, &sw.set);
        assert!(sw.state(&g), "the venue node it drives is on");
        apply(&mut g, &sw.off_entries());
        assert!(!sw.state(&g));
        let described = sw.describe(Some(&g));
        assert_eq!(described["kind"], "fixture");
        assert_eq!(described["fixture"], "wash-1");
    }

    #[test]
    fn a_rig_switch_round_trips_its_group_through_json() {
        let sw = rig_button();
        let json = serde_json::to_value(&sw).unwrap();
        assert_eq!(json["kind"], "rig");
        assert_eq!(json["group"], "front-wash");
        assert_eq!(json["fixture"], serde_json::Value::Null);
        let restored: Switch = serde_json::from_value(json).unwrap();
        assert_eq!(restored, sw);
    }

    #[test]
    fn a_node_switch_omits_the_fixture_and_group_keys() {
        let json = serde_json::to_value(mood()).unwrap();
        assert!(json.get("fixture").is_none(), "a node switch carries no fixture");
        assert!(json.get("group").is_none(), "a node switch carries no group");
        assert!(json.get("mode").is_none(), "a node switch carries no button mode");
    }

    /// G3: a fixture/rig button's `hold`/`switch` mode round-trips, and the two
    /// fire the same `set`/`off_entries` batch a toggle/hold already does — the
    /// mode only decides *when* the grid posts `on`/`off`.
    #[test]
    fn a_fixture_button_mode_round_trips_and_reads_hold() {
        let latched = fixture_button();
        assert!(!latched.holds(), "no mode is the latch a pre-G3 button had");
        let held = hold_fixture_button();
        assert!(held.holds());
        let json = serde_json::to_value(&held).unwrap();
        assert_eq!(json["kind"], "fixture");
        assert_eq!(json["fixture"], "wash-1");
        assert_eq!(json["mode"], "hold");
        let restored: Switch = serde_json::from_value(json).unwrap();
        assert_eq!(restored, held);
        // The batch is the same one a latch posts; hold only moves the edges.
        let mut g = graph_with(&[("venue-1", true)]);
        apply(&mut g, &held.set);
        assert!(held.state(&g), "pressed: the venue node is on");
        apply(&mut g, &held.off_entries());
        assert!(!held.state(&g), "released: off again");
        let described = held.describe(Some(&g));
        assert_eq!(described["mode"], "hold");
    }

    #[test]
    fn a_switch_without_a_mode_deserializes_and_describes_with_switch() {
        let legacy = serde_json::json!({
            "id": "old", "name": "old", "kind": "fixture", "graph": "g1",
            "fixture": "wash-1", "set": [{ "node": "a", "on": true }]
        });
        let sw: Switch = serde_json::from_value(legacy).unwrap();
        assert_eq!(sw.mode, None);
        assert!(!sw.holds());
        let described = sw.describe(None);
        assert_eq!(described["mode"], serde_json::Value::Null);
    }

    #[test]
    fn a_fixture_or_rig_switch_without_them_still_deserializes() {
        let legacy = serde_json::json!({
            "id": "old", "name": "old", "kind": "toggle", "graph": "g1",
            "set": [{ "node": "a", "on": true }]
        });
        let sw: Switch = serde_json::from_value(legacy).unwrap();
        assert_eq!(sw.fixture, None);
        assert_eq!(sw.group, None);
    }

    /// Backward compatibility: a switch saved before `param`/`value` existed
    /// deserializes and re-serializes exactly as it was — the `skip_serializing_if`
    /// on both new fields means no key appears on an entry that does not hold one.
    #[test]
    fn a_switch_without_param_or_value_round_trips_unchanged() {
        let legacy = serde_json::json!({
            "id": "old", "name": "old", "kind": "toggle", "graph": "g1",
            "set": [{ "node": "a", "on": true }, { "node": "b", "on": false }]
        });
        let sw: Switch = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(sw.set[0].param, None);
        assert_eq!(sw.set[0].value, None);
        assert_eq!(sw.set[1].param, None);
        assert_eq!(sw.set[1].value, None);
        let back = serde_json::to_value(&sw).unwrap();
        assert_eq!(back, legacy, "no param/value key appears on a legacy switch");
    }

    /// A parameter entry round-trips, `describe` shows `param`/`value`, and an
    /// on/off sibling still describes without those keys.
    #[test]
    fn a_param_entry_round_trips_and_describes_its_param_and_value() {
        let sw = Switch {
            id: "dial-1".into(),
            name: "dial".into(),
            kind: SwitchKind::Toggle,
            graph: "g1".into(),
            set: vec![SwitchEntry::param("a", "amount", 0.5)],
            unset: None,
            fixture: None,
            group: None,
            mode: None,
        };
        assert_eq!(sw.set[0].on, true, "a param entry keeps the on-side bypass write");
        let json = serde_json::to_value(&sw).unwrap();
        assert_eq!(json["set"][0]["param"], "amount");
        assert_eq!(json["set"][0]["value"], 0.5);
        let restored: Switch = serde_json::from_value(json).unwrap();
        assert_eq!(restored, sw);
        let described = sw.describe(None);
        assert_eq!(described["set"][0]["param"], "amount");
        assert_eq!(described["set"][0]["value"], 0.5);
        // A plain entry beside the param one omits both keys.
        let mixed = Switch {
            set: vec![SwitchEntry::node("a", true), SwitchEntry::param("b", "amount", 0.25)],
            ..sw.clone()
        };
        let d = mixed.describe(None);
        assert!(d["set"][0].get("param").is_none());
        assert!(d["set"][0].get("value").is_none());
        assert_eq!(d["set"][1]["param"], "amount");
        assert_eq!(d["set"][1]["value"], 0.25);
    }
}
