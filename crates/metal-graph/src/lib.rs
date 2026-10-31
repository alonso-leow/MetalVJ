//! The node-graph model — what a `Graph` is, independent of anything that
//! draws one.
//!
//! This crate is to the node editor what `metal-schema` is to the float
//! manifest: the data half, lifted out on its own, so that a headless
//! exporter, a test harness, or a future native painter can all depend on
//! *the graph* without depending on `wgpu`, `winit`, or an HTTP server. See
//! `the design notes` §10, Conjunto A, for the plan this crate implements.
//!
//! **No `wgpu`. No knowledge that an app exists.** `serde_json` is used
//! internally to serialize/deserialize (`Graph::to_json`/`from_json`), but it
//! never appears in a public signature — a consumer that wants a `Graph`
//! calls `Graph::from_json(&str) -> Result<Graph, Error>` and never sees a
//! `serde_json::Value`. This is the same discipline `metal-schema` follows and
//! for the same reason: the moment this crate's public API mentions
//! `serde_json`, every consumer is one `cargo update` away from a breaking
//! change it didn't ask for.

pub mod bind;
pub mod camera_track;
pub mod collection;
pub mod compile;
pub mod effects;
pub mod layout;
pub mod lint;
pub mod materials;
pub mod migrate;
pub mod owner;
pub mod params;
pub mod relevance;
pub mod parity;
pub mod script_floats;
pub mod source;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use metal_schema::{Capabilities, Capability};
pub use compile::{Op, Plan};
pub use lint::{Issue, Severity};
pub use layout::{Dir, Hit, Primitive};
pub use parity::{is_script_kind, CTL_MATH_KIND, PARITY_EFFECT_IDS, RAMP_KIND};

// ---------------------------------------------------------------------------
// ids
// ---------------------------------------------------------------------------

/// A stable node id, e.g. `gen-7`, `fx-3`.
///
/// **Ids are assigned on creation and never reused after a deletion.** The
/// repo already paid for the alternative once: `Mask` gained ids because a
/// source addressed "the second mask" by position and that reference silently
/// moved to a different mask when an earlier one was deleted (see
/// `metal-dash::output::Mask`). A node id that could come back after a delete has
/// the same bug waiting in it — an edge or a `cam.track` association written
/// against `gen-7` must mean "the node that was gen-7", forever, even after
/// gen-7 is gone, and never "whatever node happens to be seventh now".
pub type NodeId = String;

/// A monotonic counter that hands out ids like `gen-7`, `fx-3` and never
/// hands the same one out twice for a given prefix — the mechanism behind the
/// "never reused" rule on [`NodeId`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdGen {
    next: BTreeMap<String, u64>,
}

impl IdGen {
    pub fn new() -> IdGen {
        IdGen::default()
    }

    /// The counters of both, each prefix at the larger of the two: a graph
    /// edited through its projection never hands out an id the other copy
    /// already used.
    pub fn merged(&self, other: &IdGen) -> IdGen {
        let mut next = self.next.clone();
        for (prefix, n) in &other.next {
            let slot = next.entry(prefix.clone()).or_insert(0);
            *slot = (*slot).max(*n);
        }
        IdGen { next }
    }

    /// Hands out the next id for `prefix` (e.g. `"gen"` -> `"gen-7"`) and
    /// advances the counter. The counter never rewinds, including when the
    /// node it names is later deleted — see [`NodeId`].
    pub fn next(&mut self, prefix: &str) -> NodeId {
        let n = self.next.entry(prefix.to_string()).or_insert(0);
        let id = format!("{prefix}-{n}");
        *n += 1;
        id
    }
}

// ---------------------------------------------------------------------------
// domain
// ---------------------------------------------------------------------------

/// `Live` or `Offline` — inferred from the graph's audio source, never chosen
/// by hand. See `the design notes` §2 rule 3: a graph with `audio.live` is
/// `Live`, one with `audio.file` is `Offline`, one with both is invalid the
/// moment the second cable lands, not at save time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Live,
    Offline,
}

// ---------------------------------------------------------------------------
// ports
// ---------------------------------------------------------------------------

/// What a port carries.
///
/// `Control` was named `Digest` in the first draft of this plan and renamed
/// before any code shipped — see `the design notes` §13.0. The type was
/// never audio-specific: it is "a number 0..1, sampled on its own cadence,
/// that can drive a float", and `sensor.yolo` producing eye/hand channels is
/// the same shape as `audio.live` producing RMS/centroid/onset. Naming it
/// `Digest` from the start would have made every later consumer (§13's
/// `sensor.yolo`, §14.5's `osc.in`/`midi.in`/`http.in`) either misname
/// themselves after audio or force a rename that touches every port
/// declaration in this crate. Calling it `Control` from A1 costs nothing and
/// saves that rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    Control,
    Image,
}

/// One port on a node: a name (unique within the node) and what it carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    pub ty: PortType,
    /// `true` for a **material** port (23-sept-2026): an image input whose
    /// upstream is a *second source* for the node rather than part of the
    /// picture — the `stroke` of a `mask` is the first one (the outline's
    /// material), and a future fill/texture/modulation port would be more of
    /// the same. Declaring it here is what makes the upstream a **secondary
    /// sink** (`docs/plan-architecture.md` addendum D7): its pixels are
    /// rendered and kept by [`crate::Graph::compile`]/`Paint::run` even
    /// though they never reach an `output.device`, because nothing else would
    /// ever produce a frame to sample.
    ///
    /// A material port is still an [`PortType::Image`] — the annotation
    /// classifies the *role* of the input, never the kind of data, so no new
    /// `PortType` is invented.
    #[serde(default)]
    pub material: bool,
}

impl Port {
    pub fn new(name: &str, ty: PortType) -> Port {
        Port { name: name.to_string(), ty, material: false }
    }

    /// An image port that is a *material* source for the node — see
    /// [`Port::material`]. The runtime index of these is
    /// [`is_material_port`]; the catalog is the truth and a test ties the
    /// two together, so a new material port cannot be added to one and
    /// forgotten in the other.
    pub fn material(name: &str, ty: PortType) -> Port {
        Port { name: name.to_string(), ty, material: true }
    }
}

/// Every `(kind, input-port)` the catalog declares as **material** — see
/// [`Port::material`]. The catalog (built by [`NodeKind::catalog`]) is the
/// source of truth; this is the table `Paint::run` can read without a schema
/// manifest in hand, and a `metal-graph` test asserts the two agree port for
/// port. Adding a material port is one row here and one `Port::material` in
/// the catalog, never a new branch in the render path.
pub const MATERIAL_PORTS: &[(&str, &str)] = &[("mask", "stroke")];

/// Whether `(kind, port)` is a material port — see [`MATERIAL_PORTS`]. Every
/// generated `gen.*` kind also has one `tex.<part>` material input per surface
/// its model declares (plan: surface sources), so those answer true by prefix:
/// the manifest, not this table, lists the parts.
pub fn is_material_port(kind: &str, port: &str) -> bool {
    MATERIAL_PORTS.iter().any(|(k, p)| *k == kind && *p == port)
        || (kind.starts_with("gen.") && port.starts_with("tex."))
}

// ---------------------------------------------------------------------------
// node kinds
// ---------------------------------------------------------------------------

/// Which family a node belongs to, and the fixed ones that are not generated
/// from `data/schema.json` — see [`NodeKind::catalog`] for the generated
/// ones (every live generator, and fifteen effects).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeFamily {
    Audio,
    Generator,
    Reel,
    Still,
    /// Words on screen: a title, a lyric, a name.
    ///
    /// Its own family rather than a second `still` kind because the two are
    /// only alike after the fact. A still is a file the operator points at;
    /// text is content they type, plus how it is set — and nothing that reads
    /// a `still` node's config (a path) can read a `text` node's (a string, a
    /// size, an alignment) without first asking which of the two it is, which
    /// is the family not doing its job.
    Text,
    Capture,
    Effect,
    Router,
    /// O.6: runs a function over a row of floats and writes a row of floats
    /// — transforms, does not draw. Ranked at [`Stage::Modulate`] next to
    /// [`NodeFamily::Effect`]/[`NodeFamily::CameraTrack`] for the same
    /// reason those two are: a script has no picture of its own, only a
    /// `control` in and a `control` out, and `Source → Script → Mask`
    /// (the O.6 shot's own shape: a body's position script'd into a
    /// left/right + width decision that feeds a mask) has to be a forward
    /// edge like any other transform.
    ///
    /// **What runtime a `script.*` kind runs is in the `kind` string, not a
    /// second field** — `script.py`, `script.rs` and `script.js` (`S.G`)
    /// all share this one family, the same way `gen.turing` vs `gen.chamber`
    /// is the whole difference between two [`NodeFamily::Generator`] kinds.
    /// The node's own floats (name/range/default) and its source live in
    /// `Node.config`, read by whatever crate executes this `kind` — see
    /// `metal_perform::script` for the Python one.
    Script,
    Mask,
    /// Where a picture lands: a warp of what a `mask` kept, in source space,
    /// onto one surface of the venue, in the device's own 0..1.
    ///
    /// # Why a family, and why `Mask`
    ///
    /// `mask` decides **what** part of the source is kept, in source pixels;
    /// this decides **where** those pixels land. They are two jobs, so they
    /// are two nodes — but the surface is a step of the same rank as the mask
    /// (`Source → … → mask → venue.surface → output.device`), which is what
    /// lets `mask → venue.surface` and `venue.surface → output.device` both
    /// be legal forward edges without a stage exception. The device is *not*
    /// in the node's config: it is read from the graph (the `output.device`
    /// this node reaches), so a venue document stays a reading of the graph
    /// rather than a second place the chain is written down.
    ///
    /// `NeedsGpu`: a warp renders. Its ports still derive `ImageIn`,
    /// `ControlIn` and `ImageOut` from the catalog entry — the `control` input
    /// is for the body/spotlight that follows.
    ///
    /// One family, three kinds: `venue.surface` (P1) warps a picture onto a
    /// surface; `venue.fixtures` (M3) reads the venue's fixtures and drives
    /// them from the frame; `venue.fixture` (F1) drives one of those rows from
    /// the frame, a model's colour slots or a `sig_*` row. Their stage and
    /// capabilities differ per kind, not per family.
    Venue,
    OutputDevice,
    /// A destination that is not a screen: a bulb, a lamp bar, an instrument.
    ///
    /// Split from [`NodeFamily::OutputDevice`] rather than folded into it
    /// because the two disagree about the one thing a family is for — what a
    /// node accepts. An `output.device` takes an `image` and is meaningless
    /// without one; every kind in this family takes a `signal` and most of
    /// them cannot render a picture at all. Sharing a family would mean every
    /// consumer that branches on family (the catalog menu, the lint, the
    /// device compiler) immediately branching on `kind` underneath it, which
    /// is the family not doing its job.
    ///
    /// Which protocols: DDP and JSON for screw-in smart bulbs; E1.31 (sACN),
    /// OSC, HTTP and MIDI for DMX lighting. They were a stub dropdown
    /// ("extensiones") before they were nodes.
    OutputSignal,
    ExportVideo,
    Sensor,
    CameraTrack,
    /// A source of **control** that is not a picture and not a digest: a colour
    /// or a whole palette, offered for a script or the properties panel to
    /// drive a consumer's parameter.
    ///
    /// # Why a family and not two more `gen.*` kinds
    ///
    /// A colour is a source in the pipeline's own terms — it is the origin of
    /// the value, nothing upstream feeds it, and it sits at [`Stage::Source`]
    /// so `Colour → consumer` is a forward edge that lints like any other. But
    /// it is *not* a generator: it has no `image`, no `row`, no GPU pass. Its
    /// one output is `control` ([`PortType::Control`], no new port type — the
    /// same "a number 0..1 sampled on its own cadence" the digest columns and
    /// the `sig_*` floats already are), carrying a [`params::VarKind::Colour`]
    /// variable instead of a float. A family of its own is what lets
    /// `NodeFamily::stage`/`capabilities` stay honest: a colour needs no GPU
    /// and no audio, exactly like [`NodeFamily::Script`], and every consumer
    /// that branches on family can tell "this feeds a colour slot" from "this
    /// draws a picture" without reading `kind`.
    Colour,
    /// A trained reaction: a source of **control** that hears a digest row and
    /// answers the offsets a consumer's own floats should move by.
    ///
    /// # Why a family, and why `Source`
    ///
    /// The three trainable models the suite already has (the Colourist, the
    /// Router and the Curator) were never nodes: the Colourist was loaded
    /// through `state.reels.colourist`, the Router was `router::Mode::Model`
    /// on `State` and the Curator a per-app coupling. This family is where
    /// they become wireable. It sits at [`Stage::Source`] next to
    /// [`NodeFamily::Audio`]/[`NodeFamily::Sensor`] because it is a source of
    /// *control*, not of images: it has a `control` in and a `control` out and
    /// draws nothing. At rank 0 every edge the plan needs (`audio → ai →
    /// gen`, `ai → fx`, `ai.router → router.select`, `ai.curator → reel.*`) is
    /// a legal forward edge without touching [`STAGE_EDGE_EXCEPTIONS`].
    ///
    /// # `HasAux`, not `NeedsGpu`
    ///
    /// The family declares [`Capability::HasAux`]: each kind's inference is a
    /// runtime of `metal_logic::AuxLogic` (D1) — input is a row plus context,
    /// output is a row/control, never pixels; a failure is a value and the
    /// cost carries a hard budget. `metal-reel`/`metal-router` keep
    /// the training and the weights; the runtime that the engine calls through
    /// the node is the shared CPU-before-drawing seam. Neither `NeedsGpu` nor
    /// `NeedsAudio` belongs here — the digest arrives over the cable, from an
    /// `audio.*` node.
    Ai,
    /// A control source read off a wire the machine receives (P-D: Art-Net
    /// `in.artnet`). Its row is written by a listener outside the frame, not
    /// computed from the graph: a source of control, no GPU, no audio, no aux.
    Input,
    /// The **math** family: a wave, a constant, a random draw or a pure noise
    /// stream, offered on a `control` port (M1 of the show-control lote, plan
    /// "Value sources"). Its own gallery category ("Matemáticas"), not a
    /// source of pictures or of colour.
    ///
    /// # Why a family, and why `Source`
    ///
    /// A `source.*` node computes a number from the clock — nothing upstream,
    /// nothing drawn — exactly the shape [`NodeFamily::Colour`] has, so it sits
    /// at [`Stage::Source`] and needs no GPU and no audio. It is a family of its
    /// own rather than three more [`NodeFamily::Colour`] kinds because the two
    /// disagree about what they carry (a colour is a slot, a value is a float)
    /// and about how a consumer reads them; folding them together would make
    /// every reader branch on `kind` underneath the family, which is the family
    /// not doing its job. The runtime is `metal_perform::source`. Kind ids
    /// (`source.osc`, `source.const`, …) are unchanged; only the family moved.
    Math,
    /// What a 3D model's surface is made of: `mat.<kind>`, a new family beside
    /// `fx.*` (the materials brief, Decision 1).
    ///
    /// # Why a family, and why `Modulate`
    ///
    /// A material is not an effect: an `fx.*` node transforms the *picture*,
    /// while a `mat.*` node acts on the *model's surfaces* (the 3D `gen.*`
    /// upstream) and passes the picture through unchanged. It is not a source
    /// (it draws nothing of its own) and not a mask (it keeps no region), so it
    /// is a family of its own. It rides at [`Stage::Modulate`] because it sits
    /// on the same `image` chain an effect does: `gen → mat.wood → output` is a
    /// forward edge, and so is `gen → mat.wood → fx.vignette → output`. The
    /// node's kind names the material and its config names the part and the
    /// fixed overrides; the resolver reads the chain back off the graph
    /// ([`materials`]).
    Material,
    /// What a device shows *before* there is anything to show — the mode.
    ///
    /// # Why this is a family of its own, and why its one kind has no ports
    ///
    /// `the design notes` II.1 named this as the last thing keeping
    /// `#legacyTransmision` alive, and the reason it could not be a `gen.*`
    /// node's config: `stack.mode` decides **which `Vec<Layer>` exists** —
    /// automix synthesises its own pair, `Collection(id)` reads a whole
    /// different list — before `compile_device_plan` ever asks whether a graph
    /// is attached. A generator's config cannot reach back and change which
    /// layers were resolved to build the very list it is a member of. See
    /// `metal-dash`'s `node_layer.rs`, which says exactly that and stops there.
    ///
    /// So the mode is a node that is **not in the image chain at all**. It has
    /// no inputs and no outputs on purpose: an edge would mean it takes part
    /// in making a picture, and it does not — it decides what the picture is
    /// made *of*, one step earlier. That is also why it cannot be lint-checked
    /// as a source or a sink, and why [`lint::lint_orphans`] has to know it is
    /// legitimately unconnected.
    ///
    /// **Superseded by [`NodeFamily::Compose`] (LAY.7b).** `stack.mode` was
    /// the right construct for the mode before the stage contract existed, and
    /// it is now the *legacy* one: the render path reads a `compose.*` node
    /// first and only falls back to a `stack.mode` node, so an old file keeps
    /// working and a migrated one has one truth. The family stays in the
    /// catalog for exactly that fallback — a graph saved before the migration
    /// must still lint and still render — but nothing writes a new one.
    Stack,
    /// The composition: what a device shows, resolved into an ordered set of
    /// members one step before anything is drawn — `docs/plan-architecture.md` §2.1
    /// and §2.4's bridge.
    ///
    /// # Why the mode became a composition
    ///
    /// `Stage::Compose` was declared by LAY.1 with no family behind it ("the
    /// stage exists so the migration has a destination"). This is that
    /// destination. The bridge table is one mapping in one place:
    /// `DeviceMode::Automix` → `compose.automix`, `Collection(id)` →
    /// `compose.collection`, `Own` → `compose.layers`, `Disabled` → the node
    /// is retired (a device with no composition declared is off).
    ///
    /// The classification is not cosmetic. `stack.mode` sat at
    /// `Stage::Schedule` — orthogonal, outside the pipeline — which was true
    /// while the mode was a *switch* the render path consulted before the
    /// list existed. It is still read before the list exists (that is what
    /// makes it a decision and not a mixer), but it is no longer orthogonal:
    /// it is the seat of the composition axis, the thing every future mixer
    /// kind grows out of, and the rank-2 stage is where `lint` expects the
    /// `Source → Compose → Mask → Output` chain to pass through.
    ///
    /// # The mixer's ports (LAY.7c)
    ///
    /// §2.1's table says `Compose` takes N images and emits one. The kinds
    /// carry that shape now: one `image` input (N edges into a single named
    /// port, exactly as `router` models its fan-in — N is not known until the
    /// graph is built) and one `image` output. The node decides **which**
    /// composition applies *and* is the step that mixes: its `image` inputs
    /// are the members, its output the mixed frame, and the compositor's blend
    /// rule runs where the plan says the node is. A compose node saved before
    /// the ports existed (no edges) keeps its meaning: member-less is the
    /// delegation case, where the pool's routing decides — the ports are
    /// append-only in the same sense the whole contract is.
    Compose,
}

