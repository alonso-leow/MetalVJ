//! The float manifest — what a model accepts, with no opinion about how it is
//! drawn.
//!
//! This is `metal-render::model`'s data half, lifted out whole. The types are
//! byte-for-byte what they were and `metal_render::model` re-exports them, so
//! nothing that used them had to change; what changed is that they no longer
//! sit in a crate that pulls wgpu, winit and egui behind them.
//!
//! # Why it was split
//!
//! `manifest.rs` already made this argument once, about `fw-corpus`:
//!
//! > *"chose a shared JSON file over a shared Rust type so the two crates
//! > could be built and iterated on independently"*
//!
//! …and then recorded what that cost — a hand-written file and the renderers
//! silently disagreeing about three models. The conclusion there was to
//! *generate* the JSON from the renderers. This crate is the other half of the
//! same conclusion: the consumers that want the manifest as a **type** should
//! get the type, and only the consumers that want to draw should pay for a
//! graphics stack.
//!
//! The consumer that forced it is `metal-dash` on a host with no GPU — see
//! [`Manifest`] and `metal-dash`'s `stand` module. A venue laptop has an adapter;
//! a 1-core VPS serving the app for review does not, and refusing to run there
//! would mean the operator's surface can only be looked at by someone sitting
//! in front of the machine it renders on.
//!
//! # Two ways in, one type out
//!
//! - **From the renderers.** `metal-render`'s models declare `&'static Schema`
//!   consts, as they always have. Nothing about that path changed.
//! - **From `data/schema.json`.** [`Manifest::load`] parses the file
//!   `emit-schema` writes and hands back the same `&'static Schema`, leaked
//!   once at startup. Leaking is the honest allocation here: a suite manifest
//!   lives exactly as long as the process, and the alternative is threading a
//!   lifetime through every consumer to describe a thing that is never freed.
//!
//! The two must agree, and `metal-render`'s `tests/schema.rs` is what makes them:
//! it fails if the file on disk has drifted from the models.

use serde::{Deserialize, Serialize};

/// The facial rig's control enum, re-exported from [`metal_face`] so that
/// [`FloatSpec::rig`] and every model schema name one type — the plan's
/// `metal_schema::FaceControl` spelling. `metal_face` is a pure CPU leaf (glam +
/// serde, no GPU), which is why this crate can carry the dependency and stay
/// the graphics-free half of the manifest.
pub use metal_face::FaceControl;

// ---------------------------------------------------------------------------
// capabilities — what a node or model can do / needs (LAY.2)
// ---------------------------------------------------------------------------

/// One declared capability of a node kind or a renderer.
///
/// **Why this is a type and not three booleans plus prose.** `Model` carried
/// `footage`/`backdrop`/`camera_track` and `NodeKind` carried a family, and
/// nothing said "needs the GPU", "keeps state between frames", "reads a
/// footage library", "has auxiliary CPU logic". A consumer that wanted to know
/// had to read a doc comment or special-case an id. Capabilities make that
/// query a set membership, which `lint` (LAY.3) can check and which a future
/// editor can show without knowing each kind by name. See
/// `docs/plan-architecture.md` D4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Accepts an `Image` input.
    ImageIn,
    /// Produces an `Image` output.
    ImageOut,
    /// Accepts a `Control` input.
    ControlIn,
    /// Produces a `Control` output (a generator's `row`, a sensor's channels).
    ControlOut,
    /// Drives something off-screen: a bulb, a DMX fixture, an instrument.
    SignalOut,
    /// Needs a GPU adapter to draw.
    NeedsGpu,
    /// Keeps state between frames (a trail, a detector's last box).
    Stateful,
    /// Reads a footage library rather than generating a picture.
    NeedsFootage,
    /// Draws its own background through the shared backdrop pass.
    Backdrop,
    /// Declares floats a `cam.track` node may drive.
    CameraTrack,
    /// Needs auxiliary CPU logic from `metal-logic` (`docs/plan-architecture.md` D1).
    HasAux,
    /// Needs an audio source to be `Live`.
    NeedsAudio,
}

impl Capability {
    /// The single bit this capability occupies in [`Capabilities`].
    pub const fn bit(self) -> u16 {
        match self {
            Capability::ImageIn => 1 << 0,
            Capability::ImageOut => 1 << 1,
            Capability::ControlIn => 1 << 2,
            Capability::ControlOut => 1 << 3,
            Capability::SignalOut => 1 << 4,
            Capability::NeedsGpu => 1 << 5,
            Capability::Stateful => 1 << 6,
            Capability::NeedsFootage => 1 << 7,
            Capability::Backdrop => 1 << 8,
            Capability::CameraTrack => 1 << 9,
            Capability::HasAux => 1 << 10,
            Capability::NeedsAudio => 1 << 11,
        }
    }

    /// The wire/JSON name, for `emit-schema` and anything that lists them.
    pub const fn name(self) -> &'static str {
        match self {
            Capability::ImageIn => "image_in",
            Capability::ImageOut => "image_out",
            Capability::ControlIn => "control_in",
            Capability::ControlOut => "control_out",
            Capability::SignalOut => "signal_out",
            Capability::NeedsGpu => "needs_gpu",
            Capability::Stateful => "stateful",
            Capability::NeedsFootage => "needs_footage",
            Capability::Backdrop => "backdrop",
            Capability::CameraTrack => "camera_track",
            Capability::HasAux => "has_aux",
            Capability::NeedsAudio => "needs_audio",
        }
    }

    /// Every capability, in bit order — one list, so `emit-schema`, tests and
    /// any future editor menu cannot each be a different list.
    pub const ALL: &'static [Capability] = &[
        Capability::ImageIn,
        Capability::ImageOut,
        Capability::ControlIn,
        Capability::ControlOut,
        Capability::SignalOut,
        Capability::NeedsGpu,
        Capability::Stateful,
        Capability::NeedsFootage,
        Capability::Backdrop,
        Capability::CameraTrack,
        Capability::HasAux,
        Capability::NeedsAudio,
    ];
}

/// A set of [`Capability`], small, `Copy`, and dependency-free.
///
/// A hand-rolled bitset rather than a `bitflags` dependency: this crate is
/// deliberately "nothing but serde" (see the crate doc), and twelve bits do
/// not justify the first non-serde dependency in the workspace's most-shared
/// data crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capabilities(u16);

impl Capabilities {
    pub const fn empty() -> Capabilities {
        Capabilities(0)
    }

    pub const fn has(self, c: Capability) -> bool {
        self.0 & c.bit() != 0
    }

    pub const fn with(self, c: Capability) -> Capabilities {
        Capabilities(self.0 | c.bit())
    }

