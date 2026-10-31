//! Overlays: playing a movement that is already saved over a layer that is
//! already live.
//!
//! `the design notes` §16.7. The ask was *"make an overlay play back a lane
//! made earlier"*, and §15 had shipped Overlays as an honest empty
//! state because no `overlay.*` node family exists in `metal-graph`'s catalog.
//! That is still true of a *node*. It was never true of the data: a
//! "previously-made lane" is a `author::library` **movement** — a named run of
//! keyframes, already on disk, already starrable, already pasteable — and a
//! model movement's ticks are runs of that model's float rows, which is exactly
//! the shape [`Layer::base`] takes. What was missing was the trigger.
//!
//! # Why this is a pure function of the clock and not a scheduler
//!
//! The obvious build is a scheduler that posts one row per tick. It would have
//! been the wrong one, and the reason is measured rather than guessed:
//!
//! - the server is one thread taking one `Mutex` per request (`main`'s accept
//!   loop), so every post lands in the same queue as every frame;
//! - `POST /layers` writes the whole of `outputs.json` back to disk on every
//!   call (`Outputs::save`), because a mapping that half-applied is a mapping
//!   to redo in front of a room;
//! - and the old HTML page gave image pumps three connections total (`INFLIGHT`
//!   there; deleted 15-sept-2026, S.U18 — that ceiling existed because the page
//!   froze solid during a live set).
//!
//! Eight posts a second, each one a full serialisation of the device file,
//! queued in front of the frames, on the surface whose entire purpose is to be
//! used *during a transmission*. That is not a risk to weigh, it is the freeze
//! reproduced deliberately.
//!
//! So nothing here is pushed. A [`Play`] records which tick it started on and
//! the row is computed from `State::time()` wherever a base row is already
//! being read — which is four places that already run once per frame. The
//! whole network cost of playing a movement is **one request to start it and
//! one to stop it**; the stepping is free, because the reads were happening
//! anyway.
//!
//! This is the same property the Router has, and it is why the Router could be
//! a module rather than a rewrite: *"the rest of the app was already a pure
//! function of `state.time()`"*. Two things fall out of it for free. Pausing
//! the transport freezes overlays with everything else, because `State::time`
//! is what freezes — the panic button that already exists already stops these.
//! And resuming does not skip: `POST /transport` rewinds `started` rather than
//! letting the clock jump the length of the pause, so a movement armed while
//! paused holds on tick zero and starts when the room does.
//!
//! # The guard that matters
//!
//! A movement's rows fit **one model**, checked against that model's row width
//! when it was saved (`author::library::check`). A layer's model is not fixed:
//! a source on automix or on the trained router is `showing` whatever the
//! router says this instant. So a play that was correct when it was armed can
//! be pointed at a different model one bar later, and pushing a 48-float
//! `turing` row at an 18-float `geoterrain` would not be refused by anything
//! downstream — `drive::row_with` resizes silently and the renderer draws the
//! result. Silent, in range, and wrong: the failure `the design notes`
//! §3.3 and §11.7 both name.
//!
//! Hence [`Overlays::row`] takes the model the layer is *actually showing* and
//! answers `None` when it is not the movement's own. The overlay goes quiet for
//! as long as the router has the layer somewhere else and comes back when it
//! returns, rather than scrambling a row. The route refuses the arming case up
//! front for the same reason, so the ordinary mistake is reported instead of
//! being drawn.

use serde_json::{json, Value};

use crate::compose;
use crate::digest;
use crate::layers::{Airing, Blend, ClipMode, Layer, Sector};
use crate::output::Mask;

/// A blackout overlay: all outputs turned off for a duration.
pub struct Blackout {
    pub device: String,
    pub started: f32,
    pub duration: f32,
}

impl Blackout {
    /// True if this blackout is still active at the given time.
    pub fn is_active(&self, time: f32) -> bool {
        let elapsed = time - self.started;
        elapsed >= 0.0 && elapsed < self.duration
    }

    fn describe(&self, time: f32) -> Value {
        json!({
            "device": self.device,
            "type": "blackout",
            "started": self.started,
            "duration": self.duration,
            "active": self.is_active(time),
            "elapsed": (time - self.started).max(0.0),
        })
    }
}

/// A solid overlay as it reads out ([`Overlays::solid_view`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SolidView {
    pub rgb: [u8; 3],
    pub masks: Vec<(String, [u8; 3])>,
    pub grid: bool,
    pub started: f32,
    pub duration: f32,
}

/// A solid color overlay: a fixed color for a duration.
pub struct SolidColorOverlay {
    pub device: String,
    /// The colour of the whole device — and, when `masks` is set, of
    /// everything outside the listed masks.
    pub rgb: [u8; 3],
    /// Per-mask test colours (a venue's `rgb` per mask): each listed mask's
    /// region is painted its own colour over `rgb`, cut by that mask's own
    /// CPU coverage ([`compose::Tile::clip`]) — never a mask shader. Empty,
    /// which is every one-colour body, is `rgb` over the whole device, byte
    /// for byte what a solid always painted.
    pub masks: Vec<(String, [u8; 3])>,
    /// P-F: a test pattern — black grid lines over the colour (`rgb` white
    /// for the edge-blend check). `false` is every solid before it.
    pub grid: bool,
    pub started: f32,
    pub duration: f32,
}

impl SolidColorOverlay {
    /// True if this color overlay is still active at the given time.
    pub fn is_active(&self, time: f32) -> bool {
        let elapsed = time - self.started;
        elapsed >= 0.0 && elapsed < self.duration
    }

    fn describe(&self, time: f32) -> Value {
        let mut v = json!({
            "device": self.device,
            "type": "solid_color",
            "rgb": self.rgb.to_vec(),
            "started": self.started,
            "duration": self.duration,
            "active": self.is_active(time),
            "elapsed": (time - self.started).max(0.0),
        });
        if self.grid {
            v["pattern"] = json!("grid");
        }
        // Only when set, so a one-colour solid reports exactly what it did.
        if !self.masks.is_empty() {
            v["masks"] = Value::Object(
                self.masks
                    .iter()
                    .map(|(id, rgb)| (id.clone(), json!(rgb.to_vec())))
                    .collect(),
            );
        }
        v
    }
}

