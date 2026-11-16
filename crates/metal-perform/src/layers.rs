//! Layers — several animations on one screen at once.
//!
//! > *"There is the stage where they can overlap with different forms of
//! > opacity, and also the stage where they are part of different sectors of
//! > the device and do not cross, to frame doors and windows of an interior for
//! > example, or frames. It should be able to overlap by choosing forms of
//! > opacity, and choosing that from the simple table of the main interface, as
//! > well as the organisation of the layers or z index."*
//! > — 2026-08-19
//!
//! # This is the one thing in the app that overturns an earlier decision
//!
//! `output.rs` opens with *"always with the same origin, no multi-origin"*, and
//! everything in it follows from that: there is one picture, and an output is a
//! *view* of it. On 2026-08-19 that was deliberately superseded. The redesign
//! calls it **multi-source** — *"one same output device can have several masks
//! with different content"* — and the overlays handoff makes it a house rule:
//! *"Multi-source stays. Do not regress to duplicated multi-output."*
//!
//! So the rule that survived is narrower than the one that was written down,
//! and it is the one that was actually load-bearing: **one digest**. The music
//! is analysed once and every layer on every device reads the same sixteen
//! columns. What is no longer true is one *picture*. That is not drift, it is
//! the ask, and it is written here rather than left for someone to discover
//! from a contradiction between two module headers.
//!
//! # Two words that both got translated "mask"
//!
//! The redesign calls a configured model instance a "mask" (UI word *máscara*).
//! `output.rs` already calls a videomapping contour a `Mask`. They are different
//! things and renaming either one would break something durable — the contour is
//! in `data/outputs.json` on a real machine, the UI word is in the ask.
//!
//! They are kept apart by using the redesign's *other* word for the new thing:
//!
//! | thing | here | in the UI | lives in |
//! |---|---|---|---|
//! | a polygon cut out of a device | [`crate::output::Mask`] | mask (*máscara*) | the device |
//! | a model composited onto a device | [`Layer`] | layer (*capa*) | the device |
//!
//! A layer is a **source**; a mask is a **shape**. They compose in that order,
//! and the order is the whole reason there is no conflict: layers are combined
//! into one frame, and *then* the device's contours cut that frame. Contour the
//! pillar once and whatever you put on it lands on the pillar.
//!
//! # 2026-08-24: a source binds to a contour, and decides its own content
//!
//! Two things changed on the same day, and they are the same change seen from
//! two sides — the app stopped being a list of devices that contain sources and
//! became a list of sources that choose where to go.
//!
//! - **[`Layer::mask`]**. A source may name one of its device's contours, and
//!   then it is cut to that contour rather than to the device. *"It is important
//!   to let different sources sit on the same output device, to frame real-world
//!   objects."* Before this, every contour on a device showed the
//!   same composite: contouring two pillars lit two pillars with one animation.
//!   Now each pillar is a source. The device-wide cut in
//!   [`crate::output::apply_masks`] is unchanged and still runs last, so the
//!   union of every contour is still the lit region — binding only decides
//!   *which* source fills which contour.
//! - **[`Feed`]**. A source chooses its own content: an animation by hand, or a
//!   subscription to the router. *"Each source decides on its own."* The router is
//!   still one engine analysing one digest — see [`Airing`] — but there is no
//!   longer a global mode switch that makes every screen in the room show the
//!   same animation.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use metal_schema::Post;

// ---------------------------------------------------------------------------
// blend modes
// ---------------------------------------------------------------------------

/// How a layer's pixels combine with what is already under them.
///
/// **Four, and extensible.** The handoff recommends exactly these
/// (*"normal, add (suma), multiply, screen — extensible"*) and there is a
/// reason not to ship twenty: every one of them has to be legible from a
/// dropdown in a dark room by someone who is also beat-matching. Resolume ships
/// dozens and the ones anybody reaches for live are these four. Adding a fifth
/// is a variant here, an arm in [`Blend::mix`], and a name in the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Blend {
    /// Straight alpha. The layer covers what is under it.
    #[default]
    Normal,
    /// Additive — *suma* (the ask's word, accepted as an alias). Never darkens, so overlapping
    /// bright models bloom rather than fight. The one that reads as "more
    /// light in the room".
    Add,
    /// Multiplicative. Darkens; a bright high-contrast layer over another one
    /// works as a stencil, which is how a layer becomes a shape without a
    /// contour being drawn.
    Multiply,
    /// Inverse multiply. Lightens like `Add` but cannot clip to white, so a
    /// stack of three or four layers stays legible where `Add` would go flat.
    Screen,
}