    /// The union of two sets.
    pub const fn union(self, other: Capabilities) -> Capabilities {
        Capabilities(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The capabilities in this set, in [`Capability::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = Capability> {
        Capability::ALL.iter().copied().filter(move |c| self.has(*c))
    }

    /// Wire names, for `emit-schema` and for tests that want to read a
    /// failure.
    pub fn names(self) -> Vec<&'static str> {
        self.iter().map(Capability::name).collect()
    }
}

// I.3: the typed IR between planning and building, the closed vocabulary it
// validates against, and the resume table keyed by the directory's state.
// Separate modules because they are a different kind of data from the float
// manifest below, not an extension of it.
pub mod ir;
pub mod platform;
pub mod keys;
pub mod resume;
/// `fx.warp`: the coordinate-mesh notation (the loops design notes T12).
pub mod warp;
/// Spanish parameter names kept as aliases of their English names (see the module).
pub mod param_aliases;

// ---------------------------------------------------------------------------
// the types
// ---------------------------------------------------------------------------

/// The state of an effect in a generator node's configuration.
///
/// Three states correspond to the three views: off (no node exposed),
/// reactive (driven by audio digest), and fixed (value held at last computed).
/// This enum is used in generator node configs to declare which effects are
/// active and how they behave.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EffectState {
    /// Effect is off; no `fx.*` node is materialized.
    Off,
    /// Effect is reactive; driven by the digest (audio control port).
    /// Presence of a control port connection is the source of truth.
    Reactive,
    /// Effect is fixed; does not respond to the digest, holds a value.
    Fixed(f32),
}

/// One step in an explicitly ordered effect chain.
///
/// **Why this exists.** Through 2026-09-03 the order effects composed in was
/// hardcoded in `fs_main` (`shaders/post.wgsl`): shake, then the leaks, then
/// crt, glitch, posterize, zoom, fire, lorenz, vignette, blur, sharpen,
/// grain — always that order, whatever the operator wanted. That is fine
/// while an effect is "a float on the model's row", and wrong the moment
/// effects are *nodes the operator arranges*, because "vignette then zoom"
/// and "zoom then vignette" are different images and the graph is where that
/// difference is expressed.
///
/// A `Post` carrying a non-empty [`Post::chain`] means "apply exactly these
/// steps, in exactly this order". An empty one means "legacy fixed order",
/// which is what every preset written before this field existed says, and
/// what [`Post::resolved_chain`] reproduces from the flat fields.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EffectStep {
    /// Index into [`EFFECTS_FLOATS`] for a row-float effect (`0..EFFECTS_FLOATS.len()`),
    /// or [`CHAIN_KIND_ESTELA`] for Conjunto F's promoted footage effect.
    /// [`Post::resolved_chain`] is where the two kinds are told apart and
    /// handled differently.
    pub effect: u32,
    /// How much of it, in that effect's own declared range.
    ///
    /// For a row-float effect, [`Post::resolved_chain`] re-reads this from
    /// the flat fields, so whatever is stored here going in is ignored —
    /// the chain says *when*, the fields say *how much* (see [`Post::chain`]).
    /// A footage kind has no flat field to re-read: there, this **is** the
    /// amount, verbatim.
    pub amount: f32,
    /// Which params block this step reads, for a footage kind — meaningless
    /// for a row-float effect, which has none.
    ///
    /// `0` is the only value anything in this codebase produces today: v1
    /// carries exactly one params block per footage kind (see
    /// `metal-render::shared::post::EstelaParams`), not one per node, so a
    /// chain naming `fx.estela` twice has both steps read the same slot —
    /// this field exists so that limit has somewhere to be lifted later
    /// without another format change, not because anything can pick a
    /// different slot yet.
    ///
    /// `#[serde(default)]` for the reason every field here is: a preset or
    /// a saved graph written before this field existed must still
    /// deserialize — see this module's own rule on [`Post`].
    #[serde(default)]
    pub slot: u32,
}

/// How many steps a chain may carry to the GPU.
///
/// Sixteen because that is the number of chain-step kinds the shader was
/// written against (the fourteen common row effects plus `wake`), so a chain
/// that used every one of them once still fits. A graph that somehow asks for
/// more is truncated rather than rejected — see [`Post::resolved_chain`].
/// Nothing about the shader gets cheaper by making this smaller; the uniform
/// is one array either way.
pub const MAX_CHAIN: usize = 16;

/// How many floats the **common** effects occupy at the front of
/// [`EFFECTS_FLOATS`] — the 14 rows of [`EFFECT_SPECS`], and the number a
/// saved `wake` chain step is numbered past. The advanced effects
/// ([`ADVANCED_EFFECTS`]) are appended after them, so a flat effect index in
/// `CHAIN_FLOATS..EFFECTS_FLOATS.len()` names an advanced float rather than a
/// chain step of its own.
pub const CHAIN_FLOATS: usize = N_EFFECTS;

/// Estela's own chain-step position — Conjunto F ("B7"): Estela was promoted
/// to an `fx.*` chain node, and `metal-graph::NodeKind::catalog` publishes it as
/// `fx.estela`.
///
/// The promoted-effect slots start one past the **common** row floats
/// ([`CHAIN_FLOATS`], not `EFFECTS_FLOATS.len()` now that the advanced effects
/// trail the common ones) and Estela is the second of them, so its value is
/// `CHAIN_FLOATS as u32 + 1`. The slot at `CHAIN_FLOATS` is left as a reserved
/// gap: it is not written as an independent literal, so the advanced floats'
/// arrival cannot move Estela's value and a saved estela chain step keeps its
/// numbering.
pub const CHAIN_KIND_ESTELA: u32 = CHAIN_FLOATS as u32 + 1;

/// Whether `effect` names Conjunto F's promoted footage kind rather than a
/// position in [`EFFECTS_FLOATS`].
///
/// The one rule [`Post::resolved_chain`] (Rust) and the `switch` in
/// `post.wgsl` (GPU) must agree on, named once so the two cannot drift
/// apart — the same reasoning [`is_effects_block`] already applies one
/// level down, for telling the effects block apart from a model's own.
pub fn is_footage_kind(effect: u32) -> bool {
    effect == CHAIN_KIND_ESTELA
}

/// Whether `effect` is something the shader can run as a chain step: a common
/// row effect, the footage kind, or an advanced group in
/// [`ADVANCED_CHAIN_EFFECTS`]. Anything else (the other advanced floats, an
/// index no schema declares) is dropped by [`Post::resolved_chain`].
pub fn is_chain_kind(effect: u32) -> bool {
    (effect as usize) < CHAIN_FLOATS
        || is_footage_kind(effect)
        || ADVANCED_CHAIN_EFFECTS.iter().any(|id| advanced_chain_kind(id) == Some(effect))
}

/// One float in a governance block: name, range, the value it starts at, and
/// how freely a live signal may move it.
#[derive(Clone, Copy)]
pub struct FloatSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// 0..1 — how much of this float's range something driving the model from
    /// outside (the digest, or `metal-dash`'s stand-in for it) may swing it
    /// across, relative to what it does to an ordinary float.
    ///
    /// **Why this is not always 1.** A declared range is how far a parameter
    /// may legally go; it is not how far it should go while music is playing.
    /// Some floats are expressive — thickness, brightness, speed — and want
    /// the whole range. Others are *structural*: they select which regime the
    /// model is in, and moving them does not make the model more dramatic, it
    /// makes it a different and usually worse model.
    ///
    /// Turing is the case that forced this field to exist. Its Gray-Scott feed
    /// and kill rates produce a labyrinth only in a narrow band; driven hard
    /// the reaction leaves that band and the mesh renders as a flat blank
    /// sheet, with nothing logged and nothing out of range. A structural float
    /// says so here, next to the value, where the person tuning the model is
    /// already looking.
    ///
    /// Sliders ignore this entirely — a human dragging a parameter to a
    /// degenerate state is exploring, and that is the studio's whole job.
    pub drive: f32,
    /// 0..1 — how much this float rides the *interpolated* digest rather than
    /// the raw one. `None` means "decide from the column driving it", which is
    /// the right answer for almost every float; see `metal-dash`'s
    /// `drive::smooth_default`.
    ///
    /// **This is a latency dial, not a quality dial, and that is the whole
    /// point.** The digest is one row per 125 ms and holds between rows, so an
    /// undriven float is a staircase. Interpolating across the row removes the
    /// steps — and costs 125 ms, because a value that moves from the previous
    /// row toward the current one only arrives at the current one when the
    /// window ends.
    ///
    /// For a continuous envelope — colour, thickness, brightness — that lag is
    /// invisible and the smoothing is free. For a transient it is fatal: a
    /// shake or a light leak that lands an eighth-note behind the hit reads as
    /// a mistake, not as a softer hit. Onset is pooled *by max* upstream
    /// precisely so transients survive; smoothing an onset-driven float throws
    /// that away.
    ///
    /// So: set this to 0 on anything percussive, positional or strobing that
    /// happens to be driven by a continuous column, and to 1 on anything
    /// gradual that happens to be driven by onset. Leave it `None` otherwise.
    pub smooth: Option<f32>,
    /// Which facial-rig control this float *is*, or `None` for an ordinary
    /// model float.
    ///
    /// **The face solver's only handle on a model's rig.** `metal-perform::face`
    /// turns the voice columns into a value for every [`metal_face::FaceControl`]
    /// and writes it into the float whose spec carries that control here —
    /// there is no second table of offsets and no name-matching. A model
    /// declares its rig floats **appended at the end** (plan rule 2), each
    /// [`spec_rig`], so a file saved before the rig existed loads with those
    /// floats absent at worst, never reordered.
    ///
    /// `None` on every ordinary float, which is why this is safe for every
    /// manifest on disk: it is not carried through `data/schema.json` yet, so a
    /// model loaded from a file has no rig until V5's `monster` writes one —
    /// and until then `metal_perform::face::has_rig` is simply false.
    pub rig: Option<FaceControl>,
}

/// A float a live signal may swing freely. The common case.
pub const fn spec(name: &'static str, min: f32, max: f32, default: f32) -> FloatSpec {
    FloatSpec { name, min, max, default, drive: 1.0, smooth: None, rig: None }
}

/// A float whose smoothing disagrees with the column that happens to drive it
/// — see [`FloatSpec::smooth`]. `smooth` 0 lands instantly and stairsteps, 1
/// interpolates across the digest row and lags it by 125 ms.
pub const fn spec_timed(
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    smooth: f32,
) -> FloatSpec {
    FloatSpec { name, min, max, default, drive: 1.0, smooth: Some(smooth), rig: None }
}

/// A float that selects a regime rather than an intensity — see
/// [`FloatSpec::drive`]. `drive` is how much of its range a live signal may
/// use; 0 pins it to whatever the operator set.
pub const fn spec_held(
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    drive: f32,
) -> FloatSpec {
    FloatSpec { name, min, max, default, drive, smooth: None, rig: None }
}

/// A facial-rig float: its name, range and rest come from the control itself,
/// and it is **never driven** (`drive: 0`) — the only writer is the face solver
/// (or a lane/loop the operator authored by hand). Append these after every
/// other float (plan rule 2).
///
/// The name is [`FaceControl::name`], so the model float and the control the
/// solver writes cannot drift apart; a lane or a binding can name it like any
/// other float.
pub const fn spec_rig(control: FaceControl) -> FloatSpec {
    let spec = control.spec();
    FloatSpec {
        name: control.name(),
        min: spec.lo,
        max: spec.hi,
        default: spec.rest,
        drive: 0.0,
        smooth: None,
        rig: Some(control),
    }
}

/// A named group of floats. `ColorSlot` and `Camera` are the two shared
/// blocks every model can opt into instead of hand-rolling its own color or
/// camera floats — see `docs/SUITE.md`.
#[derive(Clone, Copy)]
pub enum Block {
    Floats(&'static [FloatSpec]),
    /// 4 colours x RGB = 12 floats, and the row they start at. `grad4` in
    /// shared.wgsl reads them. Build it with [`color`] or [`color_at`].
    ColorSlot(&'static str, &'static [f32; 12]),
    /// pos.xyz + target.xyz + fov = 7 floats, carrying where this model wants
    /// to be seen from.
    ///
    /// The default is per model rather than shared because "a plain 3/4 view"
    /// is not one view: a cube of grids, a hand, an eye and a sheet of cloth
    /// sit at different places and different sizes, and one camera that suits
    /// all of them does not exist. A single shared default had three of the
    /// five 3D models opening edge-on or off-frame — see `docs/CORRECTIONS.md`
    /// §43.
    Camera([f32; 7]),
}

/// `Block::Camera` spelled the way it reads: where the camera is, what it
/// looks at, and how wide.
pub const fn camera(pos: [f32; 3], target: [f32; 3], fov_degrees: f32) -> Block {
    Block::Camera([pos[0], pos[1], pos[2], target[0], target[1], target[2], fov_degrees])
}

/// Solid white, the colour a slot starts at unless its model says otherwise.
pub const WHITE4: [f32; 12] = [1.0; 12];

/// A colour slot that starts white. What almost every slot wants: white is the
/// blank the operator paints over, and it shows the model's own shape rather
/// than a colour choice made for them.
pub const fn color(name: &'static str) -> Block {
    Block::ColorSlot(name, &WHITE4)
}

/// Solid black, the colour a frame was cleared to before any of this existed.
pub const BLACK4: [f32; 12] = solid4(0.0, 0.0, 0.0);

/// The background a model draws on top of, named the same everywhere.
///
/// It starts solid black on purpose: every model that has this slot cleared
/// its frame to black before it had one, so adding the slot changes nothing
/// about how any of them opens — it only makes what was a literal reachable.
/// Four stops rather than one because a background is the largest area in the
/// frame and a flat fill is the least interesting thing to do with it; the
/// shared backdrop pass in `metal-render` walks the slot across the frame.
///
/// Not every model wants one. A model whose own field already covers the
/// frame — Fractals, Kaleido, Turing, Domain — has no background to control,
/// and a slot there would be a colour picker that does nothing.
pub const fn background() -> Block {
    color_at("background", &BLACK4)
}

/// A colour slot that starts somewhere else, given as 4 colours x RGB.
///
/// For the slots where white is not neutral but simply wrong. Oracle's pupil is
/// the case: the black of an eye was a literal in the shader until it became a
/// slot, and a slot that defaulted white would open the model with a white
/// pupil on a white sclera — an eye with no pupil at all, in range, rendering
/// fine, and wrong in exactly the way nothing downstream can detect.
pub const fn color_at(name: &'static str, start: &'static [f32; 12]) -> Block {
    Block::ColorSlot(name, start)
}

/// One colour, repeated across all four control points — a solid.
pub const fn solid4(r: f32, g: f32, b: f32) -> [f32; 12] {
    [r, g, b, r, g, b, r, g, b, r, g, b]
}

impl Block {
    pub const fn width(&self) -> usize {
        match self {
            Block::Floats(fs) => fs.len(),
            Block::ColorSlot(..) => 12,
            Block::Camera(_) => 7,
        }
    }
}

pub struct Schema {
    pub blocks: &'static [Block],
}

impl Schema {
    pub fn width(&self) -> usize {
        self.blocks.iter().map(Block::width).sum()
    }

    /// The row a fresh preset starts from: every model's own floats at their
    /// declared default, each colour slot where its model asked to start
    /// (solid white unless it says otherwise), and each camera where its own
    /// model asked to be seen from.
    pub fn defaults(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.width());
        for b in self.blocks {
            match b {
                Block::Floats(fs) => out.extend(fs.iter().map(|s| s.default)),
                Block::ColorSlot(_, start) => out.extend(*start),
                Block::Camera(view) => out.extend(view),
            }
        }
        out
    }

    /// Byte offset (in floats, not bytes) of the start of block `i`.
    pub fn offset(&self, i: usize) -> usize {
        self.blocks[..i].iter().map(Block::width).sum()
    }
}

// ---------------------------------------------------------------------------
// the file
// ---------------------------------------------------------------------------

/// `data/schema.json` as it is on disk — the shape `emit-schema` writes and
/// `fw-corpus` already reads.
///
/// Field for field with `metal_render::manifest`, and that is not a coincidence
/// to be tidied away: the emitter is the authority (the renderers are what
/// consumes a row) and this is a reader of what it emitted. `metal-render`'s
/// `tests/schema.rs` is what stops them drifting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// The digest grid the suite is defined on, in Hz. Carried through rather
    /// than assumed, because `docs/AUDIO_FEATURES.md` fixes it in one place
    /// and a second copy of an 8 typed into a consumer is how the two drift.
    pub rate: f32,
    pub models: Vec<ManifestModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestModel {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub technique: String,
    /// Whether this model draws over footage rather than generating a picture
    /// — a *transmission*, see `docs/TRANSMISSIONS.md`.
    ///
    /// Defaults to false, and unlike [`ManifestFloat::drive`] that default is
    /// safe: a manifest written before this key existed describes thirty models
    /// none of which is one, and a transmission read back as generative would
    /// be *selectable with nothing loaded* rather than silently wrong — which
    /// the app notices immediately, because it draws a slate.
    #[serde(default)]
    pub footage: bool,
    /// Whether this model draws its background via the shared `Backdrop`
    /// pass in `metal-render` rather than covering the frame with its own field
    /// — see `metal_render::model::Model::backdrop`. Exactly the twelve models
    /// where marking `fondo` as alpha does anything (`the design notes PARTE IV`
    /// §17.3).
    ///
    /// Defaults to false, and a manifest written before this key existed
    /// describes 34 models, none of which is known to use the Backdrop pass.
    /// Reading it as `false` is exactly right for those old files; no
    /// information is lost.
    #[serde(default)]
    pub backdrop: bool,
    /// Whether this model declares a 3D viewpoint a `cam.track` node may
    /// drive — see `metal_render::model::Model::camera_track` and
    /// `the design notes` §13.2. A model with this set carries three
    /// extra reserved floats (`cam_x`, `cam_y`, `cam_z`) at the end of its
    /// own blocks, appended the same way the effects blocks are (§3.3):
    /// after everything the model already declared, never in the middle.
    ///
    /// Defaults to false, and a manifest written before this key existed
    /// describes no model that had this capability yet — §13.2 is explicit
    /// that only models with an existing 3D camera concept qualify, and none
    /// did before this field was added, so `false` loses nothing for old
    /// files.
    #[serde(default)]
    pub camera_track: bool,
    /// The named parts a picture can be laid on (plan: surface sources), e.g.
    /// `skin` and `eyes` on monstruo — see `metal_render::model::Model::surfaces`.
    /// Each becomes a `tex.<part>` input on the model's generated graph node.
    /// Absent (empty) in files written before surfaces existed, which
    /// describes no model with one.
    #[serde(default)]
    pub surfaces: Vec<String>,
    pub blocks: Vec<ManifestBlock>,
}

/// Internally tagged by `type`, in snake_case — the shape the emitter writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ManifestBlock {
    Floats { fields: Vec<ManifestFloat> },
    ColorSlot {
        name: String,
        /// 4 colours x RGB. Absent in files written before a slot could start
        /// anywhere but white, so it falls back to white rather than failing.
        #[serde(default)]
        default: Option<Vec<f32>>,
    },
    Camera {
        /// pos.xyz + target.xyz + fov. Absent in the oldest files; the
        /// fallback is a plain 3/4 view, which is wrong for about half the
        /// models — hence `docs/CORRECTIONS.md` §43 and hence the emitter
        /// writing it.
        #[serde(default)]
        default: Option<Vec<f32>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFloat {
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// How freely a live signal may swing this float — see [`FloatSpec::drive`].
    ///
    /// **Defaults to 1.0, and that default is a real risk worth naming.** A
    /// manifest written before this key existed says nothing about which
    /// floats are structural, so every float reads as freely drivable and
    /// Turing goes back to rendering a blank sheet at full drive. The default
    /// is 1.0 anyway because it is what the overwhelming majority of floats
    /// are and a missing key must not mean "pinned"; the real protection is
    /// that `emit-schema` writes the key and the schema test fails if the
    /// file on disk has drifted.
    #[serde(default = "one")]
    pub drive: f32,
    /// See [`FloatSpec::smooth`]. Absent means "decide from the driving
    /// column", which is what every manifest written before this key existed
    /// meant anyway — so unlike `drive`, a missing key here is not a silent
    /// loss of the author's intent. It is the intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smooth: Option<f32>,
    /// Which facial-rig control this float is — see [`FloatSpec::rig`].
    ///
    /// **Absent on every ordinary float**, and on every manifest written
    /// before V5. It is carried through the file so a consumer with no
    /// renderers (`metal-perform::stand`, `metal-dash` on a host with no adapter)
    /// still answers `metal_perform::face::has_rig` correctly for `monster`,
    /// rather than only the GPU-built schemas carrying the rig.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rig: Option<FaceControl>,
}

fn one() -> f32 {
    1.0
}

/// One model, as this crate hands it to a consumer that has no renderers.
pub struct Entry {
    pub id: String,
    pub name: String,
    /// See [`ManifestModel::footage`].
    pub footage: bool,
    /// See [`ManifestModel::backdrop`].
    pub backdrop: bool,
    pub schema: &'static Schema,
}

impl Manifest {
    pub fn parse(text: &str) -> serde_json::Result<Manifest> {
        serde_json::from_str(text)
    }

    /// The manifest as `&'static Schema`s, so a consumer written against the
    /// renderers' types does not need a second code path.
    ///
    /// **This leaks, once, on purpose.** Everything it allocates lives for the
    /// life of the process by construction — a suite does not gain or lose a
    /// model while the app is running — and the alternative is a lifetime
    /// parameter on `Schema` that would infect `metal-render`'s models, which are
    /// genuinely `'static` and should not have to prove it.
    pub fn into_suite(self) -> Vec<Entry> {
        self.models
            .into_iter()
            .map(|m| {
                let blocks: Vec<Block> = m.blocks.into_iter().map(into_block).collect();
                let schema: &'static Schema =
                    Box::leak(Box::new(Schema { blocks: Box::leak(blocks.into_boxed_slice()) }));
                Entry { id: m.id, name: m.label, footage: m.footage, backdrop: m.backdrop, schema }
            })
            .collect()
    }
}

fn into_block(b: ManifestBlock) -> Block {
    match b {
        ManifestBlock::Floats { fields } => {
            let specs: Vec<FloatSpec> = fields
                .into_iter()
                .map(|f| FloatSpec {
                    name: Box::leak(f.name.into_boxed_str()),
                    min: f.min,
                    max: f.max,
                    default: f.default,
                    drive: f.drive.clamp(0.0, 1.0),
                    smooth: f.smooth.map(|s| s.clamp(0.0, 1.0)),
                    // The rig is carried in the manifest from V5 on; an older
                    // file has nothing here and reads as an ordinary float.
                    rig: f.rig,
                })
                .collect();
            Block::Floats(Box::leak(specs.into_boxed_slice()))
        }
        ManifestBlock::ColorSlot { name, default } => Block::ColorSlot(
            Box::leak(name.into_boxed_str()),
            Box::leak(Box::new(fixed::<12>(default, &WHITE4))),
        ),
        // The fallback view is the one the whole suite used before per-model
        // cameras existed, and it is only ever reached by a file older than
        // that change.
        ManifestBlock::Camera { default } => {
            Block::Camera(fixed::<7>(default, &[0.0, 0.6, 3.2, 0.0, 0.0, 0.0, 45.0]))
        }
    }
}

/// A fixed-width array out of a `Vec` that may be missing, short or long.
///
/// Short and long are both treated as "take what is there, keep the fallback
/// for the rest" rather than as errors: a manifest is a file, and a suite that
/// refuses to start because one colour slot has eleven numbers in it is worse
/// than one that starts with a white fourth stop.
fn fixed<const N: usize>(given: Option<Vec<f32>>, fallback: &[f32; N]) -> [f32; N] {
    let mut out = *fallback;
    if let Some(v) = given {
        for (dst, src) in out.iter_mut().zip(v.iter()) {
            *dst = *src;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// the audio digest's columns
// ---------------------------------------------------------------------------

/// The sixteen columns of the music prefix, in `docs/AUDIO_FEATURES.md`'s
/// order — the first half of **the one declaration**. This is the width the
/// corpus, `digest_audio.py`, the Colourist weights and the Router
/// descriptors are defined on, so it stays a named prefix and never moves.
pub const MUSIC_COLUMNS: [&str; 16] = [
    "rms",
    "bandwidth",
    "onset",
    "centroid",
    "chroma C",
    "chroma C#",
    "chroma D",
    "chroma D#",
    "chroma E",
    "chroma F",
    "chroma F#",
    "chroma G",
    "chroma G#",
    "chroma A",
    "chroma A#",
    "chroma B",
];

/// The thirteen columns the voice analyser appends, in
/// `metal_listen::voice`'s order. Indices 0..13 of a voice row.
pub const VOICE_COLUMNS: [&str; 13] = [
    "voice",
    "voice level",
    "pitch",
    "voicing",
    "steady",
    "syllables",
    "open",
    "wide",
    "hiss",
    "sing",
    "rap",
    "speak",
    "whisper",
];

/// The six columns the tone reducer appends, in `metal_listen::tone`'s order —
/// indices 0..6 of a tone block, and 29..35 of the digest row. It is the
/// Basic Pitch harmony summary, reduced to a mood rather than a note list: a
/// hue stored as a vector so it survives pooling and smoothing (D3), the
/// register and density a colour needs for lightness and chroma, a major/minor
/// bias, and a confidence that lets consumers fall back to chroma when there
/// is no pitched content.
pub const TONE_COLUMNS: [&str; 6] = [
    "tone_x",
    "tone_y",
    "tone_register",
    "tone_density",
    "tone_mode",
    "tone_conf",
];

/// How many columns the music prefix is.
pub const MUSIC: usize = MUSIC_COLUMNS.len();
/// How many columns the voice block is.
pub const VOICE: usize = VOICE_COLUMNS.len();
/// How many columns the tone block is.
pub const TONE: usize = TONE_COLUMNS.len();
/// The width of a digest row: the music prefix, the voice block, then the tone
/// block.
pub const FEATURES: usize = MUSIC + VOICE + TONE;

const fn concat_columns() -> [&'static str; FEATURES] {
    let mut out = [""; FEATURES];
    let mut i = 0;
    while i < MUSIC {
        out[i] = MUSIC_COLUMNS[i];
        i += 1;
    }
    let mut j = 0;
    while j < VOICE {
        out[MUSIC + j] = VOICE_COLUMNS[j];
        j += 1;
    }
    let mut k = 0;
    while k < TONE {
        out[MUSIC + VOICE + k] = TONE_COLUMNS[k];
        k += 1;
    }
    out
}

/// The thirty-five columns of one audio digest row, in order — the music
/// prefix (0..16), the voice block (16..29) then the tone block (29..35).
/// `metal_perform::digest::COLUMNS` is this array, and `metal_graph::params` names an
/// audio node's `control` variables with it (`control.rms`, `control.tone_x`),
/// so the monitor's labels, the variables a binding can pick and the row the
/// digester writes cannot disagree about which column is which.
///
/// Names may carry spaces and `#`: they are display names first. A binding
/// addresses one as `<port>.<name>` and splits at the first `.` only.
pub const DIGEST_COLUMNS: [&str; FEATURES] = concat_columns();

// ---------------------------------------------------------------------------
// the tick ladder
// ---------------------------------------------------------------------------
//
// `the voice design notes` § "The ladder, as a type". One canonical grid, one switch,
// two complete levels — see § "Two complete levels, one switch". The rate is
// deliberately *not* stored in a node, sequence or file: it is a property of
// the machine (`metal-config`'s `tick_level`), so rule 4 holds and a base-level
// peer can pool a canonical row on receive.

/// The base tick rate — the minimum, and the unit the whole ladder is built
/// on. `TickRate::hz` is always `BASE_HZ × 2^k`.
pub const BASE_HZ: u32 = 8;

/// A tick level: `BASE_HZ × 2^k`.
///
/// The plan enables two arms (`Base` 8 Hz, `Fine` 32 Hz), and the ladder type
/// allows `k ∈ {0, 1, 2, 3}` (8/16/32/64). A future level is one arm plus its
/// tests and no new contract — every consumer reads `hz()` instead of a
/// constant. The value is derived from [`TickRate::k`], never written twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TickRate {
    /// 8 Hz — one tick per 125 ms. The minimum the suite is defined on, and
    /// what a machine that cannot afford the fine level runs.
    #[default]
    Base,
    /// 32 Hz — one tick per 31.25 ms. Sized for a mouth that talks fluidly
    /// (a p/b/m closure gets ≥ 2 ticks).
    Fine,
}

impl TickRate {
    /// The level everything is stored and shared on. The fine level, so a base
    /// machine pools **on read or on receive** rather than anyone losing
    /// detail: switching base→fine→base changes no stored byte.
    pub const CANONICAL: TickRate = TickRate::Fine;

    /// Every enabled level, in coarse-to-fine order — one list, so a test
    /// sweeps the ladder instead of hard-coding rates (rule 7) and a future
    /// arm cannot be forgotten here.
    pub const ALL: &'static [TickRate] = &[TickRate::Base, TickRate::Fine];

    /// `k` in `BASE_HZ × 2^k`.
    pub const fn k(self) -> u32 {
        match self {
            TickRate::Base => 0,
            TickRate::Fine => 2,
        }
    }

    /// The rate in Hz — `BASE_HZ × 2^k`, derived so no arm can disagree with
    /// the ladder.
    pub const fn hz(self) -> u32 {
        BASE_HZ << self.k()
    }

    /// The enabled level at `hz` Hz, or `None` when `hz` is not on the ladder.
    pub fn from_hz(hz: u32) -> Option<TickRate> {
        TickRate::ALL.iter().copied().find(|r| r.hz() == hz)
    }

    /// The wire/TOML name — `"base"` or `"fine"` — the same string serde
    /// writes, spelled once so a route validating a POST body and a config
    /// file cannot disagree with the enum.
    pub const fn wire(self) -> &'static str {
        match self {
            TickRate::Base => "base",
            TickRate::Fine => "fine",
        }
    }

    /// [`TickRate::wire`] back. `None` for anything off the ladder.
    pub fn parse(s: &str) -> Option<TickRate> {
        TickRate::ALL.iter().copied().find(|r| r.wire() == s)
    }

    /// Seconds per tick at this level.
    pub const fn seconds_per_tick(self) -> f64 {
        1.0 / self.hz() as f64
    }

    /// How many canonical tick boundaries lie between two of this level's
    /// ticks. The one place a `× 4` lives.
    pub const fn canonical_per_tick(self) -> i64 {
        (TickRate::CANONICAL.hz() / self.hz()) as i64
    }

    /// Convert a tick index at this level into the canonical grid.
    ///
    /// The only multiplier: nothing downstream multiplies by 4 itself. A base
    /// tick index `n` becomes canonical `4n` because each 8 Hz tick is exactly
    /// four 32 Hz ticks (the ladder nests).
    pub const fn to_canonical(self, tick: i64) -> i64 {
        tick * self.canonical_per_tick()
    }

    /// Convert a canonical tick index into this level's grid.
    ///
    /// Exact for an index that lands on this level's grid (`tick % ratio ==
    /// 0`); an off-grid index is floored, which is the honest answer for "which
    /// base tick contains this canonical tick".
    pub const fn from_canonical(self, tick: i64) -> i64 {
        tick / self.canonical_per_tick()
    }
}

/// A per-tick decay factor restated for another tick level.
///
/// A smoothing constant `alpha` is written as "how much of the gap to the
/// target is closed in one tick". Halve the tick length and the same wall-clock
/// smoothing needs a smaller per-tick step; quarter it (8 Hz → 32 Hz, the plan's
/// two levels) and `α_fine = 1 − (1 − α_base)^(1/4)`. `from` is the level the
/// constant was authored at (the base grid), `to` the level it must run at.
/// `alpha` is clamped to `0..=1`, and `1.0` (instant) stays `1.0`.
pub fn per_tick_decay(alpha: f32, from: TickRate, to: TickRate) -> f32 {
    let a = alpha.clamp(0.0, 1.0);
    if a >= 1.0 || from.hz() == to.hz() {
        return a;
    }
    // `from.hz / to.hz` is the exponent: fine→base is ×4 (one base tick is
    // four fine ones), base→fine is 1/4 — `α_fine = 1 − (1 − α_base)^(1/4)`.
    let exponent = from.hz() as f64 / to.hz() as f64;
    (1.0 - (1.0 - a as f64).powf(exponent)) as f32
}

/// The on-disk name of a digest matrix on the `rate` grid:
/// `<stem>.<hz>hz.npy`. The rate is **in the filename**, so a reader can tell
/// which grid a file is on without opening it, and no literal `"8hz"`/`"32hz"`
/// exists anywhere else — every path is built from this one function.
///
/// There is deliberately no in-file rate field: a `.npy` is a bare float
/// matrix (numpy has nowhere to put one without breaking `np.load`), so the
/// suffix is the record. A legacy file with no `.<hz>hz` part is the pre-V2b
/// 8 Hz matrix, read as [`TickRate::Base`].
pub fn digest_file_name(stem: &str, rate: TickRate) -> String {
    format!("{stem}.{}hz.npy", rate.hz())
}

/// The rate a digest filename names, or `None` for a legacy `<stem>.npy` with
/// no rate suffix (which is read as [`TickRate::Base`]).
pub fn digest_rate_of(name: &str) -> Option<TickRate> {
    let stem = name.strip_suffix(".npy")?;
    let (_, tag) = stem.rsplit_once('.')?;
    let hz: u32 = tag.strip_suffix("hz")?.parse().ok()?;
    TickRate::from_hz(hz)
}

/// Whether `name` is a legacy digest with no rate suffix — a `<stem>.npy` whose
/// dot-part is not a `<hz>hz`.
pub fn digest_is_legacy(name: &str) -> bool {
    name.ends_with(".npy") && digest_rate_of(name).is_none()
}

/// The stem of a digest filename — `<song>.32hz.npy` → `<song>`,
/// `<song>.8hz.npy` → `<song>`, legacy `<song>.npy` → `<song>`. Lets a writer
/// that was handed a base path derive the canonical sibling.
pub fn digest_stem(name: &str) -> &str {
    match name.strip_suffix(".npy") {
        Some(body) => match body.rsplit_once('.') {
            Some((stem, tag)) if tag.strip_suffix("hz").is_some_and(|h| h.parse::<u32>().is_ok()) => {
                stem
            }
            _ => body,
        },
        None => name,
    }
}

/// How one column combines when several fine rows are pooled into one coarser
/// row. Per column, because the digest is not homogeneous: a transient and a
/// held state must not average away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// Arithmetic mean — a continuous envelope.
    Mean,
    /// The largest value, so a transient survives pooling.
    Max,
    /// The middle value of the sorted values — robust to one outlier, used for
    /// `pitch`.
    Median,
    /// The last (fine) value, for state that is held rather than averaged —
    /// `steady`, `syllables` and the four style scores.
    Last,
}

/// The pooling rule for one column, by index into [`DIGEST_COLUMNS`].
///
/// Mean by default; `onset` and `hiss` are max so a hit anywhere in the window
/// survives (the same reason the music window pools onset by max); `pitch` is
/// median; `steady`/`syllables`/style are `Last`. The voice block carries the
/// same names at the same indices, so a 35-wide row pools whole. The six tone
/// columns (29..35) land on the default arm and pool by `Mean` — none is a
/// transient, so averaging is exactly the "recent mood" they encode.
pub const fn pool_rule(column: usize) -> Pool {
    match column {
        2 => Pool::Max,                  // onset
        18 => Pool::Median,              // pitch
        20 | 21 => Pool::Last,           // steady, syllables
        24 => Pool::Max,                 // hiss (MUSIC + 8)
        25 | 26 | 27 | 28 => Pool::Last, // sing, rap, speak, whisper
        _ => Pool::Mean,
    }
}

/// Pool a slice of fine rows (already on a finer grid) into one coarser row by
/// [`pool_rule`]. An empty slice is an all-zero row — the honest "nothing
/// measured here", matching the digester's own empty window.
pub fn pool_rows(rows: &[[f32; FEATURES]]) -> [f32; FEATURES] {
    let mut out = [0.0f32; FEATURES];
    if rows.is_empty() {
        return out;
    }
    // One reused buffer, sorted only when a median actually needs it.
    let mut vals: Vec<f32> = Vec::with_capacity(rows.len());
    for c in 0..FEATURES {
        vals.clear();
        vals.extend(rows.iter().map(|r| r[c]));
        out[c] = match pool_rule(c) {
            Pool::Mean => {
                let sum: f64 = vals.iter().map(|&v| v as f64).sum();
                (sum / vals.len() as f64) as f32
            }
            Pool::Max => vals.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            Pool::Median => {
                vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
                let n = vals.len();
                if n % 2 == 1 {
                    vals[n / 2]
                } else {
                    ((vals[n / 2 - 1] as f64 + vals[n / 2] as f64) / 2.0) as f32
                }
            }
            Pool::Last => *vals.last().unwrap_or(&0.0),
        };
    }
    out
}

/// The exact 4→1 base projection: one 8 Hz row from its four 32 Hz rows.
///
/// Exactly four because the ladder nests, and this is the projection a base
/// machine applies to a canonical (`.32hz.npy`) row. It is [`pool_rows`] on an
/// array, named so the intent at every call site is the nesting rule.
pub fn pool_base(four: &[[f32; FEATURES]; 4]) -> [f32; FEATURES] {
    pool_rows(four)
}

// ---------------------------------------------------------------------------
// the shared effects
// ---------------------------------------------------------------------------
//
// `Post` is here rather than in `metal-render::preset` for the same reason
// `Schema` is: it is plain data, several things want it, and it has no
// graphics in it. `preset.rs` re-exports it and still owns saving and loading.

/// The shared effects as they were when the preset was saved.
///
/// They live in the preset because they are part of the image. A preset that
/// restored only the model's own row came back looking different whenever
/// shake or the leaks had been moved since — which broke the one promise the
/// preset makes ("load it and get the same picture").
///
/// **Every field added here must be `#[serde(default)]`.** Presets are files on
/// disk written by earlier builds, and a required field is a preset library
/// that stops loading the day an effect is added — see the legacy case in this
/// module's own test.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Post {
    /// Version of the effects schema. Incremented when new effects are added or
    /// existing ones are reorganized. Used to migrate presets from older versions.
    /// Preset files must carry this version to ensure effects read from the correct
    /// offsets.
    #[serde(default)]
    pub version: u32,
    pub shake: f32,
    pub light_leak: f32,
    pub leak_color: Vec<f32>,
    /// How long an old frame keeps showing, 0..1 — the trail asked for on
    /// 2026-08-17. **0 is off, and off means the pass does not run at all**,
    /// which is what keeps a frame a pure function of `(floats, time)` for
    /// every gate that renders once from a cold renderer.
    #[serde(default)]
    pub ghost: f32,
    /// How many mirrors the whole frame is folded into. Below 2 is off. The
    /// same fold the Kaleidoscope model draws with, applied to whatever just
    /// rendered — see `docs/NEXT.md` §3, which recommended it as an effect
    /// exactly because it is worth eighteen looks for six lines of shader.
    #[serde(default)]
    pub kaleido: f32,
    /// Six picks ported in from radiance (2026-08-25) — see
    /// `import/shader-picks/IMPORT.md`. All six (plus `vignette`/`blur`/
    /// `sharpen`/`grain`, added the same way just after) are common the same
    /// way `kaleido` is: one float against whatever came before it in the
    /// chain, 0 is exactly off (`mix(chain_color, effect, 0.0)` returns
    /// `chain_color`, unchanged to the bit). Through 2026-08-30 none of them
    /// chained through another's warp — turning on `crt` and `zoom` together
    /// cross-faded one over the other instead of compositing. E5
    /// (the design notes PARTE IV §18.11) fixed that: they now thread one shared warped
    /// UV/color pair, in a fixed order, the way `kaleido`/shake/crop already
    /// did — see the block comment above `effect_crt` in `shaders/post.wgsl`
    /// for exactly what composes and the one disclosed exception
    /// (`blur`/`sharpen`'s neighbour taps, which still need a real
    /// multi-pass to fully chain).
    #[serde(default)]
    pub crt: f32,
    #[serde(default)]
    pub glitch: f32,
    #[serde(default)]
    pub posterize: f32,
    /// Signed: positive punches in, negative pulls out. One float rather than
    /// two (`zoomin`/`zoomout`) because radiance's own two files are the same
    /// six lines with the transform inverted — see `effect_zoom` in
    /// `shaders/post.wgsl`.
    #[serde(default)]
    pub zoom: f32,
    #[serde(default)]
    pub fire: f32,
    #[serde(default)]
    pub lorenz: f32,
    /// Four more, added the same way the six radiance picks were — see
    /// `sharpen`'s field name: called `sharpen`, not `sharp`, to match the
    /// slider's wire key.
    #[serde(default)]
    pub vignette: f32,
    #[serde(default)]
    pub blur: f32,
    #[serde(default)]
    pub sharpen: f32,
    #[serde(default)]
    pub grain: f32,
    /// The order the effects above compose in, when the operator has said.
    ///
    /// Empty is the honest default and means "the fixed order this shader
    /// always used" — every preset on disk written before this field existed
    /// deserializes to exactly that, and [`Post::resolved_chain`] turns it
    /// back into the same sequence `fs_main` hardcoded. Non-empty comes from
    /// a node graph: one entry per `fx.*` node between a generator and its
    /// sink, in graph order, which is the whole point of letting someone drag
    /// those nodes around.
    ///
    /// The flat fields above stay the source of truth for *how much* of each
    /// effect. This is only *when*. Keeping the two apart is what lets a
    /// preset written by the graph editor still load in a build that has no
    /// graph editor.
    #[serde(default)]
    pub chain: Vec<EffectStep>,
    /// The advanced effects' floats — the colour grade plus texture, clarity,
    /// dehaze, denoise, optics and geometry — in [`ADVANCED_EFFECTS`] order.
    /// One flat vector rather than 79 named fields, and `#[serde(default)]`
    /// so a v1 preset written before they existed loads with an empty one
    /// (and `amount` past [`CHAIN_FLOATS`] answers `0.0`).
    ///
    /// Their home in a *row* is the tail of [`EFFECTS_FLOATS`], before
    /// `leak_color`; this mirror exists so the AI Colourist's manifest walk
    /// and the preset both see them as ordinary floats.
    #[serde(default)]
    pub advanced: Vec<f32>,
}

/// The declared defaults of every advanced float, in [`ADVANCED_EFFECTS`]
/// order — what [`Post::default`] starts `advanced` at, so a fresh post is the
/// neutral grade rather than all zeros (the split/refine/denoise/optics/
/// geometry floats have non-zero neutral points).
fn advanced_defaults() -> Vec<f32> {
    let mut out = Vec::with_capacity(N_ADVANCED);
    for (_, floats) in ADVANCED_EFFECTS {
        out.extend(floats.iter().map(|s| s.default));
    }
    out
}

impl Default for Post {
    fn default() -> Post {
        Post {
            version: EFFECTS_VERSION,
            shake: 0.0,
            light_leak: 0.0,
            leak_color: vec![1.0, 0.9, 0.6, 1.0, 0.3, 0.9, 0.9, 0.2, 0.6, 0.6, 0.8, 1.0],
            ghost: 0.0,
            kaleido: 0.0,
            crt: 0.0,
            glitch: 0.0,
            posterize: 0.0,
            zoom: 0.0,
            fire: 0.0,
            lorenz: 0.0,
            vignette: 0.0,
            blur: 0.0,
            sharpen: 0.0,
            grain: 0.0,
            chain: Vec::new(),
            advanced: advanced_defaults(),
        }
    }
}

/// One common effect, declared once — `docs/plan-architecture.md` D3/LAY.4.
///
/// **Why this table exists.** Adding an effect used to mean six edits in six
/// files: a field on [`Post`], a row in `EFFECTS_FLOATS`, a width, a column in
/// `effect_column_map`, a function in `post.wgsl`, an id in
/// `metal_graph::COMMON_EFFECT_IDS` and a step in the resolver. Four of those six
/// are derived from this table now, so a new effect is one row here plus one
/// WGSL function. The declarative half and the drawn half stop being able to
/// disagree about the list.
///
/// **Append-only.** The row order is the float order (`EFFECTS_FLOATS`) and
/// the `kind` a graph step names, so a new effect is appended, never inserted
/// — see `docs/plan-architecture.md` PARTE VII.
pub struct EffectSpec {
    /// The float, including its name and range. The name is also the `fx.*`
    /// suffix and the digest column's owner.
    pub spec: FloatSpec,
    /// Which of the 0..16 digest columns drives it.
    pub column: usize,
    /// The WGSL function that draws it. Empty for the four that are applied
    /// inline in `fs_main` rather than through a function (shake, light_leak)
    /// or entirely outside the post shader (ghost, kaleido).
    pub wgsl: &'static str,
    /// Whether it is one of the chain steps `fs_main` loops over. The first
    /// four are applied above the loop and are never steps.
    pub chains: bool,
    /// Whether it genuinely needs a second ping-pong pass to compose with its
    /// neighbours (blur/sharpen's neighbour taps) rather than the shared
    /// warped UV/color pair.
    pub needs_multipass: bool,
}

/// The fourteen common effects, in float/kind order. Everything else in this
/// module about them is derived from this one list.
pub const EFFECT_SPECS: &[EffectSpec] = &[
    // The original four. `shake` and `kaleido` warp the frame's own
    // coordinates before the chain starts; `light_leak` is a screen-locked
    // overlay; `ghost` is a trail entirely outside this shader. None is a
    // chain step and none has a the post function.
    EffectSpec { spec: spec("shake", 0.0, 1.0, 0.0), column: 0, wgsl: "", chains: false, needs_multipass: false },
    EffectSpec { spec: spec("light_leak", 0.0, 1.0, 0.0), column: 2, wgsl: "", chains: false, needs_multipass: false },
    EffectSpec { spec: spec("ghost", 0.0, 1.0, 0.0), column: 2, wgsl: "", chains: false, needs_multipass: false },
    EffectSpec { spec: spec("kaleido", 0.0, 16.0, 0.0), column: 3, wgsl: "", chains: false, needs_multipass: false },
    // The six radiance picks plus the four after them — the chain steps.
    EffectSpec { spec: spec("crt", 0.0, 1.0, 0.0), column: 1, wgsl: "effect_crt", chains: true, needs_multipass: false },
    EffectSpec { spec: spec("glitch", 0.0, 1.0, 0.0), column: 2, wgsl: "effect_glitch", chains: true, needs_multipass: false },
    EffectSpec { spec: spec("posterize", 0.0, 1.0, 0.0), column: 2, wgsl: "effect_posterize", chains: true, needs_multipass: false },
    EffectSpec { spec: spec("zoom", -1.0, 1.0, 0.0), column: 0, wgsl: "effect_zoom", chains: true, needs_multipass: false },
    EffectSpec { spec: spec("fire", 0.0, 1.0, 0.0), column: 2, wgsl: "effect_fire", chains: true, needs_multipass: false },
    EffectSpec { spec: spec("lorenz", 0.0, 1.0, 0.0), column: 3, wgsl: "effect_lorenz", chains: true, needs_multipass: false },
    EffectSpec { spec: spec("vignette", 0.0, 1.0, 0.0), column: 2, wgsl: "effect_vignette", chains: true, needs_multipass: false },
    EffectSpec { spec: spec("blur", 0.0, 1.0, 0.0), column: 1, wgsl: "effect_blur", chains: true, needs_multipass: true },
    EffectSpec { spec: spec("sharpen", 0.0, 1.0, 0.0), column: 0, wgsl: "effect_sharpen", chains: true, needs_multipass: true },
    EffectSpec { spec: spec("grain", 0.0, 1.0, 0.0), column: 2, wgsl: "effect_grain", chains: true, needs_multipass: false },
];

/// How many common effects there are — one count, derived, so no literal
/// "fourteen" appears anywhere.
pub const N_EFFECTS: usize = EFFECT_SPECS.len();

// ---------------------------------------------------------------------------
// the advanced effects — Lightroom-style colour grade plus six effects,
// appended to the common row so every model carries them
// ---------------------------------------------------------------------------

/// The Lightroom-style colour grade: the ten tone/WB sliders, four curve
/// regions, the three-way split toning, the eight-band colour mixer, the
/// three-way grading wheels plus a global wheel, and blending/balance. Every
/// float is `-100..=100` (or the wheel's own `0..360` hue) with `0` neutral,
/// except the wheel hues and the split/refine defaults — see the rows.
pub const GRADE_FLOATS: &[FloatSpec] = &[
    spec("grade_temperature", -100.0, 100.0, 0.0),
    spec("grade_tint", -100.0, 100.0, 0.0),
    spec("grade_exposure", -100.0, 100.0, 0.0),
    spec("grade_contrast", -100.0, 100.0, 0.0),
    spec("grade_highlights", -100.0, 100.0, 0.0),
    spec("grade_shadows", -100.0, 100.0, 0.0),
    spec("grade_whites", -100.0, 100.0, 0.0),
    spec("grade_blacks", -100.0, 100.0, 0.0),
    spec("grade_vibrance", -100.0, 100.0, 0.0),
    spec("grade_saturation", -100.0, 100.0, 0.0),
    spec("grade_curve_highlights", -100.0, 100.0, 0.0),
    spec("grade_curve_lights", -100.0, 100.0, 0.0),
    spec("grade_curve_darks", -100.0, 100.0, 0.0),
    spec("grade_curve_shadows", -100.0, 100.0, 0.0),
    spec("grade_split_shadows", 10.0, 70.0, 25.0),
    spec("grade_split_mid", 20.0, 80.0, 50.0),
    spec("grade_split_highlights", 30.0, 90.0, 75.0),
    spec("grade_refine_saturation", 0.0, 100.0, 100.0),
    // mixer: 8 bands x hue/sat/lum
    spec("grade_mixer_red_hue", -100.0, 100.0, 0.0), spec("grade_mixer_red_sat", -100.0, 100.0, 0.0), spec("grade_mixer_red_lum", -100.0, 100.0, 0.0),
    spec("grade_mixer_orange_hue", -100.0, 100.0, 0.0), spec("grade_mixer_orange_sat", -100.0, 100.0, 0.0), spec("grade_mixer_orange_lum", -100.0, 100.0, 0.0),
    spec("grade_mixer_yellow_hue", -100.0, 100.0, 0.0), spec("grade_mixer_yellow_sat", -100.0, 100.0, 0.0), spec("grade_mixer_yellow_lum", -100.0, 100.0, 0.0),
    spec("grade_mixer_green_hue", -100.0, 100.0, 0.0), spec("grade_mixer_green_sat", -100.0, 100.0, 0.0), spec("grade_mixer_green_lum", -100.0, 100.0, 0.0),
    spec("grade_mixer_aqua_hue", -100.0, 100.0, 0.0), spec("grade_mixer_aqua_sat", -100.0, 100.0, 0.0), spec("grade_mixer_aqua_lum", -100.0, 100.0, 0.0),
    spec("grade_mixer_blue_hue", -100.0, 100.0, 0.0), spec("grade_mixer_blue_sat", -100.0, 100.0, 0.0), spec("grade_mixer_blue_lum", -100.0, 100.0, 0.0),
    spec("grade_mixer_purple_hue", -100.0, 100.0, 0.0), spec("grade_mixer_purple_sat", -100.0, 100.0, 0.0), spec("grade_mixer_purple_lum", -100.0, 100.0, 0.0),
    spec("grade_mixer_magenta_hue", -100.0, 100.0, 0.0), spec("grade_mixer_magenta_sat", -100.0, 100.0, 0.0), spec("grade_mixer_magenta_lum", -100.0, 100.0, 0.0),
    // 3-way + global grading
    spec("grade_grading_shadows_hue", 0.0, 360.0, 0.0), spec("grade_grading_shadows_sat", 0.0, 100.0, 0.0), spec("grade_grading_shadows_lum", -100.0, 100.0, 0.0),
    spec("grade_grading_midtones_hue", 0.0, 360.0, 0.0), spec("grade_grading_midtones_sat", 0.0, 100.0, 0.0), spec("grade_grading_midtones_lum", -100.0, 100.0, 0.0),
    spec("grade_grading_highlights_hue", 0.0, 360.0, 0.0), spec("grade_grading_highlights_sat", 0.0, 100.0, 0.0), spec("grade_grading_highlights_lum", -100.0, 100.0, 0.0),
    spec("grade_grading_global_hue", 0.0, 360.0, 0.0), spec("grade_grading_global_sat", 0.0, 100.0, 0.0), spec("grade_grading_global_lum", -100.0, 100.0, 0.0),
    spec("grade_blending", 0.0, 100.0, 50.0),
    spec("grade_balance", -100.0, 100.0, 0.0),
];

/// Texture: local-contrast detail at the pixel scale.
pub const TEXTURE_FLOATS: &[FloatSpec] = &[ spec("fx_texture", -100.0, 100.0, 0.0) ];
/// Clarity: mid-tone local contrast, a broader radius than texture.
pub const CLARITY_FLOATS: &[FloatSpec] = &[ spec("fx_clarity", -100.0, 100.0, 0.0) ];
/// Dehaze: removes (or adds) atmospheric haze.
pub const DEHAZE_FLOATS: &[FloatSpec] = &[ spec("fx_dehaze", -100.0, 100.0, 0.0) ];
/// Denoise: luminance and colour noise reduction, each with its detail and
/// smoothness controls.
pub const DENOISE_FLOATS: &[FloatSpec] = &[
    spec("fx_denoise_luminance", 0.0, 100.0, 0.0),
    spec("fx_denoise_detail", 0.0, 100.0, 50.0),
    spec("fx_denoise_contrast", 0.0, 100.0, 0.0),
    spec("fx_denoise_color", 0.0, 100.0, 0.0),
    spec("fx_denoise_color_detail", 0.0, 100.0, 50.0),
    spec("fx_denoise_color_smoothness", 0.0, 100.0, 50.0),
];
/// Optics: lens correction — distortion, vignetting, defringe and chromatic
/// aberration.
pub const OPTICS_FLOATS: &[FloatSpec] = &[
    spec("fx_optics_distortion", -100.0, 100.0, 0.0),
    spec("fx_optics_vignetting", -100.0, 100.0, 0.0),
    spec("fx_optics_vignetting_midpoint", 0.0, 100.0, 50.0),
    spec("fx_optics_defringe_purple", 0.0, 20.0, 0.0),
    spec("fx_optics_defringe_green", 0.0, 20.0, 0.0),
    spec("fx_optics_ca_red", -100.0, 100.0, 0.0),
    spec("fx_optics_ca_blue", -100.0, 100.0, 0.0),
];
/// Geometry: perspective and crop — the transform controls.
pub const GEOMETRY_FLOATS: &[FloatSpec] = &[
    spec("fx_geometry_vertical", -100.0, 100.0, 0.0),
    spec("fx_geometry_horizontal", -100.0, 100.0, 0.0),
    spec("fx_geometry_rotate", -10.0, 10.0, 0.0),
    spec("fx_geometry_aspect", -100.0, 100.0, 0.0),
    spec("fx_geometry_scale", 50.0, 150.0, 100.0),
    spec("fx_geometry_offset_x", -100.0, 100.0, 0.0),
    spec("fx_geometry_offset_y", -100.0, 100.0, 0.0),
];

/// FM: frequency-modulation warp (after TouchDesigner's FM idea). A carrier
/// wave displaces the picture along `angulo`; its phase is itself bent by a
/// modulator wave, `indice` deep. Unlike the groups above it **is a chain
/// step** ([`ADVANCED_CHAIN_EFFECTS`]): `fx_fm_cant` is its amount, the rest
/// ride in the post uniform's `ChainFx` block.
///
/// `cant` is held (drive 0) so the music cannot switch the warp on by itself;
/// `forma_onda` (0 sine, 1 triangle, 2 saw, 3 square) is structural, so held.
/// `realim` is in-frame recursive modulation, not a previous-frame feedback
/// (the post pass has no previous-frame texture): the modulator's phase is
/// pushed by the luma of the already-displaced sample, then it resamples.
pub const FM_FLOATS: &[FloatSpec] = &[
    spec_held("fx_fm_cant", 0.0, 1.0, 0.0, 0.0),
    spec("fx_fm_portadora_freq", 0.5, 40.0, 6.0),
    spec("fx_fm_moduladora_freq", 0.1, 40.0, 2.0),
    spec("fx_fm_indice", 0.0, 10.0, 2.0),
    spec("fx_fm_fase", 0.0, 1.0, 0.0),
    spec("fx_fm_angulo", 0.0, 360.0, 0.0),
    spec_held("fx_fm_forma_onda", 0.0, 3.0, 0.0, 0.0),
    spec("fx_fm_x_cant", -1.0, 1.0, 1.0),
    spec("fx_fm_y_cant", -1.0, 1.0, 0.5),
    spec("fx_fm_color_cant", 0.0, 1.0, 0.0),
    spec("fx_fm_realim", 0.0, 1.0, 0.0),
];

/// Wave Warp (after After Effects): a travelling wave displaces the picture
/// sideways to its direction of travel. A **chain step** like `fm`:
/// `fx_wavewarp_cant` is the amount (it scales the height), the rest ride in
/// `ChainFx`. `cant` is held (drive 0) so music cannot switch it on.
///
/// Structural floats are held: `tipo` (0 sine, 1 triangle, 2 square, 3 saw,
/// 4 circle, 5 semicircle, 6 uncircle, 7 noise), `pin` (0 none, 1 all edges,
/// 2 left, 3 right, 4 top, 5 bottom, 6 centre) and `borde` (what the warp
/// reads past the frame: 0 clamp, 1 mirror, 2 wrap). `largo` and `alto` are
/// fractions of the frame height; `vel` is cycles per second (time-driven);
/// `pin_x`/`pin_y` are how far the pin reaches in from the edge (or out from
/// the centre) before the wave is free. `asimetria` swells the crests and
/// shrinks the troughs. `capa2*` is a second wave (amount, wavelength ratio,
/// angle offset) and `cruce` bleeds the displacement along the travel axis.
pub const WAVEWARP_FLOATS: &[FloatSpec] = &[
    spec_held("fx_wavewarp_cant", 0.0, 1.0, 0.0, 0.0),
    spec_held("fx_wavewarp_tipo", 0.0, 7.0, 0.0, 0.0),
    spec("fx_wavewarp_largo", 0.02, 1.5, 0.25),
    spec("fx_wavewarp_alto", 0.0, 0.4, 0.06),
    spec("fx_wavewarp_angulo", 0.0, 360.0, 0.0),
    spec("fx_wavewarp_vel", -2.0, 2.0, 0.25),
    spec("fx_wavewarp_fase", 0.0, 360.0, 0.0),
    spec_held("fx_wavewarp_pin", 0.0, 6.0, 0.0, 0.0),
    spec("fx_wavewarp_pin_x", 0.05, 1.0, 0.5),
    spec("fx_wavewarp_pin_y", 0.05, 1.0, 0.5),
    spec("fx_wavewarp_asimetria", -1.0, 1.0, 0.0),
    spec("fx_wavewarp_capa2", 0.0, 1.0, 0.0),
    spec("fx_wavewarp_capa2_largo", 0.25, 4.0, 0.6),
    spec("fx_wavewarp_capa2_angulo", 0.0, 180.0, 90.0),
    spec("fx_wavewarp_cruce", -1.0, 1.0, 0.0),
    spec_held("fx_wavewarp_borde", 0.0, 2.0, 0.0, 0.0),
];

/// ASCII: the frame redrawn as a grid of characters. Each cell's luminance
/// picks a glyph from one of four ramps (the 8x8 bitmap font of the `ascii`
/// model, embedded in `post.wgsl`) and the glyph is tinted from the frame. A
/// **chain step** ([`ADVANCED_CHAIN_EFFECTS`]): `fx_ascii_cant` is how much of
/// the picture is turned to text (0 leaves the frame untouched).
///
/// `cant` is held (drive 0) so the music cannot switch the effect on. The
/// structural floats (`celda`, `rampa`, `color_modo`, `proporcion`) are held
/// too: swung by the beat the grid reflows or the character set cycles.
/// `color_modo` is 0 mono tint, 1 frame colour, 2 palette (hue walks with
/// luminance); `rampa` is 0..3 one of the four sets, 4 every set mixed per cell.
pub const ASCII_FLOATS: &[FloatSpec] = &[
    spec_held("fx_ascii_cant", 0.0, 1.0, 0.0, 0.0),
    spec_held("fx_ascii_celda", 4.0, 32.0, 8.0, 0.0),
    spec_held("fx_ascii_rampa", 0.0, 4.0, 0.0, 0.0),
    spec("fx_ascii_contraste", 0.2, 3.0, 1.0),
    spec_held("fx_ascii_color_modo", 0.0, 2.0, 1.0, 0.0),
    spec("fx_ascii_color_mezcla", 0.0, 1.0, 0.3),
    spec("fx_ascii_fondo", 0.0, 1.0, 0.12),
    spec("fx_ascii_invierte", 0.0, 1.0, 0.0),
    spec("fx_ascii_ruido", 0.0, 1.0, 0.0),
    spec("fx_ascii_gamma", 0.3, 3.0, 1.0),
    spec("fx_ascii_parpadeo", 0.0, 1.0, 0.0),
    spec_held("fx_ascii_proporcion", 0.5, 2.0, 1.0, 0.0),
    spec("fx_ascii_tinte", 0.0, 1.0, 0.33),
    spec("fx_ascii_bordes", 0.0, 1.0, 0.0),
    spec("fx_ascii_deriva", -4.0, 4.0, 0.0),
    spec("fx_ascii_grosor", 0.0, 1.0, 0.0),
];

/// Brush: the frame repainted from strokes, after `fogleman/primitive`
/// (behaviour only). A stateful **chain step** ([`ADVANCED_CHAIN_EFFECTS`]):
/// `fx_brush_cant` is how much of the painting replaces the frame (0 leaves it
/// untouched and allocates no canvas). It keeps its own persistent canvas, so
/// it runs *before* the post pass whatever its place in the chain, redrawing
/// what every other step then receives (`shared::brush` has the design).
///
/// Two modes (`modo`, held): 0 `dab` lays a grid of oriented strokes sampled
/// from the frame, `cobertura` of them per frame, accumulating; 1 `search`
/// picks each stroke as the best of `candidatos` random ones (then refines it)
/// by error against the frame at a 64x64 proxy, like primitive's `core.go`.
/// `forma` is 0 ellipse, 1 rect, 2 triangle, 3 bezier, 4 mixed. `tam_ini` ->
/// `tam_fin` is the stroke size (in grid cells) from the first stroke of a
/// frame to the last: coarse first, detail after. `renueva` pulls the canvas
/// back toward the live frame, `decae` toward the paper; `papel` is how much
/// paper (colour `papel_tono`, with grain) shows under and through the strokes.
/// `sigue` turns strokes along the frame's edges; `angulo` biases the rest.
///
/// The structural floats (`forma`, `n_trazos`, `modo`, `candidatos`) are held:
/// swung by the beat the picture would repaint from scratch.
pub const BRUSH_FLOATS: &[FloatSpec] = &[
    spec_held("fx_brush_cant", 0.0, 1.0, 0.0, 0.0),
    spec_held("fx_brush_forma", 0.0, 4.0, 0.0, 0.0),
    spec_held("fx_brush_n_trazos", 64.0, 4096.0, 1200.0, 0.0),
    spec("fx_brush_tam_ini", 0.5, 8.0, 3.0),
    spec("fx_brush_tam_fin", 0.25, 8.0, 1.0),
    spec("fx_brush_alfa", 0.05, 1.0, 0.75),
    spec("fx_brush_aleatoriedad", 0.0, 1.0, 0.4),
    spec_held("fx_brush_modo", 0.0, 1.0, 0.0, 0.0),
    spec_held("fx_brush_candidatos", 2.0, 32.0, 8.0, 0.0),
    spec("fx_brush_cobertura", 0.02, 1.0, 0.3),
    spec("fx_brush_renueva", 0.0, 1.0, 0.2),
    spec("fx_brush_contorno", 0.0, 1.0, 0.0),
    spec("fx_brush_papel", 0.0, 1.0, 0.3),
    spec("fx_brush_papel_tono", 0.0, 1.0, 0.9),
    spec("fx_brush_angulo", -180.0, 180.0, 0.0),
    spec("fx_brush_sigue", 0.0, 1.0, 0.6),
    spec("fx_brush_alargado", 0.5, 4.0, 2.0),
    spec("fx_brush_color_jitter", 0.0, 1.0, 0.15),
    spec("fx_brush_saturacion", 0.0, 2.0, 1.0),
    spec("fx_brush_decae", 0.0, 1.0, 0.0),
];

/// Hard stutter: a beat-locked gate that hides or holds frames, fast (techno,
/// psytrance). A **chain step** like `fm`, but the only *stateful* one: it
/// freezes the FINAL picture, so it runs as its own pass after the whole post
/// chain (`shared::stutter`, see its header) whatever its place in the chain;
/// it has no `ChainFx` block (the pass reads `Post.advanced`). `cant` is held
/// (drive 0) so music cannot switch it on; it is how much of the gated frame
/// replaces the live one (1 = a hard stutter).
///
/// Structural floats are held: `ritmo` (0 1/4, 1 1/8, 2 1/8 triplet, 3 1/16,
/// 4 1/16 triplet, 5 1/32), `repite_n` (0 or 1 = freeze one frame, N = loop
/// the last N frames while held, up to 8), `patron` (0 every step, 1 offbeats,
/// 2 euclid 3/8, 3 euclid 5/8, 4 random), `invierte_ritmo` (swaps which steps
/// the pattern gates) and `tempo` (BPM: the renderer has no beat clock, the
/// gate is locked to `time` at this tempo). `duty` is the visible share of a
/// gated step; `congela` is hold (1) against black (0) while hidden; `retardo`
/// slides the grid by up to one step; `jitter` wobbles each step's duty;
/// `desvanece` eases the cut over the first part of the hidden span; `tinte`
/// and `tinte_tono` colour the held frame (amount, hue).
pub const HARDSTUTTER_FLOATS: &[FloatSpec] = &[
    spec_held("fx_hardstutter_cant", 0.0, 1.0, 0.0, 0.0),
    spec_held("fx_hardstutter_ritmo", 0.0, 5.0, 3.0, 0.0),
    spec("fx_hardstutter_duty", 0.05, 0.95, 0.5),
    spec("fx_hardstutter_congela", 0.0, 1.0, 1.0),
    spec_held("fx_hardstutter_repite_n", 0.0, 8.0, 0.0, 0.0),
    spec("fx_hardstutter_retardo", 0.0, 1.0, 0.0),
    spec("fx_hardstutter_jitter", 0.0, 1.0, 0.0),
    spec_held("fx_hardstutter_patron", 0.0, 4.0, 0.0, 0.0),
    spec_held("fx_hardstutter_invierte_ritmo", 0.0, 1.0, 0.0, 0.0),
    spec_held("fx_hardstutter_tempo", 60.0, 200.0, 128.0, 0.0),
    spec("fx_hardstutter_desvanece", 0.0, 1.0, 0.0),
    spec("fx_hardstutter_tinte", 0.0, 1.0, 0.0),
    spec("fx_hardstutter_tinte_tono", 0.0, 1.0, 0.0),
];

/// Material: the one group in every row that says what a 3D model's surface is
/// made of — `mat.<kind>` in the materials brief. It rides the effects tail so
/// every model already carries it (the shared block the renderers append to a
/// schema), and the digest drives it through `effect_column_map` like any
/// other group, but it names no `fx.*` node and `metal-graph` builds no catalog
/// entry from it. It is read back off the row past the advanced effects.
///
/// **All defaults are 0, and 0 is off.** `mat_mix` at 0 is "no material": the
/// surface keeps the host's own colour, which is what keeps a row saved before
/// this group existed byte-identical after [`migrate_row`] pads it with its
/// declared defaults. A float at 0 means "the kind decides", so the migrated
/// all-zero group and a fresh default row say the same thing.
///
/// `mat_kind` is held ([`spec_held`], `drive` 0): a live signal that swung the
/// kind would switch the surface's whole regime rather than season it. The
/// glitch rate and seed are held for the same reason — the operator or the
/// automix sets how fast and with which seed, and the music does not re-roll
/// them mid-bar. `mat_glitch` itself is drivable and shares the strobe's
/// column (onset), so a material glitch and a beat flash land on the same hit.
pub const MATERIAL_FLOATS: &[FloatSpec] = &[
    spec_held("mat_kind", 0.0, 6.0, 0.0, 0.0),
    spec("mat_mix", 0.0, 1.0, 0.0),
    spec("mat_scale", 0.0, 8.0, 0.0),
    spec("mat_rough", 0.0, 1.0, 0.0),
    spec("mat_metal", 0.0, 1.0, 0.0),
    spec("mat_bump", 0.0, 1.0, 0.0),
    spec("mat_env", 0.0, 1.0, 0.0),
    spec("mat_tint", 0.0, 1.0, 0.0),
    spec("mat_tint_hue", 0.0, 1.0, 0.0),
    spec("mat_glitch", 0.0, 1.0, 0.0),
    spec_held("mat_glitch_rate", 0.0, 24.0, 0.0, 0.0),
    spec_held("mat_glitch_seed", 0.0, 1.0, 0.0, 0.0),
];

/// How many floats the material group adds to every row.
pub const N_MATERIAL: usize = MATERIAL_FLOATS.len();

/// The digest column each material float is driven by, in [`MATERIAL_FLOATS`]
/// order. The continuous look floats ride bandwidth (1); the glitch trio rides
/// [`ONSET_COLUMN`], the beat column the strobe's own flash is locked to, so a
/// material glitch and a strobe hit land on the same beat.
const MATERIAL_COLUMNS: [usize; N_MATERIAL] = [
    2, // mat_kind        (held)
    1, // mat_mix
    1, // mat_scale
    1, // mat_rough
    1, // mat_metal
    1, // mat_bump
    1, // mat_env
    1, // mat_tint
    1, // mat_tint_hue
    ONSET_COLUMN, // mat_glitch       — the beat, shared with the strobe
    ONSET_COLUMN, // mat_glitch_rate  (held)
    ONSET_COLUMN, // mat_glitch_seed  (held)
];


// ---------------------------------------------------------------------------
// Ported Sketch filter tools (2026-10-21)
// ---------------------------------------------------------------------------

/// Pixelator: quantise the picture into square/circle/diamond cells.
pub const MOSAIC_FLOATS: &[FloatSpec] = &[
    spec("fx_mosaic_amount", 0.0, 1.0, 0.0),
    spec_held("fx_mosaic_cell", 4.0, 128.0, 32.0, 0.1),
    spec("fx_mosaic_gap", 0.0, 1.0, 0.05),
    spec_held("fx_mosaic_shape", 0.0, 3.0, 0.0, 0.0),
];

/// Facets: a jittered cell mosaic with bevel edges.
pub const FACETED_FLOATS: &[FloatSpec] = &[
    spec("fx_faceted_amount", 0.0, 1.0, 0.0),
    spec_held("fx_faceted_count", 3.0, 40.0, 12.0, 0.1),
    spec("fx_faceted_edge", 0.0, 1.0, 0.2),
    spec("fx_faceted_jitter", 0.0, 1.0, 0.5),
];

/// Thermograph: luminance mapped onto a thermal palette in flat levels.
pub const THERMOGRAPH_FLOATS: &[FloatSpec] = &[
    spec("fx_thermograph_amount", 0.0, 1.0, 0.0),
    spec_held("fx_thermograph_levels", 2.0, 16.0, 6.0, 0.1),
    spec("fx_thermograph_contrast", 0.0, 1.0, 0.5),
    spec("fx_thermograph_tint", 0.0, 1.0, 0.5),
];

/// Depth map: luminance as depth, used to displace the picture in parallax.
pub const DEPTHFIELD_FLOATS: &[FloatSpec] = &[
    spec("fx_depthfield_amount", 0.0, 1.0, 0.0),
    spec("fx_depthfield_range", 0.0, 1.0, 0.4),
    spec("fx_depthfield_parallax", 0.0, 1.0, 0.3),
    spec_held("fx_depthfield_invert", 0.0, 1.0, 0.0, 0.0),
];

/// Prism: chromatic refraction through an angled wedge.
pub const REFRACT_FLOATS: &[FloatSpec] = &[
    spec("fx_refract_amount", 0.0, 1.0, 0.0),
    spec("fx_refract_strength", 0.0, 1.0, 0.3),
    spec("fx_refract_dispersion", 0.0, 1.0, 0.3),
    spec("fx_refract_angle", 0.0, 360.0, 0.0),
];

/// Relight: a directional light re-shades the picture from its own luminance.
pub const RELAMP_FLOATS: &[FloatSpec] = &[
    spec("fx_relamp_amount", 0.0, 1.0, 0.0),
    spec("fx_relamp_light_x", 0.0, 1.0, 0.35),
    spec("fx_relamp_light_y", 0.0, 1.0, 0.35),
    spec("fx_relamp_intensity", 0.0, 1.0, 0.6),
];

/// Letterform: the picture redrawn as a grid of block glyphs.
pub const LETTERFORM_FLOATS: &[FloatSpec] = &[
    spec("fx_letterform_amount", 0.0, 1.0, 0.0),
    spec_held("fx_letterform_cell", 4.0, 64.0, 16.0, 0.1),
    spec("fx_letterform_threshold", 0.0, 1.0, 0.5),
    spec_held("fx_letterform_invert", 0.0, 1.0, 0.0, 0.0),
];

/// Data vision: the picture as a field of sampled bars or a heat matrix.
pub const DATAFIELD_FLOATS: &[FloatSpec] = &[
    spec("fx_datafield_amount", 0.0, 1.0, 0.0),
    spec_held("fx_datafield_cell", 4.0, 64.0, 24.0, 0.1),
    spec("fx_datafield_gain", 0.0, 1.0, 0.6),
    spec_held("fx_datafield_mode", 0.0, 2.0, 0.0, 0.0),
];

/// The advanced effect node ids, in float order. Each is a group of one or more
/// floats appended to [`EFFECTS_FLOATS`] after the 14 common effects.
/// **Append-only**: a new group goes at the end *and* its id goes on
/// [`LATE_EFFECT_GROUPS`] so rows saved without it still migrate.
pub const ADVANCED_EFFECTS: &[(&str, &[FloatSpec])] = &[
    ("grade", GRADE_FLOATS),
    ("texture", TEXTURE_FLOATS),
    ("clarity", CLARITY_FLOATS),
    ("dehaze", DEHAZE_FLOATS),
    ("denoise", DENOISE_FLOATS),
    ("optics", OPTICS_FLOATS),
    ("geometry", GEOMETRY_FLOATS),
    ("fm", FM_FLOATS),
    ("wavewarp", WAVEWARP_FLOATS),
    ("ascii", ASCII_FLOATS),
    ("brush", BRUSH_FLOATS),
    ("hardstutter", HARDSTUTTER_FLOATS),
    ("mosaic", MOSAIC_FLOATS),
    ("faceted", FACETED_FLOATS),
    ("thermograph", THERMOGRAPH_FLOATS),
    ("depthfield", DEPTHFIELD_FLOATS),
    ("refract", REFRACT_FLOATS),
    ("relamp", RELAMP_FLOATS),
    ("letterform", LETTERFORM_FLOATS),
    ("datafield", DATAFIELD_FLOATS),
];

/// The advanced effect ids in the same order as [`ADVANCED_EFFECTS`] — the
/// list `metal-graph` builds its extra `fx.*` catalog entries from.
pub const ADVANCED_EFFECT_IDS: &[&str] = &[
    "grade",
    "texture",
    "clarity",
    "dehaze",
    "denoise",
    "optics",
    "geometry",
    "fm",
    "wavewarp",
    "ascii",
    "brush",
    "hardstutter",
    "mosaic",
    "faceted",
    "thermograph",
    "depthfield",
    "refract",
    "relamp",
    "letterform",
    "datafield",
];

/// The groups appended to [`ADVANCED_EFFECTS`] **after** the first seven, in
/// the order they were appended (a suffix of `ADVANCED_EFFECTS`). A saved row
/// that predates one has its floats missing from the middle of the row, just
/// before `leak_color`; [`migrate_effects_row`] puts them back at their
/// defaults. Every new advanced group is added here too.
pub const LATE_EFFECT_GROUPS: &[&str] = &[
    "fm",
    "wavewarp",
    "ascii",
    "brush",
    "hardstutter",
    "mosaic",
    "faceted",
    "thermograph",
    "depthfield",
    "refract",
    "relamp",
    "letterform",
    "datafield",
];

/// The material group's id in the row tail, for [`LATE_ROW_GROUPS`] and the
/// migration that names a group by id. It is not an advanced effect — see
/// [`MATERIAL_FLOATS`].
pub const MATERIAL_GROUP: &str = "material";

/// Every group appended to the effects tail after the first seven advanced
/// effects, newest last: the advanced groups [`LATE_EFFECT_GROUPS`] already
/// names, then the [`MATERIAL_FLOATS`] group. A saved row that predates one has
/// its floats missing just before `leak_color`; [`migrate_effects_row`] puts a
/// whole suffix of this list back at its declared defaults.
pub const LATE_ROW_GROUPS: &[&str] = &[
    "fm",
    "wavewarp",
    "ascii",
    "brush",
    "hardstutter",
    "mosaic",
    "faceted",
    "thermograph",
    "depthfield",
    "refract",
    "relamp",
    "letterform",
    "datafield",
    MATERIAL_GROUP,
];

/// The floats of a late tail group by id: [`advanced_effect_floats`] for the
/// advanced groups, [`MATERIAL_FLOATS`] for the material group. `None` for any
/// other id.
fn row_tail_floats(id: &str) -> Option<&'static [FloatSpec]> {
    if id == MATERIAL_GROUP {
        Some(MATERIAL_FLOATS)
    } else {
        advanced_effect_floats(id)
    }
}

/// The advanced groups that are also **chain steps** (their first float is
/// the step's amount; the rest reach the shader through `ChainFx`). Their kind
/// is the flat index of that first float, see [`advanced_chain_kind`].
pub const ADVANCED_CHAIN_EFFECTS: &[&str] = &[
    "fm",
    "wavewarp",
    "ascii",
    "brush",
    "hardstutter",
    "mosaic",
    "faceted",
    "thermograph",
    "depthfield",
    "refract",
    "relamp",
    "letterform",
    "datafield",
];

/// The chain-step kind of an advanced chain effect: the position of its first
/// float in [`EFFECTS_FLOATS`] (so [`Post::amount`] answers its amount and
/// [`Post::resolved_chain`] keeps the step). `None` for anything else.
pub fn advanced_chain_kind(id: &str) -> Option<u32> {
    if !ADVANCED_CHAIN_EFFECTS.contains(&id) {
        return None;
    }
    advanced_effect_span(id).map(|r| r.start as u32)
}

const fn advanced_float_count() -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < ADVANCED_EFFECTS.len() {
        n += ADVANCED_EFFECTS[i].1.len();
        i += 1;
    }
    n
}