/// The stage a node sits in on the way from a source to a screen.
///
/// **Why this exists.** `NodeFamily` is a *taxonomy* — it says what a node is
/// made of (audio, generator, effect…). It does not say where the node sits in
/// the pipeline, and until this type existed nothing did: the order
/// `source → modulate → compose → mask → output` lived only in prose and in
/// the shape of the app's own render code. A contract of stages makes that
/// order **data** — see `docs/plan-architecture.md` §2 and [`Stage::rank`], which
/// `lint` enforces so an impossible cable is rejected before a frame is drawn.
///
/// [`Stage::Schedule`] is orthogonal: it has no rank and never takes part in
/// the order. It is `stack.mode`, which decides *which* chain exists rather
/// than being a step in one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Generates or captures a picture; may be stateful; may carry `AuxLogic`.
    Source,
    /// Transforms a picture (an effect/filter) or emits control (an animation,
    /// `cam.track`); an effect at `amount = 0` is the same frame it came in as.
    Modulate,
    /// Mixes N pictures into one with blend + opacity + order.
    Compose,
    /// Alpha / exclusion; the sense may arrive on `control` (from a script).
    Mask,
    /// A device, a signal or an export — the end of the chain.
    Output,
    /// **Not an image stage.** `stack.mode` decides what the picture is made
    /// *of*, one step before there is a picture to be part of. No rank, no
    /// edges — see [`NodeFamily::Stack`].
    Schedule,
}

impl Stage {
    /// Position in the render order. `None` for [`Stage::Schedule`], which is
    /// orthogonal and never ordered. The four ranks are spaced so a future
    /// stage can be inserted between two without renumbering (append-only,
    /// `docs/plan-architecture.md` PARTE VII).
    pub const fn rank(self) -> Option<u8> {
        match self {
            Stage::Source => Some(0),
            Stage::Modulate => Some(1),
            Stage::Compose => Some(2),
            Stage::Mask => Some(3),
            Stage::Output => Some(4),
            Stage::Schedule => None,
        }
    }

    /// Whether this stage is the orthogonal one with no rank.
    pub const fn is_scheduled(self) -> bool {
        matches!(self, Stage::Schedule)
    }

    /// Every stage, for the exhaustiveness test and anything that wants to
    /// enumerate them without hard-coding the list a second time.
    pub const ALL: &'static [Stage] = &[
        Stage::Source,
        Stage::Modulate,
        Stage::Compose,
        Stage::Mask,
        Stage::Output,
        Stage::Schedule,
    ];
}

/// Whether the `+ nodo` menu offers `kind` at all — the data half of
/// `the design notes` R22.5a's decision that the menu shows what serves the operator.
///
/// **One place, like [`kind_domains`].** `stack.mode` and every `compose.*`
/// kind are retired from the menu: the mode is the output's own config
/// (R22.5b) and the legacy node is the migration's business, so neither is
/// something an operator reaches for mid-drawing. Nothing else is hidden —
/// a kind the operator cannot reach for would be a kind that is not in the
/// catalog at all.
///
/// `ai.colourist` is retired too: it is not a node an operator adds. It is
/// the configured sub-feature of the `gen.*` nodes (switched on from their
/// own config, which mints the owned node), so it stays in the catalog for
/// lint and the canvas but is never offered in a gallery or an add list.
pub fn kind_menu(kind: &str) -> bool {
    !(kind == STACK_MODE_KIND || kind == AI_COLOURIST_KIND || is_compose_kind(kind))
}

/// The domains a kind may exist in — **the same rule `lint` applies, in one
/// place**.
///
/// [`lint::lint_domain_restricted_kinds`] reads this list rather than
/// matching on kinds a second time, and [`NodeKind::new`] stamps the same
/// answer on every catalog entry, so the menu a graph offers and the errors
/// that graph gets can never disagree about where a kind belongs.
///
/// The two exceptions to "every kind lives in both domains" are the kinds
/// that are *about* a domain rather than about a picture:
///
/// - `audio.live`/`audio.file` decide the domain ([`Graph::inferred_domain`]),
///   so each implies the other's absence; `lint_domain` reports the mismatch
///   when a graph declares the wrong one.
/// - `export.video` only renders Offline; `output.device`, `capture` and
///   every `out.*` only exist Live — a screen, a camera and a lamp are
///   things happening *now*, and a `.mp4` has no device to be in a mode.
///
/// `stack.mode`/`compose.*` are Live for the same "a timeline has no device
/// to be in a mode" reason, even though the menu no longer offers them
/// ([`kind_menu`]).
pub fn kind_domains(kind: &str) -> &'static [Domain] {
    if kind == "audio.live"
        || kind == "output.device"
        || kind == "capture"
        || kind.starts_with("out.")
        || kind == VENUE_FIXTURES_KIND
        || kind == VENUE_FIXTURE_KIND
        || kind == STACK_MODE_KIND
        || is_compose_kind(kind)
    {
        &[Domain::Live]
    } else if kind == "audio.file" || kind == "export.video" {
        &[Domain::Offline]
    } else {
        &[Domain::Live, Domain::Offline]
    }
}

/// [`NodeKind::menu`]'s answer for a catalog entry that does not carry the
/// field: offered. A missing field must never silently empty a menu, and
/// "offered" is the same answer [`kind_menu`] gives every kind it does not
/// name.
fn menu_default() -> bool {
    true
}

/// One entry in the catalog: a `kind` string (e.g. `gen.turing`, `fx.shake`,
/// `output.device`), its family, human label, and fixed port list.
///
/// This is data, not a trait — a `NodeKind` does not know how to render
/// itself, only what it looks like from the outside (ports) and where it
/// belongs (family). Anything else lives in the app that draws it, which this
/// crate does not know exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeKind {
    pub kind: String,
    pub family: NodeFamily,
    pub label: String,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    /// Whether the `+ nodo` menu offers this kind at all — see [`kind_menu`].
    /// `stack.mode`/`compose.*` are `false`; every other kind is `true`.
    #[serde(default = "menu_default")]
    pub menu: bool,
    /// The domains this kind may live in — see [`kind_domains`]. Read by
    /// `GET /catalog?domain=`, by the menu that calls it and by `lint`, all
    /// off this one list, so none of the three can disagree.
    ///
    /// Skipped on deserialization: the catalog is *built* by
    /// [`NodeKind::catalog`], never loaded, and a `&'static` slice cannot be
    /// borrowed out of a JSON buffer. A hand-parsed kind keeps the empty
    /// slice, which [`NodeKind::offered_in`] reads as "no restriction known"
    /// rather than "allowed nowhere".
    #[serde(skip_deserializing)]
    pub domains: &'static [Domain],
    /// The parameters this kind exposes on its own — a generator's floats and
    /// colour slots, an effect's `amount` — each a row a binding can drive
    /// (`param:<name>`). Stamped by [`NodeKind::catalog`] from the manifest
    /// ([`params::kind_params`], the extension point for new kinds); a
    /// script's floats are per node and come from its config
    /// ([`params::node_params`]). Empty for a kind built any other way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<params::Param>,
}

impl NodeKind {
    fn new(
        kind: &str,
        family: NodeFamily,
        label: &str,
        inputs: Vec<Port>,
        outputs: Vec<Port>,
    ) -> NodeKind {
        NodeKind {
            kind: kind.to_string(),
            family,
            label: label.to_string(),
            inputs,
            outputs,
            menu: kind_menu(kind),
            domains: kind_domains(kind),
            params: Vec::new(),
        }
    }

    /// Whether the `+ nodo` menu offers this kind in `domain` — the exact
    /// answer `GET /catalog?domain=` filters by: the kind must be in the menu
    /// at all ([`NodeKind::menu`]) and must name the domain in
    /// [`NodeKind::domains`]. An empty `domains` (a kind parsed from a
    /// catalog that predates the field) restricts nothing.
    pub fn offered_in(&self, domain: Domain) -> bool {
        self.menu && (self.domains.is_empty() || self.domains.contains(&domain))
    }

    /// Where this kind sits in the pipeline — `docs/plan-architecture.md` §2.4.
    ///
    /// Derived from [`NodeKind::family`] so there is one mapping, not a second
    /// list that could drift from the catalog. The exhaustiveness test
    /// (`every_catalog_kind_maps_to_exactly_one_stage`) walks the catalog and
    /// fails the moment a new family arrives without a stage.
    pub fn stage(&self) -> Stage {
        // One family, three kinds: `venue.surface` warps a picture at the
        // mask rank (P1), while `venue.fixtures`/`venue.fixture` are signal
        // emitters that land at the output rank (M3/F1). The family default is
        // `Mask`.
        if self.kind == VENUE_FIXTURES_KIND || self.kind == VENUE_FIXTURE_KIND {
            return Stage::Output;
        }
        self.family.stage()
    }

    /// What this kind can do and needs — `docs/plan-architecture.md` D4/LAY.2.
    ///
    /// **Derived, not a second list.** The port-shaped capabilities come from
    /// the declared [`Port`]s and the needs come from [`NodeFamily`], so a
    /// kind cannot claim `image_in` without an `Image` input. Anything a
    /// family cannot express would be added at the kind's own constructor, in
    /// the catalog, which is the one place a kind is built.
    pub fn capabilities(&self) -> Capabilities {
        let mut c = Capabilities::empty();
        for p in &self.inputs {
            c = match p.ty {
                PortType::Image => c.with(Capability::ImageIn),
                PortType::Control => c.with(Capability::ControlIn),
            };
        }
        for p in &self.outputs {
            c = match p.ty {
                PortType::Image => c.with(Capability::ImageOut),
                PortType::Control => c.with(Capability::ControlOut),
            };
        }
        // `venue.fixtures`/`venue.fixture` share the `Venue` family with the
        // surface warp but send signal instead of rendering: no GPU pass of
        // their own (M3/F1).
        let family = if self.kind == VENUE_FIXTURES_KIND || self.kind == VENUE_FIXTURE_KIND {
            Capabilities::empty().with(Capability::SignalOut)
        } else {
            self.family.capabilities()
        };
        c.union(family)
    }
}

impl NodeFamily {
    /// The stage a whole family belongs to. See [`NodeKind::stage`].
    ///
    /// `Router` and `CameraTrack` are `Modulate` rather than a stage of their
    /// own: a router is a pass-through selector (N images in, one out, no
    /// blend) and `cam.track` writes floats — both are transforms, and ranking
    /// them at `Modulate` is what keeps `Source → Router → Effect` from being
    /// rejected as a backward edge.
    ///
    pub fn stage(&self) -> Stage {
        match self {
            NodeFamily::Audio
            | NodeFamily::Generator
            | NodeFamily::Reel
            | NodeFamily::Still
            | NodeFamily::Text
            | NodeFamily::Capture
            | NodeFamily::Sensor
            | NodeFamily::Colour
            | NodeFamily::Input
            | NodeFamily::Math
            | NodeFamily::Ai => Stage::Source,
            NodeFamily::Effect
            | NodeFamily::Router
            | NodeFamily::CameraTrack
            | NodeFamily::Material
            | NodeFamily::Script => Stage::Modulate,
            NodeFamily::Compose => Stage::Compose,
            NodeFamily::Mask | NodeFamily::Venue => Stage::Mask,
            NodeFamily::OutputDevice | NodeFamily::OutputSignal | NodeFamily::ExportVideo => {
                Stage::Output
            }
            NodeFamily::Stack => Stage::Schedule,
        }
    }