impl Blend {
    pub fn as_str(self) -> &'static str {
        match self {
            Blend::Normal => "normal",
            Blend::Add => "add",
            Blend::Multiply => "multiply",
            Blend::Screen => "screen",
        }
    }

    pub fn parse(s: &str) -> Option<Blend> {
        match s {
            "normal" => Some(Blend::Normal),
            // `suma` because that is the word in the ask, and an operator who
            // reads Spanish should be able to send what they see.
            "add" | "suma" => Some(Blend::Add),
            "multiply" | "multiplicar" => Some(Blend::Multiply),
            "screen" | "pantalla" => Some(Blend::Screen),
            _ => None,
        }
    }

    /// Every mode this build knows, for the page's dropdown. Sent rather than
    /// typed into the page, so adding one here is the only edit needed.
    pub fn all() -> [Blend; 4] {
        [Blend::Normal, Blend::Add, Blend::Multiply, Blend::Screen]
    }

    /// Combines one channel, all values 0..1.
    ///
    /// `opacity` is applied *after* the mode, not before, and that is the whole
    /// difference between a fade and a wash: `Add` at opacity 0.5 must add half
    /// as much light, and `Multiply` at opacity 0.5 must darken half as much —
    /// which means interpolating from `under` toward the blended result, not
    /// scaling the layer and then blending it. Scaling first makes `Multiply`
    /// at low opacity *darker* than at high, because a dimmed layer multiplies
    /// harder. That is a real bug this ordering exists to prevent.
    pub fn mix(self, under: f32, over: f32, opacity: f32) -> f32 {
        let blended = match self {
            Blend::Normal => over,
            Blend::Add => under + over,
            Blend::Multiply => under * over,
            Blend::Screen => 1.0 - (1.0 - under) * (1.0 - over),
        };
        (under + (blended - under) * opacity).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// sectors
// ---------------------------------------------------------------------------

/// Where on the device a layer lives, in 0..1 of the device.
///
/// Normalised for the same reason a contour is: *"even if they have different
/// resolutions"*. A projector swapped for one with a different native size
/// keeps every sector, because a door is still where the door is.
///
/// A rectangle rather than a shape, deliberately. The handoff left it open and
/// recommended *"normalised rectangle + frame presets"*, and the reason to
/// take that recommendation is that the app already has an answer for
/// non-rectangular things: [`crate::output::Mask`] contours them with key
/// points. A second polygon system for sectors would be the same feature twice,
/// with two places to get a door wrong.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sector {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The smallest side a sector may have, as a fraction of the device.
const MIN_SIDE: f32 = 0.01;

impl Default for Sector {
    /// The whole device — what a layer in *overlap* (superposición) wants, and the only
    /// sensible thing for a layer created before anyone has thought about
    /// sectors.
    fn default() -> Sector {
        Sector {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }
    }
}

impl Sector {
    /// A zero-width sector is a layer that renders nothing and reports nothing,
    /// which reads exactly like a broken renderer. The floor is one percent of
    /// the device: small enough to be any real detail, big enough to be visible
    /// as a mistake.
    ///
    /// **The origin is clamped to leave room for that floor.** Clamping `x` to
    /// `0..1` and then `w` to `0.01..(1 - x)` looks right and panics: at
    /// `x = 0.99` the upper bound comes out at `0.00999999` by float error,
    /// `f32::clamp` asserts `min <= max`, and a sector nudged into the corner
    /// of a device takes the whole app down. Found by this module's own
    /// out-of-range test, which is why it is written as three cases and not as
    /// two `clamp` calls that read more neatly.
    pub fn clamped(self) -> Sector {
        let x = self.x.clamp(0.0, 1.0 - MIN_SIDE);
        let y = self.y.clamp(0.0, 1.0 - MIN_SIDE);
        Sector {
            x,
            y,
            w: self.w.clamp(MIN_SIDE, (1.0 - x).max(MIN_SIDE)),
            h: self.h.clamp(MIN_SIDE, (1.0 - y).max(MIN_SIDE)),
        }
    }

    pub fn is_full(&self) -> bool {
        self.x <= 1e-4 && self.y <= 1e-4 && self.w >= 1.0 - 1e-4 && self.h >= 1.0 - 1e-4
    }

    /// This sector in device pixels, as `(x, y, w, h)`, never zero-sized.
    pub fn pixels(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let s = self.clamped();
        let x = (s.x * width as f32).round() as u32;
        let y = (s.y * height as f32).round() as u32;
        let w = ((s.w * width as f32).round() as u32).clamp(1, width.saturating_sub(x).max(1));
        let h = ((s.h * height as f32).round() as u32).clamp(1, height.saturating_sub(y).max(1));
        (x, y, w, h)
    }

    pub fn overlaps(&self, other: &Sector) -> bool {
        let (a, b) = (self.clamped(), other.clamped());
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    /// The named starting rectangles the creation panel offers.
    ///
    /// Presets rather than a blank pair of number fields, because the ask is
    /// about *"doors and windows of an interior"* and the operator standing in
    /// that interior is holding a laptop, not a tape measure. Every one of them
    /// is an ordinary rectangle afterwards — a preset picks a starting point,
    /// it does not become a kind of sector.
    pub fn presets() -> Vec<(&'static str, &'static str, Sector)> {
        vec![
            (
                "full",
                "full",
                Sector {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
            ),
            (
                "left",
                "left",
                Sector {
                    x: 0.0,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0,
                },
            ),
            (
                "right",
                "right",
                Sector {
                    x: 0.5,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0,
                },
            ),
            (
                "top",
                "top",
                Sector {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 0.5,
                },
            ),
            (
                "bottom",
                "bottom",
                Sector {
                    x: 0.0,
                    y: 0.5,
                    w: 1.0,
                    h: 0.5,
                },
            ),
            // Tall and narrow, centred: a doorway seen straight on.
            (
                "door",
                "door",
                Sector {
                    x: 0.38,
                    y: 0.30,
                    w: 0.24,
                    h: 0.70,
                },
            ),
            // Wide and short, sitting above the floor: a window.
            (
                "window",
                "window",
                Sector {
                    x: 0.20,
                    y: 0.22,
                    w: 0.60,
                    h: 0.38,
                },
            ),
            (
                "third-left",
                "third left",
                Sector {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0 / 3.0,
                    h: 1.0,
                },
            ),
            (
                "third-mid",
                "third middle",
                Sector {
                    x: 1.0 / 3.0,
                    y: 0.0,
                    w: 1.0 / 3.0,
                    h: 1.0,
                },
            ),
            (
                "third-right",
                "third right",
                Sector {
                    x: 2.0 / 3.0,
                    y: 0.0,
                    w: 1.0 / 3.0,
                    h: 1.0,
                },
            ),
        ]
    }
}

// ---------------------------------------------------------------------------
// what a source draws
// ---------------------------------------------------------------------------

/// What one source draws: an animation chosen by hand, or a subscription to the
/// router.
///
/// > *"Each source decides on its own."* — 2026-08-24
///
/// This replaces the global router mode the dashboard used to carry. The
/// argument against a global mode is the room: a venue with a projector on the
/// back wall and two pillars is three surfaces that want different things, and a
/// single switch that makes all three rotate together is a switch nobody can
/// use. The argument *for* it — that the music should be analysed once — is
/// untouched, because it was never about the mode. There is still one digest,
/// one beat clock and one trained model; see [`Airing`], which is the whole of
/// what a source may subscribe to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Feed {
    /// The animation named by [`Layer::model`]. The default, and what every
    /// layer written before this existed already was.
    #[default]
    Model,
    /// Whatever the router has on air over the whole suite.
    Automix,
    /// The router's fast clock, rotating this source's **own** ticks.
    Group,
    /// The trained router, restricted to this source's own ticks.
    Router0,
}

impl Feed {
    pub fn as_str(self) -> &'static str {
        match self {
            Feed::Model => "model",
            Feed::Automix => "automix",
            Feed::Group => "group",
            Feed::Router0 => "router0",
        }
    }

    /// Accepts the English spelling the payload uses and the Spanish one the
    /// operator reads, for the same reason [`Blend::parse`] does.
    pub fn parse(s: &str) -> Option<Feed> {
        match s {
            "model" | "modelo" | "fijo" => Some(Feed::Model),
            "automix" => Some(Feed::Automix),
            "group" | "group" => Some(Feed::Group),
            "router0" | "modelo0" | "modelo 0" => Some(Feed::Router0),
            _ => None,
        }
    }

    pub fn all() -> [Feed; 4] {
        [Feed::Model, Feed::Automix, Feed::Group, Feed::Router0]
    }
}

/// What the automatic half of the app is doing this instant.
///
/// Built once per served frame and read by every source, so subscribing costs
/// nothing and — more to the point — two sources on `automix` cannot disagree
/// about what is on air because they are reading the same three numbers rather
/// than each asking the router again.
#[derive(Clone, Debug, Default)]
pub struct Airing {
    /// What the router has on air, over the whole suite.
    pub on_air: usize,
    /// How many sixteen-beat switches have happened since the group formed.
    /// A source rotating its own ticks steps on the same clock as the router,
    /// so everything in the room changes on the beat together even when it is
    /// changing to different things.
    pub step: usize,
    /// The trained router's ranking over the whole suite, best first. Empty
    /// when there is no model, or while it is still filling its window.
    pub ranked: Vec<usize>,
}

/// The set a source may draw from: its ticks, cleaned up, or nothing.
///
/// Returns empty rather than the whole suite when nothing is ticked, because
/// the two callers want opposite things from that and the decision belongs at
/// the call site — see [`Layer::showing`].
fn curated(picked: &[usize], models: usize) -> Vec<usize> {
    let mut p: Vec<usize> = picked.iter().copied().filter(|i| *i < models).collect();
    p.sort_unstable();
    p.dedup();
    p
}

// ---------------------------------------------------------------------------
// giro — the design notes PARTE IV §17.2
// ---------------------------------------------------------------------------

/// How a source turned by [`Layer::giro`] behaves at the edges of its
/// [`Sector`]. Meaningless while `giro` is zero — there are no edges to
/// behave differently at yet.
///
/// **Two, and no third.** *"What there is not is expanding the sector so it fits"* —
/// growing the sector to fit a turned source would step on
/// whatever sits next to it in [`Composition::Sectors`], where a layer's
/// rectangle is its own to fill and nobody else's to enlarge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ClipMode {
    /// *"crop"* — the default. The content turns as if the sector were a
    /// window onto it: whatever the turn carries past the sector's own edge
    /// is gone. At 45° a sector filled edge to edge loses its four corners
    /// to this — cheap, and the right choice whenever the sector already
    /// reads as a window rather than as the whole picture.
    #[default]
    Crop,
    /// *"fit"* — one more multiplication in `compose::source_at`, which
    /// shrinks what gets sampled so that every pixel of the sector, however
    /// far the turn would otherwise carry it, still lands inside the
    /// source. The trade runs the other way from `Crop`: no corner of the
    /// sector goes empty, at the cost of never reading the source's own
    /// corners. At 45° on a square sector this is the difference between
    /// four dark triangles and a picture that reads as whole, slightly
    /// zoomed in instead.
    Fit,
}

impl ClipMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ClipMode::Crop => "crop",
            ClipMode::Fit => "fit",
        }
    }

    pub fn parse(s: &str) -> Option<ClipMode> {
        match s {
            "crop" | "recortar" => Some(ClipMode::Crop),
            "fit" | "encajar" => Some(ClipMode::Fit),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// a layer
// ---------------------------------------------------------------------------

/// Where a layer's frame is produced. `Each` — the default, and what every
/// stack written before this existed means — is today's behaviour: whichever
/// machine composes the device compiles this layer too. `Node(name)` pins it
/// to one machine by name (the same names `Output.node`/`composed_on` use);
/// a layer pinned to a machine that is not the one compiling right now is
/// left out of the local plan entirely (see `compile_device_plan`'s `local`
/// parameter and its `remote` return value) — the frame-shipping half is
/// `S.R` point 5, not built yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum RenderOn {
    #[default]
    Each,
    Node(String),
}

impl RenderOn {
    /// `"each"` or `{ "node": "…" }` — the wire shape `apply_fields` reads
    /// and a saved `outputs.json`'s embedded layers already carry once this
    /// lands. Anything else (including an empty node name) is not a
    /// `RenderOn` at all, so a malformed body leaves the field untouched
    /// rather than silently defaulting.
    pub fn parse(v: &Value) -> Option<RenderOn> {
        if v.as_str() == Some("each") {
            return Some(RenderOn::Each);
        }
        let node = v["node"].as_str()?;
        (!node.is_empty()).then(|| RenderOn::Node(node.to_string()))
    }

    /// The same two shapes [`RenderOn::parse`] reads, for `/state`.
    pub fn describe(&self) -> Value {
        match self {
            RenderOn::Each => json!("each"),
            RenderOn::Node(node) => json!({ "node": node }),
        }
    }
}

/// A `mask` node's 3D binding, resolved for one frame: the id of the
/// [`crate::escena::Wall`] the source is drawn on, and — when the contour is
/// not the whole wall — the id of the [`crate::escena::WallMask`] that cuts
/// it.
///
/// This is the 3D counterpart of [`Layer::mask`]. It is carried on the layer
/// only for the frame about to be drawn: [`Layer`] never serializes it (a 3D
/// binding lives in the graph, and the graph is the truth — rule 4), so every
/// mapping written before this existed is unchanged and no caller can mistake
/// it for a persisted field.
#[derive(Clone, Debug, PartialEq)]
pub struct WallBinding {
    /// Which wall, by [`crate::escena::Wall::id`].
    pub wall: String,
    /// Which contour on that wall, by [`crate::escena::WallMask::id`].
    /// `None` is the whole wall: the source fills the surface and nothing
    /// cuts it.
    pub contour: Option<String>,
}

/// A `venue.surface` node's binding, resolved for one frame: the venue it
/// reads and the surface of that venue the source lands on.
///
/// The 3D counterpart of [`Layer::mask`] and the successor of
/// [`WallBinding`]: `mask` decides **what** part of the source is kept, this
/// decides **where** it lands. Like [`WallBinding`] it is carried on the
/// layer only for the frame about to be drawn — a binding lives in the graph,
/// which is the truth (rule 4) — so every mapping written before it existed
/// is unchanged and no caller can mistake it for a persisted field.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceBinding {
    /// Which venue, by `data/venues/<venue>.toml` id.
    pub venue: String,
    /// Which surface of that venue, by [`crate::venue::Surface::id`].
    pub surface: String,
    /// P-V: which cell of the surface's `sectors` this source was dealt, in
    /// the order its branch reached the node's `image` port. `None` is a
    /// branch that predates sectors (a whole-surface warp).
    pub sector: Option<usize>,
}