/// How many floats the advanced effects add to the common ones — the width of
/// [`Post::advanced`] and the tail of [`EFFECTS_FLOATS`] past
/// [`CHAIN_FLOATS`].
pub const N_ADVANCED: usize = advanced_float_count();

/// The full flat effects row width: the common effects, every advanced float,
/// then the material group.
const N_EFFECTS_TOTAL: usize = N_EFFECTS + N_ADVANCED + N_MATERIAL;

const fn effects_floats_flat() -> [FloatSpec; N_EFFECTS_TOTAL] {
    let mut out = [spec("", 0.0, 1.0, 0.0); N_EFFECTS_TOTAL];
    let mut k = 0;
    let mut i = 0;
    while i < N_EFFECTS {
        out[k] = EFFECT_SPECS[i].spec;
        k += 1;
        i += 1;
    }
    let mut g = 0;
    while g < ADVANCED_EFFECTS.len() {
        let group = ADVANCED_EFFECTS[g].1;
        let mut j = 0;
        while j < group.len() {
            out[k] = group[j];
            k += 1;
            j += 1;
        }
        g += 1;
    }
    let mut m = 0;
    while m < N_MATERIAL {
        out[k] = MATERIAL_FLOATS[m];
        k += 1;
        m += 1;
    }
    out
}
const EFFECT_FLOATS_FULL: [FloatSpec; N_EFFECTS_TOTAL] = effects_floats_flat();