    /// The family-wide capabilities every kind in it inherits. See
    /// [`NodeKind::capabilities`]. Port-shaped capabilities are added there
    /// from the actual ports; this is only what the family implies.
    pub fn capabilities(&self) -> Capabilities {
        match self {
            NodeFamily::Audio => Capabilities::empty().with(Capability::NeedsAudio),
            NodeFamily::Generator => Capabilities::empty().with(Capability::NeedsGpu),
            NodeFamily::Reel => {
                Capabilities::empty().with(Capability::NeedsGpu).with(Capability::NeedsFootage)
            }
            NodeFamily::Still | NodeFamily::Text | NodeFamily::Capture => {
                Capabilities::empty().with(Capability::NeedsGpu)
            }
            NodeFamily::Effect | NodeFamily::Router | NodeFamily::Mask => {
                Capabilities::empty().with(Capability::NeedsGpu)
            }
            // A material acts on a 3D model's surfaces, which the host draws:
            // it needs the GPU the model does and nothing else — the picture
            // passes through it unchanged.
            NodeFamily::Material => Capabilities::empty().with(Capability::NeedsGpu),
            // A warp renders: the surface's own perspective is a GPU pass over
            // the picture it is handed, like the mask's cut. Ports add
            // `ImageIn`/`ControlIn`/`ImageOut` at the kind.
            NodeFamily::Venue => Capabilities::empty().with(Capability::NeedsGpu),
            NodeFamily::OutputDevice | NodeFamily::ExportVideo => {
                Capabilities::empty().with(Capability::NeedsGpu)
            }
            NodeFamily::OutputSignal => Capabilities::empty().with(Capability::SignalOut),
            NodeFamily::Sensor => {
                Capabilities::empty().with(Capability::NeedsGpu).with(Capability::HasAux)
            }
            NodeFamily::CameraTrack => Capabilities::empty().with(Capability::HasAux),
            // No GPU, no audio: a script runs on whatever process isolates
            // its own runtime (a subprocess, for `script.py` — see
            // `metal_perform::script`), not on the device this crate's other
            // families reserve. Nothing here to declare beyond the
            // port-shaped `ControlIn`/`ControlOut` `NodeKind::capabilities`
            // already derives from its ports.
            NodeFamily::Script => Capabilities::empty(),
            // No GPU, no audio: a colour or a palette is data the operator
            // typed (or a script wrote), offered on a `control` port. Like
            // `Script`, nothing here to declare beyond the port-shaped
            // `ControlOut` `NodeKind::capabilities` already derives.
            NodeFamily::Colour => Capabilities::empty(),
            // A socket read on its own thread (`metal_perform::artnet`): no
            // GPU, no audio, nothing for the engine to call.
            NodeFamily::Input => Capabilities::empty(),
            // A computed number: pure CPU arithmetic on the clock, nothing the
            // engine has to reserve beyond the port-shaped `ControlOut`.
            NodeFamily::Math => Capabilities::empty(),
            // The trained reaction: a CPU inference, not a GPU pass. `HasAux`
            // is the D1 declaration that the engine may call an `AuxLogic`
            // through this node — see `NodeFamily::Ai`.
            NodeFamily::Ai => Capabilities::empty().with(Capability::HasAux),
            // No ports, no needs: a composition is a statement about which
            // members exist, resolved by the app before the GPU is asked for
            // anything. See `NodeFamily::Compose`.
            NodeFamily::Compose | NodeFamily::Stack => Capabilities::empty(),
        }
    }
}

/// The single declared table of edges that the base `rank(from) ≤ rank(to)`
/// rule (`docs/plan-architecture.md` §2.3) would otherwise reject.
///
/// **A `Mask` is the same datum at two points of the pipeline.** As a stage it
/// sits after `Compose` (rank 3), because the ordinary use is masking the
/// composed frame. But a mask may also wrap a single `Source` and then feed
/// the composite — and that `Mask → Compose` cable is a genuine backward edge
/// the pipeline needs, so it is the one entry here. The `Source → Mask` and
/// `Compose → Mask` cables are listed too, so both ways a mask may be fed are
/// declared in the same one place rather than being implied by the base rule.
pub const STAGE_EDGE_EXCEPTIONS: &[(Stage, Stage)] = &[
    (Stage::Mask, Stage::Compose),
    (Stage::Source, Stage::Mask),
    (Stage::Compose, Stage::Mask),
    // The wire after the device (2026-10-22): the device's `venue` output
    // carries its finished frame to a `venue.surface`, which ranks `Mask` —
    // one stage *earlier* than the device. That is the plan's order read
    // backwards on purpose (`the design notes`: `… → venue.surface → output.device`;
    // the device is last), so the room's grid is a view of the frame, never
    // a second composition. Only `output.device.venue →
    // venue.surface.image` is allowed; `lint::lint_venue_device_port` names
    // every other use of the port.
    (Stage::Output, Stage::Mask),
];

/// Whether an edge `from → to` is allowed by the stage contract. `Ok(())` if
/// it is; `Err(message)` naming the violation if it is not.
///
/// Schedule is never ordered: an edge into or out of a scheduled node is a
/// violation by itself, because a scheduled node has no ports to begin with.
pub fn stage_edge_allowed(from: Stage, to: Stage) -> Result<(), String> {
    if from.is_scheduled() {
        return Err(format!("{from:?} is orthogonal and may not feed any stage"));
    }
    if to.is_scheduled() {
        return Err(format!("{to:?} is orthogonal and may not be fed by any stage"));
    }
    let (Some(f), Some(t)) = (from.rank(), to.rank()) else {
        return Ok(());
    };
    if f <= t || STAGE_EDGE_EXCEPTIONS.contains(&(from, to)) {
        Ok(())
    } else {
        Err(format!("{from:?} → {to:?} is a backward edge; {from:?} comes after {to:?}"))
    }
}

// ---------------------------------------------------------------------------
// nodes and edges
// ---------------------------------------------------------------------------

/// One node in the graph. `kind` is the string from [`NodeKind::kind`] — the
/// graph does not hold a `&NodeKind`, so a graph can be deserialized and
/// linted even against a catalog it disagrees with (an older save, a kind
/// that was retired — see `the design notes` §Conjunto F5 on tombstones).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: String,
    pub pos: [f32; 2],
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub bypass: bool,
    #[serde(default)]
    pub solo: bool,
    /// Whether the canvas shows this node's whole parameter list or folds it
    /// behind a `▸ n more` row. Persisted so a graph file remembers that its
    /// author opened a node — a pure view preference, but one the layout
    /// ([`crate::layout::Layout`]) reads to decide the box's height, so it
    /// travels with the document rather than in a panel's memory.
    #[serde(default)]
    pub expanded: bool,
    /// Free-form per-kind configuration. A node's config shape belongs to
    /// whatever reads `kind` and knows what it means — the app, not this
    /// crate: compile and layout never look inside it. The one lint rule
    /// that does is the mask stroke's (22-sept-2026, tarde): a wired
    /// `stroke` port with no `stroke_width` is a dead cable only the lint
    /// can name, because that pairing spans a config and a cable.
    #[serde(default)]
    pub config: Config,
    /// The source this node is embedded in, if any (`ai.embed`, plan
    /// "AI embedded in the source's properties"). An embedded `ai.*` node is
    /// owned by its source: it is excluded from layout and hit-testing as a
    /// free node (it draws as a chip on the source instead), and deleting the
    /// source deletes it. `None` is the ordinary, free-standing node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded_in: Option<NodeId>,
}

impl Node {
    pub fn new(id: impl Into<String>, kind: impl Into<String>, pos: [f32; 2]) -> Node {
        Node {
            id: id.into(),
            kind: kind.into(),
            pos,
            label: String::new(),
            bypass: false,
            solo: false,
            expanded: false,
            config: Config::default(),
            embedded_in: None,
        }
    }
}

/// A node's free-form config, opaque outside this crate.
///
/// **Why a newtype and not `pub config: serde_json::Value`.** This crate's
/// one hard rule is "no `serde_json` in the public API surface" — the same
/// rule `metal-schema` follows, see the crate doc comment. A raw
/// `serde_json::Value` field would mean every consumer of `Node`, including
/// ones that only want to move a graph around without reading a single
/// config value, pulls `serde_json` into their own public API by extension.
/// `Config` wraps a JSON value (a `serde newtype struct`, so it serializes
/// exactly as the value it wraps — no `{"0": ...}` wrapper in the file) and
/// exposes only string in/out, so a consumer that already speaks JSON can
/// still read and write one without this crate ever naming `serde_json` in a
/// signature.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config(serde_json::Value);

impl Config {
    /// Wraps an already-built `serde_json::Value`. Crate-internal — only
    /// `migrate` needs to construct a `Config` from data it already has as
    /// JSON; anything outside this crate goes through [`Config::from_json_str`].
    pub(crate) fn from_value(v: serde_json::Value) -> Config {
        Config(v)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_null()
    }

    /// The value at `key`, when this config is an object. Read-only, and the
    /// only way `lint` (a sibling module) can look at a config without
    /// re-parsing its JSON — `stroke_width` is the first config fact a lint
    /// rule depends on.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.get(key)
    }

    /// `self` with `from`'s value at `key` carried over — how a writer that
    /// replaces a node's whole config (an effect's `FxConfig`, a generator's
    /// layer config) keeps a key it does not own, such as the loop choice
    /// (`loop`, the loops design notes T7/T8). Nothing changes when `from` has no `key`.
    pub fn carrying(self, from: &Config, key: &str) -> Config {
        let Some(v) = from.0.get(key) else { return self };
        let mut obj = match self.0 {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        obj.insert(key.to_string(), v.clone());
        Config(serde_json::Value::Object(obj))
    }

    pub fn from_json_str(s: &str) -> Result<Config, ParseError> {
        serde_json::from_str(s).map(Config).map_err(|e| ParseError(e.to_string()))
    }

    pub fn to_json_str(&self) -> String {
        // unwrap: a `Config` can only be built from a value that already
        // parsed (`from_json_str`) or from this crate's own construction, so
        // this cannot fail.
        serde_json::to_string(&self.0).expect("Config always serializes")
    }
}

/// One endpoint of an edge: a node id and the port name on that node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub node: NodeId,
    pub port: String,
}

impl Endpoint {
    pub fn new(node: impl Into<String>, port: impl Into<String>) -> Endpoint {
        Endpoint { node: node.into(), port: port.into() }
    }
}

/// A cable: `from` an output port to `to` an input port.
///
/// Edges get an id too, for the same reason nodes do: `check-graph.mjs`
/// (Conjunto D) cuts a specific cable, and "the third edge in the list" is
/// exactly the addressing bug `NodeId` exists to avoid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub from: Endpoint,
    pub to: Endpoint,
}

impl Edge {
    pub fn new(id: impl Into<String>, from: Endpoint, to: Endpoint) -> Edge {
        Edge { id: id.into(), from, to }
    }
}

// ---------------------------------------------------------------------------
// the graph
// ---------------------------------------------------------------------------

/// A node graph: one collection (`the design notes` §9 — "one graph per
/// collection, with N outputs inside").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub id: String,
    pub name: String,
    pub domain: Domain,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// Carried in the file so an id is never reused across a save/load cycle
    /// — see [`NodeId`].
    #[serde(default)]
    pub ids: IdGen,
    /// The format version this graph was last upgraded to
    /// ([`migrate::CURRENT`]). A file without one is version 0 and is
    /// upgraded on load by [`migrate::upgrade`] (`Graph::from_json`).
    #[serde(default)]
    pub version: u32,
    /// Set on a *device graph* that is the published projection of a
    /// collection graph ([`collection`]): the collection graph's id. The
    /// engine runs `device-<id>` exactly as before; the file is generated, and
    /// the canvas edits the collection instead. `None` on every graph the
    /// operator authors directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Anything that can go wrong turning bytes into a `Graph`. Wraps
/// `serde_json`'s error as a string rather than re-exporting its type, so
/// `serde_json` still never appears in this crate's public API.
#[derive(Debug)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "metal-graph: {}", self.0)
    }
}
impl std::error::Error for ParseError {}

impl Graph {
    pub fn new(id: impl Into<String>, name: impl Into<String>, domain: Domain) -> Graph {
        Graph {
            id: id.into(),
            name: name.into(),
            domain,
            nodes: Vec::new(),
            edges: Vec::new(),
            ids: IdGen::new(),
            // A graph made now is current: nothing to upgrade, and a source
            // left unwired in it is static on purpose.
            version: migrate::CURRENT,
            source: None,
        }
    }

    /// Adds a node with a freshly-minted id for `kind_prefix` (e.g. `"gen"`)
    /// and returns the id.
    pub fn add_node(&mut self, kind_prefix: &str, kind: &str, pos: [f32; 2]) -> NodeId {
        let id = self.ids.next(kind_prefix);
        self.nodes.push(Node::new(id.clone(), kind, pos));
        id
    }