/// One model, composited onto one device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: String,
    pub name: String,
    /// Which animation. An index into the built models, the same number
    /// `/select` takes — see `registry.rs` on why that list is append-only.
    pub model: usize,
    pub opacity: f32,
    /// Stacking order. **Higher is nearer the front**, which is the direction
    /// every layer panel an operator has ever used works in, and the table
    /// lists them highest first for the same reason.
    pub z: i32,
    pub blend: Blend,
    pub sector: Sector,
    /// Off means the layer is skipped entirely — not rendered, not composited,
    /// not paid for. Resolume calls it *bypass*; the table calls it a dot.
    pub active: bool,
    /// Solo. When any layer on a device is soloed, only soloed layers draw.
    /// The one control that answers "which of these five is the one I can see"
    /// without un-ticking four things and then having to put them back.
    #[serde(default)]
    pub solo: bool,
    /// This layer's own float row, if it has been given one.
    ///
    /// `None` — the normal case — means the layer uses the row the operator set
    /// for that model, so two layers of the same model track one set of
    /// sliders. The field exists because the redesign says a source carries
    /// *"different parameters"*, and because the studio is where a row is
    /// authored; the dash does not edit it, it carries it.
    #[serde(default)]
    pub base: Option<Vec<f32>>,
    /// Which folder of footage this source draws, when it draws a
    /// transmission.
    ///
    /// > *"the footage selection should be per source and not global, because
    /// > some sources may work with one folder and others with another."*
    ///
    /// `None` — the normal case, and what every stack written before this
    /// existed says — means the library the footage sheet has open. So a venue
    /// that wants one folder never sees this field, and a venue that wants two
    /// names the second one here.
    ///
    /// Ignored by a source showing a model that is not a transmission, rather
    /// than being refused: a source flipped from Estela to Turing and back
    /// should not lose which folder it was pointed at.
    #[serde(default)]
    pub library: Option<String>,
    /// What this source draws. [`Feed::Model`] — the default — means `model`
    /// above, which is why every stack written before feeds existed still
    /// shows exactly what it showed.
    #[serde(default)]
    pub feed: Feed,
    /// This source's own ticks, for [`Feed::Group`] and [`Feed::Router0`].
    /// Indices into the built models; empty means "no opinion".
    #[serde(default)]
    pub picked: Vec<usize>,
    /// The contour on this source's device that cuts it, by
    /// [`crate::output::Mask::id`].
    ///
    /// `None` is a source that fills its [`Sector`] — the ordinary rectangle.
    /// Some is a source cut to a real object: the sector becomes the contour's
    /// bounding box (that is what gets rendered) and the polygon decides which
    /// of those pixels reach the frame. A name that no longer matches any
    /// contour on the device is treated as `None` rather than as an error, so
    /// deleting a contour cannot black out a source with no way to see why.
    #[serde(default)]
    pub mask: Option<String>,
    /// Task #8 / R23.4: a 3D binding resolved for this frame from a `mask`
    /// node's `{ "wall": …, "contour": …? }` config — the source is drawn on
    /// a wall and warped through that output's homography, cut to the named
    /// contour when there is one.
    ///
    /// **Transient.** `#[serde(skip)]` because the binding is a reading of
    /// the graph, not a property of the saved layer: a `mask` node in the
    /// authored graph is the only thing that can name a wall, and it is read
    /// fresh every frame. `None` is every layer ever persisted, so this field
    /// changes no existing file and no existing render.
    #[serde(skip)]
    pub mask_wall: Option<WallBinding>,
    /// P2: a `venue.surface` node's binding, resolved for this frame from its
    /// `{ "venue": …, "surface": … }` config — the source is drawn in the
    /// surface's own `(s, t)` and warped through the projector's calibration
    /// onto this device. Like [`Layer::mask_wall`] it is **transient**:
    /// `#[serde(skip)]` because it is a reading of the graph, not a property
    /// of the saved layer. `None` is every layer ever persisted, so this field
    /// changes no existing file and no existing render.
    #[serde(skip)]
    pub surface: Option<SurfaceBinding>,
    /// P-V: which cell of the surface's `sectors` grid this source was dealt,
    /// and how many cells there are — its slot in the fan-in order the graph
    /// gives a `venue.surface` node. `None` (or `0` with `sector_count` `1`)
    /// is the whole surface, which is every layer that does not come through
    /// a splitting grid, and so every layer persisted before this existed.
    /// **Transient** for the same reason as [`Layer::surface`]: the split is
    /// a reading of the graph and the venue, never a saved property.
    #[serde(skip)]
    pub sector_of: Option<usize>,
    #[serde(skip, default = "one_sector")]
    pub sector_count: usize,
    /// Whether this source's `fondo` slot renders as alpha (transparent)
    /// instead of its own colour, letting it sit over another source without
    /// its background covering it — the design notes PARTE IV §17.3.
    ///
    /// `false` — the normal case, and what every stack written before this
    /// existed says — means the backdrop pass draws its gradient as always.
    /// Meaningless for a model whose [`metal_render::model::Model::backdrop`] is
    /// `false` (22 of the 34 models): ignored there rather than refused, same
    /// disposition as [`Layer::library`] above.
    #[serde(default)]
    pub fondo_alpha: bool,
    /// Geometric rotation of this source relative to the output device, in
    /// radians. Zero is unrotated. The content turns inside [`Layer::sector`];
    /// see [`Layer::giro_mode`] for what happens at the edges —
    /// the design notes PARTE IV §17.2.
    ///
    /// This is **compositing** rotation, done in `compose::source_at` by
    /// composing the existing destination-to-source mapping with an inverse
    /// turn — the model behind this source still draws straight, and never
    /// finds out its picture was put crooked. Not the same field as the
    /// `giro` inside Escape's and Relampago's own schemas (a `FloatSpec`
    /// that turns the content *those* models draw, before this ever sees
    /// it): same word, two different things, the way "contour" and
    /// "mask" are.
    #[serde(default, alias = "giro")]
    pub turn: f32,
    /// How the turned content behaves at the sector's own edges. Meaningless
    /// while `giro` is zero — same disposition as [`Layer::library`] above:
    /// ignored rather than refused, so a source turned back to zero does not
    /// lose whichever mode it had picked.
    #[serde(default)]
    pub giro_mode: ClipMode,
    /// Which devices (by id) show this source. Empty is a valid and honest
    /// state — a source just created and not yet routed to any destination,
    /// not an error. More than one id is real fan-out: the same source
    /// feeding two outputs at once — the design notes PARTE IV §17.1/§17.4.
    ///
    /// **This is the field that replaced containment.** A `Layer` used to
    /// live inside exactly one [`Device`]'s own `layers: Vec<Layer>`, which
    /// is why it could not previously be in two places — it could not be in
    /// two `Vec`s. Every layer now lives in one flat pool
    /// (`State::layers` in `engine.rs`) and this is how it finds its way back
    /// onto a device: [`layers_on`] and [`routed_to`] filter that pool by
    /// this field rather than by which `Vec` a layer happened to sit in.
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Which part of this source each device shows, by device id — the
    /// source's own window, in 0..1 of the picture, not of the device.
    ///
    /// Fan-out alone (`outputs` above) puts the **whole** picture on every
    /// device it is routed to: the same animation twice. A room of screens
    /// that should read as one picture — a laptop, the monitor beside it and
    /// three phones leaning on the monitor — needs each device to show *its
    /// piece* of one picture instead. That is this map: a device with an
    /// entry draws only that window of the source, stretched over its
    /// [`Layer::sector`]; a device without one draws the whole source, which
    /// is what every layer written before this field did.
    ///
    /// The windows are usually computed, not typed: `metal_perform::venue`
    /// projects each display's corners through the viewer's camera and
    /// writes the bounding box here, so the pieces line up from where the
    /// audience stands.
    ///
    /// **What it cannot do yet, said plainly.** Each device renders the
    /// source at its own size and aspect, then samples its window out of
    /// that — so a window of a third of the picture is a 3× nearest-neighbour
    /// upscale, and a model that corrects for aspect draws a slightly
    /// different picture on a portrait phone than on a landscape monitor.
    /// Continuous for models drawn in plain uv space; approximate for the
    /// rest. Rendering the source once at the whole canvas's size and
    /// cropping it per device is the fix, and it is not this field's job.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub span: std::collections::BTreeMap<String, Sector>,
    /// The WGSL this source draws when its model is `forge` — an id under
    /// `data/shaders/<id>.wgsl`. Ignored by every compiled model, the same
    /// "ignored, not refused" posture as [`Layer::library`], so a source
    /// flipped from `forge` to Escape and back keeps its shader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shader: Option<String>,
    /// This source's own effect values — the per-node half of
    /// `apply_node_post`. A layer that has not touched any effect stays at
    /// `Post::default()`, which together with `fijo_mask == 0` means "fully
    /// reactive/off", exactly what a stack saved before this field existed meant.
    #[serde(default)]
    pub effects: Post,
    /// Bit i set means effect i (same order `apply_node_post` uses internally)
    /// is pinned to the value in `effects` instead of being left as
    /// `drive::row` computed it from the digest. 0 is "nothing pinned" —
    /// the design notes PARTE IV §17.4 / Conjunto E.
    #[serde(default)]
    pub fijo_mask: u32,
    /// Which machine renders this layer's frame. See [`RenderOn`].
    #[serde(default)]
    pub render_on: RenderOn,
}

fn one_sector() -> usize {
    1
}