/// The digest column each advanced group is driven by, in [`ADVANCED_EFFECTS`]
/// order: grading/optics → 3, mixer/texture/clarity/dehaze → 2,
/// colour/wb/denoise/fm → 1, wavewarp/ascii/brush/hardstutter → 2, tone/levels/geometry → 0.
const ADVANCED_COLUMNS: [usize; ADVANCED_EFFECT_IDS.len()] = [
    3, 2, 2, 2, 1, 3, 0,
    1, // fm
    2, // wavewarp
    2, // ascii
    2, // brush
    2, // hardstutter
    2, // mosaic
    2, // faceted
    2, // thermograph
    2, // depthfield
    2, // refract
    2, // relamp
    2, // letterform
    2, // datafield
];

const fn effect_columns_flat() -> [usize; N_EFFECTS_TOTAL] {
    let mut out = [0usize; N_EFFECTS_TOTAL];
    let mut k = 0;
    let mut i = 0;
    while i < N_EFFECTS {
        out[k] = EFFECT_SPECS[i].column;
        k += 1;
        i += 1;
    }
    let mut g = 0;
    while g < ADVANCED_EFFECTS.len() {
        let group = ADVANCED_EFFECTS[g].1;
        let col = ADVANCED_COLUMNS[g];
        let mut j = 0;
        while j < group.len() {
            out[k] = col;
            k += 1;
            j += 1;
        }
        g += 1;
    }
    let mut m = 0;
    while m < N_MATERIAL {
        out[k] = MATERIAL_COLUMNS[m];
        k += 1;
        m += 1;
    }
    out
}
const EFFECT_COLUMNS_FULL: [usize; N_EFFECTS_TOTAL] = effect_columns_flat();