/// One movement, running on one layer of one device.
pub struct Play {
    pub device: String,
    pub layer: String,
    /// Which movement, by the name it has in the library. Carried for the
    /// report only — the ticks below are a copy, taken when the trigger was
    /// tapped, so renaming or re-saving a movement mid-set cannot change what
    /// is already in the air.
    pub movement: String,
    /// The model index these rows are rows *of*. The whole of the width guard
    /// hangs on this; see the module note.
    pub model: usize,
    pub ticks: Vec<Vec<f32>>,
    /// `State::time()` when the trigger was tapped, in the app's seconds.
    pub started: f32,
    /// Whether it repeats. A one-shot releases the layer when it ends rather
    /// than holding its last row: an overlay that stays after it finished is a
    /// state change with nothing on screen saying so, and the operator finds
    /// out later. Looping is the deliberate opt-in for "hold this".
    pub looping: bool,
    /// `R22.8` point 4: the mask this sequence is cut by while it runs, an id
    /// in the target device's own `masks`. `None` — every play armed before
    /// this, and every sequence saved without one — is no cut.
    pub mask: Option<String>,
}

impl Play {
    /// Which tick this play is on at `time`, or `None` if it is over.
    ///
    /// `digest::RATE` is the rate, not a rate of this module's choosing: it is
    /// the one constant the digest, `metal_rig::motion::KEY_RATE` and
    /// `digest_audio.py` all restate, so a movement's ticks *are* eighths of a
    /// second and playing them at anything else would be playing the phrase at
    /// the wrong speed.
    ///
    /// A clock that has gone backwards under a running play reads as tick zero
    /// rather than as an error. It should not happen — `State::time` is
    /// monotonic across pause and resume — but the honest response to a
    /// negative elapsed is the beginning of the phrase, not a panic in a render
    /// loop.
    pub fn at(&self, time: f32) -> Option<usize> {
        let len = self.ticks.len();
        if len == 0 {
            return None;
        }
        let elapsed = time - self.started;
        let n = if elapsed <= 0.0 {
            0
        } else {
            (elapsed * digest::RATE) as usize
        };
        if self.looping {
            Some(n % len)
        } else if n < len {
            Some(n)
        } else {
            None
        }
    }

    fn describe(&self, time: f32) -> Value {
        let on = self.at(time);
        json!({
            "device": self.device,
            "layer": self.layer,
            "movement": self.movement,
            "model": self.model,
            "ticks": self.ticks.len(),
            // Where it is now, and how much is left. Both so the page can draw
            // progress off the two-second `/state` poll without running a
            // clock of its own — the page renders from the last `/state` and
            // never from its own guesses, everywhere else in this app.
            "tick": on,
            "left": on.map(|n| self.ticks.len().saturating_sub(n + 1)),
            "loop": self.looping,
            "mask": self.mask,
        })
    }
}

/// Every movement, blackout, and solid color overlay in the air.
/// At most one movement per layer, one blackout per device, and one solid color per device.
#[derive(Default)]
pub struct Overlays {
    running: Vec<Play>,
    blackouts: Vec<Blackout>,
    solid_colors: Vec<SolidColorOverlay>,
}

/// One device's overlays at one instant, so the render path can carry the
/// lookup as a single argument instead of three.
///
/// `compile_device_plan` in `engine.rs` is where this matters: it is the one
/// place `layers::layers_on` is called from on the render path, deliberately,
/// and the layer list it builds is what the generator dedup key is computed
/// from.
/// Applying overlays anywhere later would let two layers that are drawing
/// different rows share one `gen.*` node and one of them silently win. Handing
/// this in means the key is right by construction rather than by discipline.
#[derive(Clone, Copy)]
pub struct Playing<'a> {
    overlays: &'a Overlays,
    device: &'a str,
    time: f32,
}

impl Overlays {
    /// This device's overlays, at this instant.
    pub fn on<'a>(&'a self, device: &'a str, time: f32) -> Playing<'a> {
        Playing {
            overlays: self,
            device,
            time,
        }
    }

    /// Arms a movement, replacing whatever that layer had.
    ///
    /// Replacing rather than stacking: a layer has one float row, so two
    /// movements on one layer is not a thing that can be drawn, and the second
    /// tap on a trigger grid means "this one now" in every instrument that has
    /// ever had one.
    pub fn start(&mut self, play: Play) {
        self.running
            .retain(|p| !(p.device == play.device && p.layer == play.layer));
        self.running.push(play);
    }

    /// Takes one layer's movement off. True if there was one.
    pub fn stop(&mut self, device: &str, layer: &str) -> bool {
        let before = self.running.len();
        self.running
            .retain(|p| !(p.device == device && p.layer == layer));
        before != self.running.len()
    }

    /// The off switch. Everything, everywhere, at once — the one control this
    /// surface cannot ship without, because it is used during a set and the
    /// operator has to be able to get back to the stack they know with one
    /// gesture rather than by remembering what they armed.
    pub fn stop_all(&mut self) -> usize {
        let n = self.running.len() + self.blackouts.len() + self.solid_colors.len();
        self.running.clear();
        self.blackouts.clear();
        self.solid_colors.clear();
        n
    }

    /// Start a blackout on a device for the given duration (in seconds).
    /// Replaces any existing blackout on that device.
    pub fn start_blackout(&mut self, device: String, started: f32, duration: f32) {
        self.blackouts.retain(|b| b.device != device);
        self.blackouts.push(Blackout {
            device,
            started,
            duration,
        });
    }

    /// Stop the blackout on a device. Returns true if there was one.
    pub fn stop_blackout(&mut self, device: &str) -> bool {
        let before = self.blackouts.len();
        self.blackouts.retain(|b| b.device != device);
        before != self.blackouts.len()
    }

    /// Check if a device is currently blackedout at the given time.
    pub fn is_blackout(&self, device: &str, time: f32) -> bool {
        self.blackouts
            .iter()
            .any(|b| b.device == device && b.is_active(time))
    }

    /// Start a solid color overlay on a device for the given duration (in seconds).
    /// Replaces any existing solid color on that device.
    pub fn start_solid_color(&mut self, device: String, rgb: [u8; 3], started: f32, duration: f32) {
        self.start_solid_masks(device, rgb, Vec::new(), started, duration);
    }

    /// [`Self::start_solid_color`] with a colour per mask: `rgb` covers the
    /// device and each `(mask id, colour)` paints that mask's region over it
    /// (see [`SolidColorOverlay::masks`]). An empty list is the one-colour
    /// solid. Replaces any existing solid color on that device.
    pub fn start_solid_masks(
        &mut self,
        device: String,
        rgb: [u8; 3],
        masks: Vec<(String, [u8; 3])>,
        started: f32,
        duration: f32,
    ) {
        self.solid_colors.retain(|c| c.device != device);
        self.solid_colors.push(SolidColorOverlay {
            device,
            rgb,
            masks,
            grid: false,
            started,
            duration,
        });
    }