impl Layer {
    pub fn new(id: &str, name: &str, model: usize) -> Layer {
        Layer {
            id: id.to_string(),
            name: name.to_string(),
            model,
            opacity: 1.0,
            z: 0,
            blend: Blend::Normal,
            sector: Sector::default(),
            active: true,
            solo: false,
            base: None,
            library: None,
            feed: Feed::Model,
            picked: Vec::new(),
            mask: None,
            mask_wall: None,
            surface: None,
            sector_of: None,
            sector_count: 1,
            fondo_alpha: false,
            turn: 0.0,
            giro_mode: ClipMode::Crop,
            outputs: Vec::new(),
            span: std::collections::BTreeMap::new(),
            shader: None,
            effects: Post::default(),
            fijo_mask: 0,
            render_on: RenderOn::default(),
        }
    }

    /// Which animation this source is drawing right now.
    ///
    /// The one place a feed becomes an index. Everything downstream — the
    /// renderer, the float monitor, the thumbnail — asks this rather than
    /// reading `model`, so a source on `automix` cannot have its picture and its
    /// label disagree about what it is showing.
    ///
    /// Never out of range: `models` is the bound, and every branch that could
    /// return something else falls back to a value that came from the router,
    /// which is bounded already. That matters because this number is handed
    /// straight to `Paint::solo`.
    pub fn showing(&self, air: &Airing, models: usize) -> usize {
        if models == 0 {
            return 0;
        }
        let last = models - 1;
        let pool = curated(&self.picked, models);
        match self.feed {
            Feed::Model => self.model.min(last),
            Feed::Automix => air.on_air.min(last),
            // Nothing ticked is not "rotate the whole suite in index order" —
            // it is an operator who has opened the dropdown and not chosen yet,
            // and the honest picture for that is what the room is already
            // watching. One tick makes it a fixed animation, which is also what
            // it looks like it should do.
            Feed::Group if pool.is_empty() => air.on_air.min(last),
            Feed::Group => pool[air.step % pool.len()],
            Feed::Router0 => {
                let allowed = |m: &usize| pool.is_empty() || pool.contains(m);
                match air
                    .ranked
                    .iter()
                    .copied()
                    .find(|m| *m <= last && allowed(m))
                {
                    Some(m) => m,
                    // Untrained, still warming up, or the model only named
                    // animations this source has not ticked. Rotating is what
                    // the global router does in the same situation, and for the
                    // same reason: a source that stops changing is
                    // indistinguishable from a source that is broken.
                    None if pool.is_empty() => air.on_air.min(last),
                    None => pool[air.step % pool.len()],
                }
            }
        }
    }

    /// True when this source is being decided by the router rather than by hand.
    pub fn automatic(&self) -> bool {
        self.feed != Feed::Model
    }

    pub fn describe(&self, air: &Airing, models: usize) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "model": self.model,
            // What it is drawing *now*, which is `model` only when the feed is
            // by hand. The page labels the source with this.
            "showing": self.showing(air, models),
            "feed": self.feed.as_str(),
            "automatic": self.automatic(),
            "picked": self.picked,
            "library": self.library,
            "mask": self.mask,
            "fondo_alpha": self.fondo_alpha,
            "opacity": self.opacity,
            "z": self.z,
            "blend": self.blend.as_str(),
            "sector": { "x": self.sector.x, "y": self.sector.y,
                        "w": self.sector.w, "h": self.sector.h },
            "full": self.sector.is_full(),
            "turn": self.turn,
            "giro_mode": self.giro_mode.as_str(),
            "active": self.active,
            "solo": self.solo,
            "own_row": self.base.is_some(),
            // Which devices this source feeds. Two or more is real fan-out
            // (the design notes PARTE IV §17.4) rather than an editor gesture yet, but
            // the page can already show it since the field is real.
            "outputs": self.outputs,
            // Which machine renders this source's frame — `each` or
            // `{ "node": … }`, the shape `POST /layers { op: "set" }` takes.
            "render_on": self.render_on.describe(),
            // This layer's own effects, and which of them are fixed rather
            // than reactive — the design notes PARTE IV §17.4 Conjunto E. Same shape as
            // the master dial's "post" block in `/state` (`main.rs`), minus
            // `leak_color` for the same reason that block omits it too.
            // Reported so a test or a future editor can see the pair; there
            // is no per-layer effects UI yet — the master dial is still the
            // only control a table row offers.
            "effects": {
                "shake": self.effects.shake,
                "light_leak": self.effects.light_leak,
                "ghost": self.effects.ghost,
                "kaleido": self.effects.kaleido,
                "crt": self.effects.crt,
                "glitch": self.effects.glitch,
                "posterize": self.effects.posterize,
                "zoom": self.effects.zoom,
                "fire": self.effects.fire,
                "lorenz": self.effects.lorenz,
                "vignette": self.effects.vignette,
                "blur": self.effects.blur,
                "sharpen": self.effects.sharpen,
                "grain": self.effects.grain,
            },
            "fijo_mask": self.fijo_mask,
        })
    }
}

// ---------------------------------------------------------------------------
// how a device combines its layers
// ---------------------------------------------------------------------------

/// The two scenarios the ask names, which are genuinely different and not two
/// settings of one thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Composition {
    /// *"they overlap with different forms of opacity"* — the layers are
    /// mixed, back to front, each through its own blend mode and opacity.
    #[default]
    Overlap,
    /// *"they are part of different sectors of the device and do not
    /// cross"* — each layer owns its rectangle and nothing is mixed.
    ///
    /// **Not "overlap with rectangles".** In this mode a pixel belongs to
    /// exactly one layer: the front-most whose sector covers it. That is what
    /// *do not cross* asks for, and enforcing it in the compositor rather
    /// than by refusing overlapping rectangles is deliberate — a mapping is
    /// dragged into place live, and an app that rejects a sector mid-drag
    /// because it clipped the next one by a pixel is an app that gets closed.
    /// The table warns instead; see [`Device::overlapping`].
    Sectors,
}

impl Composition {
    pub fn as_str(self) -> &'static str {
        match self {
            Composition::Overlap => "overlap",
            Composition::Sectors => "sectors",
        }
    }

    pub fn parse(s: &str) -> Option<Composition> {
        match s {
            "overlap" | "overlay" | "superposición" => Some(Composition::Overlap),
            "sectors" | "sectores" => Some(Composition::Sectors),
            _ => None,
        }
    }
}

/// What a device is doing, chosen from the one dropdown the ask asks for:
/// *"in the device list there would also be an off option, being the same
/// dropdown menu"*.
///
/// Four entries and not three. The ask names automix, *disabled* (desactivado) and the
/// collections; [`DeviceMode::Own`] is the state a device is in between them —
/// it has a stack, and that stack is not a saved collection yet. Without it,
/// creating a layer would have to either invent a collection nobody asked for
/// or leave the device on automix, where the layer just created is invisible
/// and saving reads as having done nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum DeviceMode {
    /// No collection yet, so the router decides and the layers build
    /// themselves. The default, and the state a device is in before anybody
    /// has configured anything.
    Automix,
    /// Off. Renders nothing, costs nothing, and is not the same as pausing the
    /// whole app — the point is turning *one* screen off during a set.
    Disabled,
    /// Its own stack, not saved anywhere.
    Own,
    /// Showing a saved setup, live. Holds the id so the dropdown can show
    /// which one is selected.
    ///
    /// **Not copied onto the device — the design notes PARTE IV §17.5.** Selecting a
    /// collection is reading it, not applying it: every frame resolves the
    /// layers and the composition fresh from [`Collections::find`], so
    /// editing the collection after a device switches to it changes what
    /// that device shows on the next frame, with no reselect needed. The
    /// "apply" op is the one that still copies — see [`Collection`] on why —
    /// and it forks into [`DeviceMode::Own`] rather than landing here,
    /// precisely so the fork stays editable instead of being shadowed by
    /// this live read.
    Collection(String),
}

impl Default for DeviceMode {
    fn default() -> DeviceMode {
        DeviceMode::Automix
    }
}

impl DeviceMode {
    pub fn describe(&self) -> Value {
        match self {
            DeviceMode::Automix => json!({ "kind": "automix" }),
            DeviceMode::Disabled => json!({ "kind": "disabled" }),
            DeviceMode::Own => json!({ "kind": "own" }),
            DeviceMode::Collection(id) => json!({ "kind": "collection", "id": id }),
        }
    }