    /// Applies a model's default `(param, var)` bindings to `node` — the table
    /// the studio's `author::models::schema::DEFAULT_BINDINGS` carries. The
    /// graph layer does not hold those tables (they live in `metal-dash`), so the
    /// caller hands them in: this is the clearly-named hook a node-creation
    /// path (`POST /graph/node {op:"add"}` or the canvas) calls once an
    /// upstream producer exists, and it is best-effort — an entry with no
    /// upstream node offering that variable is skipped, and one refused by
    /// lint simply does not land. Returns how many bindings landed.
    pub fn apply_default_bindings(
        &mut self,
        node: &str,
        defaults: &[(&str, &str)],
        catalog: &[NodeKind],
    ) -> usize {
        if defaults.is_empty() || self.node(node).is_none() {
            return 0;
        }
        let by_kind: std::collections::HashMap<&str, &NodeKind> =
            catalog.iter().map(|k| (k.kind.as_str(), k)).collect();
        let mut landed = 0usize;
        for (param, want) in defaults {
            let from = crate::params::upstream(self, node).into_iter().find_map(|(id, _)| {
                let n = self.node(&id)?;
                let var = crate::params::node_vars(n, by_kind.get(n.kind.as_str()).copied(), None)
                    .into_iter()
                    .find(|v| v.kind == crate::params::VarKind::Float && v.port != "row" && v.name == *want)?;
                Some((id.clone(), var.port, var.name))
            });
            if let Some((from_node, port, var)) = from {
                if crate::bind::bind(self, node, param, Some((&from_node, &port, &var)), catalog).is_ok() {
                    landed += 1;
                }
            }
        }
        landed
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// Removes a node and every edge touching it. The id is never handed out
    /// again — see [`NodeId`].
    pub fn remove_node(&mut self, id: &str) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.from.node != id && e.to.node != id);
    }

    pub fn add_edge(&mut self, from: Endpoint, to: Endpoint) -> String {
        let id = self.ids.next("edge");
        self.edges.push(Edge::new(id.clone(), from, to));
        id
    }

    pub fn remove_edge(&mut self, id: &str) {
        self.edges.retain(|e| e.id != id);
    }

    /// Adds `from -> to` and keeps it only if the edge introduced no lint
    /// error — tentative-add-then-lint-then-maybe-rollback, so a cycle or a
    /// port-type mismatch is rejected by name (every error `lint` found, not
    /// just the first) instead of silently drawn and left broken.
    ///
    /// Lifted out of `metal-dash`'s own `add_edge_checked` route helper (H.4) so
    /// the same validated connect is available to anything that edits a
    /// graph — the HTTP route and the AI harness's `connect_nodes`/
    /// `create_node` tools both call this rather than each re-implementing
    /// "add, lint, maybe roll back".
    /// Adds an edge, then refuses and rolls back only if doing so makes the
    /// graph strictly worse: an error the lint did not already report
    /// before this edge went in. Errors the graph already had — an
    /// `output.device` still missing its `control` edge, say — are not
    /// grounds for refusal here, because a graph with an unrelated
    /// half-wired node is not a reason this edge in particular should be
    /// rejected. Without this distinction every edit anywhere in a graph
    /// that already has *any* standing error is refused, including edits
    /// that would have fixed nothing about that error and touched nothing
    /// it depends on — the operator wires an image cable, the route
    /// silently declines because some other node's `control` input is
    /// still open, and nothing said so.
    pub fn add_edge_checked(
        &mut self,
        from: Endpoint,
        to: Endpoint,
        catalog: &[NodeKind],
    ) -> Result<String, Vec<String>> {
        let before_errors: std::collections::HashSet<(Option<String>, String)> = self
            .lint(catalog)
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| (i.node, i.message))
            .collect();
        let before = self.edges.len();
        let edge_id = self.add_edge(from, to);
        let new_errors: Vec<String> = self
            .lint(catalog)
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| (i.node, i.message))
            .filter(|key| !before_errors.contains(key))
            .map(|(_, message)| message)
            .collect();
        if !new_errors.is_empty() {
            self.edges.truncate(before);
            return Err(new_errors);
        }
        Ok(edge_id)
    }

    /// Serializes with deterministic ordering: nodes and edges sorted by id,
    /// not insertion order. **Why this matters more than it looks like it
    /// should**: if order followed a `HashMap` (or even plain `Vec`
    /// insertion order after edits, deletes and re-inserts), saving the same
    /// graph twice in a row could produce two different byte strings, and a
    /// collection file in version control — or the "save and reload is
    /// byte-identical" test in A3 — stops being meaningful. Sorting by id
    /// makes the serialized form a pure function of the graph's *content*.
    pub fn to_json(&self) -> String {
        let mut g = self.clone();
        g.nodes.sort_by(|a, b| a.id.cmp(&b.id));
        g.edges.sort_by(|a, b| a.id.cmp(&b.id));
        // unwrap: Graph contains no maps keyed by non-string types, no NaN
        // floats produced by construction here, and no external types that
        // could fail to serialize — this cannot fail for a Graph built
        // through this crate's own API.
        serde_json::to_string_pretty(&g).expect("Graph always serializes")
    }

    /// Parses a graph and upgrades it to [`migrate::CURRENT`] in memory —
    /// every reader sees the current format; the file changes on its next
    /// save.
    pub fn from_json(text: &str) -> Result<Graph, ParseError> {
        let mut g: Graph = serde_json::from_str(text).map_err(|e| ParseError(e.to_string()))?;
        migrate::upgrade(&mut g);
        Ok(g)
    }

    /// The node kind of each node, for whatever wants to look one up without
    /// carrying its own copy of the catalog around.
    pub fn kind_of<'a>(&self, id: &str, catalog: &'a [NodeKind]) -> Option<&'a NodeKind> {
        let node = self.node(id)?;
        catalog.iter().find(|k| k.kind == node.kind)
    }

    /// What `Domain` this graph's audio nodes imply, or `None` when it has
    /// none of either and so implies nothing — see `the design notes` §2
    /// rule 3 and Conjunto G5, "the domain is inferred and shown."
    ///
    /// A free function on `Graph` rather than logic folded into `lint::Issue`
    /// text: `lint` needs "is this graph's declared domain consistent", but an
    /// editor showing the operator *which* domain a graph is building toward
    /// — before they have wired a single edge, while the field still reads
    /// whatever `Graph::new` was given — needs the inference itself, not a
    /// pass/fail. One function, so the two callers cannot drift into
    /// disagreeing about what "mixes both" means.
    pub fn inferred_domain(&self) -> Option<Domain> {
        let has_live = self.nodes.iter().any(|n| n.kind == "audio.live");
        let has_file = self.nodes.iter().any(|n| n.kind == "audio.file");
        match (has_live, has_file) {
            (true, false) => Some(Domain::Live),
            (false, true) => Some(Domain::Offline),
            // Both, or neither, imply nothing on their own — `lint_domain`
            // is what turns "both" into a named error; this function only
            // ever answers what a *single* audio kind would mean.
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// the catalog — see A2
// ---------------------------------------------------------------------------

/// The fourteen common effects that exist as `fx.*` kinds — see
/// `the design notes` Conjunto E5. Every `Post` field
/// (`metal_schema::EFFECTS_FLOATS`) gets its own node kind: a node and a float
/// are the same thing seen two ways (§3.2), and `vignette`/`blur`/`sharpen`/
/// `grain` are no exception just because they arrived after the original ten.
/// Checked against `metal_schema::EFFECTS_FLOATS` by a test in `catalog.rs` so a
/// rename over there is caught here.
/// **Derived from `metal_schema::EFFECT_SPECS`** (LAY.4), not a hand-written
/// list: a row added to the table appears here — and therefore in the catalog
/// — without a second edit, and a rename over there cannot leave a stale name
/// over here.
pub(crate) const COMMON_EFFECT_IDS: &[&str] = &metal_schema::EFFECT_NAMES;

/// The advanced effects — the Lightroom-style colour grade plus texture,
/// clarity, dehaze, denoise, optics and geometry — published as `fx.*` kinds
/// right after the common ones. Their floats live in the tail of
/// `metal_schema::EFFECTS_FLOATS`, so `params::effect_params` reads their ranges
/// straight off `metal_schema::advanced_effect_floats`.
pub(crate) const ADVANCED_EFFECT_IDS: &[&str] = metal_schema::ADVANCED_EFFECT_IDS;

/// Models promoted to effects. Empty: `fx.wake` (Estela) was removed from the
/// app 2026-10-24; the `wake` model keeps its registry index and its reserved
/// floats so no saved row shifts.
pub(crate) const PROMOTED_EFFECT_IDS: &[&str] = &[];

/// The runtime-shader effect, `fx.forge` — see its catalog entry.
pub const FORGE_EFFECT_ID: &str = "forge";
/// Its node kind.
pub const FORGE_EFFECT_KIND: &str = "fx.forge";

/// K: the speed effect, `fx.speed` — scales its generator's clock.
pub const SPEED_EFFECT_ID: &str = "speed";
/// K: the movement effect, `fx.move` — translates the picture behind its mask.
pub const MOVE_EFFECT_ID: &str = "move";

/// K: the two motion effects, named once so the catalog's loop and the count
/// in its test read the same pair instead of two literals that could drift.
pub(crate) const MOTION_EFFECT_IDS: &[&str] = &[SPEED_EFFECT_ID, MOVE_EFFECT_ID];

/// B: the particles effect, `fx.particles` — stateless dust, hashed from
/// `(seed, i, t)`.
pub const PARTICLES_EFFECT_ID: &str = "particles";
/// B: the media effect, `fx.image-pop` — reel media drawn as N instances.
pub const IMAGE_POP_EFFECT_ID: &str = "image-pop";

/// B: the two media effects, named once so the catalog's loop and the count
/// in its test read the same pair instead of two literals that could drift.
pub(crate) const MEDIA_EFFECT_IDS: &[&str] = &[PARTICLES_EFFECT_ID, IMAGE_POP_EFFECT_ID];

/// Loops T12: the space-distortion effect, `fx.warp` — a displacement field
/// written as a coordinate mesh (`metal_schema::warp`), with mosh.
pub const WARP_EFFECT_ID: &str = "warp";
/// Its node kind.
pub const WARP_EFFECT_KIND: &str = "fx.warp";

/// Loops T13: the white flash, `fx.flash` — a diffuse (screen / soft-light)
/// strobe that keeps the picture visible (`metal_perform::fx::flash`).
pub const FLASH_EFFECT_ID: &str = "flash";
/// Its node kind.
pub const FLASH_EFFECT_KIND: &str = "fx.flash";

/// S.W: the per-glyph `fx.text.*` vocabulary, derived from
/// `metal_schema::TEXT_EFFECT_NAMES` for the same reason `COMMON_EFFECT_IDS`
/// derives from `EFFECT_NAMES` — one declaration, no drift.
pub(crate) const TEXT_EFFECT_IDS: &[&str] = metal_schema::TEXT_EFFECT_NAMES;

/// S.W: the one kind an `fx.text.*` node may hang off — see
/// `lint::lint_text_effects`.
pub const TEXT_SOURCE_KIND: &str = "text.source";

impl NodeKind {
    /// The full catalog: a generator per live model in the manifest plus
    /// fifteen effects, generated from
    /// `data/schema.json` via `metal_schema`, plus the fixed kinds that are not
    /// per-model (audio, reel, still, capture, router, mask, outputs, sensor,
    /// camera track).
    ///
    /// **Why generators come from the manifest and not a literal list**: this
    /// repo already shipped the bug once — `manifest.rs` documents renderers
    /// and a hand-written list silently disagreeing about three models. A
    /// list here would rot the same way the moment `data/schema.json` grows a
    /// model. See the `catalog_matches_schema` test, which fails on drift the
    /// same way `metal-render::tests/schema.rs` does for the renderers.
    pub fn catalog(manifest: &metal_schema::Manifest) -> Vec<NodeKind> {
        let mut out = Vec::new();

        // A generator per live model in the manifest that is not a
        // transmission (`footage == false`). The one that is (`wake`) is
        // promoted to `fx.estela` instead. (`quimica`, the other, was retired
        // 2026-09-28 and is a tombstone, so it is not in the manifest at all.)
        //
        // Two outputs, not one. `image` is the picture, unchanged. `row`
        // is new: a second `Control`-typed output an `fx.*` node's own
        // `control` input can be wired from instead of `audio.*`, to mean
        // "inherit whatever this generator's row already computed for this
        // effect" rather than "read the raw digest column" — axis 2a vs 2b
        // of `the design notes` §3.2, resolved in `effects::control_source`.
        // It produces no pixels and is never on the image chain lint walks;
        // see `effects` module doc for the full reasoning and why this was
        // the cheapest way to make that choice fall out of the graph's own
        // typing instead of a config flag.
        for m in &manifest.models {
            if m.footage {
                continue;
            }
            // One material input per surface part: a picture from another node
            // laid on this model's part (`config.surfaces.<part>` on the
            // consumer; see `Paint::run`). Nothing wired = the model's own
            // colours.
            let mut inputs = vec![Port::new("control", PortType::Control)];
            for part in &m.surfaces {
                inputs.push(Port::material(&format!("tex.{part}"), PortType::Image));
            }
            out.push(NodeKind::new(
                &format!("gen.{}", m.id),
                NodeFamily::Generator,
                &m.label,
                inputs,
                vec![Port::new("image", PortType::Image), Port::new("row", PortType::Control)],
            ));
        }

        // 15 effects: the fourteen common ones, plus estela promoted from the
        // manifest's footage model. (`quimica` was the second and was retired
        // 2026-09-28.)
        for id in COMMON_EFFECT_IDS {
            out.push(effect_kind(id));
        }
        // The advanced effects, appended after the common ones in float order,
        // so the catalog and `EFFECTS_FLOATS` cannot disagree about what comes
        // where.
        for id in ADVANCED_EFFECT_IDS {
            out.push(effect_kind(id));
        }
        for id in PROMOTED_EFFECT_IDS {
            out.push(effect_kind(id));
        }
        // `fx.forge`: an effect whose WGSL is written at runtime (its config's
        // `shader`, a file under `data/shaders/`), run over the picture it is
        // handed — `metal_perform::forge`. Not a `Post` field and not a footage
        // model, so it is neither list above; the same ports as every effect.
        out.push(effect_kind(FORGE_EFFECT_ID));

        // K: the two motion effects. Same effect ports as any `fx.*`; their
        // work is on the picture (and, for `fx.speed`, on the clock) rather
        // than a `Post` float, so they are neither list above. See
        // `metal_perform::motion`.
        for id in MOTION_EFFECT_IDS {
            out.push(effect_kind(id));
        }

        // B: the two media effects of "The next leap". Same effect ports as
        // any `fx.*`; `fx.image-pop` also carries the `select` input port
        // `ai.curator` will feed in wave 2 (declared now, unwired today).
        for id in MEDIA_EFFECT_IDS {
            if *id == IMAGE_POP_EFFECT_ID {
                out.push(NodeKind::new(
                    &format!("fx.{id}"),
                    NodeFamily::Effect,
                    id,
                    vec![
                        Port::new("image", PortType::Image),
                        Port::new("control", PortType::Control),
                        Port::new("select", PortType::Control),
                    ],
                    vec![Port::new("image", PortType::Image)],
                ));
            } else {
                out.push(effect_kind(id));
            }
        }

        // Loops T12: `fx.warp` — works on the picture like `fx.move`, its
        // seven `0..1` params each under its own config key.
        out.push(effect_kind(WARP_EFFECT_ID));
        // Loops T13: `fx.flash`, two params under their own keys.
        out.push(effect_kind(FLASH_EFFECT_ID));

        // S.W: the per-glyph text effects, `fx.text.<name>`. They are
        // `Effect`-family (same stage as `fx.*`) but a separate namespace
        // because what they transform is a glyph, not a pixel; `lint` keeps
        // them hanging off a `text.source`. The vocabulary is declared once in
        // `metal_schema::TEXT_EFFECT_NAMES`.
        for id in TEXT_EFFECT_IDS {
            out.push(NodeKind::new(
                &format!("fx.text.{id}"),
                NodeFamily::Effect,
                id,
                vec![Port::new("image", PortType::Image), Port::new("control", PortType::Control)],
                vec![Port::new("image", PortType::Image)],
            ));
        }

        // The fixed kinds — one per row of the family table in §2/§13.
        out.push(NodeKind::new(
            "audio.live",
            NodeFamily::Audio,
            "Audio (live)",
            vec![],
            vec![Port::new("control", PortType::Control)],
        ));
        out.push(NodeKind::new(
            "audio.file",
            NodeFamily::Audio,
            "Audio (file)",
            vec![],
            vec![Port::new("control", PortType::Control)],
        ));
        out.push(NodeKind::new(
            "reel.clip",
            NodeFamily::Reel,
            "Reel (clip)",
            vec![Port::new("control", PortType::Control)],
            vec![Port::new("image", PortType::Image)],
        ));
        out.push(NodeKind::new(
            REEL_BATCH_KIND,
            NodeFamily::Reel,
            "Reel (batch)",
            vec![Port::new("control", PortType::Control)],
            vec![Port::new("image", PortType::Image)],
        ));
        out.push(NodeKind::new(
            "still",
            NodeFamily::Still,
            "Still image",
            vec![],
            vec![Port::new("image", PortType::Image)],
        ));
        out.push(NodeKind::new(
            SOLID_KIND,
            NodeFamily::Still,
            "Solid colour",
            vec![],
            vec![Port::new("image", PortType::Image)],
        ));
        out.push(NodeKind::new(
            "text.source",
            NodeFamily::Text,
            "Texto",
            // A `control` input, which `still` does not have: a title that
            // pulses with the music is the ordinary ask, and without the port
            // the only way to get it would be a text node feeding an `fx.*`
            // chain — which fades the letters as a picture rather than
            // driving how they are set.
            vec![Port::new("control", PortType::Control)],
            vec![Port::new("image", PortType::Image)],
        ));
        out.push(NodeKind::new(
            "capture",
            NodeFamily::Capture,
            "Capture",
            vec![],
            vec![Port::new("image", PortType::Image)],
        ));
        out.push(NodeKind::new(
            "router",
            NodeFamily::Router,
            "Router",
            // N image inputs — modeled as a single named port; the graph
            // allows multiple edges into it (see lint::compatible_ports and
            // compile's fan-in handling). A fixed catalog entry cannot
            // express "N" as a list of distinct port names because N is not
            // known until the graph is built.
            vec![Port::new("image", PortType::Image)],
            vec![Port::new("image", PortType::Image)],
        ));
        // O.6/S.G: the three script runtimes — one `kind` per runtime,
        // never a second field on this one. `control` in and `control`
        // out, same as an `fx.*` node's row: a script reads whatever row
        // feeds it and writes a row of its own, which is why it types
        // identically to the `gen.*` → `fx.*` `row` port rather than
        // inventing a third `PortType`. Its own floats (name/range/default)
        // and its source live in `Node.config`, read by `metal_perform::script`
        // (`run`/`run_rust`/`run_js`), not here — this crate only knows the
        // node exists and what it plugs into.
        //
        // `S.AL.3` adds a second input, `floats`: wiring a row into it
        // overrides the declared floats' live values *positionally*
        // (`row[i]` -> `floats[i]`, clamped), the same "reactive beats
        // fixed" rule `fx.*`'s `control` port already gives an effect's own
        // amount (`crate::effects::state_of`) — `script::evaluate` is the
        // reader. Also `PortType::Control`: a row of numbers, exactly what
        // `control` already carries, not a new port type for what is really
        // the same kind of cable.
        out.push(NodeKind::new(
            "script.py",
            NodeFamily::Script,
            "Script (Python)",
            vec![Port::new("control", PortType::Control), Port::new("floats", PortType::Control)],
            vec![Port::new("control", PortType::Control)],
        ));
        out.push(NodeKind::new(
            "script.rs",
            NodeFamily::Script,
            "Script (Rust)",
            vec![Port::new("control", PortType::Control), Port::new("floats", PortType::Control)],
            vec![Port::new("control", PortType::Control)],
        ));
        out.push(NodeKind::new(
            "script.js",
            NodeFamily::Script,
            "Script (JavaScript)",
            vec![Port::new("control", PortType::Control), Port::new("floats", PortType::Control)],
            vec![Port::new("control", PortType::Control)],
        ));
        // Native Rust presets (no subprocess): one beat-synced ease-out
        // envelope per note value, and a manual fader. Same ports as the
        // interpreted scripts, so a `floats` cable and param bindings work.
        for (id, label, _) in SCRIPT_BEAT_DIVISIONS {
            out.push(NodeKind::new(
                &format!("{SCRIPT_BEAT_PREFIX}{id}"),
                NodeFamily::Script,
                &format!("Beat {label}"),
                vec![Port::new("control", PortType::Control), Port::new("floats", PortType::Control)],
                vec![Port::new("control", PortType::Control)],
            ));
        }
        out.push(NodeKind::new(
            SCRIPT_FADER_KIND,
            NodeFamily::Script,
            "Fader",
            vec![Port::new("control", PortType::Control), Port::new("floats", PortType::Control)],
            vec![Port::new("control", PortType::Control)],
        ));
        // `control` in: the contour itself, live — `Stage::Mask`'s own
        // "the sense may arrive on `control` (from a script)". A row of
        // `x, y, x, y, …` in device space (0..1) fed by a `script.*` node's
        // `control` out replaces the named device mask's points for that
        // frame; `metal_perform::engine::apply_script_masks` is the reader,
        // and which device mask this node stands for is its config's
        // `mask` field — the same key a `gen.*` member node binds with.
        //
        // `stroke` in (22-sept-2026, tarde): the second source — an image,
        // a solid colour, a video, any animation — that fills the mask's
        // *outline* rather than its interior. Optional, image-typed, and
        // only meaningful with `config.stroke_width > 0`; the lint says so
        // rather than the editor, because a cable can be drawn in either
        // order (width set, then wired; or wired, then set).
        //
        // **A material port** (23-sept-2026, addendum D7): the upstream is a
        // secondary sink, so a node wired *only* here — never reaching an
        // `output.device` — still gets its pixels rendered and handed to the
        // stroke. `Port::material` is the declaration; `MATERIAL_PORTS` is
        // its runtime index.
        out.push(NodeKind::new(
            "mask",
            NodeFamily::Mask,
            "Mask",
            vec![
                Port::new("image", PortType::Image),
                Port::material("stroke", PortType::Image),
                Port::new("control", PortType::Control),
            ],
            vec![Port::new("image", PortType::Image)],
        ));
        // P1: the warp onto a venue surface. Same rank as `mask` (see
        // `NodeFamily::Venue`), so `mask → venue.surface → output.device` is
        // the ordinary chain. Its config is `{ "venue": "<name>", "surface":
        // "<surface id>" }`; this crate does not read it — the runtime reader
        // lives in `metal-perform`, the same seam `mask_node_binding` draws. The
        // device it lands on is read from the graph, never from the config.
        out.push(NodeKind::new(
            VENUE_SURFACE_KIND,
            NodeFamily::Venue,
            "Venue Surface",
            vec![Port::new("image", PortType::Image), Port::new("control", PortType::Control)],
            vec![Port::new("image", PortType::Image)],
        ));
        // **The wire after the device** (2026-10-22): the finished frame of
        // *this hardware output*, offered as `venue` for the room's grid.
        //
        // The chain is `gen → fx… → mask → venue.surface → output.device`;
        // the device is last (`the design notes`, "The room"). The user's report —
        // "the output device node should have a wire after for connecting to
        // the venue" — is answered in that order: the port is an **output**,
        // not an input. `venue.surface.image → output.device.image` stays the
        // only way to *drive* a screen; `output.device.venue →
        // venue.surface.image` is the room reading the device's frame back,
        // a **view** of the frame that was already composed (rule 3: "one
        // origin. An output is a view of the frame"). It renders nothing of
        // its own — `metal_perform` treats it as a terminal view, so the
        // frame is pixel-identical with and without it.
        //
        // Only `venue.surface` may read it (`lint_venue_device_port`): a
        // `venue.fixture` samples the frame it is fed, it does not take a
        // device's whole picture, and nothing else in the catalog takes an
        // `image` from an output. The reverse direction is not offered at
        // all — the device has no `image` output — so the graph can never
        // close the loop the wrong way; when a surface already carries the
        // device's picture (the ordinary chain), wiring the back edge too is
        // the cycle the lint refuses by name.
        out.push(NodeKind::new(
            "output.device",
            NodeFamily::OutputDevice,
            "Output (device)",
            vec![Port::new("image", PortType::Image), Port::new("control", PortType::Control)],
            vec![Port::new("venue", PortType::Image)],
        ));
        // The six destinations that are not screens. Every one takes
        // `signal` — the eight `sig_*` floats of `metal_schema::SIGNAL_FLOATS`,
        // arriving on a generator's `row` port — and turns them into whatever
        // the wire on the other side speaks.
        //
        // **Why `signal` is a `Control` port and not a third `PortType`.**
        // `Control` already means "a row of numbers describing what is
        // happening", which is exactly what these carry; the difference
        // between a digest column and a `sig_*` float is which column, not
        // which kind of thing. A third type would make `gen.*`'s single `row`
        // output either split in two or become untypeable, and would buy
        // nothing the port *name* does not already say.
        for (kind, label, image) in OUTPUT_SIGNAL_KINDS {
            let mut inputs = vec![Port::new("signal", PortType::Control)];
            // DDP and sACN are the two that genuinely carry pixels: a bulb
            // string and an LED bar are addressed as a strip of RGB, so the
            // operator can feed one an image and have it sampled across the
            // fixtures. The other four have no pixel meaning at all — a MIDI
            // note is not a scanline — and giving them an `image` port would
            // invite a cable that can never do anything.
            if *image {
                inputs.push(Port::new("image", PortType::Image));
            }
            out.push(NodeKind::new(kind, NodeFamily::OutputSignal, label, inputs, vec![]));
        }

        out.push(NodeKind::new(
            "export.video",
            NodeFamily::ExportVideo,
            "Export (video)",
            vec![Port::new("image", PortType::Image)],
            vec![],
        ));
        out.push(NodeKind::new(
            "sensor.yolo",
            NodeFamily::Sensor,
            "Sensor (YOLO)",
            vec![Port::new("image", PortType::Image)],
            vec![Port::new("control", PortType::Control)],
        ));
        out.push(NodeKind::new(
            "cam.track",
            NodeFamily::CameraTrack,
            "Camera track",
            vec![Port::new("control", PortType::Control)],
            // Does not draw an image — see §13.2: it writes into a
            // generator's reserved floats by config association, not by an
            // image edge.
            vec![],
        ));
        // The colour family (K): a colour and a palette as sources of control,
        // each with one `control` output the operator cables to a colour slot
        // (a `param:` binding). `PortType::Control`, not a third port type: a
        // colour is a small row of numbers that drives a parameter, exactly
        // what `control` already carries — see `NodeFamily::Colour`.
        out.push(NodeKind::new(
            COLOR_KIND,
            NodeFamily::Colour,
            "Colour",
            vec![],
            vec![Port::new("control", PortType::Control)],
        ));
        out.push(NodeKind::new(
            PALETTE_KIND,
            NodeFamily::Colour,
            "Palette",
            vec![],
            vec![Port::new("control", PortType::Control)],
        ));

        // The trained reaction (S.AM, wave 1): a digest row in, a row of
        // signed offsets out, one per own float of whatever `gen.*` consumes
        // it. `control` in and `control` out — the same port type the digest
        // and a script already carry, because the offsets are numbers that
        // drive a float, not pixels. The three kinds are separate nodes, not
        // one combo inside the source: a fan-out evaluates per schema, and
        // the plan's `ai.router`/`ai.curator` arrive in their own batches.
        out.push(NodeKind::new(
            AI_COLOURIST_KIND,
            NodeFamily::Ai,
            "Colourist",
            vec![Port::new("control", PortType::Control)],
            vec![Port::new("control", PortType::Control)],
        ));

        // LAY.7b: the composition — one kind per `DeviceMode` the bridge
        // table maps, so the render path can read *which* composition applies
        // off the graph. LAY.7c gave it §2.1's mixer shape: N images in (one
        // `image` port with fan-in, as `router` models it) and one image out.
        // The members are the node's inputs; the mixed frame is its output.
        for (kind, label) in COMPOSE_KINDS {
            out.push(NodeKind::new(
                kind,
                NodeFamily::Compose,
                label,
                vec![Port::new("image", PortType::Image)],
                vec![Port::new("image", PortType::Image)],
            ));
        }

        out.push(NodeKind::new(
            "stack.mode",
            NodeFamily::Stack,
            "Stack (mode)",
            // No ports at all, in either direction. See `NodeFamily::Stack`:
            // this node is read before there is a chain to be part of.
            vec![],
            vec![],
        ));

        // M3 of the pixel map: the venue node that drives the whole rig. One
        // `image` in, no output — it samples the frame at every fixture
        // footprint of the venue it names (or one group of them) and sends
        // each fixture's wire. Config `{ "venue": "<id>", "group": "<name>" }`.
        // Appended 2026-10-07; P1's `venue.surface` is the other kind in this
        // family.
        out.push(NodeKind::new(
            VENUE_FIXTURES_KIND,
            NodeFamily::Venue,
            "Venue fixtures",
            vec![Port::new("image", PortType::Image)],
            vec![],
        ));

        // Fixtures F1: one node per venue fixture (decision 2). The group node
        // above drives a whole rig from the picture; this drives one row from
        // the frame, a model's colour slots (`colour`) or a `sig_*` row
        // (`signal`). Like the group node it is a wire terminal — nothing out —
        // and a **pointer** at the venue row, never a copy of its address,
        // universe, profile or pose (decision 1).
        out.push(NodeKind::new(
            VENUE_FIXTURE_KIND,
            NodeFamily::Venue,
            "Venue fixture",
            vec![
                Port::new("image", PortType::Image),
                Port::new("colour", PortType::Control),
                Port::new("signal", PortType::Control),
            ],
            vec![],
        ));

        // Parity lote P-A (2026-10-11), appended: the five image effects
        // (`fx.chroma_key` … `fx.projection`), the `gen.ramp` source and the
        // `ctl.math` control node — see `parity`.
        out.extend(parity::kinds());

        // M1 of the show-control lote (plan "Value sources"), appended: the
        // `source.*` value nodes (`source.osc`, `source.const`,
        // `source.random`) — pure CPU control sources at `Stage::Source`, not
        // models and not GPU passes. See `source`.
        out.extend(source::kinds());

        // M4 of the materials lote: the `mat.*` surface nodes (`mat.wood`,
        // `mat.glass`, …), a new family beside `fx.*` with the same chain
        // shape. See `materials`.
        out.extend(materials::kinds());

        // Parity lote P-D (2026-10-11), appended: SMPTE timecode in (LTC /
        // MTC / ArtTimeCode). No ports: what it drives is the sequence
        // playhead, not a row — `metal_perform::timecode`.
        out.push(NodeKind::new(CLOCK_TIMECODE_KIND, NodeFamily::Audio, "Timecode (SMPTE)", vec![], vec![]));
        // P-D, appended: Art-Net in. A control row of `count` values (`out0…`),
        // read by bindings like any live-sized source (`control.out3`).
        out.push(NodeKind::new(
            ARTNET_IN_KIND,
            NodeFamily::Input,
            "DMX in (Art-Net)",
            vec![],
            vec![Port::new("control", PortType::Control)],
        ));

        // Every kind's own parameters (the rows a binding can drive), read off
        // the same manifest — keep this the last step so a kind pushed
        // anywhere above is stamped too.
        params::stamp(&mut out, manifest);
        out
    }
}

/// The single-colour kind (K): config `{ "rgb": [r, g, b] }` (0..1) — the
/// swatch the canvas paints and the value its `rgb` variable offers.
/// Native script presets (`metal_perform::script_preset`): `script.beat.<id>`
/// sweeps its output over one note value of the song's tempo, `script.fader`
/// holds a manual value. `(id, label, length in whole notes / bars of 4/4)`.
/// P-D: the timecode source (`metal_perform::timecode`).
pub const CLOCK_TIMECODE_KIND: &str = "clock.timecode";

pub const SCRIPT_BEAT_PREFIX: &str = "script.beat.";
pub const SCRIPT_FADER_KIND: &str = "script.fader";
pub const SCRIPT_BEAT_DIVISIONS: [(&str, &str, f64); 8] = [
    ("1_16", "1/16", 0.0625),
    ("1_8", "1/8", 0.125),
    ("1_4", "1/4", 0.25),
    ("1_2", "1/2", 0.5),
    ("1", "1", 1.0),
    ("2", "2", 2.0),
    ("4", "4", 4.0),
    ("8", "8", 8.0),
];

/// The floats a preset kind declares: `(name, min, max, default)`. `None`
/// for any other kind. A new preset node is seeded with exactly these, so its
/// properties panel is never empty.
pub fn script_preset_floats(kind: &str) -> Option<&'static [(&'static str, f32, f32, f32)]> {
    const BEAT: &[(&str, f32, f32, f32)] = &[
        ("low", 0.0, 1.0, 0.0),
        ("high", 0.0, 1.0, 1.0),
        ("ease", 1.0, 8.0, 3.0),
        ("fall", 0.0, 1.0, 1.0),
        ("phase", 0.0, 1.0, 0.0),
        ("bpm", 0.0, 300.0, 0.0),
    ];
    const FADER: &[(&str, f32, f32, f32)] =
        &[("value", 0.0, 1.0, 0.0), ("low", 0.0, 1.0, 0.0), ("high", 0.0, 1.0, 1.0)];
    if kind == parity::CTL_MATH_KIND {
        return Some(parity::CTL_MATH_FLOATS);
    }
    if kind == SCRIPT_FADER_KIND {
        Some(FADER)
    } else if kind.strip_prefix(SCRIPT_BEAT_PREFIX).is_some_and(|d| SCRIPT_BEAT_DIVISIONS.iter().any(|b| b.0 == d)) {
        Some(BEAT)
    } else {
        None
    }
}