    /// P-F: the test pattern — `rgb` over the whole device with a black grid
    /// on it (16 × 9 cells and the two centre lines), for a projector's
    /// edge-blend check. Replaces any existing solid on that device.
    pub fn start_test_pattern(&mut self, device: String, rgb: [u8; 3], started: f32, duration: f32) {
        self.start_solid_masks(device.clone(), rgb, Vec::new(), started, duration);
        if let Some(c) = self.solid_colors.iter_mut().find(|c| c.device == device) {
            c.grid = true;
        }
    }

    /// Whether the solid active on `device` at `time` is a test pattern.
    pub fn solid_grid(&self, device: &str, time: f32) -> bool {
        self.solid_colors
            .iter()
            .any(|c| c.device == device && c.is_active(time) && c.grid)
    }

    /// Stop the solid color overlay on a device. Returns true if there was one.
    pub fn stop_solid_color(&mut self, device: &str) -> bool {
        let before = self.solid_colors.len();
        self.solid_colors.retain(|c| c.device != device);
        before != self.solid_colors.len()
    }

    /// Get the solid color overlay for a device at the given time, if any.
    pub fn solid_color(&self, device: &str, time: f32) -> Option<[u8; 3]> {
        self.solid_colors
            .iter()
            .find(|c| c.device == device && c.is_active(time))
            .map(|c| c.rgb)
    }

    /// The solid active on `device` at `time`, whole: what `POST /overlay
    /// {op:"to-timeline"}` turns into colour clips.
    pub fn solid_view(&self, device: &str, time: f32) -> Option<SolidView> {
        self.solid_colors.iter().find(|c| c.device == device && c.is_active(time)).map(|c| SolidView {
            rgb: c.rgb,
            masks: c.masks.clone(),
            grid: c.grid,
            started: c.started,
            duration: c.duration,
        })
    }

    /// The per-mask colours of the solid active on `device` at `time`:
    /// empty when none is armed or it has one colour only.
    pub fn solid_masks(&self, device: &str, time: f32) -> &[(String, [u8; 3])] {
        self.solid_colors
            .iter()
            .find(|c| c.device == device && c.is_active(time))
            .map(|c| c.masks.as_slice())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    /// The row a layer should be drawn with, if a movement is playing on it
    /// and that movement is for the model the layer is currently showing.
    ///
    /// `showing` is the model the layer is drawing *this instant*, not its
    /// `model` field — see the module note on why the difference is the whole
    /// guard.
    pub fn row(&self, device: &str, layer: &str, showing: usize, time: f32) -> Option<&[f32]> {
        let play = self
            .running
            .iter()
            .find(|p| p.device == device && p.layer == layer && p.model == showing)?;
        play.at(time).map(|n| play.ticks[n].as_slice())
    }

    /// `R22.8` point 4: the mask the play on `layer` is cut by right now, or
    /// `None` for no cut (or nothing playing there). The same lookup
    /// [`Self::row`] makes — one play per layer, the one whose `model` is the
    /// layer's `showing`.
    pub fn mask_for(&self, device: &str, layer: &str, showing: usize) -> Option<&str> {
        self.running
            .iter()
            .find(|p| p.device == device && p.layer == layer && p.model == showing)?
            .mask
            .as_deref()
    }

    /// Drops the plays that are over, and the ones whose layer is gone.
    /// Also drops expired blackout and solid color overlays.
    ///
    /// Called from `/state`, beside `Reels::collect` and for the same reason it
    /// is there: `/state` is what the page polls anyway, and a thread that
    /// reached into the app's state would want the lock every route already
    /// holds. Purely bookkeeping — [`row`](Self::row) already answers `None`
    /// for a finished play, so nothing is ever drawn from something this has
    /// not got to yet.
    ///
    /// `alive` answers whether a device still has that layer. A device deleted
    /// mid-set leaves a play that can never draw again; keeping it would make
    /// the report name a layer that is not there, which is a lie the page would
    /// faithfully draw.
    pub fn prune(&mut self, time: f32, alive: impl Fn(&str, &str) -> bool) {
        self.running
            .retain(|p| p.at(time).is_some() && alive(&p.device, &p.layer));
        self.blackouts.retain(|b| b.is_active(time));
        self.solid_colors.retain(|c| c.is_active(time));
    }

    /// What is in the air, for `/state`.
    pub fn report(&self, time: f32) -> Value {
        let mut all: Vec<Value> = self.running.iter().map(|p| p.describe(time)).collect();
        all.extend(self.blackouts.iter().map(|b| b.describe(time)));
        all.extend(self.solid_colors.iter().map(|c| c.describe(time)));
        json!(all)
    }
}

impl Playing<'_> {
    /// Writes every playing row into the layer list about to be drawn.
    ///
    /// **Into `Layer::base`, which is the field that already means this.** Its
    /// own doc says a layer's own float row exists because a source carries
    /// *"different parameters"*, and that the dash *"does not edit it, it
    /// carries it"* — nothing in this app had ever written one. An overlay is
    /// the first writer, and it writes the same field the four read sites
    /// already consult, so no read site had to learn a second way to get a row.
    /// The list handed here is always an owned copy of the stack, so this is
    /// not editing the operator's saved layers: a play is in the air, never on
    /// disk, and closing the app ends it.
    ///
    /// Returns how many layers it touched, which is what the caller would
    /// otherwise recompute to know whether anything happened.
    pub fn apply(&self, air: &Airing, models: usize, list: &mut [Layer]) -> usize {
        // The overwhelmingly common case, and the one that has to cost
        // nothing: no overlay anywhere, on a path that runs once per layer per
        // frame per device.
        if self.overlays.running.is_empty() {
            return 0;
        }
        let mut n = 0;
        for layer in list.iter_mut() {
            let showing = layer.showing(air, models);
            if let Some(row) = self
                .overlays
                .row(self.device, &layer.id, showing, self.time)
            {
                layer.base = Some(row.to_vec());
                n += 1;
            }
            // `R22.8` point 4: a running sequence's mask cuts its layer while
            // it runs — the same `Layer.mask` the authored graph writes
            // (`R22.6`).
            layer.mask = self
                .overlays
                .mask_for(self.device, &layer.id, showing)
                .map(str::to_string);
        }
        n
    }
}