const fn effect_names_array() -> [&'static str; N_EFFECTS] {
    let mut out = [""; N_EFFECTS];
    let mut i = 0;
    while i < N_EFFECTS {
        out[i] = EFFECT_SPECS[i].spec.name;
        i += 1;
    }
    out
}

/// The common effect names, in float/kind order — the list `metal-graph` builds
/// its `fx.*` catalog from, derived here so the two cannot drift.
pub const EFFECT_NAMES: [&'static str; N_EFFECTS] = effect_names_array();

/// S.W — the closed vocabulary of **per-glyph** text effects, hung off a
/// `text.source`. They are not rows of [`EFFECT_SPECS`]: those compose a
/// pixel chain after a generator renders, while these transform glyphs
/// *before* the string is flattened to a texture (position, index, width —
/// see `the design notes` § "Text Source y Text Effects"). `metal-graph`
/// builds `fx.text.*` from this list and `metal-perform::text` implements it,
/// with a test on each side that no name becomes a no-op.
pub const TEXT_EFFECT_NAMES: &[&str] = &[
    "typewriter",
    "wave",
    "jitter",
    "color_cycle",
    "scale_pulse",
];

/// The shared effects, appended to *every* model's own row instead of living
/// only in the preset — see `docs/EFFECTS.md`. Widening every model's schema
/// by these two blocks is what lets `shake`/`light_leak`/`ghost`/`kaleido`/
/// `leak_color` be authored at a tick the same way any other float is: `K`
/// files them, the lane shows them, `save`/`export` carry them into the
/// `.npy`. Appended at the *end* of every schema, so no model's existing
/// float offsets move.
///
/// **Derived from [`EFFECT_SPECS`]** (LAY.4), not a second list — a row added
/// to the table appears here, in `effect_column_map`, in `EFFECT_NAMES` and in
/// `metal-graph`'s catalog at once.
///
/// The **common** 14 rows sit at `0..CHAIN_FLOATS`; every advanced float
/// follows at `CHAIN_FLOATS..CHAIN_FLOATS + N_ADVANCED`, in
/// [`ADVANCED_EFFECTS`] order, and the [`MATERIAL_FLOATS`] group closes the
/// block, so a saved flat index means the same effect it always did and the
/// later groups are append-only.
pub const EFFECTS_FLOATS: &[FloatSpec] = &EFFECT_FLOATS_FULL;
// ---------------------------------------------------------------------------
// the signal a model sends out of the screen
// ---------------------------------------------------------------------------

/// The floats every model carries for what it drives **off** the screen: a
/// bulb screwed into a lamp, a DMX fixture, a MIDI instrument. Not pixels.
///
/// **Why every model and not a node of its own.** A pixel is a position; a
/// lighting cue is not. There is no honest way to read "the show is at 70%,
/// amber, strobing on the downbeat" out of a framebuffer — averaging the
/// picture gives you the average of the picture, which is a different
/// statement than the one the model is making. So the model states it: the
/// same row that already says how bright the fire is says, in these eight
/// floats, what the room should do. They are authored, driven and keyframed
/// exactly like every other float, by machinery that does not have to learn
/// anything new, and the `out.*` nodes read them off the generator's `row`
/// port rather than off its `image`.
///
/// **Why between the model's own floats and the effects, and not at the very
/// end.** Half this workspace locates the effects with `len() -
/// EFFECTS_WIDTH` — `read_post` here, `paint.rs`, `stand.rs`, `render.rs`,
/// `migrate.rs`, and the pixel tests. Appending after the effects would move
/// every one of those reads by eight and be wrong in the way that stays in
/// range and shows the wrong picture. Appending *before* them moves nothing:
/// a model's own floats still start at 0, and the effects still trail.
///
/// The same rule the effects row has: **new signal floats are appended at the
/// end of this list, never inserted in the middle**, or every saved row means
/// something else.
pub const SIGNAL_FLOATS: &[FloatSpec] = &[
    // Master. What "the lights are up" means, before colour or movement.
    spec("sig_level", 0.0, 1.0, 0.0),
    // Colour, as hue/saturation rather than RGB: a bulb and a DMX fixture
    // both take a colour, and neither of them takes a twelve-float slot with
    // four corners in it the way `Block::ColorSlot` does — that shape is for
    // a gradient across a quad, which a lamp does not have.
    spec("sig_hue", 0.0, 1.0, 0.0),
    spec("sig_sat", 0.0, 1.0, 1.0),
    // Rate, in hertz at the top of the range, so 0 is "held" rather than
    // "infinitely fast" — the direction a fader is expected to travel.
    spec("sig_strobe", 0.0, 24.0, 0.0),
    // Where a moving head points. Signed and centred, because a pan of 0 is
    // straight ahead and not hard left.
    spec("sig_pan", -1.0, 1.0, 0.0),
    spec("sig_tilt", -1.0, 1.0, 0.0),
    // Two channels this crate deliberately does not name. A fixture has
    // gobos, prisms, focus, zoom and a dozen other things no shared list can
    // enumerate honestly; the `out.*` node's own config says what these two
    // reach on the fixture in front of it, and the model just moves them.
    spec("sig_a", 0.0, 1.0, 0.0),
    spec("sig_b", 0.0, 1.0, 0.0),
    // Appended at the *end* of the signal block (2026-09-27, BLE Phase B):
    // colour temperature and the beam-shaping channels a real fixture now has.
    // Appending here — and not inserting beside `sig_hue`/`sig_sat` — is the
    // rule this list's own doc states: a saved row's earlier columns keep
    // their meaning. It still shifts where the effects block starts (the
    // signal block sits *before* them), which is why rows are padded on load;
    // see `pad_signal` in `metal-config`.
    //
    // Kelvin, not a 0..1 fader: a lamp's driver takes a temperature, and the
    // whole point of a CCT channel is that two lights at "the same" 0..1 are
    // the same white. The range is the widest real bi-colour heads reach; a
    // profile that only reaches 2800..6500 declares its own `range`.
    spec("sig_cct", 1800.0, 10000.0, 5600.0),
    // Beam shaping, each its own 0..1 travel: zoom, focus, iris and frost are
    // what a moving head's beam does beyond pan and tilt. Separate floats and
    // not one `sig_a`-style pair because these are the four a beam profile
    // actually names, and a fixture that lacks one simply omits its channel.
    spec("sig_zoom", 0.0, 1.0, 0.0),
    spec("sig_focus", 0.0, 1.0, 0.0),
    spec("sig_iris", 0.0, 1.0, 0.0),
    spec("sig_frost", 0.0, 1.0, 0.0),
    // Appended 2026-10-06: palette motion. Not a signal for the room — these
    // move the picture's own colour slots — but this is the block every model
    // carries before the effects, so it is where a float every row needs
    // goes without moving anything. The palette a song starts from is the
    // origin; these are HSB variations around it that the Colourist can
    // learn like any float, instead of twelve raw RGB numbers per slot,
    // which it cannot (averaged across songs they turn to mud). See
    // [`apply_palette_motion`].
    //
    // Hue rotation in turns, kept within a quarter turn either way so a
    // movement stays a variation of the palette, not a different one.
    spec("col_hue", -0.25, 0.25, 0.0),
    // Saturation and brightness as multipliers of the origin's own.
    spec("col_sat", 0.0, 2.0, 1.0),
    spec("col_bri", 0.0, 2.0, 1.0),
    // Spreads the slot's four stops apart in brightness (highlights brighter,
    // shadows darker) when positive, pulls them together when negative.
    spec("col_contrast", -1.0, 1.0, 0.0),
];
pub const SIGNAL_FLOATS_BLOCK: Block = Block::Floats(SIGNAL_FLOATS);
/// How many trailing floats of a row belong to the signal block. Unlike
/// [`EFFECTS_WIDTH`] there is no colour slot in it — see `sig_hue`.
pub const SIGNAL_WIDTH: usize = SIGNAL_FLOATS.len();

/// The signal block's width **before** the BLE Phase B floats (`sig_cct`,
/// `sig_zoom`, `sig_focus`, `sig_iris`, `sig_frost`) were appended. A saved
/// row written before that append has this many signal floats, and reading it
/// with the current [`read_signal`] would start five floats too early — into
/// the model's own block, which is exactly the "stays in range and shows the
/// wrong thing" failure this crate's signal doc warns about.
pub const SIGNAL_WIDTH_BEFORE_BLE: usize = 8;

/// How many floats the BLE Phase B append and every append after it added to
/// every row.
pub const SIGNAL_FLOATS_ADDED: usize = SIGNAL_WIDTH - SIGNAL_WIDTH_BEFORE_BLE;

/// The signal block's width before the palette-motion floats (`col_*`).
pub const SIGNAL_WIDTH_BEFORE_COLOUR: usize = 13;

/// Rewrites a row saved before the BLE Phase B signal append into the current
/// row shape, **in place**. Returns `true` when it migrated something.
///
/// The signal block sits *before* the effects block, so appending floats to it
/// shifts where the effects begin even though the effects themselves trail from
/// the end. A row of the old shape has
/// `schema_width - SIGNAL_FLOATS_ADDED` floats; this inserts the new floats at
/// their schema defaults immediately before the effects block (at
/// `old_len - EFFECTS_WIDTH`), which is where the current `read_signal` and
/// `read_post` both expect them. Everything the row already said keeps its
/// meaning: the model's own floats do not move and the effects stay trailing.
///
/// `schema_width` is the row's current, full width (a model's
/// [`Schema::width`] or the manifest's block sum) — the only thing that tells
/// an old row apart from a deliberately short one. A row already at
/// `schema_width`, or one too short to hold the effects block at all, is left
/// untouched and answers `false`: this never guesses a shape it cannot prove.
pub fn migrate_signal_row(row: &mut Vec<f32>, schema_width: usize) -> bool {
    // A row from before any append is short by the floats added since: the
    // BLE ones and the palette motion (pre-BLE), or the palette motion alone.
    let from = [SIGNAL_WIDTH_BEFORE_BLE, SIGNAL_WIDTH_BEFORE_COLOUR]
        .into_iter()
        .find(|w| row.len() == schema_width.saturating_sub(SIGNAL_WIDTH - w));
    let Some(from) = from else {
        return false;
    };
    if row.len() < EFFECTS_WIDTH {
        return false;
    }
    let at = row.len() - EFFECTS_WIDTH;
    let defaults: Vec<f32> = SIGNAL_FLOATS[from..]
        .iter()
        .map(|s| s.default)
        .collect();
    row.splice(at..at, defaults);
    true
}

/// Rewrites a row saved before one or more [`LATE_ROW_GROUPS`] were
/// appended into the current shape, **in place**; `true` when it migrated.
///
/// The effects block trails the row, but the groups appended to it sit
/// *before* the 12-float `leak_color`, so an old row has them missing in the
/// middle of its tail. This inserts the missing groups' declared defaults
/// immediately before `leak_color` (`row.len() - 12`); nothing the row said
/// moves or changes meaning. The row may also be short by the signal floats
/// (`SIGNAL_WIDTH_BEFORE_*`); both gaps are recognised together so
/// [`migrate_row`] can then fix the signal block. Like [`migrate_signal_row`]
/// it only acts on a length it can account for exactly.
pub fn migrate_effects_row(row: &mut Vec<f32>, schema_width: usize) -> bool {
    let signal_gaps = [0, SIGNAL_WIDTH - SIGNAL_WIDTH_BEFORE_BLE, SIGNAL_WIDTH - SIGNAL_WIDTH_BEFORE_COLOUR];
    // `j` = how many of the newest late groups the row lacks.
    for j in 1..=LATE_ROW_GROUPS.len() {
        let missing: Vec<f32> = LATE_ROW_GROUPS[LATE_ROW_GROUPS.len() - j..]
            .iter()
            .filter_map(|id| row_tail_floats(id))
            .flat_map(|f| f.iter().map(|s| s.default))
            .collect();
        for gap in signal_gaps {
            if row.len() + missing.len() + gap == schema_width && row.len() >= 12 {
                let at = row.len() - 12;
                row.splice(at..at, missing);
                return true;
            }
        }
    }
    false
}

/// Every row-width migration, in the order it must run: the effects block
/// first (its length does not depend on the signal block), then the signal
/// block. The one entry point a loader calls. `true` when anything changed.
pub fn migrate_row(row: &mut Vec<f32>, schema_width: usize) -> bool {
    let a = migrate_effects_row(row, schema_width);
    let b = migrate_signal_row(row, schema_width);
    a || b
}

/// The one digest column a beat rides: **onset**, column 2. It is the
/// transient `drive.rs` rests at zero and lands raw (`rest_value`/
/// `smooth_default`), the column `fx.glitch` and the other hit effects take,
/// and the column the material glitch shares so a strobe hit and a material
/// glitch land on the same beat (materials brief, M8). Named once here so the
/// material table, the effects table and the tests cannot disagree about which
/// column the beat is.
pub const ONSET_COLUMN: usize = 2;

/// The material group's floats of a row, in [`MATERIAL_FLOATS`] order —
/// located from the end like [`read_post`], between the advanced effect floats
/// and `leak_color`. `None` for a row too short to hold the effects block, the
/// same contract the other tail readers have.
pub fn read_material(floats: &[f32]) -> Option<&[f32]> {
    let at = floats.len().checked_sub(EFFECTS_WIDTH)? + CHAIN_FLOATS + N_ADVANCED;
    floats.get(at..at + N_MATERIAL)
}

/// The palette-motion floats of a row: `[hue, sat, bri, contrast]`, located
/// from the end like [`read_signal`]. `None` for a row too short to hold them.
pub fn read_palette_motion(floats: &[f32]) -> Option<[f32; 4]> {
    let at = floats.len().checked_sub(EFFECTS_WIDTH + SIGNAL_WIDTH)? + SIGNAL_WIDTH_BEFORE_COLOUR;
    Some([floats[at], floats[at + 1], floats[at + 2], floats[at + 3]])
}

/// The row with every colour slot of `schema` moved by its palette-motion
/// floats, the slots' own values being the origin. `None` when the motion is
/// neutral (or the row is not this schema's width), so the caller draws the
/// row as it is without a copy.
///
/// Per stop: brightness spread around the slot's mean (`col_contrast`), then
/// hue rotated, saturation and brightness scaled, in HSV. The effects' own
/// `leak_color` is left alone: it is a light on the picture, not its palette.
pub fn apply_palette_motion(schema: &Schema, floats: &[f32]) -> Option<Vec<f32>> {
    if floats.len() != schema.width() {
        return None;
    }
    let [hue, sat, bri, contrast] = read_palette_motion(floats)?;
    if hue == 0.0 && sat == 1.0 && bri == 1.0 && contrast == 0.0 {
        return None;
    }
    let mut out = floats.to_vec();
    let leak_at = floats.len() - 12;
    let mut at = 0;
    for block in schema.blocks {
        if let Block::ColorSlot(..) = block {
            if at != leak_at {
                let mut hsv: Vec<[f32; 3]> = out[at..at + 12].chunks(3).map(|c| rgb_to_hsv([c[0], c[1], c[2]])).collect();
                let mean = hsv.iter().map(|c| c[2]).sum::<f32>() / 4.0;
                for c in &mut hsv {
                    c[0] = (c[0] + hue).rem_euclid(1.0);
                    c[1] = (c[1] * sat).clamp(0.0, 1.0);
                    c[2] = ((mean + (c[2] - mean) * (1.0 + contrast)) * bri).clamp(0.0, 1.0);
                }
                for (i, c) in hsv.into_iter().enumerate() {
                    out[at + i * 3..at + i * 3 + 3].copy_from_slice(&hsv_to_rgb(c));
                }
            }
        }
        at += block.width();
    }
    Some(out)
}

fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    [h, if max > 0.0 { d / max } else { 0.0 }, max]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

/// The signal a row is asking the room for, read back out of it.
///
/// Located from the end like [`read_post`], and *past* the effects, because
/// the signal block sits immediately before them. Panics if `floats` is
/// shorter than both blocks together, the same contract `read_post` has.
pub fn read_signal(floats: &[f32]) -> Signal {
    let at = floats.len() - EFFECTS_WIDTH - SIGNAL_WIDTH;
    Signal {
        level: floats[at],
        hue: floats[at + 1],
        sat: floats[at + 2],
        strobe: floats[at + 3],
        pan: floats[at + 4],
        tilt: floats[at + 5],
        a: floats[at + 6],
        b: floats[at + 7],
        cct: floats[at + 8],
        zoom: floats[at + 9],
        focus: floats[at + 10],
        iris: floats[at + 11],
        frost: floats[at + 12],
    }
}

/// What one row asks of the room, in the units the row states them in — no
/// protocol, no fixture, no channel numbers. Turning this into DDP bytes or a
/// DMX universe is the `out.*` node's job, and each of them does it
/// differently; this is the one shape they all start from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub level: f32,
    pub hue: f32,
    pub sat: f32,
    pub strobe: f32,
    pub pan: f32,
    pub tilt: f32,
    pub a: f32,
    pub b: f32,
    /// Colour temperature in kelvin, the float `sig_cct` reads. Only the
    /// profiles that actually have a `cct` channel look at it; a fixture
    /// without one ignores it (see `fixture::Fixture::light_state`).
    pub cct: f32,
    /// Beam shaping, in `0..1` each. Named rather than folded into `a`/`b`
    /// because a beam profile names them: a moving head's zoom, focus, iris
    /// and frost are not "two spare channels", they are four specific ones.
    pub zoom: f32,
    pub focus: f32,
    pub iris: f32,
    pub frost: f32,
}