    pub fn parse(kind: &str, id: Option<&str>) -> Option<DeviceMode> {
        match kind {
            "automix" => Some(DeviceMode::Automix),
            "disabled" | "desactivado" => Some(DeviceMode::Disabled),
            "own" | "propio" => Some(DeviceMode::Own),
            "collection" | "coleccion" => Some(DeviceMode::Collection(id?.to_string())),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// collections
// ---------------------------------------------------------------------------

/// A saved setup — *"alternative configured set ups"*.
///
/// A snapshot of a stack of layers, not a reference to one. Applying it copies
/// the layers onto a device, so editing a device afterwards does not silently
/// rewrite the collection, and a collection can be applied to two devices at
/// once without them sharing state.
///
/// **Not bound to a device**, per the handoff's own recommendation. A
/// collection made against the projector on the back wall is a stack of models
/// with opacities and rectangles in 0..1 — nothing in it is about *which*
/// screen, which is exactly why the sectors are normalised.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub layers: Vec<Layer>,
    /// The composition mode the stack was saved with. Part of the snapshot:
    /// a set of rectangles saved in *sectors* and applied in *overlap*
    /// is a different picture, and the operator who saved it meant the first.
    #[serde(default)]
    pub composition: Composition,
}

impl Collection {
    /// A snapshot, so a rotating feed is described as itself rather than as
    /// whatever it happened to be showing when it was saved.
    pub fn describe(&self, models: usize) -> Value {
        let air = Airing::default();
        json!({
            "id": self.id,
            "name": self.name,
            "composition": self.composition.as_str(),
            "layers": self.layers.iter()
                .map(|l| l.describe(&air, models)).collect::<Vec<_>>(),
        })
    }
}

/// Every saved collection, and the file they live in.
///
/// A separate file from `data/outputs.json` on purpose. That file is *this
/// room* — which projector, pointed at what, contoured how — and it is the one
/// thing in the app that must survive a laptop being swapped. Collections are
/// *this act*: they travel with the operator and mean the same thing in a
/// different venue. Keeping them apart is what lets either be copied without
/// the other.
#[derive(Default)]
pub struct Collections {
    pub list: Vec<Collection>,
}

fn collections_path() -> std::path::PathBuf {
    crate::output::data_root().join("collections.json")
}

impl Collections {
    pub fn load() -> Collections {
        let Ok(text) = metal_vfs::read_to_string(collections_path()) else {
            return Collections::default();
        };
        Collections {
            list: serde_json::from_str(&text).unwrap_or_default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = collections_path();
        if let Some(dir) = path.parent() {
            metal_vfs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&self.list)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::atomic::write(&path, format!("{text}\n"))
    }

    pub fn find(&self, id: &str) -> Option<&Collection> {
        self.list.iter().find(|c| c.id == id)
    }

    pub fn describe(&self, models: usize) -> Value {
        json!(self
            .list
            .iter()
            .map(|c| c.describe(models))
            .collect::<Vec<_>>())
    }
}

// ---------------------------------------------------------------------------
// limits
// ---------------------------------------------------------------------------

/// *"I want to give freedom and they could have a lot of processing capacity on
/// some set ups, be generous but do set limits."*
///
/// So: generous, present, and **fixed**. Every field here is reported in
/// `/state` and the page warns before the ceiling rather than at it — a limit an
/// operator only meets as a frame rate collapsing at 1 a.m. is not a limit, it
/// is a bug they cannot report.
///
/// **They stopped being controls on 2026-08-24.** They were two sliders in
/// *settings → limits* (ajustes → límites), and the pane is gone: *"Limits and Settings status are removed too, the limits will be hardcoded."*
/// The reason that is the right call and not a loss of a feature is that neither number is a taste —
/// they are what this machine can hold, an operator has no way to know the right
/// value, and a slider offering a wrong one is a slider that makes the app look
/// broken. [`Limits::default`] is now the only value there is.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Limits {
    /// Layers per device. The handoff suggests 8–16; this is the top of that
    /// range because the cost that actually matters is pixels, not rows, and a
    /// stack of sixteen small sectors is cheaper than two full-frame layers.
    pub max_layers: u32,
    /// Pixels a device may ask the renderers for in one composed frame, summed
    /// over its layers.
    ///
    /// 1920x1080 is ~2.07M, so this is four full-frame layers at 1080p, or
    /// sixteen quarter-frame ones. Past it, layers are drawn smaller and scaled
    /// up rather than dropped — *"lower resolution/quality before dropping FPS"* —
    /// which degrades the picture visibly and gradually instead of
    /// making the app stutter.
    pub max_pixels: u64,
    /// The fraction of `max_layers` at which the table starts warning.
    pub warn_at: f32,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_layers: 16,
            max_pixels: 8_300_000,
            warn_at: 0.75,
        }
    }
}

impl Limits {
    pub fn describe(&self) -> Value {
        json!({
            "max_layers": self.max_layers,
            "max_pixels": self.max_pixels,
            "warn_at": self.warn_at,
        })
    }

    /// How much every layer must be scaled by to fit the pixel budget.
    ///
    /// One factor for the whole device rather than per layer: shrinking the
    /// cheap layers to pay for an expensive one would make the small sectors —
    /// the doors and windows, the ones aimed at real objects — the first thing
    /// to go soft, which is exactly backwards.
    pub fn scale_for(&self, wanted: u64) -> f32 {
        if wanted <= self.max_pixels || wanted == 0 {
            return 1.0;
        }
        // Area scales with the square of the linear factor.
        (self.max_pixels as f32 / wanted as f32)
            .sqrt()
            .clamp(0.15, 1.0)
    }
}

// ---------------------------------------------------------------------------
// the stack on one device
// ---------------------------------------------------------------------------

/// Everything about a device that is *not* layers: what it is doing and how
/// it combines whatever the pool routes to it.
///
/// **Layers moved out on 2026-08-30 — the design notes PARTE IV §17.1/§17.4.** This
/// used to also hold `layers: Vec<Layer>`, which is exactly what made a
/// source unable to feed two destinations: it could not be in two `Vec`s at
/// once. A `Layer` now lives in one flat pool (`State::layers` in `engine.rs`)
/// and finds its way onto a device through
/// [`Layer::outputs`] instead of through containment — see [`layers_on`] and
/// [`routed_to`], which is what every method below used to be before it had
/// a device id and a pool handed to it instead of `self.layers`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Device {
    #[serde(default)]
    pub composition: Composition,
    #[serde(default)]
    pub mode: DeviceMode,
    /// O.10: Named triggers that are active and can be fired on this device.
    /// Empty by default (no triggers active). Each trigger has a unique id and
    /// names a target action (overlay, sequence, or "transport").
    #[serde(default)]
    pub triggers: Vec<crate::trigger::Trigger>,
    /// O.20/S.AC: the DMX head this device drives, if any. A device with a
    /// head is a fixture, not a screen; the profile in `data/fixtures/` says
    /// how its signal becomes slots.
    #[serde(default)]
    pub head: Option<crate::fixture::Head>,
}

impl Device {
    pub fn describe(
        &self,
        device_id: &str,
        layers: &[Layer],
        limits: &Limits,
        air: &Airing,
        models: usize,
    ) -> Value {
        // Every layer routed here, in the order they were created — not
        // necessarily the order they draw in. What the activity table
        // lists, including layers that are bypassed or hidden by solo, so
        // the operator can still find and re-enable them.
        let routed = routed_to(layers, device_id);
        let drawing = layers_on(layers, device_id);
        let crossing = overlapping_on(layers, device_id, self.composition);
        json!({
            "composition": self.composition.as_str(),
            "mode": self.mode.describe(),
            "layers": routed.iter()
                .map(|l| l.describe(air, models)).collect::<Vec<_>>(),
            "drawing": drawing.len(),
            "soloing": routed.iter().any(|l| l.solo && l.active),
            "at_limit": routed.len() as u32 >= limits.max_layers,
            "near_limit": routed.len() as f32
                >= limits.max_layers as f32 * limits.warn_at,
            "crossing": crossing.iter()
                .map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// filtering the pool by device — the design notes PARTE IV §17.1/§17.4
// ---------------------------------------------------------------------------
//
// Free functions rather than methods on `Device`, because the thing being
// filtered — `State::layers` in `engine.rs` — no longer lives on `Device` at
// all. Each one takes the whole pool and a device id, which is the same
// shape [`automix`] below already used for "logic that is not one struct's
// to own".

/// Every layer routed to `device_id`, unfiltered by [`Layer::active`] or
/// [`Layer::solo`] — what the activity table lists, and what a bulk op like
/// "apply a collection" or "clear" replaces wholesale.
pub fn routed_to<'a>(layers: &'a [Layer], device_id: &str) -> Vec<&'a Layer> {
    layers
        .iter()
        .filter(|l| l.outputs.iter().any(|o| o == device_id))
        .collect()
}

/// Takes `device_id` off every layer that was routed to it, and drops the
/// layers that had no other destination. The other half of deleting a device.
///
/// **Why this is not just `retain`.** Since §17.1 a layer is a source in one
/// pool with a *list* of destinations, not a child of one device — so a
/// device going away has two different meanings depending on the layer.
/// A source feeding only that screen has nowhere left to draw and is gone;
/// a source fanned out to two screens loses one of them and keeps drawing on
/// the other. Deleting the whole `Vec` entry unconditionally would silently
/// take the second one off the screen it is still supposed to be on.
///
/// **Why it has to happen at all.** The pool is global and the device list is
/// not, so `outputs.list.retain(...)` alone leaves the layers behind with a
/// dangling destination on them — invisible, because nothing lists a device
/// that is gone, and resurrected the moment a device is created under the
/// same id. `POST /outputs {op:"remove"}` already learned this once for
/// composition and mode (see the note at its call site); the layers are the
/// same mistake one field over.
/// **Why only the layers that named it.** A layer may legitimately have an
/// empty `outputs` — one loaded from a file written before the field existed
/// (`a_layer_written_before_outputs_existed_loads_unrouted`), or one being
/// built before it is routed anywhere. A blanket "drop everything with no
/// destination" would delete those as a side effect of deleting an unrelated
/// screen, so this only ever drops a layer whose *last* destination was the
/// device being removed.
pub fn unroute_device(layers: &mut Vec<Layer>, device_id: &str) {
    layers.retain_mut(|l| {
        if !l.outputs.iter().any(|o| o == device_id) {
            return true;
        }
        l.outputs.retain(|o| o != device_id);
        !l.outputs.is_empty()
    });
}

/// The layers on `device_id` that will actually draw, back to front.
///
/// Sorted ascending by `z` so the compositor can walk it in order and the
/// last one written is the front one. The table shows the reverse, because a
/// layer panel reads top-down. This is what `Device::draw_order` was before
/// a layer could be routed to more than one device: same solo/active rule,
/// applied after filtering the pool down to this device's own routing.
pub fn layers_on<'a>(layers: &'a [Layer], device_id: &str) -> Vec<&'a Layer> {
    let routed = routed_to(layers, device_id);
    let soloing = routed.iter().any(|l| l.solo && l.active);
    let mut on: Vec<&Layer> = routed
        .into_iter()
        .filter(|l| l.active && (!soloing || l.solo))
        .collect();
    // Stable, so two layers at the same z keep the order they were created
    // in rather than swapping about between frames.
    on.sort_by_key(|l| l.z);
    on
}