// ---------------------------------------------------------------------------
// S.O — from state to pixels
// ---------------------------------------------------------------------------
//
// Everything above this line is *state*: what is armed, on which device,
// until when. `POST /overlay` and `POST /overlay/blackout` write it; `/state`
// reports it. Nothing above turns it into a frame, and until 20-sept-2026
// nothing anywhere did — `plan-sonnet.md § S.O` point 2 names the hole. This
// section is the read side, and it follows the one rule the render path
// already has: **read live, every frame, off the same state the routes
// wrote** (rule 4). There is no "blackout flag" cached beside the frame;
// `render_device` asks these functions at the instant it composes, and a
// route that flipped the graph or armed a colour between two frames is seen
// on the very next one.
//
// Two halves, because blackout has two halves (O.28 point 5):
//
// - **The screen half is the graph.** `POST /overlay/blackout` writes
//   `bypass` onto every `output.device` node; [`screen_off`] reads it back
//   off the authored graph `compile_device_plan` already loads per frame,
//   and a bypassed screen compiles to nothing — black at zero GPU cost, the
//   escalation condition O.28 itself sets ("un nodo apagado tiene que
//   costar cero").
// - **The picture half is `Overlays`.** A timed [`Blackout`] or a
//   [`SolidColorOverlay`] is a colour that covers the whole device for a
//   while; [`Overlays::paint`] composites it over the finished frame through
//   [`compose::over`] — the same `Tile`/`blend_tile` path every layer takes,
//   not a second fill loop — so the day an overlay is a real RGBA picture
//   instead of one colour, it is one more `Tile` in the same list.
//
// The fixture half (`out.*`, `FixtureConfig::forced`) lives in
// `fixture.rs`, next to the profile that turns a signal into slots.

/// The screen half of blackout, read off the device's authored graph: `true`
/// when any `output.device` node in it is bypassed. A graph with no
/// `output.device` at all — every graph before `ensure_device_graph_at`, a
/// hand-written one — is not off, the same "nothing to be off *of*" rule
/// `switch::Switch::state` applies to an empty set.
pub fn screen_off(graph: &metal_graph::Graph) -> bool {
    graph
        .nodes
        .iter()
        .any(|n| n.kind == "output.device" && n.bypass)
}

/// Black grid lines for the test pattern: 16 × 9 cells, 1 px lines plus a
/// 3 px cross at the centre, alpha untouched.
fn paint_grid(frame: &mut [u8], w: u32, h: u32) {
    grid_hits(w, h, |x, y| {
        let i = ((y * w + x) * 4) as usize;
        frame[i..i + 3].copy_from_slice(&[0, 0, 0]);
    });
}

/// Calls `hit(x, y)` for every pixel of the test pattern's grid.
fn grid_hits(w: u32, h: u32, mut hit: impl FnMut(u32, u32)) {
    if w == 0 || h == 0 {
        return;
    }
    let on_line = |p: u32, size: u32, cells: u32| {
        let cell = (p * cells) / size;
        let next = ((p + 1) * cells) / size;
        cell != next || p == 0 || p + 1 == size || (p as i64 - (size / 2) as i64).abs() <= 1
    };
    for y in 0..h {
        let row = on_line(y, h, 9);
        for x in 0..w {
            if row || on_line(x, w, 16) {
                hit(x, y);
            }
        }
    }
}

impl Overlays {
    /// The one colour that covers `device` whole at `time`, if any: black
    /// for a timed [`Blackout`], the solid's own rgb for a
    /// [`SolidColorOverlay`]. Blackout wins when both are armed — the off
    /// switch is the one control that must never lose to a colour somebody
    /// tapped a second earlier.
    pub fn cover(&self, device: &str, time: f32) -> Option<[u8; 3]> {
        if self.is_blackout(device, time) {
            return Some([0, 0, 0]);
        }
        self.solid_color(device, time)
    }

    /// Paints whatever covers `device` at `time` over `frame` — RGBA8,
    /// `w * h * 4`, the buffer [`crate::engine::render_device`] just
    /// composed. Returns whether anything was painted, so a caller that
    /// wants to skip the masks pass or log it does not have to ask twice.
    ///
    /// Goes through [`compose::over`] with a one-pixel tile stretched to
    /// the whole device rather than a fill loop of its own, so a solid
    /// colour and a future rasterised overlay are one mechanism (see the
    /// section note). Nothing armed — the overwhelmingly common frame —
    /// costs one `Vec` lookup per overlay kind and no pixel is touched.
    pub fn paint(&self, frame: &mut [u8], w: u32, h: u32, device: &str, time: f32) -> bool {
        self.paint_masked(frame, w, h, device, time, &[])
    }

    /// [`Self::paint`], plus the per-mask colours of a solid
    /// ([`SolidColorOverlay::masks`]): after the cover, each listed mask of
    /// `masks` (the device's own list, the per-frame copy
    /// [`crate::engine::render_device`] holds) is painted its colour through
    /// the same [`compose::over`], as a one-pixel tile clipped to the mask —
    /// the CPU coverage every contour-bound source already takes, `parent`,
    /// `compose` and `alpha` included, never a mask shader.
    ///
    /// Order: shallow masks first, so where contours nest the innermost
    /// colour is the one left on top (`coverage_of`'s "the deepest mask
    /// wins"); masks at the same depth go in the device's own order. A
    /// listed id the device no longer has, or an unfinished contour, is
    /// skipped. A timed blackout covers everything: the off switch never
    /// lets a mask's colour through.
    pub fn paint_masked(
        &self,
        frame: &mut [u8],
        w: u32,
        h: u32,
        device: &str,
        time: f32,
        masks: &[Mask],
    ) -> bool {
        let Some(rgb) = self.cover(device, time) else {
            return false;
        };
        let pixel = [rgb[0], rgb[1], rgb[2], 255];
        let tile = compose::Tile {
            pixels: &pixel,
            w: 1,
            h: 1,
            sector: Sector::default(),
            opacity: 1.0,
            blend: Blend::Normal,
            turn: 0.0,
            giro_mode: ClipMode::default(),
            clip: None,
            masks: &[],
        };
        compose::over(frame, w, h, std::slice::from_ref(&tile));
        if self.is_blackout(device, time) {
            return true;
        }
        if self.solid_grid(device, time) {
            paint_grid(frame, w, h);
        }
        let listed = self.solid_masks(device, time);
        if listed.is_empty() {
            return true;
        }
        let mut order: Vec<(u8, usize, &Mask, [u8; 4])> = listed
            .iter()
            .filter_map(|(id, c)| {
                let at = masks.iter().position(|m| m.id == *id)?;
                let m = &masks[at];
                m.closed().then(|| (nesting(masks, m), at, m, [c[0], c[1], c[2], 255]))
            })
            .collect();
        order.sort_by_key(|(depth, at, _, _)| (*depth, *at));
        let tiles: Vec<compose::Tile> = order
            .iter()
            .map(|(_, _, m, px)| compose::Tile {
                pixels: px,
                w: 1,
                h: 1,
                sector: m.bounds_in(masks),
                opacity: 1.0,
                blend: Blend::Normal,
                turn: 0.0,
                giro_mode: ClipMode::default(),
                clip: Some(*m),
                masks,
            })
            .collect();
        compose::over(frame, w, h, &tiles);
        true
    }
}