impl Signal {
    /// `level`/`hue`/`sat` as linear RGB in `0..1`, which is what every one of
    /// the six transports wants first and none of them agrees on after.
    ///
    /// Plain HSV rather than a perceptual space on purpose: a lamp's driver
    /// is not colour-managed, the operator is looking at the room and not at
    /// a monitor, and a hue ramp that is "wrong" by a colourimeter reads as
    /// even to the eye at the saturations stage lights actually run at.
    pub fn rgb(&self) -> [f32; 3] {
        let h = (self.hue.rem_euclid(1.0)) * 6.0;
        let s = self.sat.clamp(0.0, 1.0);
        let v = self.level.clamp(0.0, 1.0);
        let c = v * s;
        let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
        let m = v - c;
        let [r, g, b] = match h as u32 {
            0 => [c, x, 0.0],
            1 => [x, c, 0.0],
            2 => [0.0, c, x],
            3 => [0.0, x, c],
            4 => [x, 0.0, c],
            _ => [c, 0.0, x],
        };
        [r + m, g + m, b + m]
    }
}

/// Same default `Post::leak_color` always started at.
pub const EFFECTS_LEAK_DEFAULT: [f32; 12] =
    [1.0, 0.9, 0.6, 1.0, 0.3, 0.9, 0.9, 0.2, 0.6, 0.6, 0.8, 1.0];
pub const EFFECTS_FLOATS_BLOCK: Block = Block::Floats(EFFECTS_FLOATS);
pub const EFFECTS_COLOR_BLOCK: Block = Block::ColorSlot("leak_color", &EFFECTS_LEAK_DEFAULT);
/// How many trailing floats of every row belong to the appended effects
/// blocks ([`EFFECTS_FLOATS`] and one 12-float colour slot).
pub const EFFECTS_WIDTH: usize = EFFECTS_FLOATS.len() + 12;
/// Current version of the effects schema. Incremented each time effects are
/// added or reorganized. New presets should be tagged with this version.
///
/// v2 appended the advanced effects ([`ADVANCED_EFFECTS`]) between the 14
/// common floats and `leak_color`, so a v1 row's `leak_color` sat at
/// `at + 14` and a v2 row's sits at `at + EFFECTS_FLOATS.len()`. Rows are
/// width-checked on load; `Post::migrate` records the bump.
pub const EFFECTS_VERSION: u32 = 2;

/// Explicit mapping of effect names to their digest column assignments.
/// This ensures that the same effect takes the same column across all 32 models.
/// The map is indexed by effect index (position in [`EFFECTS_FLOATS`]) and
/// returns the digest column (0–3) that drives this effect.
///
/// Rule: **New effect floats are appended after the existing ones, never in
/// the middle.** Reordering existing effects breaks saved presets and is not
/// allowed.
pub fn effect_column_map() -> &'static [usize] {
    // Derived from `EFFECT_SPECS` (LAY.4) for the common floats and from
    // `ADVANCED_COLUMNS` for the advanced ones: the column is the group's own,
    // so the map cannot disagree with the float list or with `metal-graph`'s
    // catalog about the order.
    &EFFECT_COLUMNS_FULL
}

/// Check if this block is the effects floats block.
/// Used to determine whether to use effect_column_map() or the regular
/// column_for() assignment during row driving.
///
/// **By name, not by pointer.** The pointer test
/// (`specs.as_ptr() == EFFECTS_FLOATS.as_ptr()`) only ever recognised the
/// compile-time static, so a schema rebuilt from names — every
/// [`Manifest::into_suite`] schema, which is what the CPU painter and the
/// author run on — fell back to the positional [`column_for`] round robin.
/// That made the *look* of the effects depend on how many floats sat before
/// them: appending a float to the signal block (BLE Phase B, 2026-09-27)
/// shifted every effect's digest column while the row itself was unchanged.
/// Matching on the names makes the effect columns a property of the effect,
/// which is what `effect_column_map` has always meant.
pub fn is_effects_block(specs: &[FloatSpec]) -> bool {
    specs.len() == EFFECTS_FLOATS.len()
        && specs.iter().zip(EFFECTS_FLOATS).all(|(a, b)| a.name == b.name)
}