/// Pairs of layers on `device_id` whose sectors cross, for the warning in the
/// table. Only meaningful in [`Composition::Sectors`]; in overlap, crossing
/// is the point. `composition` is a parameter rather than read off a
/// `Device` because this has no `Device` to read it from — the caller (today,
/// only [`Device::describe`]) already has one.
pub fn overlapping_on(
    layers: &[Layer],
    device_id: &str,
    composition: Composition,
) -> Vec<(String, String)> {
    if composition != Composition::Sectors {
        return Vec::new();
    }
    let on = layers_on(layers, device_id);
    let mut out = Vec::new();
    for (i, a) in on.iter().enumerate() {
        for b in &on[i + 1..] {
            if a.sector.overlaps(&b.sector) {
                out.push((a.name.clone(), b.name.clone()));
            }
        }
    }
    out
}

/// The layer with this id, in the global pool.
///
/// **Ids are unique across the whole pool now, not just within one device's
/// old `Vec`.** Before layers moved out of `Device`, `POST /layers` op
/// `"add"` only had to mint an id that was unique against `stack.layers` —
/// one device's own list. Two devices could mint the same id by pure
/// coincidence and nothing would notice, because nothing ever searched
/// across devices. Now one search does, so the id generator was changed to
/// check the whole pool (`main.rs`, `POST /layers` op `"add"`) and
/// `output::load_devices` renames any collision it finds migrating old data
/// — see that function's own note.
pub fn find_mut<'a>(layers: &'a mut [Layer], id: &str) -> Option<&'a mut Layer> {
    layers.iter_mut().find(|l| l.id == id)
}