pub const COLOR_KIND: &str = "color";

/// The multi-source reel (2026-10-01): an ordered list of videos, stills,
/// saved/favourite scenes and whole favourites groups, stepped by its
/// `interval`/`unit`/`order`/`seed` floats, each held under its own config
/// key beside `items` — `metal_perform::reel_batch` reads both.
/// The solid-colour source: one flat `rgb` over the frame (`{"rgb":[r,g,b]}`),
/// what a timeline colour clip resolves to.
pub const SOLID_KIND: &str = "solid";
pub const REEL_BATCH_KIND: &str = "reel.batch";

/// The palette kind (K): config `{ "palette": "<id>" | null, "colors":
/// [[r,g,b], …] }` — the node holds its resolved colours so the canvas can
/// paint them without reading a second file, and its slots are the variables a
/// consumer binds to. Linking it to a stored palette keeps the two in step
/// through `POST /palettes`.
pub const PALETTE_KIND: &str = "palette";

/// The first [`NodeFamily::Ai`] kind: a fitted [`metal_reel::grade`]-style grade
/// over the digest's sixteen columns, offered as `control` offsets. Named once
/// so the catalog, the lint and the engine cannot spell it three ways. Its
/// config is `{ "weights": "<name>" | null }`; `null` uses the downstream
/// model's default weights.
pub const AI_COLOURIST_KIND: &str = "ai.colourist";