impl Overlays {
    /// [`Self::paint_masked`] on a texture (G3): the cover colour, the test
    /// grid and each listed mask's colour as shade passes, so an armed overlay
    /// no longer reads the frame back. `None` when this painter cannot shade
    /// on the GPU; nothing armed returns `frame` as it is.
    pub fn paint_gpu(
        &self,
        paint: &mut dyn crate::paint::Paint,
        frame: &crate::gpu_frame::GpuFrame,
        device: &str,
        time: f32,
        masks: &[Mask],
    ) -> Option<crate::gpu_frame::GpuFrame> {
        use crate::gpu_frame::Shade;
        let Some(rgb) = self.cover(device, time) else {
            return Some(frame.clone());
        };
        let (w, h) = (frame.width(), frame.height());
        let f = |c: u8| c as f32 / 255.0;
        let mut cur = paint.shade_gpu(frame, &Shade::Tint { rgb: [f(rgb[0]), f(rgb[1]), f(rgb[2])], k: 1.0, cover: None })?;
        if self.is_blackout(device, time) {
            return Some(cur);
        }
        if self.solid_grid(device, time) {
            let mut rgba = vec![0u8; w as usize * h as usize * 4];
            grid_hits(w, h, |x, y| rgba[((y * w + x) * 4 + 3) as usize] = 255);
            cur = paint.shade_gpu(&cur, &Shade::Over { rgba: &rgba })?;
        }
        let listed = self.solid_masks(device, time);
        let mut order: Vec<(u8, usize, &Mask, [u8; 3])> = listed
            .iter()
            .filter_map(|(id, c)| {
                let at = masks.iter().position(|m| m.id == *id)?;
                let m = &masks[at];
                m.closed().then(|| (nesting(masks, m), at, m, *c))
            })
            .collect();
        order.sort_by_key(|(depth, at, _, _)| (*depth, *at));
        for (_, _, m, c) in order {
            let b = m.bounds_in(masks);
            let (_, y0, _, sh) = b.pixels(w, h);
            let key = crate::output::clip_cover_key(masks, m, w, h, y0, y0 + sh);
            let cover = paint.cover_gpu(key, w, h, &mut || crate::output::clip_cover_bytes(masks, m, w, h, y0, y0 + sh))?;
            cur = paint.shade_gpu(&cur, &Shade::Tint { rgb: [f(c[0]), f(c[1]), f(c[2])], k: 1.0, cover: Some(&cover) })?;
        }
        Some(cur)
    }
}