/// `effect:amount:slot` triples, comma-separated: a resolved effect chain on
/// the wire (a remote layer's `fx.*` steps travel to the peer that draws it).
/// Amounts travel as their shortest round-trip decimal, so the peer reads the
/// same bits. Non-finite amounts are sent as `0`.
pub fn chain_to_query(chain: &[EffectStep]) -> String {
    chain
        .iter()
        .map(|s| {
            let amount = if s.amount.is_finite() { s.amount } else { 0.0 };
            format!("{}:{}:{}", s.effect, amount, s.slot)
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Reads [`chain_to_query`]'s form back. `None` for anything malformed.
pub fn chain_from_query(text: &str) -> Option<Vec<EffectStep>> {
    text.split(',')
        .filter(|s| !s.is_empty())
        .map(|step| {
            let mut f = step.split(':');
            let effect = f.next()?.parse().ok()?;
            let amount = f.next()?.parse().ok()?;
            let slot = f.next()?.parse().ok()?;
            f.next().is_none().then_some(EffectStep { effect, amount, slot })
        })
        .collect()
}

/// Reads the shared effects back out of a row's trailing floats — the mirror
/// of appending [`EFFECTS_FLOATS_BLOCK`] and [`EFFECTS_COLOR_BLOCK`] to a
/// schema. Panics if `floats` is shorter than [`EFFECTS_WIDTH`], the same
/// contract every other fixed-offset read against a row already has.
pub fn read_post(floats: &[f32]) -> Post {
    let at = floats.len() - EFFECTS_WIDTH;
    Post {
        version: EFFECTS_VERSION,
        shake: floats[at],
        light_leak: floats[at + 1],
        ghost: floats[at + 2],
        kaleido: floats[at + 3],
        crt: floats[at + 4],
        glitch: floats[at + 5],
        posterize: floats[at + 6],
        zoom: floats[at + 7],
        fire: floats[at + 8],
        lorenz: floats[at + 9],
        vignette: floats[at + 10],
        blur: floats[at + 11],
        sharpen: floats[at + 12],
        grain: floats[at + 13],
        advanced: floats[at + CHAIN_FLOATS..at + CHAIN_FLOATS + N_ADVANCED].to_vec(),
        leak_color: floats[at + EFFECTS_FLOATS.len()..at + EFFECTS_WIDTH].to_vec(),
        // A row is fourteen amounts and a colour; it has never carried an
        // order and does not start now. Rows come from models and from the
        // lane, neither of which knows about the graph — the chain is
        // attached later, by whoever resolved the graph, or left empty and
        // read back as the legacy order.
        chain: Vec::new(),
    }
}

/// The position of an effect in [`EFFECTS_FLOATS`], by the name `metal-graph`
/// calls it (`fx.shake` → `shake` → `0`).
///
/// `None` for a name no effect float declares, which includes the footage
/// effect Conjunto F promotes (`wake`): it is an `fx.*` node kind without a
/// column in the common effects row, so a caller that blindly unwrapped this
/// would start panicking the day it lands.
pub fn effect_index(name: &str) -> Option<usize> {
    EFFECTS_FLOATS.iter().position(|s| s.name == name)
}

/// The floats of one advanced effect group (`"grade"`, `"texture"`, …), by the
/// id `metal-graph` names its `fx.*` kind with. `None` for a common effect, a
/// footage kind or anything else — the groups only here.
pub fn advanced_effect_floats(id: &str) -> Option<&'static [FloatSpec]> {
    ADVANCED_EFFECTS.iter().find(|(gid, _)| *gid == id).map(|(_, floats)| *floats)
}

/// The range of [`EFFECTS_FLOATS`] one advanced group occupies,
/// `CHAIN_FLOATS + sum of preceding group lengths .. + this length`. `None`
/// for an id that is not an advanced effect.
pub fn advanced_effect_span(id: &str) -> Option<core::ops::Range<usize>> {
    let mut start = CHAIN_FLOATS;
    for (gid, floats) in ADVANCED_EFFECTS {
        if *gid == id {
            return Some(start..start + floats.len());
        }
        start += floats.len();
    }
    None
}

/// [`CHAIN_KIND_ESTELA`] by the name `metal-graph` calls it (`fx.estela` ->
/// `wake` -> [`CHAIN_KIND_ESTELA`]) — the mirror of [`effect_index`] for
/// exactly the name that function refuses.
///
/// `None` for any other name, including every name [`EFFECTS_FLOATS`]
/// declares: the two lookups are disjoint on purpose, so a caller resolving
/// one `fx.*` node's name never has to ask both and reconcile which one
/// answered.
pub fn footage_chain_kind(name: &str) -> Option<u32> {
    match name {
        "wake" => Some(CHAIN_KIND_ESTELA),
        _ => None,
    }
}

impl Post {
    /// Migrate a Post from an older version to the current version.
    ///
    /// This is called when loading a preset that was saved with an older
    /// version of the effects schema. Defaults are used for any new effects
    /// that did not exist in the loaded version.
    pub fn migrate(&mut self, from_version: u32) {
        if from_version == EFFECTS_VERSION {
            return; // Already at current version
        }
        // Version 0 → 1: All effects present, just set version.
        // Version 1 → 2: the advanced effects were inserted between the 14
        // common floats and `leak_color`. The 14 named fields and `chain` are
        // untouched; `advanced` is `#[serde(default)]`, so a v1 preset loads
        // with an empty one and nothing it named moved.
        self.version = EFFECTS_VERSION;
    }

    /// The time constant the `ghost` slider means, in seconds.
    ///
    /// **Per second, not per frame.** Both apps render on demand — the browser
    /// asks for the next frame only once the last one arrived — so a heavy
    /// model runs at a lower rate by design. A trail that faded by a fixed
    /// fraction per frame would be long on a busy frame and short on an idle
    /// one, and different in the grid than in solo at the same setting.
    pub fn ghost_tau(&self) -> f32 {
        0.03 + self.ghost.clamp(0.0, 1.0) * 1.1
    }

    /// How much of the effect at `i` ([`EFFECTS_FLOATS`] position) this post
    /// asks for. `0.0` for an index past the end, because a caller iterating
    /// a chain that outlived a schema change should render nothing, not
    /// crash mid-frame.
    pub fn amount(&self, i: usize) -> f32 {
        match i {
            0 => self.shake,
            1 => self.light_leak,
            2 => self.ghost,
            3 => self.kaleido,
            4 => self.crt,
            5 => self.glitch,
            6 => self.posterize,
            7 => self.zoom,
            8 => self.fire,
            9 => self.lorenz,
            10 => self.vignette,
            11 => self.blur,
            12 => self.sharpen,
            13 => self.grain,
            i if i >= CHAIN_FLOATS => {
                self.advanced.get(i - CHAIN_FLOATS).copied().unwrap_or(0.0)
            }
            _ => 0.0,
        }
    }

    /// The mirror of [`Post::amount`]. An index past the end is dropped
    /// rather than panicking, for the same reason.
    pub fn set_amount(&mut self, i: usize, v: f32) {
        match i {
            0 => self.shake = v,
            1 => self.light_leak = v,
            2 => self.ghost = v,
            3 => self.kaleido = v,
            4 => self.crt = v,
            5 => self.glitch = v,
            6 => self.posterize = v,
            7 => self.zoom = v,
            8 => self.fire = v,
            9 => self.lorenz = v,
            10 => self.vignette = v,
            11 => self.blur = v,
            12 => self.sharpen = v,
            13 => self.grain = v,
            i if i >= CHAIN_FLOATS => {
                // A preset saved before a group was appended carries a
                // shorter `advanced`; fill the gap with declared defaults so
                // the write lands instead of vanishing. Stop at `N_ADVANCED`:
                // the material group trails the advanced floats in
                // `EFFECTS_FLOATS` but has no place in `Post::advanced`.
                if i < EFFECTS_FLOATS.len() && self.advanced.len() < N_ADVANCED {
                    let have = self.advanced.len();
                    self.advanced
                        .extend(EFFECTS_FLOATS[CHAIN_FLOATS + have..CHAIN_FLOATS + N_ADVANCED].iter().map(|s| s.default));
                }
                if let Some(slot) = self.advanced.get_mut(i - CHAIN_FLOATS) {
                    *slot = v;
                }
            }
            _ => {}
        }
    }

    /// The sequence of steps that will actually be applied.
    ///
    /// With an explicit [`Post::chain`], that chain — clamped to
    /// [`MAX_CHAIN`], with an effect index that is neither a valid
    /// [`EFFECTS_FLOATS`] position nor a footage kind dropped. What happens
    /// next splits in two:
    ///
    /// - A **row-float** step (`effect < EFFECTS_FLOATS.len()`) has its
    ///   `amount` re-read from the flat fields, so the two can never
    ///   disagree about how much (the chain says *when*, the fields say
    ///   *how much* — see the field comment on [`EffectStep::amount`]).
    /// - A **footage-kind** step ([`is_footage_kind`] — Estela) is
    ///   passed through untouched, `amount` and `slot` both: there is no
    ///   flat field on `Post` for either to re-read, on purpose (see
    ///   [`effect_index`]'s own doc comment), so the step's own stored
    ///   values *are* the answer.
    ///
    /// With an empty [`Post::chain`], declaration order over the row floats
    /// only, skipping effects that are exactly zero — a footage kind never
    /// appears here, because nothing about a flat row can name one (see
    /// [`read_post`]'s own doc comment on why `chain` always comes back
    /// empty from a bare row). Skipping the zeros matters: `mix(c, effect(..),
    /// 0.0)` returns `c` unchanged, so a zero step is a no-op the shader can
    /// be spared, and spending chain slots on no-ops is how a legacy post
    /// would blow past [`MAX_CHAIN`] for no picture at all.
    ///
    /// `zoom` is the one signed row-float effect, so its "off" is zero, not
    /// its minimum — `abs` is what decides, matching `fs_main`'s own
    /// `clamp(abs(zoom_amt), 0.0, 1.0)`.
    pub fn resolved_chain(&self) -> Vec<EffectStep> {
        if self.chain.is_empty() {
            // The common effects in declaration order, then the advanced
            // groups that are chain steps (their amount is their first float).
            let late = ADVANCED_CHAIN_EFFECTS.iter().filter_map(|id| advanced_chain_kind(id)).map(|k| k as usize);
            return (0..CHAIN_FLOATS)
                .chain(late)
                .filter(|&i| self.amount(i).abs() > 0.0)
                .map(|i| EffectStep { effect: i as u32, amount: self.amount(i), slot: 0 })
                .take(MAX_CHAIN)
                .collect();
        }
        self.chain
            .iter()
            .filter(|s| is_chain_kind(s.effect))
            .map(|s| {
                if is_footage_kind(s.effect) {
                    *s
                } else {
                    EffectStep { effect: s.effect, amount: self.amount(s.effect as usize), slot: 0 }
                }
            })
            .take(MAX_CHAIN)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "rate": 8.0,
      "models": [{
        "id": "demo", "label": "Demo", "technique": "",
        "blocks": [
          { "type": "floats", "fields": [
            { "name": "radius", "min": 0.0, "max": 2.0, "default": 0.5, "drive": 1.0 },
            { "name": "feed",   "min": 0.0, "max": 1.0, "default": 0.037, "drive": 0.1 }
          ]},
          { "type": "color_slot", "name": "tint", "default": [0,0,0, 1,1,1, 1,1,1, 1,1,1] },
          { "type": "camera", "default": [0,1,2, 0,0,0, 50] }
        ]
      }]
    }"#;

    #[test]
    fn a_manifest_round_trips_into_the_renderers_own_types() {
        let suite = Manifest::parse(SAMPLE).expect("parses").into_suite();
        assert_eq!(suite.len(), 1);
        let e = &suite[0];
        assert_eq!(e.id, "demo");
        assert_eq!(e.name, "Demo");
        // 2 floats + 12 colour + 7 camera.
        assert_eq!(e.schema.width(), 21);
        assert_eq!(e.schema.defaults()[0], 0.5);
        // The camera's own view survived, rather than falling back.
        assert_eq!(&e.schema.defaults()[14..], &[0.0, 1.0, 2.0, 0.0, 0.0, 0.0, 50.0]);
    }

    /// The one field that is easy to lose on the way through a file, and the
    /// only one whose loss is invisible: a structural float read back as
    /// freely drivable renders in range, logs nothing, and stops being the
    /// model. See `FloatSpec::drive` and `metal-dash`'s SWING note.
    #[test]
    fn a_structural_float_keeps_its_drive_weight_through_the_file() {
        let suite = Manifest::parse(SAMPLE).expect("parses").into_suite();
        let Block::Floats(specs) = suite[0].schema.blocks[0] else { panic!("not floats") };
        assert_eq!(specs[0].drive, 1.0);
        assert_eq!(specs[1].drive, 0.1, "the structural float came back freely drivable");
    }

    /// The one field V4 adds to a float: which face control it *is*. A rig
    /// float is never driven (the solver is its only writer), its range and
    /// rest come from the control, and its name is the control's own — so the
    /// solver's offset table and the model's float names cannot drift.
    #[test]
    fn a_rig_float_reads_its_name_range_and_rest_from_the_control() {
        for &c in FaceControl::ALL {
            let f = spec_rig(c);
            let s = c.spec();
            assert_eq!(f.rig, Some(c));
            assert_eq!(f.name, c.name());
            assert_eq!(f.min, s.lo);
            assert_eq!(f.max, s.hi);
            assert_eq!(f.default, s.rest);
            assert_eq!(f.drive, 0.0, "the solver, not the digest, writes a rig float");
        }
        // An ordinary float is not a rig float.
        assert_eq!(spec("x", 0.0, 1.0, 0.5).rig, None);
    }

    #[test]
    fn a_manifest_written_before_drive_existed_still_loads() {
        let old = r#"{"rate":8.0,"models":[{"id":"a","label":"A","blocks":[
          {"type":"floats","fields":[{"name":"x","min":0,"max":1,"default":0.5}]}]}]}"#;
        let suite = Manifest::parse(old).expect("legacy manifest still parses").into_suite();
        let Block::Floats(specs) = suite[0].schema.blocks[0] else { panic!("not floats") };
        assert_eq!(specs[0].drive, 1.0, "a missing drive key must not read as pinned");
    }

    #[test]
    fn a_short_colour_slot_keeps_white_for_the_stops_it_does_not_name() {
        // A file is a file. Refusing to start over eleven numbers would be a
        // suite that will not open at a venue because of a stray comma.
        let short = r#"{"rate":8.0,"models":[{"id":"a","label":"A","blocks":[
          {"type":"color_slot","name":"t","default":[0.0,0.0,0.0]}]}]}"#;
        let suite = Manifest::parse(short).expect("parses").into_suite();
        let row = suite[0].schema.defaults();
        assert_eq!(&row[0..3], &[0.0, 0.0, 0.0]);
        assert_eq!(&row[3..12], &[1.0; 9]);
    }

    #[test]
    fn a_post_with_no_chain_still_reads_back_as_the_order_the_shader_hardcoded() {
        // The compatibility promise: every preset on disk predates `chain`,
        // deserializes with it empty, and must keep composing in declaration
        // order — with the zeros skipped, because a zero step is a no-op.
        let mut post = Post::default();
        post.zoom = 0.4;
        post.shake = 0.2;
        let chain = post.resolved_chain();
        let order: Vec<u32> = chain.iter().map(|s| s.effect).collect();
        assert_eq!(order, vec![0, 7], "shake (0) before zoom (7), declaration order");
        assert_eq!(chain[0].amount, 0.2);
        assert_eq!(chain[1].amount, 0.4);
    }

    #[test]
    fn an_explicit_chain_reverses_the_order_without_touching_the_amounts() {
        // The whole point of the field: same two amounts, opposite order,
        // which is a different image once the shader threads them.
        let mut post = Post::default();
        post.zoom = 0.4;
        post.shake = 0.2;
        post.chain = vec![
            EffectStep { effect: 7, amount: 0.0, slot: 0 },
            EffectStep { effect: 0, amount: 0.0, slot: 0 },
        ];
        let chain = post.resolved_chain();
        assert_eq!(chain.iter().map(|s| s.effect).collect::<Vec<_>>(), vec![7, 0]);
        // `amount` came from the flat fields, not from the zeros written in
        // the steps above — the chain says when, the fields say how much.
        assert_eq!(chain[0].amount, 0.4, "zoom's amount is still zoom's field");
        assert_eq!(chain[1].amount, 0.2, "shake's amount is still shake's field");
    }

    #[test]
    fn a_negative_zoom_is_not_mistaken_for_an_effect_that_is_off() {
        // zoom is the one signed float: -0.5 pulls out, and pulling out is
        // as much an effect as punching in. A naive `> 0.0` skip would drop
        // it from the legacy chain and silently stop rendering it.
        let mut post = Post::default();
        post.zoom = -0.5;
        assert_eq!(
            post.resolved_chain().iter().map(|s| s.effect).collect::<Vec<_>>(),
            vec![7]
        );
    }

    #[test]
    fn a_chain_naming_an_effect_that_no_longer_exists_is_dropped_not_panicked_on() {
        // A graph saved against a wider schema, loaded by a narrower build.
        // Rendering the steps that still resolve beats crashing mid-frame.
        let mut post = Post::default();
        post.shake = 0.3;
        post.chain = vec![
            EffectStep { effect: EFFECTS_FLOATS.len() as u32 + 50, amount: 1.0, slot: 0 },
            EffectStep { effect: 0, amount: 1.0, slot: 0 },
        ];
        let chain = post.resolved_chain();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].effect, 0);
    }

    /// A footage kind is the one case `resolved_chain` must *not* drop the
    /// way it drops 99 above, even though it has no column in
    /// `EFFECTS_FLOATS` either — the two lookups (`< EFFECTS_FLOATS.len()`,
    /// `is_footage_kind`) are supposed to disagree about it.
    #[test]
    fn a_chain_step_naming_estela_keeps_its_own_amount_instead_of_reading_a_row_field() {
        // Estela has no `Post` field to read an amount from — the whole
        // reason `EffectStep` carries its own now. Contrast with
        // `an_explicit_chain_reverses_the_order_without_touching_the_amounts`
        // just above, where a row-float step's stored `amount` (there, 0.0)
        // is discarded in favour of `post.zoom`/`post.shake`: a footage-kind
        // step must not go anywhere near that substitution.
        let mut post = Post::default();
        post.chain = vec![EffectStep { effect: CHAIN_KIND_ESTELA, amount: 0.73, slot: 0 }];
        assert_eq!(
            post.resolved_chain(),
            vec![EffectStep { effect: CHAIN_KIND_ESTELA, amount: 0.73, slot: 0 }]
        );
    }

    #[test]
    fn footage_and_row_float_steps_compose_in_the_one_chain_together() {
        // The realistic shape: an operator chains a common effect and one of
        // the promotions on the same generator. Both must resolve, in
        // the order they were given, each reading its amount from wherever
        // that kind's amount actually lives.
        let mut post = Post::default();
        post.shake = 0.2;
        post.chain = vec![
            EffectStep { effect: CHAIN_KIND_ESTELA, amount: 0.6, slot: 0 },
            EffectStep { effect: 0, amount: 0.0, slot: 0 }, // shake — re-read below
        ];
        assert_eq!(
            post.resolved_chain(),
            vec![
                EffectStep { effect: CHAIN_KIND_ESTELA, amount: 0.6, slot: 0 },
                EffectStep { effect: 0, amount: 0.2, slot: 0 },
            ]
        );
    }

    #[test]
    fn a_chain_step_naming_a_kind_beyond_the_footage_promotion_is_dropped_not_panicked_on() {
        // `is_footage_kind` must not quietly grow to swallow everything past
        // `EFFECTS_FLOATS.len()` — the first number that names neither a row
        // float (advanced floats included) nor a footage kind has to keep
        // being dropped, same as the pre-existing 99 case above. (This used to
        // be `CHAIN_KIND_ESTELA + 1`; the advanced floats occupy that number
        // now, so the boundary moved to the end of `EFFECTS_FLOATS`.)
        let mut post = Post::default();
        post.shake = 0.3;
        post.chain = vec![
            EffectStep { effect: EFFECTS_FLOATS.len() as u32, amount: 1.0, slot: 0 },
            EffectStep { effect: 0, amount: 1.0, slot: 0 },
        ];
        let chain = post.resolved_chain();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].effect, 0);
    }

    #[test]
    fn a_chain_longer_than_the_uniform_is_truncated_to_what_the_gpu_can_hold() {
        let mut post = Post::default();
        post.shake = 0.1;
        post.chain = vec![EffectStep { effect: 0, amount: 0.1, slot: 0 }; MAX_CHAIN + 8];
        assert_eq!(post.resolved_chain().len(), MAX_CHAIN);
    }

    // -- the signal block ---------------------------------------------------

    /// A row shaped the way a model's schema builds one: some floats of its
    /// own, then the signal block, then the effects block and its colour slot.
    fn row_with_blocks(own: usize) -> Vec<f32> {
        let mut row = vec![7.0f32; own];
        row.extend((0..SIGNAL_WIDTH).map(|i| 100.0 + i as f32));
        row.extend((0..EFFECTS_FLOATS.len()).map(|i| 200.0 + i as f32));
        row.extend([0.5f32; 12]);
        row
    }

    #[test]
    fn the_signal_block_sits_before_the_effects_and_moves_neither_neighbour() {
        // The one thing that makes appending a block to thirty-four schemas
        // safe: a model's own floats still start at 0, and `read_post` still
        // finds the effects `EFFECTS_WIDTH` from the end. Put the signal
        // block after the effects instead and both of these go wrong while
        // staying in range — the failure mode that has no error message.
        let row = row_with_blocks(6);
        assert_eq!(&row[..6], &[7.0; 6], "a model's own floats did not move");

        let post = read_post(&row);
        assert_eq!(post.shake, 200.0, "the effects are still the trailing block");
        assert_eq!(post.grain, 200.0 + 13.0, "the last of the 14 common rows");
        assert_eq!(post.advanced.len(), N_ADVANCED, "the advanced floats read back too");
        assert_eq!(post.advanced[0], 200.0 + CHAIN_FLOATS as f32);
        assert_eq!(post.leak_color, vec![0.5; 12], "the colour slot trails them all");

        let sig = read_signal(&row);
        assert_eq!(sig.level, 100.0);
        assert_eq!(sig.b, 100.0 + 7.0, "the original eight read in declared order");
        // The BLE Phase B floats sit after `sig_b` and before the effects —
        // the declaration order `read_signal` reads them in.
        assert_eq!(sig.cct, 100.0 + 8.0);
        assert_eq!(sig.zoom, 100.0 + 9.0);
        assert_eq!(sig.frost, 100.0 + (SIGNAL_WIDTH_BEFORE_COLOUR - 1) as f32, "last BLE float");
        // The palette motion closes the block.
        assert_eq!(read_palette_motion(&row).unwrap()[3], 100.0 + (SIGNAL_WIDTH - 1) as f32, "last signal float");
    }

    /// A row saved before the BLE Phase B append is five floats short, and
    /// because the signal block sits before the effects, it has to be padded
    /// in the middle rather than at the end. This is the whole of the
    /// row-width migration, tested against the shape such a row actually had.
    #[test]
    fn an_old_row_is_padded_where_the_effects_begin_and_nothing_else_moves() {
        // The pre-Phase-B shape for a model with 6 own floats: own, the old
        // eight-float signal block, then the effects and their colour slot.
        let mut old = vec![7.0f32; 6];
        old.extend((0..SIGNAL_WIDTH_BEFORE_BLE).map(|i| 100.0 + i as f32));
        old.extend((0..EFFECTS_FLOATS.len()).map(|i| 200.0 + i as f32));
        old.extend([0.5f32; 12]);

        // What today's schema would say that row is wide.
        let schema_width = 6 + SIGNAL_WIDTH + EFFECTS_FLOATS.len() + 12;
        assert_eq!(old.len(), schema_width - SIGNAL_FLOATS_ADDED);

        assert!(migrate_signal_row(&mut old, schema_width));
        assert_eq!(old.len(), schema_width);

        // The model's own floats did not move.
        assert_eq!(&old[..6], &[7.0; 6]);
        // Reading the current way finds the old signal floats in their old
        // places, then the new ones at their defaults, in declared order.
        let sig = read_signal(&old);
        assert_eq!(sig.level, 100.0);
        assert_eq!(sig.b, 100.0 + (SIGNAL_WIDTH_BEFORE_BLE - 1) as f32);
        assert_eq!(sig.cct, 5600.0, "the appended floats land at their defaults");
        assert_eq!(sig.frost, 0.0);
        // And the effects are still the trailing block, intact.
        let post = read_post(&old);
        assert_eq!(post.shake, 200.0, "the effects did not move");
        assert_eq!(post.leak_color, vec![0.5; 12]);
    }

    #[test]
    fn a_row_from_before_the_palette_motion_gets_it_at_neutral() {
        let mut old = vec![7.0f32; 6];
        old.extend((0..SIGNAL_WIDTH_BEFORE_COLOUR).map(|i| 100.0 + i as f32));
        old.extend((0..EFFECTS_FLOATS.len()).map(|i| 200.0 + i as f32));
        old.extend([0.5f32; 12]);
        let schema_width = 6 + SIGNAL_WIDTH + EFFECTS_FLOATS.len() + 12;
        assert!(migrate_signal_row(&mut old, schema_width));
        assert_eq!(old.len(), schema_width);
        assert_eq!(read_signal(&old).frost, 100.0 + 12.0, "the BLE floats kept their values");
        assert_eq!(read_palette_motion(&old), Some([0.0, 1.0, 1.0, 0.0]));
        assert_eq!(read_post(&old).shake, 200.0);
    }

    /// The effects block as it was before the late groups (`fm`, …) were
    /// appended: the common floats and the first seven advanced groups, the
    /// same floats without the late groups' — or the material group's — floats,
    /// in the same order.
    fn old_effects_floats() -> Vec<f32> {
        let late: usize = LATE_EFFECT_GROUPS.iter().map(|id| advanced_effect_floats(id).unwrap().len()).sum();
        (0..CHAIN_FLOATS + N_ADVANCED - late).map(|i| 200.0 + i as f32).collect()
    }

    /// A row saved before `fm` was appended has the late groups' floats
    /// missing just before `leak_color`; they come back at their defaults and
    /// every effect the row already said keeps its meaning.
    #[test]
    fn a_row_from_before_the_late_effect_groups_gets_them_at_their_defaults() {
        let mut old = vec![7.0f32; 6];
        old.extend((0..SIGNAL_WIDTH).map(|i| 100.0 + i as f32));
        old.extend(old_effects_floats());
        old.extend([0.5f32; 12]);
        let schema_width = 6 + SIGNAL_WIDTH + EFFECTS_FLOATS.len() + 12;
        assert!(old.len() < schema_width);

        assert!(migrate_row(&mut old, schema_width));
        assert_eq!(old.len(), schema_width);
        assert_eq!(&old[..6], &[7.0; 6], "the model's own floats did not move");
        assert_eq!(read_signal(&old).level, 100.0, "the signal block did not move");
        let post = read_post(&old);
        assert_eq!(post.shake, 200.0, "the common effects did not move");
        assert_eq!(post.grain, 200.0 + 13.0);
        assert_eq!(post.leak_color, vec![0.5; 12], "leak_color is still the trailing slot");
        // The newest pre-fm advanced float kept its value ...
        let geo = advanced_effect_span("geometry").unwrap();
        assert_eq!(post.advanced[geo.end - 1 - CHAIN_FLOATS], 200.0 + (geo.end - 1) as f32);
        // ... and fm is neutral: off, with its declared defaults.
        let fm = advanced_effect_span("fm").unwrap();
        for (i, spec) in advanced_effect_floats("fm").unwrap().iter().enumerate() {
            assert_eq!(post.advanced[fm.start - CHAIN_FLOATS + i], spec.default, "{}", spec.name);
        }
        assert_eq!(post.amount(fm.start), 0.0, "fm_cant defaults to off");
        assert!(post.resolved_chain().iter().all(|s| s.effect != fm.start as u32), "a migrated row has no fm step");
    }

    /// The same row also lacks the BLE signal floats and the palette motion:
    /// both gaps are closed in one `migrate_row`.
    #[test]
    fn a_doubly_old_row_migrates_both_the_effects_and_the_signal_gap() {
        let mut old = vec![7.0f32; 6];
        old.extend((0..SIGNAL_WIDTH_BEFORE_BLE).map(|i| 100.0 + i as f32));
        old.extend(old_effects_floats());
        old.extend([0.5f32; 12]);
        let schema_width = 6 + SIGNAL_WIDTH + EFFECTS_FLOATS.len() + 12;
        assert!(migrate_row(&mut old, schema_width));
        assert_eq!(old.len(), schema_width);
        assert_eq!(read_signal(&old).b, 100.0 + (SIGNAL_WIDTH_BEFORE_BLE - 1) as f32);
        assert_eq!(read_post(&old).shake, 200.0);
        assert_eq!(read_post(&old).leak_color, vec![0.5; 12]);
    }

    /// The material group sits at the very end of the effects tail, behind the
    /// advanced floats and in front of `leak_color`, and every one of its
    /// floats round-trips through the row and back through [`read_material`].
    #[test]
    fn the_material_floats_round_trip_at_the_end_of_the_effects_tail() {
        let mut row = row_with_blocks(6);
        let first = row.len() - EFFECTS_WIDTH + CHAIN_FLOATS + N_ADVANCED;
        let mid = |s: &FloatSpec| s.min + (s.max - s.min) * 0.5;
        for (i, spec) in MATERIAL_FLOATS.iter().enumerate() {
            row[first + i] = mid(spec);
        }
        let material = read_material(&row).expect("the row holds the effects block");
        assert_eq!(material.len(), N_MATERIAL, "read_material returns the declared group");
        for (i, spec) in MATERIAL_FLOATS.iter().enumerate() {
            assert_eq!(material[i], mid(spec), "{} lost its value", spec.name);
        }
        // The neighbours on either side are untouched by writing material.
        assert_eq!(read_post(&row).leak_color, vec![0.5; 12], "leak_color still trails the block");
        assert_eq!(
            row[first - 1],
            200.0 + (CHAIN_FLOATS + N_ADVANCED - 1) as f32,
            "the last advanced float is still its own"
        );
    }

    /// The material glitch rides **the beat**, and the beat is one named
    /// column: the onset the strobe's flash is locked to. If the material glitch
    /// were driven by any other column a strobe hit and a material glitch would
    /// drift apart, which is the one thing M8 exists to prevent.
    #[test]
    fn the_material_glitch_rides_the_strobes_beat_column() {
        assert_eq!(MUSIC_COLUMNS[ONSET_COLUMN], "onset", "the beat column is onset");
        let glitch = MATERIAL_FLOATS
            .iter()
            .position(|s| s.name == "mat_glitch")
            .expect("mat_glitch is a material float");
        assert_eq!(MATERIAL_COLUMNS[glitch], ONSET_COLUMN, "the glitch shares the beat column");
        // And it is the same column the hit effects — `glitch` among them —
        // already take, read off the shared effect column map.
        let grey = effect_index("glitch").expect("fx.glitch is an effect");
        assert_eq!(effect_column_map()[grey], ONSET_COLUMN, "fx.glitch is on the beat too");
        assert_eq!(effect_column_map()[glitch + CHAIN_FLOATS + N_ADVANCED], ONSET_COLUMN, "the row's material glitch is on the beat");
    }

    /// A row saved before the material group existed loads all-zero there, and
    /// every float it already carried keeps its exact place and value — the
    /// frame a migrated row renders is the frame the old row rendered, because
    /// the new group is off (`mat_mix` 0) and nothing before it moved.
    #[test]
    fn an_old_row_without_material_migrates_to_all_zero_and_keeps_every_float() {
        let mut old = vec![7.0f32; 6];
        old.extend((0..SIGNAL_WIDTH).map(|i| 100.0 + i as f32));
        old.extend((0..CHAIN_FLOATS + N_ADVANCED).map(|i| 200.0 + i as f32));
        old.extend([0.5f32; 12]);
        let schema_width = 6 + SIGNAL_WIDTH + EFFECTS_FLOATS.len() + 12;
        assert_eq!(old.len() + N_MATERIAL, schema_width, "the row is exactly the material group short");

        let before = old.clone();
        assert!(migrate_row(&mut old, schema_width));
        assert_eq!(old.len(), schema_width);
        // Byte for byte: everything in front of `leak_color` is untouched ...
        assert_eq!(&old[..before.len() - 12], &before[..before.len() - 12], "nothing before leak_color moved");
        // ... the group arrives all-zero just before it ...
        assert!(old[before.len() - 12..before.len()].iter().all(|v| *v == 0.0), "the migrated material is all-zero");
        assert_eq!(read_material(&old).unwrap(), &[0.0f32; N_MATERIAL], "and reads back as the group");
        assert_eq!(read_material(&old).unwrap(), MATERIAL_FLOATS.iter().map(|s| s.default).collect::<Vec<_>>().as_slice());
        // ... `leak_color` keeps its own value, moved by nothing but the insert ...
        assert_eq!(&old[before.len()..], &before[before.len() - 12..], "leak_color kept its value");
        // ... and none of it leaked into the `Post` mirror.
        assert_eq!(read_post(&old).advanced.len(), N_ADVANCED, "material did not leak into Post.advanced");
    }

    /// The bookkeeping lists of a new advanced effect agree with each other:
    /// ids match the groups one for one, the late groups are the tail of the
    /// list in the same order, every chain effect is a group, and every float
    /// of a group is named `fx_<id>_*`.
    #[test]
    fn the_advanced_effect_lists_agree() {
        let ids: Vec<&str> = ADVANCED_EFFECTS.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ADVANCED_EFFECT_IDS);
        assert_eq!(&ids[ids.len() - LATE_EFFECT_GROUPS.len()..], LATE_EFFECT_GROUPS);
        // The late row groups are the advanced late groups, then material —
        // the physical tail order `effects_floats_flat` lays down.
        assert_eq!(&LATE_ROW_GROUPS[..LATE_EFFECT_GROUPS.len()], LATE_EFFECT_GROUPS);
        assert_eq!(LATE_ROW_GROUPS[LATE_EFFECT_GROUPS.len()], MATERIAL_GROUP);
        for id in ADVANCED_CHAIN_EFFECTS {
            assert!(LATE_EFFECT_GROUPS.contains(id), "{id} is a chain effect but not a late group");
        }
        for id in LATE_EFFECT_GROUPS {
            for f in advanced_effect_floats(id).unwrap() {
                assert!(f.name.starts_with(&format!("fx_{id}_")), "{} is not named fx_{id}_*", f.name);
            }
        }
    }

    #[test]
    fn a_current_row_is_not_touched_by_the_effects_migration() {
        let mut row = row_with_blocks(6);
        let before = row.clone();
        let w = row.len();
        assert!(!migrate_effects_row(&mut row, w));
        assert_eq!(row, before);
    }

    /// A v2 preset written before `fm` carries a shorter `advanced`; it still
    /// loads, fm reads as off, and writing an fm float lands (the gap is
    /// filled with declared defaults first).
    #[test]
    fn an_old_preset_without_fm_loads_off_and_accepts_an_fm_write() {
        let old_len = N_ADVANCED - advanced_effect_floats("fm").unwrap().len();
        let mut post = Post::default();
        post.advanced.truncate(old_len);
        let kind = advanced_chain_kind("fm").unwrap() as usize;
        assert_eq!(post.amount(kind), 0.0);
        post.set_amount(kind + 1, 9.0); // portadora_freq
        assert_eq!(post.advanced.len(), N_ADVANCED);
        assert_eq!(post.amount(kind + 1), 9.0);
        assert_eq!(post.amount(kind + 2), advanced_effect_floats("fm").unwrap()[2].default);
    }

    /// A row saved after `fm` but before `ascii` gets ascii (and every group
    /// appended after it, the material group included) back, at its defaults,
    /// and keeps fm's values.
    #[test]
    fn a_row_with_fm_but_without_ascii_gets_just_ascii_at_its_defaults() {
        let from = ADVANCED_EFFECTS.iter().position(|(id, _)| *id == "ascii").unwrap();
        let n_missing: usize =
            ADVANCED_EFFECTS[from..].iter().map(|(_, f)| f.len()).sum::<usize>() + N_MATERIAL;
        let mut old = vec![7.0f32; 6];
        old.extend((0..SIGNAL_WIDTH).map(|i| 100.0 + i as f32));
        old.extend((0..EFFECTS_FLOATS.len() - n_missing).map(|i| 200.0 + i as f32));
        old.extend([0.5f32; 12]);
        let schema_width = 6 + SIGNAL_WIDTH + EFFECTS_FLOATS.len() + 12;
        assert!(migrate_row(&mut old, schema_width));
        assert_eq!(old.len(), schema_width);
        let post = read_post(&old);
        let fm = advanced_effect_span("fm").unwrap();
        assert_eq!(post.amount(fm.start), 200.0 + fm.start as f32, "fm kept its value");
        for (id, floats) in &ADVANCED_EFFECTS[from..] {
            let a = advanced_effect_span(id).unwrap();
            for (i, spec) in floats.iter().enumerate() {
                assert_eq!(post.advanced[a.start - CHAIN_FLOATS + i], spec.default, "{}", spec.name);
            }
        }
        for (i, spec) in MATERIAL_FLOATS.iter().enumerate() {
            assert_eq!(read_material(&old).unwrap()[i], spec.default, "{}", spec.name);
        }
        assert_eq!(post.leak_color, vec![0.5; 12]);
    }

    #[test]
    fn ascii_is_a_chain_step_after_fm_and_off_by_default() {
        let kind = advanced_chain_kind("ascii").unwrap();
        assert_eq!(kind as usize, EFFECTS_FLOATS.iter().position(|s| s.name == "fx_ascii_cant").unwrap());
        assert!(is_chain_kind(kind));
        assert!(!is_chain_kind(kind + 1));
        assert_ne!(kind, advanced_chain_kind("fm").unwrap());
        let mut post = Post::default();
        assert!(post.resolved_chain().is_empty());
        post.set_amount(kind as usize, 0.5);
        assert_eq!(post.resolved_chain(), vec![EffectStep { effect: kind, amount: 0.5, slot: 0 }]);
    }

    #[test]
    fn fm_is_a_chain_step_only_when_it_is_on() {
        let kind = advanced_chain_kind("fm").unwrap();
        assert_eq!(kind as usize, EFFECTS_FLOATS.iter().position(|s| s.name == "fx_fm_cant").unwrap());
        assert!(is_chain_kind(kind));
        assert!(!is_chain_kind(kind + 1), "the other fm floats are not steps");
        let mut post = Post::default();
        assert!(post.resolved_chain().is_empty());
        post.set_amount(kind as usize, 0.6);
        let chain = post.resolved_chain();
        assert_eq!(chain, vec![EffectStep { effect: kind, amount: 0.6, slot: 0 }]);
        // Explicitly chained, the amount is re-read from the flat float.
        post.chain = vec![EffectStep { effect: kind, amount: 0.0, slot: 0 }];
        assert_eq!(post.resolved_chain()[0].amount, 0.6);
    }

    static MOTION_BLOCKS: [Block; 4] = [
        Block::ColorSlot("palette", &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.5, 0.5, 0.5]),
        SIGNAL_FLOATS_BLOCK,
        EFFECTS_FLOATS_BLOCK,
        EFFECTS_COLOR_BLOCK,
    ];
    static MOTION_SCHEMA: Schema = Schema { blocks: &MOTION_BLOCKS };

    #[test]
    fn palette_motion_moves_the_slots_around_their_origin() {
        let mut row = MOTION_SCHEMA.defaults();
        assert_eq!(apply_palette_motion(&MOTION_SCHEMA, &row), None, "neutral motion: no copy");
        let at = 12 + SIGNAL_WIDTH_BEFORE_COLOUR;
        row[at] = 1.0 / 3.0 - 0.25; // hue, written out of range on purpose
        row[at] = 0.25;
        let moved = apply_palette_motion(&MOTION_SCHEMA, &row).unwrap();
        // Red rotated a quarter turn is chartreuse (h = 0.25).
        assert!((moved[0] - 0.5).abs() < 1e-5 && (moved[1] - 1.0).abs() < 1e-5 && moved[2].abs() < 1e-5, "{:?}", &moved[..3]);
        // Grey has no hue to rotate.
        assert_eq!(&moved[9..12], &[0.5, 0.5, 0.5]);
        // The leak colour is not the palette.
        assert_eq!(&moved[moved.len() - 12..], &row[row.len() - 12..]);

        row[at] = 0.0;
        row[at + 2] = 0.5; // half brightness
        let dim = apply_palette_motion(&MOTION_SCHEMA, &row).unwrap();
        assert!((dim[0] - 0.5).abs() < 1e-5 && (dim[9] - 0.25).abs() < 1e-5);

        row[at + 2] = 1.0;
        row[at + 3] = 1.0; // contrast: stops pulled apart from their mean
        let wide = apply_palette_motion(&MOTION_SCHEMA, &row).unwrap();
        assert!(wide[9] < 0.5, "the grey stop sits below the mean and gets darker");
    }

    #[test]
    fn migration_leaves_a_current_or_unrecognised_row_alone() {
        // Already current: no change.
        let mut current = row_with_blocks(6);
        let before = current.clone();
        let width = current.len();
        assert!(!migrate_signal_row(&mut current, width));
        assert_eq!(current, before);

        // A width this cannot account for (not exactly five short) is not
        // guessed at.
        let mut odd = vec![0.0f32; 30];
        let odd_before = odd.clone();
        assert!(!migrate_signal_row(&mut odd, 100));
        assert_eq!(odd, odd_before);

        // Too short to even hold the effects block: refused, not panicked on.
        let mut tiny = vec![0.0f32; 4];
        assert!(!migrate_signal_row(&mut tiny, 4 + SIGNAL_FLOATS_ADDED));
    }

    #[test]
    fn every_signal_float_is_reachable_and_named_once() {
        // A duplicate name would make `named_floats` — and the lane that
        // labels a keyframe from it — show two rows that write one column.
        let mut seen = std::collections::HashSet::new();
        for spec in SIGNAL_FLOATS {
            assert!(seen.insert(spec.name), "{} is declared twice", spec.name);
            assert!(spec.name.starts_with("sig_") || spec.name.starts_with("col_"), "{} would collide with a model's own float", spec.name);
        }
        assert_eq!(seen.len(), SIGNAL_WIDTH, "SIGNAL_WIDTH must count the list it describes");
    }

    #[test]
    fn a_signal_at_full_level_and_no_saturation_is_white() {
        // The degenerate case every fixture hits first: level up, colour not
        // chosen yet. `sat` defaults to 1, so this is asserted explicitly
        // rather than assumed off the defaults.
        let s = Signal { level: 1.0, hue: 0.0, sat: 0.0, strobe: 0.0, pan: 0.0, tilt: 0.0, a: 0.0, b: 0.0, cct: 5600.0, zoom: 0.0, focus: 0.0, iris: 0.0, frost: 0.0 };
        assert_eq!(s.rgb(), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn the_hue_ramp_reaches_each_primary_and_wraps_at_one() {
        let at = |hue: f32| {
            Signal { level: 1.0, hue, sat: 1.0, strobe: 0.0, pan: 0.0, tilt: 0.0, a: 0.0, b: 0.0, cct: 5600.0, zoom: 0.0, focus: 0.0, iris: 0.0, frost: 0.0 }.rgb()
        };
        assert_eq!(at(0.0), [1.0, 0.0, 0.0], "hue 0 is red");
        assert_eq!(at(1.0 / 3.0), [0.0, 1.0, 0.0], "a third round is green");
        assert_eq!(at(2.0 / 3.0), [0.0, 0.0, 1.0], "two thirds is blue");
        // A hue is an angle, and an angle authored past 1 — or below 0, which
        // a keyframe eased between two values can pass through — must land
        // somewhere on the wheel rather than clamp to red at both ends.
        assert_eq!(at(1.0), at(0.0), "the wheel wraps");
        // Approximate going backwards, and only going backwards: `-1/3`
        // survives `rem_euclid` as `0.6666667` rather than `2.0/3.0` exactly,
        // which is a float landing a ulp away from itself and not the wheel
        // failing to wrap. A lamp cannot show the difference.
        let (back, fwd) = (at(-1.0 / 3.0), at(2.0 / 3.0));
        for i in 0..3 {
            assert!((back[i] - fwd[i]).abs() < 1e-5, "and wraps backwards too: {back:?} vs {fwd:?}");
        }
    }

    #[test]
    fn level_is_the_only_thing_that_can_turn_a_fixture_off() {
        // The rule every `out.*` node depends on: `level` 0 is dark whatever
        // the hue and saturation say, so a blackout is one float and not a
        // conversation about colour.
        for hue in [0.0, 0.25, 0.5, 0.9] {
            for sat in [0.0, 0.5, 1.0] {
                let s = Signal { level: 0.0, hue, sat, strobe: 12.0, pan: 1.0, tilt: -1.0, a: 1.0, b: 1.0, cct: 5600.0, zoom: 0.0, focus: 0.0, iris: 0.0, frost: 0.0 };
                assert_eq!(s.rgb(), [0.0, 0.0, 0.0], "hue {hue} sat {sat} lit a dark fixture");
            }
        }
    }

    #[test]
    fn effect_index_names_every_float_and_refuses_the_footage_only_one() {
        for (i, spec) in EFFECTS_FLOATS.iter().enumerate() {
            assert_eq!(effect_index(spec.name), Some(i), "{} lost its index", spec.name);
        }
        // Conjunto F's promotion is an `fx.*` kind with no column in the
        // common row — a caller unwrapping this would panic the day it lands.
        assert_eq!(effect_index("wake"), None);
    }

    #[test]
    fn footage_chain_kind_names_the_promotion_and_refuses_every_row_float() {
        assert_eq!(footage_chain_kind("wake"), Some(CHAIN_KIND_ESTELA));
        for spec in EFFECTS_FLOATS {
            assert_eq!(
                footage_chain_kind(spec.name),
                None,
                "{} is a row float, not a footage kind",
                spec.name
            );
        }
        assert_eq!(footage_chain_kind("nonsense"), None);
    }

    /// The two lookups this module now offers for an `fx.*` node's name —
    /// [`effect_index`] and [`footage_chain_kind`] — must never both answer
    /// `Some` for the same name; a caller that tried both and got two
    /// different chain positions back would have no principled way to pick.
    #[test]
    fn effect_index_and_footage_chain_kind_never_both_know_the_same_name() {
        for spec in EFFECTS_FLOATS {
            assert!(footage_chain_kind(spec.name).is_none());
        }
        assert!(effect_index("wake").is_none());
    }

    #[test]
    fn amount_and_set_amount_round_trip_every_declared_effect() {
        // `Post` mirrors the common effects and the advanced groups; the
        // material group trails them and is read off the row, not through
        // `Post`, so it is not part of this round trip.
        let mirrored = CHAIN_FLOATS + N_ADVANCED;
        let mut post = Post::default();
        for i in 0..mirrored {
            post.set_amount(i, 0.25 + i as f32);
        }
        for i in 0..mirrored {
            assert_eq!(post.amount(i), 0.25 + i as f32, "{} did not round trip", i);
        }
        // A material position is inert here: it belongs to the row's tail, not
        // to `Post.advanced`.
        for i in mirrored..EFFECTS_FLOATS.len() {
            post.set_amount(i, 1.0);
            assert_eq!(post.amount(i), 0.0, "material float {i} leaked into Post");
        }
        // Past the end is inert in both directions, never a panic.
        post.set_amount(999, 1.0);
        assert_eq!(post.amount(999), 0.0);
    }

    #[test]
    fn a_preset_written_before_chain_existed_still_deserializes() {
        // The rule this module's own header states: every added field is
        // `#[serde(default)]`, or the preset library stops loading.
        let legacy = r#"{"version":1,"shake":0.5,"light_leak":0.0,
            "leak_color":[1,1,1,1,1,1,1,1,1,1,1,1]}"#;
        let post: Post = serde_json::from_str(legacy).expect("legacy preset loads");
        assert_eq!(post.shake, 0.5);
        assert!(post.chain.is_empty());
    }

    /// The same compatibility promise, one level down: `EffectStep` itself
    /// gained `slot` after `chain` already existed, so a step written by an
    /// older build has no `"slot"` key at all, not even `0` — this is what
    /// `#[serde(default)]` on the field is for, see its own doc comment.
    #[test]
    fn a_step_written_before_slot_existed_still_deserializes_to_slot_zero() {
        let legacy = r#"{"effect":7,"amount":0.4}"#;
        let step: EffectStep = serde_json::from_str(legacy).expect("legacy step still deserializes");
        assert_eq!(step, EffectStep { effect: 7, amount: 0.4, slot: 0 });
    }

    #[test]
    fn post_version_migration_updates_version() {
        let mut post = Post {
            version: 0,
            shake: 0.5,
            light_leak: 0.3,
            leak_color: vec![1.0; 12],
            ghost: 0.1,
            kaleido: 2.0,
            crt: 0.2,
            glitch: 0.15,
            posterize: 0.05,
            zoom: 0.1,
            fire: 0.0,
            lorenz: 0.0,
            vignette: 0.1,
            blur: 0.0,
            sharpen: 0.1,
            grain: 0.05,
            chain: Vec::new(),
            advanced: advanced_defaults(),
        };
        let old_values = (post.shake, post.ghost, post.kaleido);
        post.migrate(0);
        assert_eq!(post.version, EFFECTS_VERSION);
        // Values should remain unchanged through migration
        assert_eq!(post.shake, old_values.0);
        assert_eq!(post.ghost, old_values.1);
        assert_eq!(post.kaleido, old_values.2);
    }

    #[test]
    fn post_version_migration_is_idempotent() {
        let mut post = Post::default();
        let before = post.clone();
        post.migrate(EFFECTS_VERSION);
        assert_eq!(post, before);
    }

    #[test]
    fn effect_column_map_has_all_effects() {
        let map = effect_column_map();
        assert_eq!(map.len(), EFFECTS_FLOATS.len(), "map must cover all effects");
        for (i, &col) in map.iter().enumerate() {
            assert!(col < 4, "column {} for effect {} is out of range (0-3)", col, i);
        }
    }

    /// The common 14 rows stay exactly `EFFECT_NAMES` at the front, every
    /// advanced float follows them in [`ADVANCED_EFFECTS`] order, the material
    /// group closes the block, and a group's span is the contiguous range its
    /// own floats occupy.
    #[test]
    fn the_advanced_floats_are_appended_in_order_after_the_common_ones() {
        let common: Vec<&str> = EFFECTS_FLOATS[..CHAIN_FLOATS].iter().map(|s| s.name).collect();
        assert_eq!(common, EFFECT_NAMES.to_vec(), "the common rows did not move");

        let flat: Vec<&str> = ADVANCED_EFFECTS
            .iter()
            .flat_map(|(_, floats)| floats.iter().map(|s| s.name))
            .collect();
        let advanced_tail: Vec<&str> =
            EFFECTS_FLOATS[CHAIN_FLOATS..CHAIN_FLOATS + N_ADVANCED].iter().map(|s| s.name).collect();
        assert_eq!(advanced_tail, flat, "the advanced floats are the flattening of ADVANCED_EFFECTS");
        assert_eq!(flat.len(), N_ADVANCED);

        let material_tail: Vec<&str> =
            EFFECTS_FLOATS[CHAIN_FLOATS + N_ADVANCED..].iter().map(|s| s.name).collect();
        assert_eq!(
            material_tail,
            MATERIAL_FLOATS.iter().map(|s| s.name).collect::<Vec<_>>(),
            "the material group closes the effects block"
        );
        assert_eq!(EFFECTS_FLOATS.len(), CHAIN_FLOATS + N_ADVANCED + N_MATERIAL);

        let span = advanced_effect_span("grade").expect("grade is an advanced effect");
        assert_eq!(span, CHAIN_FLOATS..CHAIN_FLOATS + GRADE_FLOATS.len());
        assert_eq!(
            EFFECTS_FLOATS[span].iter().map(|s| s.name).collect::<Vec<_>>(),
            GRADE_FLOATS.iter().map(|s| s.name).collect::<Vec<_>>(),
            "grade's span is contiguous and covers exactly its floats"
        );
        assert_eq!(advanced_effect_span("nope"), None);
        // A later group starts where its predecessor ended.
        assert_eq!(
            advanced_effect_span("texture").unwrap().start,
            advanced_effect_span("grade").unwrap().end
        );
    }

    // -- the tick ladder ----------------------------------------------------

    /// Rule 7: no hand-written list of rates. Every enabled arm is swept, and
    /// the `8 × 2^k` relation is asserted from the values, never restated.
    #[test]
    fn a_tick_rate_is_8_times_a_power_of_two() {
        assert!(!TickRate::ALL.is_empty());
        for &rate in TickRate::ALL {
            let hz = rate.hz();
            assert_eq!(hz % BASE_HZ, 0, "{rate:?} is not a whole multiple of {BASE_HZ}");
            let ratio = hz / BASE_HZ;
            assert!(
                ratio.is_power_of_two(),
                "{rate:?} = {hz} Hz is {ratio}× the base, not a power of two"
            );
            // The derived helper and the raw arithmetic agree.
            assert_eq!(BASE_HZ << rate.k(), hz, "{rate:?}'s k and hz disagree");
            // And `from_hz` inverts `hz` for every arm.
            assert_eq!(TickRate::from_hz(hz), Some(rate));
        }
        // The two enabled arms are the two the plan names, and canonical is the
        // fine one.
        assert_eq!(TickRate::Base.hz(), 8);
        assert_eq!(TickRate::Fine.hz(), 32);
        assert_eq!(TickRate::CANONICAL, TickRate::Fine);
        assert_eq!(TickRate::from_hz(16), None, "16 Hz is on the ladder but not enabled");
    }

    /// The nesting rule: four 32 Hz ticks are exactly one 8 Hz tick, and the
    /// conversion pair is a round trip on-grid.
    #[test]
    fn the_ladder_nests_and_converts_through_one_pair() {
        assert_eq!(TickRate::Base.canonical_per_tick(), 4);
        assert_eq!(TickRate::Fine.canonical_per_tick(), 1);
        assert_eq!(TickRate::Base.to_canonical(3), 12);
        assert_eq!(TickRate::Fine.to_canonical(12), 12);
        for n in 0..1000i64 {
            let c = TickRate::Base.to_canonical(n);
            assert_eq!(TickRate::Base.from_canonical(c), n, "base round trip at {n}");
        }
        // Every canonical tick maps to the base tick that contains it.
        for c in 0..1000i64 {
            let b = TickRate::Base.from_canonical(c);
            assert_eq!(b, c / 4);
        }
    }

    /// The 4→1 projection is exact on a fixture, and each column uses its own
    /// rule: mean for continuous, max for `onset` and `hiss`, median for
    /// `pitch`, last for the held states.
    #[test]
    fn the_8hz_row_is_the_pool_of_its_four_32hz_rows() {
        let mut a = [0.0f32; FEATURES];
        let mut b = [0.0f32; FEATURES];
        let mut c = [0.0f32; FEATURES];
        let mut d = [0.0f32; FEATURES];

        // `rms` (0) is a mean of 1,2,3,4 → 2.5 exactly.
        for (i, r) in [&mut a, &mut b, &mut c, &mut d].into_iter().enumerate() {
            r[0] = i as f32 + 1.0;
        }
        // `onset` (2) is a max: 0.1, 0.9, 0.2, 0.3 → 0.9.
        a[2] = 0.1;
        b[2] = 0.9;
        c[2] = 0.2;
        d[2] = 0.3;
        // `pitch` (18) is a median: 0.1, 0.2, 0.3, 100 → 0.25.
        a[18] = 0.1;
        b[18] = 0.2;
        c[18] = 0.3;
        d[18] = 100.0;
        // `steady` (20) is held: the last fine value wins.
        a[20] = 1.0;
        b[20] = 2.0;
        c[20] = 3.0;
        d[20] = 7.0;
        // `hiss` (24) is a max.
        a[24] = 0.4;
        b[24] = 0.05;
        c[24] = 0.3;
        d[24] = 0.2;

        let base = pool_base(&[a, b, c, d]);
        assert_eq!(base[0], 2.5, "mean");
        assert_eq!(base[2], 0.9, "onset is max");
        assert_eq!(base[18], 0.25, "pitch is median");
        assert_eq!(base[20], 7.0, "steady is last");
        assert_eq!(base[24], 0.4, "hiss is max");
        // Every other column is a plain mean.
        assert_eq!(base[3], pool_rows(&[a, b, c, d])[3]);
    }

    #[test]
    fn pooling_an_empty_set_is_zero_not_nan() {
        let row = pool_rows(&[]);
        assert!(row.iter().all(|v| *v == 0.0));
    }

    /// A per-tick decay converts so the same wall-clock smoothing survives the
    /// level change: applying the fine alpha four times matches the base alpha
    /// once, and the ends (0, 1) are fixed.
    #[test]
    fn a_per_tick_decay_is_the_same_smoothing_at_another_level() {
        for &base in &[0.1f32, 0.3, 0.5, 0.9] {
            let fine = per_tick_decay(base, TickRate::Base, TickRate::Fine);
            // Four fine ticks must leave the same residual gap as one base tick.
            let residual_base = 1.0 - base;
            let residual_fine = (1.0 - fine).powi(4);
            assert!(
                (residual_base - residual_fine).abs() < 1e-5,
                "base {base}: residual {residual_base} vs {residual_fine}"
            );
        }
        assert_eq!(per_tick_decay(1.0, TickRate::Base, TickRate::Fine), 1.0);
        assert_eq!(per_tick_decay(0.0, TickRate::Base, TickRate::Fine), 0.0);
        assert_eq!(per_tick_decay(0.3, TickRate::Fine, TickRate::Fine), 0.3);
    }

    #[test]
    fn a_digest_filename_names_its_rate() {
        assert_eq!(digest_file_name("song", TickRate::Base), "song.8hz.npy");
        assert_eq!(digest_file_name("song", TickRate::Fine), "song.32hz.npy");
        assert_eq!(digest_file_name("10 - Ten", TickRate::CANONICAL), "10 - Ten.32hz.npy");
        assert_eq!(digest_rate_of("song.8hz.npy"), Some(TickRate::Base));
        assert_eq!(digest_rate_of("a.song.32hz.npy"), Some(TickRate::Fine));
        assert_eq!(digest_rate_of("song.npy"), None);
        assert!(digest_is_legacy("song.npy"));
        assert!(!digest_is_legacy("song.32hz.npy"));
        assert_eq!(digest_stem("song.32hz.npy"), "song");
        assert_eq!(digest_stem("a.song.8hz.npy"), "a.song");
        assert_eq!(digest_stem("song.npy"), "song");
    }

    #[test]
    fn tick_rate_round_trips_through_serde_as_a_name() {
        assert_eq!(serde_json::to_string(&TickRate::Base).unwrap(), "\"base\"");
        assert_eq!(serde_json::to_string(&TickRate::Fine).unwrap(), "\"fine\"");
        assert_eq!(serde_json::from_str::<TickRate>("\"fine\"").unwrap(), TickRate::Fine);
        // `wire`/`parse` are the same vocabulary serde just wrote.
        for &r in TickRate::ALL {
            assert_eq!(TickRate::parse(r.wire()), Some(r));
        }
        assert_eq!(TickRate::parse("30hz"), None);
    }
}