/// P-D: the Art-Net receiver (`metal_perform::artnet::in_row`). Its row is
/// the latest ArtDmx on one universe, `first..first+count` as floats 0..1.
pub const ARTNET_IN_KIND: &str = "in.artnet";

/// The first [`NodeFamily::Venue`] kind (P1): the warp onto one surface of
/// the venue. Named once so the catalog and the lint cannot spell it two
/// ways. Its config is `{ "venue": "<name>", "surface": "<surface id>" }`;
/// the runtime reader lives in `metal-perform` (the `mask_node_binding` seam),
/// and the `output.device` it lands on is read from the graph, never the
/// config.
///
/// **One node is one output's whole grid, and its `image` input fans in.**
/// N `image` edges arrive at the single named port (the same convention
/// `router` and the `compose.*` mixers use: N is not known until the graph is
/// built, so a fixed port list cannot spell it). The node does not mix them;
/// it divides its surface by the surface's own `sectors` and hands the `k`-th
/// source the `k`-th cell, in edge order — so one surface can carry many
/// generators or variations at once.
pub const VENUE_SURFACE_KIND: &str = "venue.surface";

/// The pixel-map node (M3): `image` in, it samples the frame at every fixture
/// footprint of the venue it names (or one group of them) and sends each
/// fixture's wire. Config `{ "venue": "<id>", "group": "<name>" }`. The
/// runtime lives in `metal_perform::wire` (a graph kind cannot send by itself);
/// the kind exists here so the canvas, the lint and the menu share one name.
pub const VENUE_FIXTURES_KIND: &str = "venue.fixtures";

/// The per-fixture node (fixtures F1): `image` in (the composed frame it
/// samples), a `colour` control (a model's driven colour slots) and a
/// `signal` (a `sig_*` row) in, nothing out. Config is a **pointer** at one
/// row of a venue — `{ "venue": id, "fixture": id }` — plus how its colour is
/// chosen (`colour_from`/`spread`/`pick`/`offset`/`hue_lock`) and how it
/// writes the bus (`priority`/`mode`); never a copy of the row's address,
/// universe, profile or pose (decision 1). Rename or delete the row and the
/// node follows; an orphan is a lint warning and a runtime skip, never a
/// crash and never a send. The runtime lives in `metal_perform::wire` (a graph
/// kind cannot send); the kind exists here so the canvas, the lint and the
/// menu share one name.
pub const VENUE_FIXTURE_KIND: &str = "venue.fixture";
/// The stored palette slots (K), in the fixed order `metal_config::PaletteDef`
/// materialises: `base, acento, fondo, destello`, then `extra` slots numbered
/// from there. **The keys are the stored ones** — a later pass renames them
/// with aliases — so `data/palettes.json` and a palette node agree on what
/// `c2`/`destello` means. One list so the catalog, the params, the variables
/// and the canvas cannot spell a slot four ways.
pub const PALETTE_SLOTS: &[&str] = &["base", "accent", "background", "flash"];

/// The variable name of the `k`-th palette slot in a palette node's `control`
/// output — `base`, `acento`, … for the stored four, `extra<k>` past them,
/// where a palette's `extra` list continues. The inverse of
/// [`palette_slot_index`].
pub fn palette_slot_var(index: usize) -> String {
    PALETTE_SLOTS.get(index).map(|s| s.to_string()).unwrap_or_else(|| format!("extra{}", index - PALETTE_SLOTS.len()))
}

/// The slot index a palette variable names, or `None` for anything else.
pub fn palette_slot_index(var: &str) -> Option<usize> {
    if let Some(i) = PALETTE_SLOTS.iter().position(|s| *s == var) {
        return Some(i);
    }
    let n: usize = var.strip_prefix("extra")?.parse().ok()?;
    (format!("extra{n}") == var).then_some(PALETTE_SLOTS.len() + n)
}

/// The one kind in [`NodeFamily::Stack`], named once so the app, the lint and
/// the catalog cannot spell it three ways.
///
/// **Legacy since LAY.7b**: the render path reads a [`COMPOSE_KINDS`] node
/// first and falls back to this one, so a graph saved before the migration
/// still renders. Nothing writes a new one — see [`COMPOSE_KINDS`].
pub const STACK_MODE_KIND: &str = "stack.mode";

/// The three [`NodeFamily::Compose`] kinds, `(kind, label)`, in the order the
/// bridge table in `docs/plan-architecture.md` §2.4 names them.
///
/// One list rather than a kind literal at each of the four readers (catalog,
/// lint, migration, `metal-dash`'s composition reader), for the same reason
/// [`OUTPUT_SIGNAL_KINDS`] is one list: adding a composition is one row in one
/// place, and no consumer can be the one that forgot it.
///
/// `compose.off` is deliberately absent. §2.4 maps `Disabled` to *the node
/// retired*: a device that shows nothing declares no composition at all, so
/// "off" is the absence of a compose node rather than a fourth kind that would
/// have to be kept in step with it.
pub const COMPOSE_KINDS: &[(&str, &str)] = &[
    ("compose.automix", "Composite (automix)"),
    ("compose.collection", "Composite (collection)"),
    ("compose.layers", "Compósito (capas)"),
];

/// The `compose.*` kind a `DeviceMode` resolves to, and back — the bridge
/// table of `docs/plan-architecture.md` §2.4 as code, so the migration, the route
/// and the reader cannot spell the mapping three ways.
///
/// `DeviceMode` lives in `metal-dash` and this crate must not depend on it (the
/// crate doc's one rule), so the mapping is over the mode's own vocabulary —
/// the same `{"kind": ..., "id": ...}` JSON `DeviceMode::describe` emits — and
/// both sides go through [`compose_kind_for_mode`]/[`mode_for_compose_kind`].
pub fn compose_kind_for_mode(kind: &str) -> Option<&'static str> {
    match kind {
        "automix" => Some("compose.automix"),
        "collection" => Some("compose.collection"),
        "own" => Some("compose.layers"),
        // `disabled` is the retired node — no compose kind exists for it.
        _ => None,
    }
}

/// The inverse of [`compose_kind_for_mode`]: the mode `kind` a `compose.*`
/// node stands for, or `None` for a node that is not a composition.
pub fn mode_for_compose_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "compose.automix" => Some("automix"),
        "compose.collection" => Some("collection"),
        "compose.layers" => Some("own"),
        _ => None,
    }
}

/// Whether `kind` is one of the [`COMPOSE_KINDS`].
pub fn is_compose_kind(kind: &str) -> bool {
    COMPOSE_KINDS.iter().any(|(k, _)| *k == kind)
}

/// The [`NodeFamily::OutputSignal`] kinds: `(kind, label, takes an image)`.
///
/// A list rather than a match arm so the same order reaches the catalog, the
/// menu that draws it and the lint, and so adding a seventh protocol is one
/// line in one place. Ordered as the ask names them — the two bulb transports
/// first, then the four DMX ones.
const OUTPUT_SIGNAL_KINDS: &[(&str, &str, bool)] = &[
    // Distributed Display Protocol: UDP, one datagram of RGB triples per
    // strip. What most screw-in smart bulbs and LED controllers speak natively.
    ("out.ddp", "Ampolletas (DDP)", true),
    // The same fixtures reached the other way: a JSON body per state change,
    // for a bulb behind a vendor's local HTTP API rather than a raw socket.
    // Slower and chattier; the one that works without a bridge.
    ("out.json", "Ampolletas (JSON)", false),
    // E1.31 / sACN: DMX512 over the network, 512 channels to a universe.
    // Image-capable for the same reason DDP is — a bar of cells is a strip.
    ("out.sacn", "DMX (E1.31 sACN)", true),
    // Open Sound Control: addressed messages to consoles and media servers.
    ("out.osc", "DMX (OSC)", false),
    // A plain request per change, for the fixtures and bridges that only
    // offer a web endpoint.
    ("out.http", "DMX (HTTP)", false),
    // Notes and CC, for instruments and for consoles that listen on MIDI
    // rather than on a network at all.
    ("out.midi", "DMX (MIDI)", false),
    // Bluetooth output for wireless light fixtures and controllers. Since BLE
    // Phase B this kind has a real backend in `metal_perform::wire`: the node's
    // config names a fixture profile and `metal-ble` drives it (Godox Bluetooth
    // Mesh). Not image-capable — a lamp takes a colour, not a framebuffer.
    ("out.bluetooth", "Bluetooth (Godox Mesh)", false),
    // A USB-serial LED strip speaking Adalight (`metal_perform::wire::Adalight`):
    // RGB per LED, sampled across the strip like DDP and sACN, so it takes
    // an image. Config: `port`, `baud`, `pixels`, `order`, `sample`.
    ("out.serial", "LED strip (USB serial, Adalight)", true),
    // P-D: Art-Net 4 (`metal_perform::artnet`): ArtDmx to a 15-bit
    // Port-Address, unicast or broadcast, optional ArtSync; image-capable
    // like sACN (frame-driven `heads`). Config: `net`, `subnet`, `universe`,
    // `unicast`, `broadcast`, `sync`.
    ("out.artnet", "DMX (Art-Net)", true),
];