/// How many `parent` hops `m` sits under — the order [`Overlays::paint_masked`]
/// paints nested contours in. The same walk as `Mask::nesting` (private to
/// `output.rs`), cut at [`crate::output::MAX_MASK_DEPTH`] for the same reason:
/// a cycle in hand-edited data must end.
fn nesting(masks: &[Mask], m: &Mask) -> u8 {
    let mut depth = 0u8;
    let mut at = m;
    while let Some(pid) = at.parent.as_deref() {
        if depth >= crate::output::MAX_MASK_DEPTH {
            break;
        }
        let Some(parent) = masks.iter().find(|x| x.id == pid) else {
            break;
        };
        depth += 1;
        at = parent;
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(name: &str, ticks: usize, started: f32, looping: bool) -> Play {
        Play {
            device: "preview".into(),
            layer: "l0-0".into(),
            movement: name.into(),
            model: 3,
            ticks: (0..ticks).map(|t| vec![t as f32; 4]).collect(),
            started,
            looping,
            mask: None,
        }
    }

    /// The rate is the digest's, restated nowhere: a movement's ticks are
    /// eighths of a second because that is what they were authored as.
    #[test]
    fn a_movement_steps_one_tick_every_eighth_of_a_second() {
        let p = play("drop", 8, 10.0, false);
        assert_eq!(p.at(10.0), Some(0), "the tick it was armed on");
        assert_eq!(p.at(10.124), Some(0), "still inside the first eighth");
        assert_eq!(p.at(10.125), Some(1), "one eighth in");
        assert_eq!(p.at(10.875), Some(7), "the last of eight");
        assert_eq!(
            p.at(11.0),
            None,
            "one second in, an eight-tick phrase is over"
        );
    }

    /// A one-shot releases; holding is the opt-in. An overlay that stayed
    /// after it ended would be a state change with nothing on screen saying so.
    #[test]
    fn a_one_shot_releases_the_layer_and_a_loop_comes_back_round() {
        let once = play("drop", 4, 0.0, false);
        assert_eq!(
            once.at(0.5),
            None,
            "four ticks is half a second, and then it is done"
        );
        assert_eq!(once.at(600.0), None, "and it stays done");

        let round = play("drop", 4, 0.0, true);
        assert_eq!(round.at(0.5), Some(0), "a loop is back at the start");
        assert_eq!(round.at(0.625), Some(1));
        assert_eq!(
            round.at(600.0),
            Some(0),
            "and it is still going ten minutes later"
        );
    }

    /// The guard the module exists to state: a play answers for the model its
    /// rows were authored for, and for no other.
    ///
    /// Without this, a source the router moves to another animation mid-phrase
    /// gets a row of the wrong width, `drive::row_with` resizes it in silence,
    /// and the renderer draws whatever that turned out to be.
    #[test]
    fn a_play_goes_quiet_rather_than_feeding_the_wrong_model_a_row() {
        let mut o = Overlays::default();
        o.start(play("drop", 8, 0.0, true));
        assert!(
            o.row("preview", "l0-0", 3, 0.0).is_some(),
            "its own model draws"
        );
        assert!(
            o.row("preview", "l0-0", 4, 0.0).is_none(),
            "another model must get nothing at all, not a row of the wrong width"
        );
        // And it is not consumed by the mismatch: the router moving the source
        // away and back must not have ended the phrase.
        assert!(
            o.row("preview", "l0-0", 3, 0.5).is_some(),
            "it is still running"
        );
    }

    #[test]
    fn one_layer_holds_one_movement_and_the_second_tap_replaces_the_first() {
        let mut o = Overlays::default();
        o.start(play("first", 8, 0.0, true));
        let mut second = play("second", 8, 0.0, true);
        second.ticks = (0..8).map(|_| vec![9.0; 4]).collect();
        o.start(second);
        let row = o
            .row("preview", "l0-0", 3, 0.0)
            .expect("something is playing");
        assert_eq!(row, [9.0; 4], "the second tap won");
        assert_eq!(
            o.report(0.0).as_array().expect("a list").len(),
            1,
            "and there is one, not two"
        );
    }

    #[test]
    fn stopping_takes_off_one_layer_and_the_off_switch_takes_off_everything() {
        let mut o = Overlays::default();
        o.start(play("a", 8, 0.0, true));
        let mut other = play("b", 8, 0.0, true);
        other.layer = "l1-1".into();
        o.start(other);

        assert!(o.stop("preview", "l0-0"));
        assert!(
            !o.stop("preview", "l0-0"),
            "stopping what is not playing is false, not an error"
        );
        assert!(
            o.row("preview", "l1-1", 3, 0.0).is_some(),
            "the other one is untouched"
        );

        assert_eq!(o.stop_all(), 1);
        assert!(o.is_empty());
    }

    /// Pruning is bookkeeping and never the thing that stops a row being
    /// drawn — `row` has to be right before anything prunes, because the
    /// render path runs between polls.
    #[test]
    fn a_finished_play_draws_nothing_before_anything_prunes_it() {
        let mut o = Overlays::default();
        o.start(play("drop", 4, 0.0, false));
        assert!(
            o.row("preview", "l0-0", 3, 99.0).is_none(),
            "over is over, unpruned"
        );
        assert_eq!(
            o.report(99.0).as_array().expect("a list").len(),
            1,
            "still listed until pruned"
        );
        o.prune(99.0, |_, _| true);
        assert!(o.is_empty(), "and now it is gone");
    }

    /// A device deleted mid-set must not leave the report naming a layer that
    /// is not there any more.
    #[test]
    fn a_play_on_a_layer_that_no_longer_exists_is_pruned_even_though_it_is_running() {
        let mut o = Overlays::default();
        o.start(play("drop", 800, 0.0, true));
        o.prune(1.0, |_, _| true);
        assert!(!o.is_empty(), "a running play on a live layer stays");
        o.prune(1.0, |_, _| false);
        assert!(
            o.is_empty(),
            "a running play on a layer that is gone does not"
        );
    }

    /// The clock cannot go backwards under a play — `State::time` is monotonic
    /// across pause and resume — but a render loop is not where to find out.
    #[test]
    fn a_clock_that_went_backwards_reads_as_the_start_of_the_phrase() {
        let p = play("drop", 8, 100.0, false);
        assert_eq!(p.at(0.0), Some(0));
        assert_eq!(p.at(-50.0), Some(0));
    }

    #[test]
    fn a_movement_with_no_ticks_plays_nothing_rather_than_dividing_by_zero() {
        let mut p = play("empty", 0, 0.0, true);
        p.ticks.clear();
        assert_eq!(
            p.at(0.0),
            None,
            "a loop over nothing must not be a modulo by zero"
        );
        assert_eq!(p.at(5.0), None);
    }

    /// `apply` is the seam the generator dedup key is computed from, so it has
    /// to write the row into the field that key already reads.
    #[test]
    fn apply_writes_the_playing_row_into_that_layers_own_base_and_no_others() {
        let mut o = Overlays::default();
        let mut p = play("drop", 8, 0.0, true);
        p.model = 0;
        p.ticks = (0..8).map(|t| vec![t as f32; 4]).collect();
        o.start(p);

        let air = Airing::default();
        let mut list = vec![Layer::new("l0-0", "one", 0), Layer::new("l1-1", "two", 0)];
        assert_eq!(
            o.on("preview", 0.25).apply(&air, 1, &mut list),
            1,
            "one layer was playing"
        );
        assert_eq!(list[0].base.as_deref(), Some(&[2.0; 4][..]), "two ticks in");
        assert_eq!(
            list[1].base, None,
            "the layer with nothing playing keeps its own row"
        );

        // A different device by the same layer id is a different layer.
        let mut elsewhere = vec![Layer::new("l0-0", "one", 0)];
        assert_eq!(o.on("wall", 0.25).apply(&air, 1, &mut elsewhere), 0);
        assert_eq!(elsewhere[0].base, None);
    }

    /// `R22.8` point 4: a running sequence cuts its layer with the mask it
    /// was armed with, and only that layer — the same `Layer.mask` the
    /// authored graph writes (`R22.6`).
    #[test]
    fn a_play_with_a_mask_cuts_its_layer_while_it_runs() {
        let mut o = Overlays::default();
        let mut p = play("drop", 8, 0.0, true);
        p.model = 0;
        p.mask = Some("mask-1".to_string());
        o.start(p);

        let air = Airing::default();
        let mut list = vec![Layer::new("l0-0", "one", 0), Layer::new("l1-1", "two", 0)];
        o.on("preview", 0.25).apply(&air, 1, &mut list);
        assert_eq!(list[0].mask.as_deref(), Some("mask-1"));
        assert_eq!(list[1].mask, None, "a layer with nothing playing is uncut");

        // A play with no mask leaves the layer's own mask alone (None here).
        let mut o2 = Overlays::default();
        let mut q = play("dry", 8, 0.0, true);
        q.model = 0;
        o2.start(q);
        let mut only = vec![Layer::new("l0-0", "one", 0)];
        o2.on("preview", 0.25).apply(&air, 1, &mut only);
        assert_eq!(only[0].mask, None);
    }

    /// Nothing playing must cost nothing on a path that runs per layer per
    /// frame per device.
    #[test]
    fn an_empty_overlay_set_leaves_a_stack_exactly_as_it_found_it() {
        let o = Overlays::default();
        let air = Airing::default();
        let before = vec![Layer::new("l0-0", "one", 0)];
        let mut list = before.clone();
        assert_eq!(o.on("preview", 12.0).apply(&air, 1, &mut list), 0);
        assert_eq!(list, before);
    }

    /// Blackout overlays activate and deactivate based on time.
    #[test]
    fn blackout_overlays_activate_within_duration_and_expire_after() {
        let mut o = Overlays::default();
        o.start_blackout("preview".into(), 10.0, 2.0);

        assert!(!o.is_blackout("preview", 9.9), "blackout not active yet");
        assert!(o.is_blackout("preview", 10.0), "blackout active at start");
        assert!(o.is_blackout("preview", 11.0), "blackout active in middle");
        assert!(
            !o.is_blackout("preview", 12.1),
            "blackout expired after duration"
        );
    }

    /// Solid color overlays return the RGB value while active.
    #[test]
    fn solid_color_overlays_return_rgb_while_active() {
        let mut o = Overlays::default();
        let color = [255, 128, 0];
        o.start_solid_color("preview".into(), color, 5.0, 3.0);

        assert_eq!(o.solid_color("preview", 4.9), None, "color not active yet");
        assert_eq!(
            o.solid_color("preview", 5.0),
            Some(color),
            "color active at start"
        );
        assert_eq!(
            o.solid_color("preview", 7.0),
            Some(color),
            "color active in middle"
        );
        assert_eq!(
            o.solid_color("preview", 8.1),
            None,
            "color expired after duration"
        );
    }

    /// Blackout and solid color overlays can be stopped before expiring.
    #[test]
    fn blackout_and_solid_color_can_be_stopped_before_expiring() {
        let mut o = Overlays::default();
        o.start_blackout("preview".into(), 10.0, 100.0);
        o.start_solid_color("wall".into(), [255, 0, 0], 10.0, 100.0);

        assert!(o.stop_blackout("preview"));
        assert!(o.stop_solid_color("wall"));

        assert!(!o.is_blackout("preview", 50.0), "blackout stopped");
        assert_eq!(o.solid_color("wall", 50.0), None, "solid color stopped");
    }

    /// The report includes all active overlays: movements, blackouts, and solid colors.
    #[test]
    fn report_includes_all_active_overlay_types() {
        let mut o = Overlays::default();
        o.start(play("drop", 8, 0.0, true));
        o.start_blackout("preview".into(), 0.0, 5.0);
        o.start_solid_color("preview".into(), [100, 100, 100], 0.0, 5.0);

        let report = o.report(1.0);
        let arr = report.as_array().expect("report is an array");
        assert_eq!(arr.len(), 3, "report has movement, blackout, and color");

        // A movement entry is named by `movement` and carries no `type` tag —
        // only `blackout` and `solid_color` are tagged, because those two are
        // the ones a reader could not otherwise tell apart. (This line used to
        // read `"movement" | json!(null)`, which is a thought rather than an
        // expression and did not compile.)
        assert_eq!(
            arr[0].get("movement"),
            Some(&json!("drop")),
            "movement reported"
        );
        assert!(
            arr[0].get("type").is_none(),
            "a movement entry carries no type tag"
        );
        assert_eq!(
            arr[1].get("type"),
            Some(&json!("blackout")),
            "blackout reported"
        );
        assert_eq!(
            arr[2].get("type"),
            Some(&json!("solid_color")),
            "color reported"
        );
    }
}

/// `S.O`: the read side — state to pixels, through the real render path.
#[cfg(test)]
mod paint_tests {
    use super::*;
    use crate::engine::{fan_out_stack, render_device, State, PREVIEW};
    use crate::layers::{Composition, DeviceMode};
    use crate::live::Input;
    use crate::paint;

    /// Every pixel of an RGBA8 frame is `rgb`, alpha untouched.
    fn all(frame: &[u8], rgb: [u8; 3]) -> bool {
        frame.chunks_exact(4).all(|p| p[..3] == rgb)
    }

    fn preview_stack() -> (Box<dyn paint::Paint>, crate::listeners::Listeners, State) {
        let paint = paint::painter();
        let sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.layers = fan_out_stack();
        state.preview.mode = DeviceMode::Own;
        state.preview.composition = Composition::Overlap;
        state.paused_at = Some(3.0);
        (paint, sim, state)
    }

    /// The compositor half on its own: a cover is painted over whatever was
    /// in the frame, edge to edge; nothing armed leaves the bytes alone.
    #[test]
    fn paint_covers_the_whole_frame_and_nothing_armed_touches_nothing() {
        let mut o = Overlays::default();
        let mut frame = vec![7u8; 8 * 4 * 4];
        assert!(!o.paint(&mut frame, 8, 4, "preview", 1.0));
        assert!(frame.iter().all(|b| *b == 7), "untouched");

        o.start_solid_color("preview".into(), [255, 128, 0], 0.0, 10.0);
        assert!(o.paint(&mut frame, 8, 4, "preview", 1.0));
        assert!(all(&frame, [255, 128, 0]));
        assert!(frame.chunks_exact(4).all(|p| p[3] == 7), "alpha is the frame's own");

        let mut other = vec![7u8; 8 * 4 * 4];
        assert!(!o.paint(&mut other, 8, 4, "wall", 1.0), "another device is not covered");
    }

    /// A closed rectangle as a device mask, `0..1` device space.
    fn rect(id: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> Mask {
        Mask {
            id: id.into(),
            name: id.into(),
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            ..Mask::default()
        }
    }

    /// The RGB of pixel `(x, y)` of a `w`-wide RGBA8 frame.
    fn at(frame: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * w + x) * 4) as usize;
        [frame[i], frame[i + 1], frame[i + 2]]
    }

    /// Venue test colours: two contours, two colours. Every pixel inside a
    /// listed mask is that mask's colour, a mask nobody listed and the rest
    /// of the frame keep the solid's own colour.
    #[test]
    fn a_solid_with_a_colour_per_mask_paints_each_mask_its_own_colour() {
        let masks = vec![
            rect("left", 0.1, 0.2, 0.4, 0.8),
            rect("right", 0.6, 0.2, 0.9, 0.8),
            rect("unlisted", 0.45, 0.0, 0.55, 0.1),
        ];
        let mut o = Overlays::default();
        o.start_solid_masks(
            "wall".into(),
            [0, 0, 0],
            vec![("left".into(), [255, 120, 40]), ("right".into(), [40, 140, 255])],
            0.0,
            10.0,
        );
        let (w, h) = (40, 20);
        let mut frame = vec![7u8; (w * h * 4) as usize];
        assert!(o.paint_masked(&mut frame, w, h, "wall", 1.0, &masks));

        let (mut left, mut right, mut rest) = (0, 0, 0);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let want = if masks[0].contains(&masks, fx, fy) {
                    left += 1;
                    [255, 120, 40]
                } else if masks[1].contains(&masks, fx, fy) {
                    right += 1;
                    [40, 140, 255]
                } else {
                    rest += 1;
                    [0, 0, 0]
                };
                assert_eq!(at(&frame, w, x, y), want, "pixel ({x}, {y})");
            }
        }
        assert!(left > 50 && right > 50 && rest > 50, "{left} / {right} / {rest}");
        assert!(frame.chunks_exact(4).all(|p| p[3] == 7), "alpha is the frame's own");
        assert_eq!(
            o.report(1.0)[0]["masks"],
            json!({ "left": [255, 120, 40], "right": [40, 140, 255] })
        );
    }

    /// Nested contours: the inner mask's colour wins inside it whatever
    /// order the device lists them in — `coverage_of`'s deepest-wins rule.
    #[test]
    fn a_nested_mask_keeps_its_own_colour_inside_its_parent() {
        let mut inner = rect("inner", 0.4, 0.4, 0.6, 0.6);
        inner.parent = Some("outer".into());
        let masks = vec![inner, rect("outer", 0.2, 0.2, 0.8, 0.8)];
        let mut o = Overlays::default();
        o.start_solid_masks(
            "wall".into(),
            [0, 0, 0],
            vec![("outer".into(), [0, 255, 0]), ("inner".into(), [255, 0, 0])],
            0.0,
            10.0,
        );
        let mut frame = vec![0u8; 20 * 20 * 4];
        o.paint_masked(&mut frame, 20, 20, "wall", 1.0, &masks);
        assert_eq!(at(&frame, 20, 10, 10), [255, 0, 0], "the inner mask");
        assert_eq!(at(&frame, 20, 5, 5), [0, 255, 0], "the outer ring");
        assert_eq!(at(&frame, 20, 1, 1), [0, 0, 0], "outside both");
    }

    /// The one-colour body is byte-identical: with no per-mask colours (and
    /// with per-mask colours naming no mask the device has) the masked
    /// painter writes exactly what the old one-colour painter wrote, masks
    /// or no masks.
    #[test]
    fn a_one_colour_solid_paints_the_same_bytes_as_before() {
        let masks = vec![rect("left", 0.1, 0.2, 0.4, 0.8)];
        let base: Vec<u8> = (0..16 * 8 * 4).map(|i| (i * 7 % 251) as u8).collect();

        let mut o = Overlays::default();
        o.start_solid_color("wall".into(), [12, 200, 99], 0.0, 10.0);
        let mut old = base.clone();
        o.paint(&mut old, 16, 8, "wall", 1.0);
        let mut new = base.clone();
        o.paint_masked(&mut new, 16, 8, "wall", 1.0, &masks);
        assert_eq!(new, old);
        let mut want = base.clone();
        for p in want.chunks_exact_mut(4) {
            p[..3].copy_from_slice(&[12, 200, 99]);
        }
        assert_eq!(old, want, "the whole frame, alpha untouched");
        assert!(o.report(1.0)[0].get("masks").is_none(), "and the report is unchanged");

        let mut gone = Overlays::default();
        gone.start_solid_masks("wall".into(), [12, 200, 99], vec![("deleted".into(), [1, 2, 3])], 0.0, 10.0);
        let mut still = base.clone();
        gone.paint_masked(&mut still, 16, 8, "wall", 1.0, &masks);
        assert_eq!(still, old, "a listed mask the device no longer has paints nothing");
    }

    /// A timed blackout over a per-mask solid is black everywhere.
    #[test]
    fn a_blackout_hides_the_mask_colours_too() {
        let masks = vec![rect("left", 0.0, 0.0, 1.0, 1.0)];
        let mut o = Overlays::default();
        o.start_solid_masks("wall".into(), [9, 9, 9], vec![("left".into(), [255, 0, 0])], 0.0, 10.0);
        o.start_blackout("wall".into(), 0.0, 10.0);
        let mut frame = vec![200u8; 8 * 8 * 4];
        o.paint_masked(&mut frame, 8, 8, "wall", 1.0, &masks);
        assert!(all(&frame, [0, 0, 0]));
    }

    /// A solid colour armed on a device reaches the pixels `render_device`
    /// answers with — the gap `plan-sonnet.md § S.O` point 2 named.
    #[test]
    fn a_solid_color_overlay_renders_as_that_color() {
        let (mut paint, mut sim, mut state) = preview_stack();
        let (before, _, _) = render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 64, 36);
        assert!(!all(&before, [0, 200, 30]), "the stack draws something else first");

        let t = state.time();
        state.overlays.start_solid_color(PREVIEW.into(), [0, 200, 30], t, 5.0);
        let (solid, _, _) = render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 64, 36);
        assert!(all(&solid, [0, 200, 30]), "every pixel is the solid");

        // Expired — the clock is what ends it, and the very next frame
        // draws the stack again (rule 4: read live, no cached flag).
        state.paused_at = Some(t + 6.0);
        let (after, _, _) = render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 64, 36);
        assert!(!all(&after, [0, 200, 30]), "over is over");
    }

    /// A timed blackout is black, and wins over a solid armed on the same
    /// device.
    #[test]
    fn a_timed_blackout_renders_black_and_beats_a_solid() {
        let (mut paint, mut sim, mut state) = preview_stack();
        let t = state.time();
        state.overlays.start_solid_color(PREVIEW.into(), [255, 0, 0], t, 5.0);
        state.overlays.start_blackout(PREVIEW.into(), t, 5.0);
        let (frame, _, _) = render_device(&mut *paint, &mut sim, &mut state, PREVIEW, 64, 36);
        assert!(all(&frame, [0, 0, 0]));
    }

    /// The screen half of `POST /overlay/blackout`: a bypassed
    /// `output.device` in the device's authored graph is black on the next
    /// frame, and un-bypassing it in the file is seen on the one after —
    /// nothing is cached beside the frame. Writes the device's graph under the
    /// repo's own `data/graphs/` (gitignored), like `engine.rs`'s own
    /// graph-composed test, because `output::data_root()` is a `OnceLock`.
    #[test]
    fn a_bypassed_output_device_in_the_graph_renders_black() {
        let device = format!("blackout-probe-{}", std::process::id());
        let root = crate::output::data_root();
        let mut paint = paint::painter();
        let mut sim = crate::listeners::Listeners::stand_in();
        let mut state = State::new(paint.as_ref());
        state.paused_at = Some(3.0);
        let mut a = Layer::new("layer-a", "a", 0);
        a.outputs = vec![device.clone()];
        state.layers = vec![a];
        let mut dev = crate::layers::Device::default();
        dev.mode = DeviceMode::Own;
        state.sources.insert(device.clone(), dev);

        let mut graph = metal_graph::Graph::new(
            &crate::engine::device_graph_id(&device),
            &device,
            metal_graph::Domain::Live,
        );
        let out = graph.add_node("out", "output.device", [0.0, 0.0]);
        crate::engine::save_graph_at(&root, &graph).unwrap();
        let (lit, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
        assert!(lit.iter().any(|b| *b != 0), "the stack draws");

        graph.node_mut(&out).unwrap().bypass = true;
        crate::engine::save_graph_at(&root, &graph).unwrap();
        assert!(screen_off(&graph));
        let (dark, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
        assert!(all(&dark, [0, 0, 0]), "bypassed screen: black");

        graph.node_mut(&out).unwrap().bypass = false;
        crate::engine::save_graph_at(&root, &graph).unwrap();
        let (back, _, _) = render_device(&mut *paint, &mut sim, &mut state, &device, 64, 36);
        assert_eq!(back, lit, "released: the same frame as before");

        let _ = metal_vfs::remove_file(crate::engine::graph_path_at(
            &root,
            &crate::engine::device_graph_id(&device),
        ));
    }
}