/// The next free `z` among the layers routed to `device_id`, so a layer
/// created from the panel lands in front of what is already there rather
/// than behind it and invisible.
///
/// Filtered by routing only, **not** by [`layers_on`]'s active/solo rule —
/// same as the `Device::next_z` this replaces, which read every layer on the
/// device whether or not it was currently drawing. A bypassed layer still
/// holds its `z`, and a new layer must not be handed a `z` a hidden one is
/// already sitting on.
pub fn next_z_on(layers: &[Layer], device_id: &str) -> i32 {
    routed_to(layers, device_id)
        .iter()
        .map(|l| l.z)
        .max()
        .map(|z| z + 1)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// automix
// ---------------------------------------------------------------------------

/// How much of the device a second animation may take when the music is at its
/// loudest. Never 1: automix that reaches a full crossfade is a device
/// switching animations on its own at a moment nobody chose, which is the
/// failure `Mode::Single` exists to prevent in the router.
const AUTOMIX_CEILING: f32 = 0.55;

/// The layers a device in [`Mode::Automix`] draws, built rather than authored.
///
/// *"When no collection has been created yet, automix is set on a device that is a preview."* — so this is what every device shows before
/// anybody has configured anything, and it has to be worth looking at with zero
/// setup.
///
/// It is the router's own group, two deep: whatever is on air, full frame, and
/// the next one in the group screened over it by however loud the music is.
/// The handoff's recommendation for the open question was *"automatic default
/// mix of the active layers by the digest's energy/structure"*, and
/// energy is the column that carries it.
///
/// **At `energy` 0 this is exactly one full-frame animation** — the picture the
/// app drew before layers existed. That is deliberate: automix is the default
/// state, and a default that changed what a quiet room looks like would be a
/// feature imposing itself on every existing setup.
pub fn automix(on_air: usize, group: &[usize], energy: f32) -> Vec<Layer> {
    let mut out = vec![Layer::new("automix-0", "automix · air", on_air)];

    // The next animation in the group, which is the one the router is about to
    // switch to anyway. Bleeding *that* one in means the switch, when it comes,
    // lands on something the room has already been looking at.
    //
    // **By position, not by `cycle().skip_while()`.** That reads better and
    // hangs: when what is on air is not in the group — which happens for one
    // frame on every regroup, and for as long as the router is in single mode
    // with an empty group — `skip_while` over an infinite iterator never
    // finds its predicate and spins forever, inside the default state of the
    // app, with no output and no error. This module's own test is what found
    // it.
    let next = group
        .iter()
        .position(|m| *m == on_air)
        .map(|at| group[(at + 1) % group.len()])
        .filter(|m| *m != on_air);

    if let Some(next) = next {
        let opacity = (energy.clamp(0.0, 1.0) * AUTOMIX_CEILING).clamp(0.0, AUTOMIX_CEILING);
        if opacity > 0.01 {
            let mut bleed = Layer::new("automix-1", "automix · enters", next);
            bleed.opacity = opacity;
            bleed.z = 1;
            // Screen rather than add: at the top of the ceiling two bright
            // models added together clip to white and the picture goes flat
            // exactly when the music is loudest.
            bleed.blend = Blend::Screen;
            out.push(bleed);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_scales_the_effect_of_a_mode_rather_than_the_layer() {
        // The ordering bug this exists to prevent: scaling the layer first
        // makes Multiply *darker* at low opacity than at high, because a dimmed
        // layer multiplies harder. Half-opacity multiply must be half as dark.
        let under = 0.8;
        let over = 0.0; // the darkest a multiply can ask for
        let full = Blend::Multiply.mix(under, over, 1.0);
        let half = Blend::Multiply.mix(under, over, 0.5);
        assert!(full < half, "full multiply should be darker than half");
        assert!(
            (half - 0.4).abs() < 1e-5,
            "half multiply landed at {half}, not halfway"
        );
    }

    #[test]
    fn opacity_zero_leaves_what_is_under_it_untouched_in_every_mode() {
        // A layer faded out must be *gone*, not "nearly gone". An operator
        // fading a layer to zero and still seeing it has no way to remove it
        // except deleting it.
        for blend in Blend::all() {
            for under in [0.0, 0.25, 0.5, 1.0] {
                for over in [0.0, 0.3, 1.0] {
                    assert_eq!(
                        blend.mix(under, over, 0.0),
                        under,
                        "{blend:?} at opacity 0 changed the picture"
                    );
                }
            }
        }
    }

    #[test]
    fn every_mode_stays_inside_the_channel_range() {
        for blend in Blend::all() {
            for u in 0..=10 {
                for o in 0..=10 {
                    for a in 0..=10 {
                        let v = blend.mix(u as f32 / 10.0, o as f32 / 10.0, a as f32 / 10.0);
                        assert!((0.0..=1.0).contains(&v), "{blend:?} produced {v}");
                    }
                }
            }
        }
    }

    #[test]
    fn screen_never_darkens_and_multiply_never_brightens() {
        // The property that makes each one the right choice, stated once so a
        // later edit to the formulas cannot quietly swap them.
        for u in 0..=10 {
            for o in 0..=10 {
                let (under, over) = (u as f32 / 10.0, o as f32 / 10.0);
                assert!(Blend::Screen.mix(under, over, 1.0) >= under - 1e-6);
                assert!(Blend::Multiply.mix(under, over, 1.0) <= under + 1e-6);
                assert!(Blend::Add.mix(under, over, 1.0) >= under - 1e-6);
            }
        }
    }

    #[test]
    fn a_sector_is_the_same_rectangle_at_any_resolution() {
        // The same promise a contour makes, and for the same reason: a door is
        // where the door is, whatever the projector was swapped for.
        let door = Sector {
            x: 0.25,
            y: 0.5,
            w: 0.5,
            h: 0.5,
        };
        for (w, h) in [(640u32, 360u32), (1280, 720), (1920, 1080)] {
            let (x, y, sw, sh) = door.pixels(w, h);
            assert_eq!(x, w / 4);
            assert_eq!(y, h / 2);
            assert_eq!(sw, w / 2);
            assert_eq!(sh, h / 2);
        }
    }

    #[test]
    fn a_sector_never_runs_off_the_device() {
        // Straight into a pixel loop, so this is the bound that stops a bad
        // number written by hand from indexing past the frame.
        for s in [
            Sector {
                x: 0.9,
                y: 0.9,
                w: 1.0,
                h: 1.0,
            },
            Sector {
                x: -1.0,
                y: -1.0,
                w: 5.0,
                h: 5.0,
            },
            Sector {
                x: 0.5,
                y: 0.5,
                w: 0.0,
                h: 0.0,
            },
        ] {
            let (x, y, w, h) = s.pixels(1920, 1080);
            assert!(x + w <= 1920, "{s:?} ran off the width");
            assert!(y + h <= 1080, "{s:?} ran off the height");
            assert!(w >= 1 && h >= 1, "{s:?} collapsed to nothing");
        }
    }

    #[test]
    fn every_sector_preset_is_inside_the_device_and_worth_offering() {
        for (id, label, s) in Sector::presets() {
            assert!(!id.is_empty() && !label.is_empty());
            let c = s.clamped();
            // Within a float epsilon, not exactly: a third of the device is
            // 0.33333334 and `1 - 2/3` is 0.33333331, so the rightmost third
            // is trimmed by one part in ten million. That is a preset that is
            // already valid, not a preset that is wrong.
            let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
            assert!(
                near(c.x, s.x) && near(c.y, s.y) && near(c.w, s.w) && near(c.h, s.h),
                "preset {id} is not already valid: {s:?} became {c:?}"
            );
            assert!(s.w > 0.05 && s.h > 0.05, "preset {id} is too small to aim");
        }
    }

    /// Stamps every layer's `outputs` to `[device]`, so a test can build a
    /// pool the old `d.layers = vec![...]` fixtures used to build directly on
    /// a `Device` — the routing the pool now needs in place of containment.
    fn routed(device: &str, mut layers: Vec<Layer>) -> Vec<Layer> {
        for l in &mut layers {
            l.outputs = vec![device.to_string()];
        }
        layers
    }

    #[test]
    fn solo_hides_everything_that_is_not_soloed() {
        let mut pool = routed(
            "d",
            vec![
                Layer::new("a", "a", 0),
                Layer::new("b", "b", 1),
                Layer::new("c", "c", 2),
            ],
        );
        assert_eq!(layers_on(&pool, "d").len(), 3);
        find_mut(&mut pool, "b").unwrap().solo = true;
        let on = layers_on(&pool, "d");
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].id, "b");
    }

    #[test]
    fn a_soloed_layer_that_is_bypassed_does_not_black_out_the_device() {
        // The trap in combining the two: solo on a layer whose bypass is off
        // would otherwise mean "show only this" and "do not show this", and a
        // naive read of that is a black screen with two lit buttons explaining
        // why.
        let mut pool = routed("d", vec![Layer::new("a", "a", 0), Layer::new("b", "b", 1)]);
        find_mut(&mut pool, "b").unwrap().solo = true;
        find_mut(&mut pool, "b").unwrap().active = false;
        let on = layers_on(&pool, "d");
        assert_eq!(on.len(), 1, "an inactive solo hid the rest of the device");
        assert_eq!(
            on[0].id, "a",
            "the layer that should still be drawing is not"
        );
    }

    #[test]
    fn layers_draw_back_to_front_by_z() {
        let mk = |id: &str, z: i32| {
            let mut l = Layer::new(id, id, 0);
            l.z = z;
            l
        };
        let pool = routed("d", vec![mk("front", 10), mk("back", -5), mk("mid", 0)]);
        let order: Vec<&str> = layers_on(&pool, "d")
            .iter()
            .map(|l| l.id.as_str())
            .collect();
        assert_eq!(order, ["back", "mid", "front"]);
    }

    #[test]
    fn a_new_layer_lands_in_front_rather_than_behind() {
        // Created behind an opaque full-frame layer, a new layer is invisible,
        // and the operator's reasonable conclusion is that saving did nothing.
        let mut pool: Vec<Layer> = Vec::new();
        assert_eq!(next_z_on(&pool, "d"), 0);
        pool.extend(routed("d", vec![Layer::new("a", "a", 0)]));
        assert_eq!(next_z_on(&pool, "d"), 1);
        let mut high = Layer::new("b", "b", 1);
        high.z = 40;
        pool.extend(routed("d", vec![high]));
        assert_eq!(next_z_on(&pool, "d"), 41);
    }

    #[test]
    fn crossing_sectors_are_reported_only_where_crossing_is_a_mistake() {
        let mut a = Layer::new("a", "door", 0);
        a.sector = Sector {
            x: 0.0,
            y: 0.0,
            w: 0.6,
            h: 1.0,
        };
        let mut b = Layer::new("b", "window", 1);
        b.sector = Sector {
            x: 0.4,
            y: 0.0,
            w: 0.6,
            h: 1.0,
        };
        let mut pool = routed("d", vec![a, b]);

        // In overlap, crossing is the entire feature.
        assert!(overlapping_on(&pool, "d", Composition::Overlap).is_empty());
        assert_eq!(
            overlapping_on(&pool, "d", Composition::Sectors).len(),
            1,
            "two crossing sectors went unreported"
        );

        find_mut(&mut pool, "b").unwrap().sector = Sector {
            x: 0.7,
            y: 0.0,
            w: 0.3,
            h: 1.0,
        };
        assert!(
            overlapping_on(&pool, "d", Composition::Sectors).is_empty(),
            "sectors that only touch are not crossing"
        );
    }

    #[test]
    fn a_layer_routed_to_two_devices_appears_on_both() {
        // The whole point of this change — the design notes PARTE IV §17.1/§17.4.
        // Before it, a Layer lived inside exactly one Device's own `Vec` and
        // simply could not be in two places at once; it could not be in two
        // `Vec`s. `outputs` is what makes fan-out representable at all.
        let mut l = Layer::new("shared", "shared", 0);
        l.outputs = vec!["a".to_string(), "b".to_string()];
        let pool = vec![l];

        let on_a = layers_on(&pool, "a");
        let on_b = layers_on(&pool, "b");
        assert_eq!(on_a.len(), 1, "not routed to device a");
        assert_eq!(on_b.len(), 1, "not routed to device b");
        assert_eq!(on_a[0].id, "shared");
        assert_eq!(
            on_a[0], on_b[0],
            "the same layer, byte for byte, on both devices"
        );
        assert!(
            layers_on(&pool, "c").is_empty(),
            "must not appear on a device it was not routed to"
        );
    }

    #[test]
    fn automix_at_rest_is_one_full_frame_animation() {
        // The default state of every device. If this ever became two, every
        // existing setup would change the day the app was updated.
        let out = automix(3, &[3, 7, 9, 1], 0.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].model, 3);
        assert!(out[0].sector.is_full());
        assert_eq!(out[0].opacity, 1.0);
    }

    #[test]
    fn automix_bleeds_in_the_next_animation_of_the_group_as_the_music_rises() {
        let out = automix(3, &[3, 7, 9, 1], 1.0);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[1].model, 7,
            "it should bleed in what the router plays next"
        );
        assert!(
            out[1].opacity <= AUTOMIX_CEILING + 1e-6,
            "a full crossfade is a switch"
        );
        assert!(
            out[1].opacity > 0.2,
            "loud music should be visible in the mix"
        );
        assert!(out[1].z > out[0].z);
    }

    #[test]
    fn automix_survives_a_group_that_does_not_contain_what_is_on_air() {
        // Happens for one frame every time the router regroups, and for as long
        // as an operator is in single mode with an empty group. A panic here
        // would be a panic in the default state of the app.
        assert_eq!(automix(5, &[], 1.0).len(), 1);
        assert_eq!(automix(5, &[1, 2], 1.0).len(), 1);
        assert_eq!(
            automix(5, &[5], 1.0).len(),
            1,
            "a group of one cannot bleed into itself"
        );
    }

    #[test]
    fn the_pixel_budget_shrinks_layers_instead_of_dropping_them() {
        let limits = Limits::default();
        assert_eq!(limits.scale_for(0), 1.0);
        assert_eq!(limits.scale_for(limits.max_pixels), 1.0);
        // Four times the budget is half the linear size, not a quarter of the
        // layers.
        let scale = limits.scale_for(limits.max_pixels * 4);
        assert!((scale - 0.5).abs() < 1e-3, "scale was {scale}");
        // And it has a floor: past a point, smaller is not cheaper, it is just
        // unreadable.
        assert!(limits.scale_for(limits.max_pixels * 10_000) >= 0.15);
    }

    // -----------------------------------------------------------------------
    // feeds — what a source draws when it is not choosing by hand
    // -----------------------------------------------------------------------

    fn air(on_air: usize, step: usize, ranked: &[usize]) -> Airing {
        Airing {
            on_air,
            step,
            ranked: ranked.to_vec(),
        }
    }

    #[test]
    fn a_source_by_hand_ignores_the_router_entirely() {
        // The default, and what every stack written before feeds existed is.
        // If this ever followed the router, updating the app would change what
        // every configured venue shows.
        let l = Layer::new("a", "a", 5);
        assert_eq!(l.feed, Feed::Model);
        assert!(!l.automatic());
        assert_eq!(l.showing(&air(11, 3, &[9]), 17), 5);
    }

    #[test]
    fn two_sources_on_automix_agree_because_they_read_the_same_number() {
        let mut a = Layer::new("a", "a", 0);
        let mut b = Layer::new("b", "b", 12);
        a.feed = Feed::Automix;
        b.feed = Feed::Automix;
        let now = air(7, 2, &[]);
        assert_eq!(a.showing(&now, 17), 7);
        assert_eq!(b.showing(&now, 17), 7, "two sources on automix disagreed");
    }

    #[test]
    fn a_source_rotating_its_own_ticks_steps_on_the_routers_clock() {
        let mut l = Layer::new("a", "a", 0);
        l.feed = Feed::Group;
        l.picked = vec![2, 5, 9];
        // The same three, in order, one per sixteen-beat switch — so everything
        // in the room changes together even when it changes to different things.
        assert_eq!(l.showing(&air(0, 0, &[]), 17), 2);
        assert_eq!(l.showing(&air(0, 1, &[]), 17), 5);
        assert_eq!(l.showing(&air(0, 2, &[]), 17), 9);
        assert_eq!(
            l.showing(&air(0, 3, &[]), 17),
            2,
            "the rotation did not wrap"
        );
    }

    #[test]
    fn a_group_with_nothing_ticked_shows_what_the_room_is_already_watching() {
        // An operator who opened the dropdown and has not ticked anything yet.
        // Rotating the whole suite in index order would be a different picture
        // every sixteen beats for a choice they have not made.
        let mut l = Layer::new("a", "a", 0);
        l.feed = Feed::Group;
        assert_eq!(l.showing(&air(6, 4, &[]), 17), 6);
        // And one tick makes it exactly that one, which is what a set of one
        // looks like it should do.
        l.picked = vec![13];
        assert_eq!(l.showing(&air(6, 4, &[]), 17), 13);
    }

    #[test]
    fn the_trained_feed_takes_the_best_ranked_animation_it_is_allowed() {
        let mut l = Layer::new("a", "a", 0);
        l.feed = Feed::Router0;
        // Unrestricted: the winner.
        assert_eq!(l.showing(&air(0, 0, &[8, 3, 1]), 17), 8);
        // Restricted: the best one inside the ticks, not the best one overall.
        l.picked = vec![1, 3];
        assert_eq!(l.showing(&air(0, 0, &[8, 3, 1]), 17), 3);
    }

    #[test]
    fn an_untrained_feed_rotates_rather_than_freezing() {
        // Three states that look identical in the picture: no model, a model
        // still filling its window, and a model that decided. The first two
        // reach here as an empty ranking, and a source that stopped changing is
        // indistinguishable from a source that is broken.
        let mut l = Layer::new("a", "a", 0);
        l.feed = Feed::Router0;
        l.picked = vec![4, 6];
        assert_eq!(l.showing(&air(0, 0, &[]), 17), 4);
        assert_eq!(l.showing(&air(0, 1, &[]), 17), 6);
        // Nothing ticked and nothing trained: what the router has on air.
        l.picked.clear();
        assert_eq!(l.showing(&air(15, 1, &[]), 17), 15);
    }

    #[test]
    fn no_feed_can_name_an_animation_the_renderer_does_not_have() {
        // This number goes straight to `Paint::solo`, which indexes a slice.
        // Every branch has to be bounded, including the ones fed by a router
        // that knows about a suite this build does not have all of.
        let models = 4;
        for feed in Feed::all() {
            let mut l = Layer::new("a", "a", 99);
            l.feed = feed;
            l.picked = vec![1, 2];
            assert!(
                l.showing(&air(88, 7, &[77, 2]), models) < models,
                "{feed:?}"
            );
            l.picked.clear();
            assert!(
                l.showing(&air(88, 7, &[77]), models) < models,
                "{feed:?} unticked"
            );
        }
        // And a build with nothing in it at all does not panic on the modulo.
        assert_eq!(Layer::new("a", "a", 0).showing(&air(0, 0, &[]), 0), 0);
    }

    #[test]
    fn a_feed_survives_the_round_trip_it_is_sent_over() {
        for feed in Feed::all() {
            assert_eq!(Feed::parse(feed.as_str()), Some(feed));
            let text = serde_json::to_string(&feed).expect("serialise");
            assert_eq!(
                serde_json::from_str::<Feed>(&text).expect("deserialise"),
                feed
            );
        }
        // The Spanish spellings an operator's own payload might carry.
        assert_eq!(Feed::parse("group"), Some(Feed::Group));
        assert_eq!(Feed::parse("modelo 0"), Some(Feed::Router0));
        assert_eq!(Feed::parse("nonsense"), None);
    }

    #[test]
    fn a_layer_written_before_feeds_and_contours_existed_still_loads() {
        // `data/outputs.json` and `collections.json` are real files on a real
        // machine. Every field added here has to be defaultable.
        let old = r#"{"id":"l0","name":"backdrop","model":3,"opacity":1.0,"z":0,
                      "blend":"normal","sector":{"x":0,"y":0,"w":1,"h":1},
                      "active":true}"#;
        let l: Layer = serde_json::from_str(old).expect("a pre-feed layer loads");
        assert_eq!(l.feed, Feed::Model);
        assert!(l.picked.is_empty());
        assert_eq!(l.mask, None);
        // And it still draws exactly what it drew: the model in its record.
        assert_eq!(l.showing(&air(9, 4, &[1]), 17), 3);
    }

    #[test]
    fn a_device_written_before_layers_existed_still_loads() {
        // `data/outputs.json` is a real venue's mapping. Every field added
        // here has to be defaultable or an update costs somebody their room.
        // `layers` is not a field of `Device` any more — the design notes PARTE IV
        // §17.1/§17.4 moved it to a pool keyed by `Layer::outputs` instead —
        // so there is nothing left to assert about it here; `composition`
        // and `mode` are the whole of what a bare `{}` has to default.
        let d: Device = serde_json::from_str("{}").expect("an empty device loads");
        assert_eq!(d.composition, Composition::Overlap);
        assert_eq!(d.mode, DeviceMode::Automix);
    }

    #[test]
    fn a_stack_round_trips_through_json_including_its_mode() {
        // `triggers` arrived with O.10 and this literal was never updated.
        let d = Device {
            composition: Composition::Sectors,
            mode: DeviceMode::Collection("night-1".into()),
            triggers: Vec::new(),
            head: None,
        };
        let text = serde_json::to_string(&d).expect("serialise");
        let back: Device = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back, d);
    }

    #[test]
    fn deleting_a_device_takes_the_sources_that_only_fed_it() {
        // The bug this pins: `check.mjs` passed on a fresh server and failed
        // on every run after it, because `op:"remove"` dropped the device but
        // not the layers pointing at it, and `op:"add"` under the same id
        // handed them straight back — a harness section asserting "one layer"
        // read three.
        let mut pool = vec![
            Layer::new("only-a", "only on a", 0),
            Layer::new("a-and-b", "on both", 1),
            Layer::new("unrouted", "no destination", 2),
        ];
        pool[0].outputs = vec!["a".into()];
        pool[1].outputs = vec!["a".into(), "b".into()];

        unroute_device(&mut pool, "a");

        let left: Vec<&str> = pool.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(
            left,
            vec!["a-and-b", "unrouted"],
            "only the source that had nowhere else to go"
        );
        assert_eq!(
            pool[0].outputs,
            vec!["b".to_string()],
            "the fan-out keeps the screen it is still on"
        );
        assert!(
            pool[1].outputs.is_empty(),
            "a source that never named a device is not collateral"
        );
        assert!(
            routed_to(&pool, "a").is_empty(),
            "nothing still points at the device that is gone"
        );
    }

    #[test]
    fn a_layer_with_every_field_set_round_trips_through_json() {
        // What `a_stack_round_trips_through_json_including_its_mode` used to
        // cover by pushing a fully-populated `Layer` into a `Device` — moved
        // here because `Layer` is what actually owns all of this now.
        let mut l = Layer::new("l1", "door", 4);
        l.blend = Blend::Screen;
        l.opacity = 0.42;
        l.z = 3;
        l.sector = Sector {
            x: 0.1,
            y: 0.2,
            w: 0.3,
            h: 0.4,
        };
        l.base = Some(vec![0.5, 0.25]);
        l.feed = Feed::Router0;
        l.picked = vec![1, 4];
        l.mask = Some("mask-2".into());
        // Fan-out: the same source feeding two destinations at once —
        // the design notes PARTE IV §17.1/§17.4, the field this whole change exists to
        // add. It gets its own assertion below rather than riding along as
        // an incidental field.
        l.outputs = vec!["a".into(), "b".into()];

        let text = serde_json::to_string(&l).expect("serialise");
        let back: Layer = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back, l);
        assert_eq!(back.outputs, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn a_layer_written_before_outputs_existed_loads_unrouted() {
        // `data/outputs.json` before this change has no `outputs` key on a
        // layer at all — it is `output::load_devices` that stamps one in
        // from which device the layer was found under (see that function's
        // own note); here it only has to default to empty rather than fail
        // to parse.
        let old = r#"{"id":"l0","name":"backdrop","model":3,"opacity":1.0,"z":0,
                      "blend":"normal","sector":{"x":0,"y":0,"w":1,"h":1},
                      "active":true}"#;
        let l: Layer = serde_json::from_str(old).expect("a pre-outputs layer loads");
        assert!(l.outputs.is_empty());
    }

    #[test]
    fn a_blend_mode_survives_the_round_trip_it_is_sent_over() {
        // The page sends a string. A mode that serialised as one word and
        // parsed as another would silently fall back to normal.
        for blend in Blend::all() {
            assert_eq!(Blend::parse(blend.as_str()), Some(blend));
            let text = serde_json::to_string(&blend).expect("serialise");
            let back: Blend = serde_json::from_str(&text).expect("deserialise");
            assert_eq!(back, blend);
        }
        for mode in [Composition::Overlap, Composition::Sectors] {
            assert_eq!(Composition::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn a_clip_mode_survives_the_round_trip_it_is_sent_over() {
        for mode in [ClipMode::Crop, ClipMode::Fit] {
            assert_eq!(ClipMode::parse(mode.as_str()), Some(mode));
            let text = serde_json::to_string(&mode).expect("serialise");
            assert_eq!(
                serde_json::from_str::<ClipMode>(&text).expect("deserialise"),
                mode
            );
        }
        // The Spanish spellings an operator's own payload might carry, same
        // idiom as `Blend::parse` and `Composition::parse`.
        assert_eq!(ClipMode::parse("recortar"), Some(ClipMode::Crop));
        assert_eq!(ClipMode::parse("encajar"), Some(ClipMode::Fit));
        assert_eq!(ClipMode::parse("nonsense"), None);
    }

    #[test]
    fn a_layer_written_before_giro_existed_loads_unrotated() {
        // `data/outputs.json` and `collections.json` predate §17.2. Every
        // layer already on a real machine has no `giro` key at all.
        let old = r#"{"id":"l0","name":"backdrop","model":3,"opacity":1.0,"z":0,
                      "blend":"normal","sector":{"x":0,"y":0,"w":1,"h":1},
                      "active":true}"#;
        let l: Layer = serde_json::from_str(old).expect("a pre-giro layer loads");
        assert_eq!(l.turn, 0.0);
        assert_eq!(l.giro_mode, ClipMode::Crop);
    }

    #[test]
    fn a_layer_saved_with_the_spanish_giro_key_keeps_its_turn() {
        let old = r#"{"id":"l0","name":"backdrop","model":3,"opacity":1.0,"z":0,
                      "blend":"normal","sector":{"x":0,"y":0,"w":1,"h":1},
                      "active":true,"giro":0.25}"#;
        let l: Layer = serde_json::from_str(old).expect("a layer with the old giro key loads");
        assert_eq!(l.turn, 0.25);
        let out = serde_json::to_string(&l).unwrap();
        assert!(out.contains("\"turn\"") && !out.contains("giro\":"));
    }
}