pub(crate) fn effect_kind(id: &str) -> NodeKind {
    NodeKind::new(
        &format!("fx.{id}"),
        NodeFamily::Effect,
        id,
        vec![Port::new("image", PortType::Image), Port::new("control", PortType::Control)],
        vec![Port::new("image", PortType::Image)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> metal_schema::Manifest {
        let text = metal_vfs::read_to_string(schema_path()).expect("data/schema.json readable");
        metal_schema::Manifest::parse(&text).expect("schema.json parses")
    }

    fn schema_path() -> std::path::PathBuf {
        // This crate has no data dir of its own; it reads the repo's real
        // schema.json, same as `metal-render::tests/schema.rs` does for the
        // renderers.
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json")
    }

    // -- A1 -----------------------------------------------------------------

    #[test]
    fn ids_are_never_reused_after_a_delete() {
        let mut g = Graph::new("g1", "test", Domain::Live);
        let a = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        g.remove_node(&a);
        let b = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        assert_ne!(a, b, "a fresh node after a delete must not reuse the deleted id");
    }

    #[test]
    fn removing_a_node_removes_its_edges() {
        let mut g = Graph::new("g1", "test", Domain::Live);
        let a = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let b = g.add_node("out", "output.device", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new(&b, "image"));
        assert_eq!(g.edges.len(), 1);
        g.remove_node(&a);
        assert!(g.edges.is_empty(), "an edge touching a deleted node must not survive it");
    }

    #[test]
    fn default_bindings_land_where_an_upstream_var_offers_them() {
        let manifest = sample_manifest();
        let catalog = NodeKind::catalog(&manifest);
        let mut g = Graph::new("g1", "test", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&gen, "control"));
        let landed = g.apply_default_bindings(
            &gen,
            &[
                ("scale", "onset"),
                ("flow_force", "rms"),
                ("nope", "onset"),
                ("scale", "no-such-var"),
            ],
            &catalog,
        );
        assert_eq!(landed, 2, "the two real params land; the unknown ones do not");
        let b = crate::bind::bindings(&g, &gen);
        assert!(b.iter().any(|b| b.param == "scale" && b.var == "onset"));
        assert!(b.iter().any(|b| b.param == "flow_force" && b.var == "rms"));
    }

    // -- add_edge_checked -------------------------------------------------

    /// The bug an operator actually hit: `output.device` starts with no
    /// `control` edge, which `lint_output_device_control` already reports
    /// as an error the moment the node exists — before any of this test's
    /// edges go in. Wiring an *unrelated* `image` edge onto it must not be
    /// refused for that pre-existing, unrelated reason; only for an error
    /// the new edge itself introduces.
    #[test]
    fn add_edge_checked_does_not_refuse_an_edge_for_a_pre_existing_unrelated_error() {
        let manifest = sample_manifest();
        let catalog = NodeKind::catalog(&manifest);
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [1.0, 0.0]);
        // `out` has no `control` edge yet — already an error before this
        // edge goes in.
        let result = g.add_edge_checked(
            Endpoint::new(&gen, "image"),
            Endpoint::new(&out, "image"),
            &catalog,
        );
        assert!(
            result.is_ok(),
            "an unrelated pre-existing error must not block this edge: {result:?}"
        );
        assert_eq!(g.edges.len(), 1);
    }

    /// The other half: an edge that introduces a *new* error — here, a
    /// cycle — is still refused and rolled back, exactly as before this
    /// fix.
    #[test]
    fn add_edge_checked_still_refuses_an_edge_that_introduces_a_new_error() {
        let manifest = sample_manifest();
        let catalog = NodeKind::catalog(&manifest);
        let mut g = Graph::new("g", "t", Domain::Live);
        let a = g.add_node("a", "fx.shake", [0.0, 0.0]);
        let b = g.add_node("b", "fx.shake", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new(&b, "image"));
        let before = g.edges.len();
        let result =
            g.add_edge_checked(Endpoint::new(&b, "image"), Endpoint::new(&a, "image"), &catalog);
        assert!(result.is_err(), "a cycle is a new error this edge itself causes");
        assert_eq!(g.edges.len(), before, "the refused edge must be rolled back");
    }

    /// `fx -> fx` (an effect chained into another effect) is a valid
    /// pairing on its own — every `fx.*` kind takes an `image` input and
    /// gives an `image` output (`effect_kind`), so chaining two is not a
    /// special case the lint should ever object to.
    #[test]
    fn add_edge_checked_allows_fx_to_fx() {
        let manifest = sample_manifest();
        let catalog = NodeKind::catalog(&manifest);
        let mut g = Graph::new("g", "t", Domain::Live);
        let a = g.add_node("a", "fx.shake", [0.0, 0.0]);
        let b = g.add_node("b", "fx.shake", [1.0, 0.0]);
        let result =
            g.add_edge_checked(Endpoint::new(&a, "image"), Endpoint::new(&b, "image"), &catalog);
        assert!(result.is_ok(), "{result:?}");
    }

    /// The full chain the operator is actually after — generator into an
    /// effect into an output device — wired one edge at a time, `image`
    /// before `control` (the order a cable-dragging operator would
    /// naturally use: the picture first). Each step must succeed on its
    /// own, and the finished graph must lint clean.
    #[test]
    fn add_edge_checked_lets_image_and_control_land_in_either_order() {
        let manifest = sample_manifest();
        let catalog = NodeKind::catalog(&manifest);
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        assert!(g
            .add_edge_checked(Endpoint::new(&fx, "image"), Endpoint::new(&out, "image"), &catalog)
            .is_ok());
        assert!(g
            .add_edge_checked(
                Endpoint::new(&audio, "control"),
                Endpoint::new(&out, "control"),
                &catalog
            )
            .is_ok());
        assert!(g.lint(&catalog).iter().all(|i| i.severity != Severity::Error));
    }

    /// A `mask` node accepts an `image` edge the same as an effect does —
    /// masks sit in the same `image` chain, not off to the side.
    #[test]
    fn add_edge_checked_allows_fx_to_mask() {
        let manifest = sample_manifest();
        let catalog = NodeKind::catalog(&manifest);
        let mut g = Graph::new("g", "t", Domain::Live);
        let fx = g.add_node("fx", "fx.shake", [0.0, 0.0]);
        let mask = g.add_node("mask", "mask", [1.0, 0.0]);
        let result = g.add_edge_checked(
            Endpoint::new(&fx, "image"),
            Endpoint::new(&mask, "image"),
            &catalog,
        );
        assert!(result.is_ok(), "{result:?}");
    }

    // -- A2 -------------------------------------------------------------

    #[test]
    fn catalog_generators_track_the_manifest_and_effects_the_lists() {
        let manifest = sample_manifest();
        let catalog = NodeKind::catalog(&manifest);
        let generators = catalog.iter().filter(|k| k.family == NodeFamily::Generator).count();
        let effects = catalog.iter().filter(|k| k.family == NodeFamily::Effect).count();
        // Counted from data/schema.json rather than written down: the literal
        // that stood here (34) went stale the day `forge` was appended.
        let generative = manifest.models.iter().filter(|m| !m.footage).count();
        assert_eq!(generators, generative + 1, "one gen.* per non-footage model in data/schema.json, plus gen.ramp");
        assert!(catalog.iter().any(|k| k.kind == "gen.forge"), "the runtime-shader generator");
        assert_eq!(
            effects,
            COMMON_EFFECT_IDS.len()
                + ADVANCED_EFFECT_IDS.len()
                + PROMOTED_EFFECT_IDS.len()
                + 1
                + TEXT_EFFECT_IDS.len()
                + MOTION_EFFECT_IDS.len()
                + MEDIA_EFFECT_IDS.len()
                + 2
                + PARITY_EFFECT_IDS.len(),
            "the common effects + the advanced effects, estela promoted, fx.forge, the \
             S.W per-glyph vocabulary, the K motion pair (fx.speed/fx.move), and the B \
             media pair (fx.particles/fx.image-pop), fx.warp, fx.flash"
        );
    }

    #[test]
    fn every_signal_output_takes_a_signal_and_draws_nothing() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let outs: Vec<&NodeKind> =
            catalog.iter().filter(|k| k.family == NodeFamily::OutputSignal).collect();
        assert_eq!(outs.len(), OUTPUT_SIGNAL_KINDS.len(), "six protocols, all published");
        for k in &outs {
            assert!(
                k.inputs.iter().any(|p| p.name == "signal" && p.ty == PortType::Control),
                "{} has no signal input — nothing could reach it",
                k.kind
            );
            assert!(k.outputs.is_empty(), "{} is a destination, not a stage", k.kind);
        }
    }

    #[test]
    fn only_the_two_strip_protocols_accept_an_image() {
        // DDP and sACN address a run of fixtures as a strip of RGB, so an
        // image sampled across them means something. A MIDI note is not a
        // scanline: an `image` port there would be a cable that can be drawn
        // and can never do anything, which is worse than not offering it.
        let catalog = NodeKind::catalog(&sample_manifest());
        let takes_image = |kind: &str| {
            catalog
                .iter()
                .find(|k| k.kind == kind)
                .unwrap_or_else(|| panic!("{kind} missing from the catalog"))
                .inputs
                .iter()
                .any(|p| p.ty == PortType::Image)
        };
        assert!(takes_image("out.ddp"));
        assert!(takes_image("out.sacn"));
        for kind in ["out.json", "out.osc", "out.http", "out.midi", "out.bluetooth"] {
            assert!(!takes_image(kind), "{kind} took an image it cannot use");
        }
    }

    #[test]
    fn a_generators_row_can_reach_a_signal_output_and_its_image_cannot() {
        // The wiring this whole family exists for, asserted through the port
        // types rather than through a comment: the eight `sig_*` floats leave
        // a generator on `row`, which is `Control`, and land on `signal`,
        // which is `Control`. The picture goes somewhere else entirely.
        let catalog = NodeKind::catalog(&sample_manifest());
        let gen = catalog.iter().find(|k| k.kind == "gen.turing").expect("turing");
        let row = gen.outputs.iter().find(|p| p.name == "row").expect("row port");
        let midi = catalog.iter().find(|k| k.kind == "out.midi").expect("out.midi");
        let signal = midi.inputs.iter().find(|p| p.name == "signal").expect("signal port");
        assert_eq!(row.ty, signal.ty, "row must land on signal without a conversion");

        let image = gen.outputs.iter().find(|p| p.name == "image").expect("image port");
        assert_ne!(image.ty, signal.ty, "pixels are not a light cue");
    }

    #[test]
    fn catalog_fails_if_schema_json_gains_or_loses_a_generator() {
        // Simulate drift: a manifest missing one non-footage model.
        let before = sample_manifest();
        let before_generators = NodeKind::catalog(&before)
            .iter()
            .filter(|k| k.family == NodeFamily::Generator)
            .count();
        let mut manifest = before;
        let removed = manifest.models.iter().position(|m| !m.footage).expect("has a generator");
        manifest.models.remove(removed);
        let catalog = NodeKind::catalog(&manifest);
        let generators = catalog.iter().filter(|k| k.family == NodeFamily::Generator).count();
        assert_ne!(
            generators, before_generators,
            "the catalog must move with the manifest, proving it isn't a hand-written list"
        );
    }

    // -- K: the colour family -------------------------------------------------

    /// The two colour kinds are at `Stage::Source`, each with one `control`
    /// output and nothing else — a source of control, not of pictures.
    #[test]
    fn the_colour_family_is_two_sources_of_control() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let colour: Vec<&NodeKind> =
            catalog.iter().filter(|k| k.family == NodeFamily::Colour).collect();
        assert_eq!(colour.len(), 2, "a colour and a palette");
        for (kind, label) in [(COLOR_KIND, "Colour"), (PALETTE_KIND, "Palette")] {
            let k = colour.iter().find(|k| k.kind == kind).unwrap_or_else(|| panic!("{kind} missing"));
            assert_eq!(k.label, label);
            assert_eq!(k.stage(), Stage::Source, "{kind} is where a value comes from");
            assert!(k.menu, "{kind} must be reachable from the menu");
            assert!(k.inputs.is_empty(), "{kind} is a source: nothing feeds it");
            assert_eq!(k.outputs.len(), 1, "{kind} offers exactly one thing");
            assert_eq!((k.outputs[0].name.as_str(), k.outputs[0].ty), ("control", PortType::Control));
            assert!(k.capabilities().has(Capability::ControlOut));
            assert!(!k.capabilities().has(Capability::ImageOut), "{kind} draws no picture");
            assert!(!k.capabilities().has(Capability::NeedsGpu), "{kind} needs no GPU");
        }
    }

    /// K: a colour source is a *control* source, so `Colour → consumer` is a
    /// plain forward edge — a colour may feed a generator's `control` input
    /// without a stage exception.
    #[test]
    fn a_colour_source_is_a_forward_edge_into_source_and_modulate() {
        assert!(stage_edge_allowed(Stage::Source, Stage::Source).is_ok());
        assert!(stage_edge_allowed(Stage::Source, Stage::Modulate).is_ok());
    }

    // -- S.AM: the trained reaction (`NodeFamily::Ai`) -----------------------

    /// The first `ai.*` kind is a source of control at `Stage::Source`: a
    /// `control` in and a `control` out, `HasAux` for its D1 inference, and
    /// neither `NeedsGpu` nor `NeedsAudio`.
    #[test]
    fn the_ai_family_is_a_control_source_with_aux() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let ai: Vec<&NodeKind> = catalog.iter().filter(|k| k.family == NodeFamily::Ai).collect();
        assert_eq!(ai.len(), 1, "wave 1 ships the colourist; router and curator follow");
        let k = ai.iter().find(|k| k.kind == AI_COLOURIST_KIND).expect("ai.colourist");
        assert_eq!(k.label, "Colourist");
        assert_eq!(k.stage(), Stage::Source, "a source of control, like a colour");
        assert!(!k.menu, "the colourist is a gen.* sub-feature, never a menu item");
        assert_eq!(k.inputs.len(), 1);
        assert_eq!((k.inputs[0].name.as_str(), k.inputs[0].ty), ("control", PortType::Control));
        assert_eq!(k.outputs.len(), 1);
        assert_eq!((k.outputs[0].name.as_str(), k.outputs[0].ty), ("control", PortType::Control));
        let c = k.capabilities();
        assert!(c.has(Capability::ControlIn) && c.has(Capability::ControlOut));
        assert!(c.has(Capability::HasAux), "its inference is an AuxLogic (D1)");
        assert!(!c.has(Capability::NeedsGpu), "it runs on the CPU before drawing");
        assert!(!c.has(Capability::NeedsAudio), "the digest arrives over the cable");
    }

    // -- LAY.1: the stage contract (`docs/plan-architecture.md` §2) ---------------

    #[test]
    fn every_catalog_kind_maps_to_exactly_one_stage() {
        let catalog = NodeKind::catalog(&sample_manifest());
        assert!(!catalog.is_empty());
        let mut seen = std::collections::BTreeSet::new();
        for k in &catalog {
            let stage = k.stage();
            assert!(Stage::ALL.contains(&stage), "{} mapped off the grid", k.kind);
            seen.insert(stage);
        }
        // Every stage has at least one kind — `Compose` included since
        // LAY.7b gave the seat its first family. If a family is ever added
        // without a `NodeFamily::stage` arm, this and the match's
        // exhaustiveness both fail.
        for stage in Stage::ALL {
            assert!(seen.contains(stage), "{stage:?} has no kind in the catalog");
        }
    }

    // -- LAY.7b: the compose family (`docs/plan-architecture.md` §2.4) -----------

    #[test]
    fn the_catalog_carries_one_compose_kind_per_mode_in_the_bridge_table() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let compose: Vec<&NodeKind> =
            catalog.iter().filter(|k| k.family == NodeFamily::Compose).collect();
        assert_eq!(compose.len(), COMPOSE_KINDS.len());
        for (kind, _) in COMPOSE_KINDS {
            let k = compose.iter().find(|k| k.kind == *kind).unwrap_or_else(|| {
                panic!("{kind} missing from the catalog — the bridge table and the catalog disagree")
            });
            assert_eq!(k.stage(), Stage::Compose, "{kind} must sit at Stage::Compose");
            // LAY.7c: §2.1's N-in/1-out mixer shape. N is one named `image`
            // port with fan-in (the `router` convention), not a list of port
            // names — the graph, not the catalog, knows how many members
            // there are.
            assert_eq!(k.inputs.len(), 1, "{kind} must take the mixer's N images");
            assert_eq!(k.inputs[0].name, "image");
            assert_eq!(k.inputs[0].ty, PortType::Image);
            assert_eq!(k.outputs.len(), 1, "{kind} must emit the mixed frame");
            assert_eq!(k.outputs[0].name, "image");
            assert_eq!(k.outputs[0].ty, PortType::Image);
        }
    }

    /// M3 of the pixel map: the `venue.fixtures` kind is in the catalog — one
    /// `image` in, no output, family `Venue`, stage `Output`, Live only.
    #[test]
    fn the_catalog_carries_the_pixel_map_node() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let k = catalog
            .iter()
            .find(|k| k.kind == VENUE_FIXTURES_KIND)
            .expect("venue.fixtures is in the catalog");
        assert_eq!(k.family, NodeFamily::Venue);
        assert_eq!(k.stage(), Stage::Output);
        assert_eq!(k.inputs.len(), 1);
        assert_eq!(k.inputs[0].name, "image");
        assert_eq!(k.inputs[0].ty, PortType::Image);
        assert!(k.outputs.is_empty());
        assert_eq!(k.domains, &[Domain::Live]);
        assert!(k.menu);
    }

    /// F1: the per-fixture node is in the catalog — `image`/`colour`/`signal`
    /// in, nothing out, family `Venue`, stage `Output`, Live only, and every
    /// param carries a default inside its own range.
    #[test]
    fn the_catalog_carries_the_venue_fixture_node() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let k = catalog
            .iter()
            .find(|k| k.kind == VENUE_FIXTURE_KIND)
            .expect("venue.fixture is in the catalog");
        assert_eq!(k.family, NodeFamily::Venue);
        assert_eq!(k.stage(), Stage::Output);
        assert_eq!(
            k.inputs.iter().map(|p| (p.name.as_str(), p.ty)).collect::<Vec<_>>(),
            [
                ("image", PortType::Image),
                ("colour", PortType::Control),
                ("signal", PortType::Control),
            ]
        );
        assert!(k.outputs.is_empty());
        assert_eq!(k.domains, &[Domain::Live]);
        assert!(k.menu);
        assert!(!k.params.is_empty(), "the numeric knobs are params");
        for p in &k.params {
            assert!(p.default.is_finite(), "{}.{} has no default", k.kind, p.name);
            assert!(
                p.default >= p.min && p.default <= p.max,
                "{}.{} default {} is outside [{}, {}]",
                k.kind,
                p.name,
                p.default,
                p.min,
                p.max
            );
        }
    }

    #[test]
    fn the_mode_bridge_maps_every_legacy_mode_that_has_a_composition() {
        // §2.4: automix, collection and own each get a compose kind; disabled
        // is the retired node and maps to nothing, which is what makes
        // "absence is off" the one spelling of off.
        for (mode, want) in [
            ("automix", "compose.automix"),
            ("collection", "compose.collection"),
            ("own", "compose.layers"),
        ] {
            let kind = compose_kind_for_mode(mode).expect(mode);
            assert_eq!(kind, want);
            assert_eq!(
                mode_for_compose_kind(kind),
                Some(mode),
                "the bridge must be invertible for {mode}"
            );
            assert!(is_compose_kind(kind));
        }
        assert_eq!(compose_kind_for_mode("disabled"), None);
        assert!(!is_compose_kind("stack.mode"));
        assert!(!is_compose_kind("gen.turing"));
    }

    // -- R22.5a: what the menu offers, per domain --------------------------

    /// `stack.mode` and every `compose.*` kind are out of the `+ nodo` menu
    /// — the mode is the output's own config (R22.5b) and the legacy node is
    /// the migration's business — and nothing else is: a kind the operator
    /// cannot reach for would be a kind that is not in the catalog at all.
    #[test]
    fn the_catalog_retires_stack_and_compose_from_the_menu() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let find = |kind: &str| catalog.iter().find(|k| k.kind == kind).expect(kind);
        assert!(!find(crate::STACK_MODE_KIND).menu, "stack.mode is legacy, not a menu item");
        for (kind, _) in COMPOSE_KINDS {
            assert!(!find(kind).menu, "{kind} is the output's mode, not a menu item");
        }
        assert!(!find(AI_COLOURIST_KIND).menu, "ai.colourist is a gen.* sub-feature, not a menu item");
        for kind in
            ["gen.turing", "fx.shake", "output.device", "audio.live", "audio.file", "export.video"]
        {
            assert!(find(kind).menu, "{kind} must stay reachable from the menu");
        }
    }

    /// The two kinds that only exist Offline — the export the timeline
    /// renders to a file and the file it renders from — say so in their own
    /// `domains`, the field `GET /catalog?domain=` filters by.
    #[test]
    fn the_catalog_restricts_export_video_and_audio_file_to_offline() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let find = |kind: &str| catalog.iter().find(|k| k.kind == kind).expect(kind);
        for kind in ["export.video", "audio.file"] {
            assert_eq!(find(kind).domains, &[Domain::Offline], "{kind}");
            assert!(find(kind).offered_in(Domain::Offline), "{kind} serves an Offline graph");
            assert!(!find(kind).offered_in(Domain::Live), "{kind} is useless Live");
        }
    }

    /// The mirror: a screen, a camera, a lamp and a live audio source are
    /// things happening *now*, so their `domains` name Live alone.
    #[test]
    fn the_catalog_restricts_live_only_kinds_to_live() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let find = |kind: &str| catalog.iter().find(|k| k.kind == kind).expect(kind);
        for kind in ["audio.live", "output.device", "capture", "out.sacn", "out.ddp"] {
            assert_eq!(find(kind).domains, &[Domain::Live], "{kind}");
            assert!(find(kind).offered_in(Domain::Live), "{kind}");
            assert!(!find(kind).offered_in(Domain::Offline), "{kind}");
        }
        // A generator is neither: it draws the same picture in both domains.
        assert_eq!(find("gen.turing").domains, &[Domain::Live, Domain::Offline]);
        assert!(find("gen.turing").offered_in(Domain::Live));
        assert!(find("gen.turing").offered_in(Domain::Offline));
    }

    /// R22.5a's whole point: the catalog's `domains` are the *same rule*
    /// `lint` enforces, not a second list. Dropping a node of each kind into
    /// each domain and linting must agree with `kind_domains`, so a menu can
    /// never offer a kind the validator rejects, nor hide one it accepts.
    #[test]
    fn the_catalogs_domains_are_the_same_rule_lint_applies() {
        let catalog = NodeKind::catalog(&sample_manifest());
        let is_domain_error = |i: &Issue| {
            i.severity == Severity::Error
                && (i.message.contains("only exists in")
                    || i.message.contains("does not match the audio node"))
        };
        for k in &catalog {
            for domain in [Domain::Live, Domain::Offline] {
                let mut g = Graph::new("g", "t", domain);
                g.add_node("n", &k.kind, [0.0, 0.0]);
                let issues = g.lint(&catalog);
                let rejected = issues.iter().any(is_domain_error);
                assert_eq!(
                    rejected,
                    !k.domains.contains(&domain),
                    "{} in {domain:?}: lint and kind_domains disagree — {issues:?}",
                    k.kind
                );
            }
        }
    }

    #[test]
    fn the_pipeline_ranks_are_ordered_and_schedule_is_off_the_grid() {
        assert!(Stage::Source.rank() < Stage::Modulate.rank());
        assert!(Stage::Modulate.rank() < Stage::Compose.rank());
        assert!(Stage::Compose.rank() < Stage::Mask.rank());
        assert!(Stage::Mask.rank() < Stage::Output.rank());
        assert_eq!(Stage::Schedule.rank(), None);
        assert!(Stage::Schedule.is_scheduled());
    }

    #[test]
    fn the_stage_edge_rule_rejects_backward_edges_and_honours_the_exception_table() {
        assert!(stage_edge_allowed(Stage::Source, Stage::Output).is_ok());
        assert!(stage_edge_allowed(Stage::Source, Stage::Mask).is_ok());
        assert!(stage_edge_allowed(Stage::Compose, Stage::Mask).is_ok());
        assert!(stage_edge_allowed(Stage::Mask, Stage::Compose).is_ok(), "declared exception");
        assert!(stage_edge_allowed(Stage::Output, Stage::Source).is_err());
        assert!(stage_edge_allowed(Stage::Modulate, Stage::Source).is_err());
        assert!(stage_edge_allowed(Stage::Schedule, Stage::Source).is_err());
        assert!(stage_edge_allowed(Stage::Source, Stage::Schedule).is_err());
    }

    // -- LAY.2: capabilities -------------------------------------------------

    #[test]
    fn kind_capabilities_track_declared_ports_and_family() {
        let catalog = NodeKind::catalog(&sample_manifest());
        for k in &catalog {
            let c = k.capabilities();
            let has = |ty: PortType, ports: &[Port]| ports.iter().any(|p| p.ty == ty);
            assert_eq!(
                c.has(Capability::ImageIn),
                has(PortType::Image, &k.inputs),
                "{} image_in disagrees with its ports",
                k.kind
            );
            assert_eq!(c.has(Capability::ImageOut), has(PortType::Image, &k.outputs), "{}", k.kind);
            assert_eq!(c.has(Capability::ControlIn), has(PortType::Control, &k.inputs), "{}", k.kind);
            assert_eq!(
                c.has(Capability::ControlOut),
                has(PortType::Control, &k.outputs),
                "{}",
                k.kind
            );
        }

        let find = |kind: &str| catalog.iter().find(|k| k.kind == kind).unwrap();
        assert!(find("gen.turing").capabilities().has(Capability::NeedsGpu));
        assert!(find("sensor.yolo").capabilities().has(Capability::HasAux));
        assert!(find("cam.track").capabilities().has(Capability::HasAux));
        assert!(!find("cam.track").capabilities().has(Capability::ImageOut));
        assert!(find("out.sacn").capabilities().has(Capability::SignalOut));
        assert!(find("audio.live").capabilities().has(Capability::NeedsAudio));
        // A mask transforms an image; it is not a new source of one.
        assert!(find("mask").capabilities().has(Capability::ImageIn));
        assert!(find("mask").capabilities().has(Capability::ImageOut));
        // 22-sept-2026 (tarde): its second `image` input, the stroke material.
        let mask = find("mask");
        assert!(mask.inputs.iter().any(|p| p.name == "stroke" && p.ty == PortType::Image));
    }

    /// `MATERIAL_PORTS` — the runtime index `Paint::run` reads without a
    /// manifest — must agree with the catalog's own `Port::material`
    /// annotation, port for port, in both directions. A material port added
    /// to one and forgotten in the other is exactly the drift this test is
    /// here to make impossible (docs/plan-architecture.md D7).
    #[test]
    fn the_material_port_table_agrees_with_the_catalog() {
        let catalog = NodeKind::catalog(&sample_manifest());
        for k in &catalog {
            for p in &k.inputs {
                assert_eq!(
                    p.material,
                    is_material_port(&k.kind, &p.name),
                    "{}.{}: catalog says material={}, table says {}",
                    k.kind,
                    p.name,
                    p.material,
                    is_material_port(&k.kind, &p.name)
                );
            }
        }
        // And every row names a port that really exists — a typo in the
        // table would otherwise silently excuse nothing.
        for (kind, port) in MATERIAL_PORTS {
            let k = catalog
                .iter()
                .find(|k| k.kind == *kind)
                .unwrap_or_else(|| panic!("{kind} is in MATERIAL_PORTS but not the catalog"));
            assert!(
                k.inputs.iter().any(|p| p.name == *port),
                "{kind}.{port} is in MATERIAL_PORTS but not a port of {kind}"
            );
        }
    }

    /// A model's declared surfaces are its `tex.<part>` material inputs: one
    /// per part in the manifest, each an image, each answered by
    /// `is_material_port`. Monstruo is the first consumer (`skin`, `eyes`).
    #[test]
    fn a_surface_part_is_a_material_image_input_of_its_generator() {
        let manifest = sample_manifest();
        let catalog = NodeKind::catalog(&manifest);
        let model = manifest.models.iter().find(|m| m.id == "monster").expect("monstruo is live");
        assert!(!model.surfaces.is_empty(), "monstruo declares surfaces in data/schema.json");
        let gen = catalog.iter().find(|k| k.kind == "gen.monster").expect("gen.monster");
        for part in &model.surfaces {
            let port = format!("tex.{part}");
            let p = gen
                .inputs
                .iter()
                .find(|p| p.name == port)
                .unwrap_or_else(|| panic!("gen.monster has no {port} input"));
            assert!(p.material && p.ty == PortType::Image, "{port} is a material image");
            assert!(is_material_port("gen.monster", &port));
        }
        // A model with no surfaces gets no tex.* input at all.
        let turing = catalog.iter().find(|k| k.kind == "gen.turing").expect("gen.turing");
        assert!(turing.inputs.iter().all(|p| !p.name.starts_with("tex.")));
    }

    /// A generator whose only consumer is a `tex.*` surface lints clean (no
    /// error about it reaching nothing), and the plan renders the generator
    /// before the consumer that samples its picture.
    #[test]
    fn a_generator_feeding_only_a_surface_lints_clean_and_compiles_first() {
        use crate::lint::Severity;
        let catalog = NodeKind::catalog(&sample_manifest());
        let mut g = Graph::new("g", "t", Domain::Live);
        let turing = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let monstruo = g.add_node("gen", "gen.monster", [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&monstruo, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&turing, "image"), Endpoint::new(&monstruo, "tex.skin"));
        // The output's own control requirement is not what this is about; no
        // error may point at the surface source or the surface it feeds.
        let about_surface: Vec<_> = g
            .lint(&catalog)
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .filter(|i| i.node.as_deref() == Some(turing.as_str()) || i.node.as_deref() == Some(monstruo.as_str()))
            .map(|i| i.message)
            .collect();
        assert!(about_surface.is_empty(), "a surface source is not an error: {about_surface:?}");
        let plan = g.compile();
        let pos = |id: &str| plan.ops.iter().position(|op| op.node == id).unwrap();
        assert!(pos(&turing) < pos(&monstruo), "the material source renders before its consumer");
        let consumer = plan.ops.iter().find(|op| op.node == monstruo).unwrap();
        assert_eq!(consumer.inputs.len(), 1, "the surface is an input the consumer reads");
    }

    #[test]
    fn common_effect_ids_are_real_post_fields() {
        let names: Vec<&str> = metal_schema::EFFECTS_FLOATS.iter().map(|s| s.name).collect();
        for id in COMMON_EFFECT_IDS {
            assert!(names.contains(id), "fx.{id} has no matching Post field in metal-schema");
        }
    }

    // -- A3 -------------------------------------------------------------

    #[test]
    fn round_trip_is_byte_identical() {
        let mut g = Graph::new("g1", "roundtrip", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [2.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&gen, "control"));
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
        g.add_edge(Endpoint::new(&fx, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out, "control"));

        let json1 = g.to_json();
        let back = Graph::from_json(&json1).expect("parses");
        let json2 = back.to_json();
        assert_eq!(json1, json2, "graph -> json -> graph -> json must be byte-identical");
        // `to_json` sorts a clone before writing, so compare against a
        // sorted clone of the original rather than `g` itself, which still
        // holds nodes/edges in insertion order.
        let mut sorted = g.clone();
        sorted.nodes.sort_by(|a, b| a.id.cmp(&b.id));
        sorted.edges.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(sorted, back, "the parsed graph must equal the original, order aside");
    }

    #[test]
    fn serialized_order_is_by_id_not_insertion() {
        let mut g = Graph::new("g1", "order", Domain::Live);
        // Insert out of id order on purpose.
        g.nodes.push(Node::new("gen-9", "gen.turing", [0.0, 0.0]));
        g.nodes.push(Node::new("gen-1", "gen.turing", [0.0, 0.0]));
        let json = g.to_json();
        let pos9 = json.find("gen-9").unwrap();
        let pos1 = json.find("gen-1").unwrap();
        assert!(pos1 < pos9, "nodes must serialize sorted by id regardless of insertion order");
    }

    // -- G5 -------------------------------------------------------------

    #[test]
    fn inferred_domain_reads_off_which_audio_kind_is_present() {
        let mut live = Graph::new("g", "t", Domain::Live);
        live.add_node("audio", "audio.live", [0.0, 0.0]);
        assert_eq!(live.inferred_domain(), Some(Domain::Live));

        let mut offline = Graph::new("g", "t", Domain::Offline);
        offline.add_node("audio", "audio.file", [0.0, 0.0]);
        assert_eq!(offline.inferred_domain(), Some(Domain::Offline));
    }

    #[test]
    fn inferred_domain_is_none_with_neither_or_both_audio_kinds() {
        let empty = Graph::new("g", "t", Domain::Live);
        assert_eq!(empty.inferred_domain(), None, "no audio node implies nothing");

        let mut mixed = Graph::new("g", "t", Domain::Live);
        mixed.add_node("a", "audio.live", [0.0, 0.0]);
        mixed.add_node("b", "audio.file", [0.0, 1.0]);
        // Ambiguous, not a guess — `lint_domain` is what turns this case into
        // a named error; this function just refuses to pick a side.
        assert_eq!(mixed.inferred_domain(), None);
    }
}
