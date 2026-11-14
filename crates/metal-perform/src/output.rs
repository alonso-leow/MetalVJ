//! Outputs and masks — where the picture actually goes, and what shape it is
//! when it gets there.
//!
//! > *"New configuration for the output: Mask. To outline objects for
//! > videomapping. Enable multioutput for different output devices even when they
//! > have different resolutions, or several masks within the same device but always
//! > with the same origin, not multi-origin. To configure the mask, project a grid
//! > onto the whole device and outline it by placing key points."*
//! > — 2026-08-17
//!
//! # One origin, many outputs — and what 2026-08-19 changed about that
//!
//! The sentence that decided the original design was *"always with the same
//! origin, not multi-origin"*: there was **one** picture — whatever the router
//! had on air — and an output was a *view* of it. Nothing here could select a
//! model, and every mask on every device showed the same instant of the same
//! animation.
//!
//! **Half of that is now superseded.** The multi-source redesign gives a device
//! a stack of [`crate::layers::Layer`]s, each its own animation, so a device
//! does have a picture of its own. What survived — and it is the half that was
//! actually load-bearing — is **one digest**: the music is analysed once, and
//! every layer on every device reads the same sixteen columns and the same
//! clock. `layers.rs` has the full argument and the quotes it rests on.
//!
//! What did not change at all is everything below this line. A mask is still a
//! polygon in the device's own normalised space, contours are still drawn
//! against a projected grid, and masks are still applied to the finished frame
//! — now the *composed* frame rather than the single-model one. That ordering
//! is the reason the two features do not collide: **layers make a picture,
//! contours cut it.** Contour the pillar once and every stack you ever put on
//! that device lands on the pillar.
//!
//! # 2026-08-24: a contour can belong to one source
//!
//! > *"It is important to allow different sources on the same output device,
//! > to frame objects of the real world."*
//!
//! The paragraph above is still true and is still what runs last, but it was not
//! enough. "Contour the pillar once and every stack lands on the pillar" also
//! means: contour *two* pillars and both of them show the same animation, cut in
//! two places. For a room with a pillar, a doorway and a bar front, that is one
//! picture wearing three shapes.
//!
//! So a [`Mask`] now has an [`id`](Mask::id), and a
//! [`Layer`](crate::layers::Layer) may name one. A source that names a contour
//! is rendered at that contour's bounding box and clipped to the polygon while
//! it is composited — before the device-wide cut, not instead of it. Nothing
//! about the cut changed, and a mapping with no bindings in it draws exactly the
//! frame it drew yesterday.
//!
//! The id is what made this a change to this file rather than to `layers.rs`
//! alone: a contour used to be identified by its position in a vector, and a
//! source bound to "the second mask" would silently move to a different pillar
//! the moment an earlier one was deleted.
//!
//! # A mask is a polygon in the device's own normalised space
//!
//! Key points in 0..1 of the device, not in pixels. A projector swapped for one
//! with a different native resolution keeps its mapping, which is the whole
//! reason the ask mentions different resolutions in the same breath as
//! multioutput. Outside every mask is black; with no masks at all the output is
//! the plain full-frame picture, because a screen is a mask that happens to be
//! the whole rectangle and making people draw one would be ceremony.
//!
//! # Calibrating
//!
//! *"Project a grid onto the whole device and outline it by placing key points."*
//! The grid is drawn **into the frame**, server-side, and not as
//! an overlay in the operator's browser — the grid has to land on the physical
//! surface through the actual projector, at the actual keystone, or it is a
//! grid over a picture of the room rather than over the room. The browser draws
//! the same points a second time on its own copy so they can be dragged; the
//! projector is what they are read against.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Stable id of a [`Mask`] within one device's `masks` list — what a
/// `parent` or a boolean op's `a`/`b` names.
pub type MaskId = String;

/// The three set operations "Venn diagram" asks for
/// (the design notes H.10, decided 2026-09-05: *"`Mask` needs a
/// boolean operation between shapes (`union`/`intersect`/`subtract`)"*).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BoolOp {
    Union,
    Intersect,
    Subtract,
}

/// "This mask = A ∪ B" (or ∩ / −), naming two other masks in the same
/// device's `masks` list by id. When a mask has a `compose`, its own
/// `points` are ignored — the shape comes entirely from `a` and `b`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Compose {
    pub op: BoolOp,
    pub a: MaskId,
    pub b: MaskId,
}

fn default_alpha() -> f32 {
    1.0
}

/// An outline drawn around a mask's own contour (22-sept-2026, tarde): the
/// ask's *"a mask to have a outline with a material and the main content
/// inside"*. `width` is the band's thickness in **device pixels** (0 = no
/// stroke — the mask paints exactly as before, byte for byte); `color` is the
/// RGBA the band is filled with. `source` names a material layer to sample
/// instead of a flat fill; it is the `stroke` input of the `mask` node in the
/// graph, and when it is set `color` is ignored.
///
/// The band is the difference of two coverages — `cover(polygon) −
/// cover(polygon eroded by width)` — exactly the *"outline"* of a shape, so
/// the mask reads as interior (the `image` content) inside a border (the
/// stroke), never as a double fill.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MaskStroke {
    /// Thickness in device pixels. `0.0` (the default) is no stroke.
    #[serde(default)]
    pub width: f32,
    /// The band's flat colour, RGBA in 0..1. `None` means fully opaque white;
    /// ignored when `source` is set.
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    /// A material layer to sample for the band instead of a flat colour —
    /// the `stroke` input of the `mask` node. `None` uses `color`.
    #[serde(default)]
    pub source: Option<String>,
}

/// The widest mask stroke the renderer paints, in device pixels. Above this
/// the eroded polygon would invert on a small device (the inward offset
/// swallows the shape), so [`apply_mask_strokes`] clamps to it and the
/// editor's "grosor" `DragValue` pins its range to the same number rather
/// than inventing a second bound.
pub const MASK_STROKE_MAX: f32 = 64.0;

/// One rendered **material** for a stroke band — the pixels of whatever chain
/// is wired to a `mask` node's `stroke` port (23-sept-2026, addendum D7).
///
/// Borrowed rather than owned because it comes straight out of
/// [`crate::paint::RunOutput::materials`], which lives for the call that
/// paints the frame; copying a 1080p tile just to hand it three fields
/// further down would be the one readback this whole path is avoiding.
///
/// **RGBA8, `width * height * 4`**, the same shape every painter returns —
/// the stroke samples it, it does not draw it as a layer.
pub struct StrokeMaterial<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
}

impl<'a> StrokeMaterial<'a> {
    /// The RGBA of the material at device pixel `(x, y)`, the material mapped
    /// to the mask's own [`bounds_in`](Mask::bounds_in) rectangle — nearest
    /// neighbour, the same choice `compose.rs` makes for a shrunken tile.
    ///
    /// Mapped to the bounds rather than sampled one-to-one at the device
    /// coordinates: the material is a picture *of the object's outline*, and a
    /// contour in the corner of a 1080p projection should show the whole
    /// animation along its border, not whichever ninth of it happens to fall
    /// over that corner.
    fn at(&self, bounds: crate::layers::Sector, x: u32, y: u32, w: u32, h: u32) -> [f32; 4] {
        if self.width == 0 || self.height == 0 || self.pixels.len() < 4 {
            return [1.0, 1.0, 1.0, 1.0];
        }
        let (bx, by, bw, bh) = bounds.pixels(w, h);
        let u = if bw == 0 { 0.0 } else { (x.saturating_sub(bx)) as f32 / bw as f32 };
        let v = if bh == 0 { 0.0 } else { (y.saturating_sub(by)) as f32 / bh as f32 };
        let sx = ((u * self.width as f32) as u32).min(self.width - 1);
        let sy = ((v * self.height as f32) as u32).min(self.height - 1);
        let p = ((sy * self.width + sx) as usize) * 4;
        [
            self.pixels[p] as f32 / 255.0,
            self.pixels[p + 1] as f32 / 255.0,
            self.pixels[p + 2] as f32 / 255.0,
            self.pixels[p + 3] as f32 / 255.0,
        ]
    }
}

/// One contour on one device. Points are in 0..1 of the device, `(0,0)` top
/// left.
///
/// # Nesting and booleans (H.5b / H.10, decided 2026-09-05)
///
/// Two asks — nested masks for a single object's stroke/shadow/highlight,
/// and Venn-diagram fusion between two layers — turned out to be the same
/// requirement seen twice, so they share this one design rather than two
/// parallel mask structures (see the design notes H.10):
///
/// - `parent`: this mask's region is only ever "inside `parent`, too". A
///   mask with no `parent` is unconstrained, exactly like today.
/// - `alpha`: how opaque the region reads once combined with what it sits
///   on. Defaults to `1.0` — fully opaque — which is today's implicit
///   behaviour for a plain calibration polygon.
/// - `compose`: when set, this mask's shape is a boolean combination of two
///   *other* masks (by id) instead of its own `points`. `None` — the
///   default — is today's plain polygon.
///
/// `points` deliberately stays a bare field rather than folding into a
/// `MaskShape` enum: `data/outputs.json` and every existing caller
/// (`compose.rs`'s per-source contours and `main.rs`'s mask-editing route
/// and its native shell controls) read and write `points` directly,
/// and none of that has to change for a mask that is neither nested nor
/// composed. `compose` sits next to `points` instead, as the thing that
/// *overrides* it when present — the least disruptive shape for a feature
/// that, per H.5b/H.10, is additive over the calibration case, not a
/// replacement for it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Mask {
    /// Stable within a device, and the only thing a source ever holds.
    ///
    /// `#[serde(default)]` because `data/outputs.json` on a real machine
    /// predates it; [`Outputs::load`] fills the blanks in on the way through, so
    /// nothing below here ever sees an empty one.
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// The key points, in order around the contour. Under three points is not
    /// a shape and is treated as "not finished yet" rather than as an error —
    /// an operator placing the first two points has not made a mistake.
    ///
    /// Ignored when `compose` is `Some` — see [`Mask::compose`].
    #[serde(default)]
    pub points: Vec<[f32; 2]>,
    /// Another mask in the same device that this one's region is clipped to.
    /// `None` (the default) is unconstrained, matching every mask that
    /// existed before H.5b.
    #[serde(default)]
    pub parent: Option<MaskId>,
    /// Opacity of this mask's region once composited. `1.0` — fully
    /// opaque — is the default and matches the behaviour of every mask
    /// that existed before H.5b.
    #[serde(default = "default_alpha")]
    pub alpha: f32,
    /// When set, this mask's shape is `a <op> b` of two other masks in the
    /// same list, and `points` is ignored. `None` (the default) is today's
    /// plain polygon.
    #[serde(default)]
    pub compose: Option<Compose>,
    /// An outline drawn around this mask's contour (22-sept-2026, tarde).
    /// `None` (the default) is every mask that existed before this feature,
    /// and is byte-identical to it.
    #[serde(default)]
    pub stroke: Option<MaskStroke>,
}

impl Default for Mask {
    fn default() -> Self {
        Mask {
            id: String::new(),
            name: String::new(),
            points: Vec::new(),
            parent: None,
            alpha: default_alpha(),
            compose: None,
            stroke: None,
        }
    }
}

impl Mask {
    pub fn closed(&self) -> bool {
        self.compose.is_some() || self.points.len() >= 3
    }

    /// Whether `(x, y)` — in the same 0..1 device space as `points` — falls
    /// inside this mask's composed shape, resolving `parent` and `compose`
    /// against the rest of `masks` (typically an `Output`'s `masks` list).
    ///
    /// This is the point-wise reference for the geometry. The render path
    /// does not call it per pixel — see [`Scanline::fill`], which is the same
    /// algorithm over a row that has already been walked — but the two are
    /// tested against each other, so this stays the definition of what a
    /// composed mask *means*.
    pub fn contains(&self, masks: &[Mask], x: f32, y: f32) -> bool {
        self.contains_at(masks, x, y, 0)
    }

    fn contains_at(&self, masks: &[Mask], x: f32, y: f32, depth: u8) -> bool {
        // A cycle in `parent`/`compose` (bad data, never produced by the
        // editor) must not blow the stack — past this depth, treat the
        // reference as absent rather than recursing forever.
        if depth > MAX_MASK_DEPTH {
            return false;
        }
        let own = match &self.compose {
            Some(Compose { op, a, b }) => {
                let a = find_mask(masks, a)
                    .map(|m| m.contains_at(masks, x, y, depth + 1))
                    .unwrap_or(false);
                let b = find_mask(masks, b)
                    .map(|m| m.contains_at(masks, x, y, depth + 1))
                    .unwrap_or(false);
                match op {
                    BoolOp::Union => a || b,
                    BoolOp::Intersect => a && b,
                    BoolOp::Subtract => a && !b,
                }
            }
            None => point_in_polygon(&self.points, x, y),
        };
        match &self.parent {
            // A parent id that does not resolve is treated as absent —
            // matching how `points`/`compose` degrade to "not finished yet"
            // rather than making the mask disappear on bad data.
            Some(pid) => {
                own && find_mask(masks, pid)
                    .map(|m| m.contains_at(masks, x, y, depth + 1))
                    .unwrap_or(true)
            }
            None => own,
        }
    }

    /// [`Mask::contains`], modulated by [`Mask::alpha`]: `alpha` where the
    /// point is inside the composed shape, `0.0` outside it.
    pub fn coverage(&self, masks: &[Mask], x: f32, y: f32) -> f32 {
        if self.contains(masks, x, y) {
            self.alpha
        } else {
            0.0
        }
    }

    /// How many `parent` hops this mask sits under, saturating at
    /// [`MAX_MASK_DEPTH`] so cyclic data terminates here for the same reason
    /// [`Mask::contains_at`] does.
    ///
    /// This is what makes an inner mask *win* over the one it nests in — see
    /// [`coverage_of`].
    fn nesting(&self, masks: &[Mask]) -> u8 {
        let mut depth = 0u8;
        let mut at = self;
        while let Some(pid) = at.parent.as_deref() {
            if depth >= MAX_MASK_DEPTH {
                break;
            }
            let Some(parent) = find_mask(masks, pid) else {
                break;
            };
            depth += 1;
            at = parent;
        }
        depth
    }
}

/// A cycle in `parent`/`compose` is bad data — never produced by the editor —
/// and every walk over those two fields stops here rather than recursing
/// forever. Shared by [`Mask::contains_at`], [`Mask::nesting`] and
/// [`Scanline::fill`]'s resolver so the three cannot disagree about where a
/// cycle is cut.
pub const MAX_MASK_DEPTH: u8 = 32;

/// The masks on a device that are **objects being lit**, as opposed to
/// building blocks — the list [`apply_masks`] unions.
///
/// A mask named as `a` or `b` in some other mask's [`Compose`] is a building
/// block and drops out. Without that rule the Venn case of H.10 could not
/// work at all: `C = A ∩ B` unioned back together with `A` and `B` is just
/// `A ∪ B`, and the same goes for `subtract`. An operand is how a shape is
/// *described*, not a thing on the wall.
///
/// `parent` deliberately does **not** drop a mask out. Nesting is a
/// constraint on the child ("only ever inside this, too"), not a definition
/// of it, and H.5b's whole case is the outer one defining a silhouette while
/// the inner adds or subtracts within it — so both are lit.
///
/// With no `compose` anywhere in the list — every mapping that exists today —
/// nothing is an operand and this is every closed mask, exactly as before.
pub fn lit_masks(masks: &[Mask]) -> Vec<&Mask> {
    let operand = |id: &str| {
        masks
            .iter()
            .any(|m| m.compose.as_ref().is_some_and(|c| c.a == id || c.b == id))
    };
    masks
        .iter()
        .filter(|m| m.closed() && !operand(&m.id))
        .collect()
}

/// What [`apply_masks`] leaves of a pixel at `(x, y)`: `0.0` blacks it out,
/// `1.0` leaves it exactly as it was.
///
/// **The deepest containing mask wins**, and ties at the same nesting depth
/// take the larger `alpha`. That rule is what H.5b's *"the outer one defines the
/// silhouette, the inner one is subtracted or added with alpha"* actually asks for: a
/// child at `alpha = 0.4` inside an opaque parent has to read at `0.4` in its
/// own region, and a child at `0.0` has to punch the hole that makes a
/// shadow. Plain max over the whole list would let the parent win both times
/// and neither would be expressible.
///
/// Ties by max is the pre-existing rule, unchanged: *"several masks within
/// the same device"* is several objects being lit, so they union.
/// With no nesting and no alpha — every mapping that exists today — every
/// mask is at depth 0 with `alpha = 1.0`, so this is `1.0` inside the union
/// and `0.0` outside it: the old black-out, arithmetic unchanged.
pub fn coverage_of(masks: &[Mask], x: f32, y: f32) -> f32 {
    let mut best: Option<(u8, f32)> = None;
    for m in lit_masks(masks) {
        if !m.contains(masks, x, y) {
            continue;
        }
        let here = (m.nesting(masks), m.alpha);
        if best.is_none_or(|(d, a)| here.0 > d || (here.0 == d && here.1 > a)) {
            best = Some(here);
        }
    }
    best.map(|(_, a)| a).unwrap_or(0.0)
}

impl Mask {
    /// The smallest rectangle that holds the contour.
    ///
    /// This is what a source bound to it gets rendered at: asking a model for
    /// exactly the pixels the polygon can use is the same argument
    /// `compose.rs` makes for sectors, and it is worth more here — a doorway
    /// contour in the corner of a 1080p projection is a tenth of the frame, and
    /// rendering the whole frame to throw nine tenths of it away is nine tenths
    /// of a GPU pass per source per frame.
    ///
    /// An unfinished contour has no area, so it reports the whole device rather
    /// than a degenerate box: the source stays visible while its shape is being
    /// drawn, which is the same call [`Mask::closed`] makes for the same reason.
    pub fn bounds(&self) -> crate::layers::Sector {
        if !self.closed() || self.points.is_empty() {
            return crate::layers::Sector::default();
        }
        let (mut x0, mut y0) = (f32::INFINITY, f32::INFINITY);
        let (mut x1, mut y1) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for p in &self.points {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        crate::layers::Sector {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
        .clamped()
    }

    /// [`Mask::bounds`], resolved through `compose` and `parent` against the
    /// rest of `masks` — the rectangle a source bound to *this* mask should
    /// actually be rendered at.
    ///
    /// A composed mask has no `points` of its own, so [`Mask::bounds`] alone
    /// reports the whole device for it. That is not just imprecise, it is the
    /// cost this file already refuses to pay elsewhere: a contour in the
    /// corner of a 1080p projection asking for a full frame is nine tenths of
    /// a GPU pass per source per frame thrown away.
    ///
    /// The three operations bound differently, and each is the tightest
    /// rectangle that is still guaranteed to hold the shape:
    ///
    /// - `union`: the box around both operands.
    /// - `intersect`: the overlap of the two boxes — empty if they do not
    ///   touch, which is correct, the intersection really is empty there.
    /// - `subtract`: `a`'s box. Cutting `b` out of `a` can only shrink it,
    ///   never move it outside, and computing where would need the polygons.
    ///
    /// `parent` then clips whatever came out, for the same reason: a nested
    /// mask can never reach past the one it nests in.
    pub fn bounds_in(&self, masks: &[Mask]) -> crate::layers::Sector {
        self.bounds_at(masks, 0)
    }

    fn bounds_at(&self, masks: &[Mask], depth: u8) -> crate::layers::Sector {
        if depth > MAX_MASK_DEPTH {
            return crate::layers::Sector::default();
        }
        let own = match &self.compose {
            Some(Compose { op, a, b }) => {
                // A dangling operand degrades the same way it does in
                // `contains_at`: it contributes nothing, so it bounds
                // nothing.
                let a = find_mask(masks, a).map(|m| m.bounds_at(masks, depth + 1));
                let b = find_mask(masks, b).map(|m| m.bounds_at(masks, depth + 1));
                match (op, a, b) {
                    (BoolOp::Union, Some(a), Some(b)) => union_box(a, b),
                    (BoolOp::Union, Some(only), None) | (BoolOp::Union, None, Some(only)) => only,
                    (BoolOp::Intersect, Some(a), Some(b)) => intersect_box(a, b),
                    (BoolOp::Subtract, Some(a), _) => a,
                    // Every operand dangling: the shape is empty, so ask for
                    // as little as this module lets anything ask for.
                    _ => crate::layers::Sector {
                        x: 0.0,
                        y: 0.0,
                        w: 0.0,
                        h: 0.0,
                    }
                    .clamped(),
                }
            }
            None => self.bounds(),
        };
        match self.parent.as_deref().and_then(|pid| find_mask(masks, pid)) {
            Some(p) => intersect_box(own, p.bounds_at(masks, depth + 1)),
            None => own,
        }
    }
}

fn union_box(a: crate::layers::Sector, b: crate::layers::Sector) -> crate::layers::Sector {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    let (x1, y1) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
    crate::layers::Sector {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
    .clamped()
}

/// The overlap of two boxes. Two shapes that do not touch collapse to the
/// smallest box [`crate::layers::Sector::clamped`] allows rather than to the
/// whole device: their intersection really is empty, and reporting a full
/// frame for it would be the opposite of the truth. The floor is that
/// method's, not this one's — a zero-width sector reads like a broken
/// renderer, which is the call `layers.rs` already made.
fn intersect_box(a: crate::layers::Sector, b: crate::layers::Sector) -> crate::layers::Sector {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    crate::layers::Sector {
        x: x0,
        y: y0,
        w: (x1 - x0).max(0.0),
        h: (y1 - y0).max(0.0),
    }
    .clamped()
}

fn find_mask<'a>(masks: &'a [Mask], id: &str) -> Option<&'a Mask> {
    masks.iter().find(|m| m.id == id)
}

/// Standard even-odd ray cast. Fewer than three points is "not finished
/// yet" (see [`Mask::closed`]), not a shape, so it never contains a point.
fn point_in_polygon(points: &[[f32; 2]], x: f32, y: f32) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut inside = false;
    let n = points.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (points[i][0], points[i][1]);
        let (xj, yj) = (points[j][0], points[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// One physical destination.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Output {
    /// The display id `devices::displays` reports, or any name the operator
    /// gives a screen that is not attached yet.
    pub id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub masks: Vec<Mask>,
    /// Whether this device is currently showing the calibration grid. Per
    /// device: calibrating the projector on the back wall must not put a grid
    /// over the one above the bar.
    #[serde(default)]
    pub calibrating: bool,
    /// The machine that shows this screen, by device name — `None` is the
    /// local machine, so a mapping saved before S.R still projects where it
    /// always did.
    #[serde(default)]
    pub node: Option<String>,
    /// The machine that composes this screen's frame, by device name —
    /// `None` defers to whichever machine shows it ([`Output::node`]).
    #[serde(default)]
    pub composed_on: Option<String>,
    /// Who this screen has been handed to — another program on the same
    /// machine, by a free-text label, not a machine name like
    /// `node`/`composed_on`. `None` means MetalVJ still owns the pixels;
    /// `Some` cuts both the local render (black) and `GET /frame` (409).
    #[serde(default)]
    pub ceded_to: Option<String>,
    /// P-C: another program this screen's finished frame is also handed to
    /// — a virtual camera, an NDI/Spout/Syphon sender. A view of the frame
    /// (rule 3): the sink is fed exactly what `GET /output.jpg` would show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sink: Option<OutputSink>,
}

/// P-C: where a screen's frame also goes. `kind` is one of
/// `metal_videoio::KINDS`; `target` is `/dev/videoN[:rgb24]` for
/// `virtualcam`, the published name for the others.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputSink {
    pub kind: String,
    #[serde(default)]
    pub target: String,
}

impl Output {
    pub fn new(id: &str, label: &str, width: u32, height: u32) -> Output {
        Output {
            id: id.to_string(),
            label: label.to_string(),
            width: width.max(1),
            height: height.max(1),
            masks: Vec::new(),
            calibrating: false,
            node: None,
            composed_on: None,
            ceded_to: None,
            sink: None,
        }
    }

    /// Which machine renders this screen's frame: `composed_on`, else
    /// whichever machine shows it, else `local` — the one chain S.R needs,
    /// centralised so nobody re-derives it at each call site.
    pub fn composer<'a>(&'a self, local: &'a str) -> &'a str {
        self.composed_on
            .as_deref()
            .unwrap_or_else(|| self.shown_on(local))
    }

    /// Which machine shows this screen: `node`, else `local`.
    pub fn shown_on<'a>(&'a self, local: &'a str) -> &'a str {
        self.node.as_deref().unwrap_or(local)
    }
}

/// This machine's name, set once at start-up (`--node`). A screen with no
/// `node` is "this machine" in RAM, but a file written with `null` in it means
/// "the machine that wrote the file" to every peer that lands it — so the
/// save stamps the name in.
static LOCAL_NODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_local_node(name: &str) {
    let _ = LOCAL_NODE.set(name.to_string());
}

/// Every configured output, and the file they live in.
#[derive(Default)]
pub struct Outputs {
    pub list: Vec<Output>,
}

/// Where the files this app writes live.
///
/// Defaults to the repo's `data/`, resolved from this crate rather than from
/// the working directory — which is right on a laptop and wrong on a server,
/// where the build tree is a git checkout that `git pull` will fight with the
/// running service over. `--data` overrides it once, at startup.
static DATA_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_data_root(dir: &std::path::Path) {
    let _ = DATA_ROOT.set(dir.to_path_buf());
}

pub fn data_root() -> PathBuf {
    DATA_ROOT
        .get()
        .cloned()
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
}

fn store_path() -> PathBuf {
    data_root().join("outputs.json")
}

/// The on-disk shape of a device's own JSON object, from before this pass —
/// `layers` still lives inside it there, exactly as 17.1 (the previous pass)
/// left it. The real [`crate::layers::Device`] no longer has that field —
/// the design notes PARTE IV §17.1/§17.4 moved it to a flat pool so a layer can be
/// routed to more than one device — so this local shape is what
/// [`load_devices`] actually deserializes a device *into*, before splitting
/// it into `(Device, its layers)`.
///
/// `layers` here already deserializes as real [`crate::layers::Layer`]s, not
/// a second private copy of that type: `Layer::outputs` has
/// `#[serde(default)]`, so a layer written by 17.1 (which predates that
/// field) comes back with `outputs: vec![]`, and [`load_devices`] is what
/// turns that into the routing it always implicitly had.
#[derive(serde::Deserialize)]
struct DeviceCompat {
    #[serde(default)]
    layers: Vec<crate::layers::Layer>,
    #[serde(default)]
    composition: crate::layers::Composition,
    #[serde(default)]
    mode: crate::layers::DeviceMode,
    #[serde(default)]
    head: Option<crate::fixture::Head>,
}

/// Loads devices *and* layers from `outputs.json`.
///
/// Returns the metadata map (composition/mode, keyed by device id — what
/// [`crate::layers::Device`] is now) and the one flat pool every layer lives
/// in regardless of which device(s) it is routed to — the design notes PARTE IV
/// §17.1/§17.4. A device that used to own `layers: Vec<Layer>` directly now
/// only owns its id in each of those layers' [`crate::layers::Layer::outputs`].
///
/// **Three cases, not one**, because this file can hold data written by three
/// different versions of this app:
///
/// - **A layer with empty `outputs`** predates this field entirely (17.1's
///   own shape, or older). Its only home was the device it was found nested
///   under, so that is stamped in.
/// - **A layer with non-empty `outputs`, seen for the first time** was
///   written by this version of the app. Its `outputs` is already authoritative
///   — left alone.
/// - **A layer with non-empty `outputs`, seen again under a second device** is
///   not a second layer — it is [`Outputs::save`]'s own projection of one
///   fan-out layer into every device it routes to, read back. Skipped rather
///   than pushed twice, or a save/load/save cycle would keep multiplying it.
///
/// Only the first case can collide on id — see the comment at its call site
/// below — because only it invents a routing rather than trusting one that
/// is already on the layer.
pub fn load_devices() -> (
    std::collections::HashMap<String, crate::layers::Device>,
    Vec<crate::layers::Layer>,
) {
    let (sources, layers, collisions) = load_devices_from(&store_path());
    if collisions > 0 {
        eprintln!(
            "  outputs: resolved {collisions} duplicate layer id(s) while migrating to a \
             global layer pool (data/outputs.json predates the design notes PARTE IV §17.1/§17.4)"
        );
    }
    (sources, layers)
}

/// The testable half of [`load_devices`] — split out for the same reason
/// `engine.rs`'s `migrate_collections_to_graphs_once`/`_at` are: `store_path()`
/// resolves through `data_root()`'s process-wide `OnceLock`, which can only be
/// set once per process, so a test that wants a scratch `outputs.json` needs a
/// function that takes a path instead of reaching for the global one.
///
/// The third element is how many layer id collisions the migration resolved
/// (see the loop below) — returned rather than only logged, so a test can
/// assert on it directly instead of scraping stderr.
fn load_devices_from(
    path: &Path,
) -> (
    std::collections::HashMap<String, crate::layers::Device>,
    Vec<crate::layers::Layer>,
    usize,
) {
    let Ok(text) = metal_vfs::read_to_string(path) else {
        return (std::collections::HashMap::new(), Vec::new(), 0);
    };

    #[derive(serde::Deserialize)]
    struct OutputCompat {
        pub id: String,
        #[serde(default)]
        pub device: Option<DeviceCompat>,
    }

    let compat_list: Vec<OutputCompat> = serde_json::from_str(&text).unwrap_or_default();

    let mut sources = std::collections::HashMap::new();
    let mut layers: Vec<crate::layers::Layer> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut collisions = 0usize;

    for c in compat_list {
        let Some(dc) = c.device else { continue };
        for mut layer in dc.layers {
            if layer.outputs.is_empty() {
                layer.outputs = vec![c.id.clone()];
                // Old data only: ids were previously unique per device (see
                // `POST /layers` op "add"'s id generator), so two different
                // devices' layers could coincidentally share one once they
                // land in a single global pool. Renamed rather than dropped
                // — either twin is a real layer an operator built — and
                // logged, because a renamed id is exactly the kind of
                // silent-but-in-range change this house of code does not let
                // pass without a word.
                if seen.contains(&layer.id) {
                    collisions += 1;
                    let base = layer.id.clone();
                    let mut n = 1;
                    loop {
                        let candidate = format!("{base}-dup{n}");
                        if !seen.contains(&candidate) {
                            layer.id = candidate;
                            break;
                        }
                        n += 1;
                    }
                }
                seen.insert(layer.id.clone());
                layers.push(layer);
            } else if !seen.contains(&layer.id) {
                seen.insert(layer.id.clone());
                layers.push(layer);
            }
            // else: the same fan-out layer, met again via a second device's
            // own copy of it — see the doc comment's third case.
        }
        sources.insert(
            c.id,
            crate::layers::Device {
                composition: dc.composition,
                mode: dc.mode,
                triggers: Vec::new(), // O.10: triggers default to empty (no active triggers)
                head: dc.head,
            },
        );
    }

    (sources, layers, collisions)
}

impl Outputs {
    /// Reads `data/outputs.json`, or an empty set.
    ///
    /// **This one persists, and the router's history does not.** A mapping is a
    /// property of the room: the projector is still pointed at the same pillar
    /// tomorrow, and an operator who had to re-contour it after every restart
    /// would stop using masks. A play history is a property of one set.
    pub fn load() -> Outputs {
        Outputs::load_from(&store_path())
    }

    /// The testable half of [`Outputs::load`] — same reason [`Outputs::save`]
    /// has `save_to`: `store_path()` resolves through `data_root()`'s
    /// process-wide `OnceLock`, so a test that wants a scratch `outputs.json`
    /// needs a function that takes a path instead of reaching for the global
    /// one.
    fn load_from(path: &Path) -> Outputs {
        let Ok(text) = metal_vfs::read_to_string(path) else {
            return Outputs::default();
        };

        // 17.1: Device field was moved out of Output. For backward compatibility,
        // deserialize into a temporary format and extract devices if present.
        #[derive(serde::Deserialize)]
        struct OutputCompat {
            pub id: String,
            pub label: String,
            pub width: u32,
            pub height: u32,
            pub masks: Vec<Mask>,
            #[serde(default)]
            pub calibrating: bool,
            #[serde(default)]
            pub node: Option<String>,
            #[serde(default)]
            pub composed_on: Option<String>,
            #[serde(default)]
            pub ceded_to: Option<String>,
            #[serde(default)]
            pub sink: Option<OutputSink>,
        }

        let compat_list: Vec<OutputCompat> = serde_json::from_str(&text).unwrap_or_default();
        let list = compat_list
            .into_iter()
            .map(|c| Output {
                id: c.id,
                label: c.label,
                width: c.width,
                height: c.height,
                masks: c.masks,
                calibrating: c.calibrating,
                node: c.node,
                composed_on: c.composed_on,
                ceded_to: c.ceded_to,
                sink: c.sink,
            })
            .collect();
        let mut outputs = Outputs { list };
        outputs.name_masks();
        outputs
    }

    /// Gives every contour an id, and makes sure no two on one device share one.
    ///
    /// Runs on load and after every write. A mapping written before ids existed
    /// comes back with `mask-1`, `mask-2` … in the order it was contoured, which
    /// is the order the operator drew them in and therefore the order they are
    /// listed in — so an id assigned here is one they can recognise. Duplicates
    /// are only possible through a hand-edited file, and one contour shadowing
    /// another would send a source to the wrong pillar.
    ///
    /// # An assigned id is new on every device, not only on its own
    ///
    /// A [`crate::layers::Layer`] routed to two devices names one
    /// [`crate::layers::Layer::mask`], and each device resolves that id
    /// against its own contours. So an id is an object's identity across
    /// projectors: `bar.panel-left` on both projectors that light the panel
    /// is one panel, and one source bound to it is cut to it on both. That
    /// only works if an id nobody chose never collides by counting — two
    /// devices each numbering their first contour `mask-1` bound one source
    /// to the pillar on one projector and to the cup on the other. Ids a
    /// caller chose are kept as they are; only the ones assigned here skip
    /// every id in use on any device.
    fn name_masks(&mut self) {
        let mut taken: std::collections::HashSet<String> = self
            .list
            .iter()
            .flat_map(|o| o.masks.iter())
            .map(|m| m.id.clone())
            .filter(|id| !id.trim().is_empty())
            .collect();
        for out in self.list.iter_mut() {
            let mut seen: Vec<String> = Vec::new();
            for (i, mask) in out.masks.iter_mut().enumerate() {
                if mask.id.trim().is_empty() || seen.contains(&mask.id) {
                    let mut n = i + 1;
                    while taken.contains(&format!("mask-{n}")) {
                        n += 1;
                    }
                    mask.id = format!("mask-{n}");
                    taken.insert(mask.id.clone());
                }
                seen.push(mask.id.clone());
            }
        }
    }

    pub fn save(
        &mut self,
        sources: &std::collections::HashMap<String, crate::layers::Device>,
        layers: &[crate::layers::Layer],
    ) -> std::io::Result<()> {
        let path = store_path();
        self.save_to(sources, layers, &path)
    }

    /// The testable half of [`Outputs::save`] — same reason [`load_devices`]
    /// has `load_devices_from`: a test wants a scratch path, and
    /// `store_path()` only ever answers the process-wide one.
    fn save_to(
        &mut self,
        sources: &std::collections::HashMap<String, crate::layers::Device>,
        layers: &[crate::layers::Layer],
        path: &Path,
    ) -> std::io::Result<()> {
        // Before the write, not after: a contour added through `/outputs` has no
        // id until this runs, and a file on disk that is missing one is a file
        // the next load has to guess about.
        self.name_masks();

        // Reconstructs each device's own JSON object — `layers` embedded,
        // exactly the shape `load_devices` reads back — from the metadata
        // map (`sources`, composition/mode only since the design notes PARTE IV
        // §17.1/§17.4) plus the one flat pool every layer actually lives in
        // now. See `DeviceCompat` in `load_devices` for the other half of
        // this projection.
        #[derive(serde::Serialize)]
        struct DeviceForSave {
            layers: Vec<crate::layers::Layer>,
            composition: crate::layers::Composition,
            mode: crate::layers::DeviceMode,
            head: Option<crate::fixture::Head>,
        }

        #[derive(serde::Serialize)]
        struct OutputForSave {
            pub id: String,
            pub label: String,
            pub width: u32,
            pub height: u32,
            pub masks: Vec<Mask>,
            #[serde(default)]
            pub calibrating: bool,
            pub device: DeviceForSave,
            pub node: Option<String>,
            pub composed_on: Option<String>,
            pub ceded_to: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub sink: Option<OutputSink>,
        }

        let to_save: Vec<OutputForSave> = self
            .list
            .iter()
            .map(|o| {
                let meta = sources.get(&o.id).cloned().unwrap_or_default();
                // Fan-out means a layer can appear in more than one device's
                // list here — the design notes PARTE IV §17.1/§17.4. That is correct
                // and expected, not a bug to avoid: `outputs.json` is a
                // projection of the pool, written so each device's own
                // object still reads the way it always did, not the source
                // of truth. The pool in RAM (`State.layers`, one entry per
                // layer, each carrying its own `outputs: Vec<String>`) is
                // what actually owns it — `load_devices` folds this same
                // duplication back down to one entry by id on the way in.
                let routed: Vec<crate::layers::Layer> = layers
                    .iter()
                    .filter(|l| l.outputs.iter().any(|d| d == &o.id))
                    .cloned()
                    .collect();
                OutputForSave {
                    id: o.id.clone(),
                    label: o.label.clone(),
                    width: o.width,
                    height: o.height,
                    masks: o.masks.clone(),
                    calibrating: o.calibrating,
                    device: DeviceForSave {
                        layers: routed,
                        composition: meta.composition,
                        mode: meta.mode,
                        head: meta.head,
                    },
                    // Stamped, not null: see `LOCAL_NODE`.
                    node: o.node.clone().or_else(|| LOCAL_NODE.get().cloned()),
                    composed_on: o.composed_on.clone(),
                    ceded_to: o.ceded_to.clone(),
                    sink: o.sink.clone(),
                }
            })
            .collect();

        if let Some(dir) = path.parent() {
            metal_vfs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&to_save)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::atomic::write(path, format!("{text}\n"))
    }

    pub fn find(&self, id: &str) -> Option<&Output> {
        self.list.iter().find(|o| o.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Output> {
        self.list.iter_mut().find(|o| o.id == id)
    }

    pub fn describe(
        &self,
        limits: &crate::layers::Limits,
        air: &crate::layers::Airing,
        models: usize,
        sources: &std::collections::HashMap<String, crate::layers::Device>,
        layers: &[crate::layers::Layer],
    ) -> Value {
        json!(self
            .list
            .iter()
            .map(|o| {
                let mut v = json!({
                    "id": o.id,
                    "label": o.label,
                    "width": o.width,
                    "height": o.height,
                    "calibrating": o.calibrating,
                    "preview": false,
                    "masks": o.masks.iter().map(|m| m.describe(&o.masks)).collect::<Vec<_>>(),
                    // S.R: which machine shows/composes this screen, null
                    // when it defers — see `Output::shown_on`/`composer`.
                    "node": o.node,
                    "composed_on": o.composed_on,
                    "ceded_to": o.ceded_to,
                    "sink": o.sink,
                });
                // Merged rather than nested under a key: the page's activity
                // table reads one row shape for the built-in preview device and
                // for every configured screen, and a device that carried its
                // stack one level deeper would need a second reader.
                // Device metadata lives in `sources` (composition/mode);
                // its layers are `layers`, filtered by routing — the design notes PARTE IV
                // §17.1/§17.4.
                if let Some(device) = sources.get(&o.id) {
                    merge(&mut v, device.describe(&o.id, layers, limits, air, models));
                }
                v
            })
            .collect::<Vec<_>>())
    }
}

impl Output {
    /// A contour on this device by id.
    pub fn mask(&self, id: &str) -> Option<&Mask> {
        self.masks.iter().find(|m| m.id == id)
    }
}

impl Mask {
    /// `siblings` is the rest of the device's masks — the same resolution
    /// context every other method here takes, and what `bounds` needs to be
    /// truthful for a nested or composed mask.
    pub fn describe(&self, siblings: &[Mask]) -> Value {
        let b = self.bounds_in(siblings);
        json!({
            "id": self.id,
            "name": self.name,
            "points": self.points,
            "closed": self.closed(),
            // H.5b/H.10's three fields, sent whether or not they are set: the
            // page has to be able to tell "no parent" from "this build does
            // not know about parents", and a mask editor that cannot see a
            // `compose` would happily overwrite one by writing `points`.
            "parent": self.parent,
            "alpha": self.alpha,
            "compose": self.compose.as_ref().map(|c| json!({
                "op": c.op, "a": c.a, "b": c.b,
            })),
            // 22-sept-2026 (tarde): the stroke, sent whether or not it is
            // set — same reason as the three H.5b fields above.
            "stroke": self.stroke.as_ref().map(|s| json!({
                "width": s.width,
                "color": s.color,
                "source": s.source,
            })),
            // The rectangle a source bound to this gets rendered at. Sent
            // rather than derived in the page, so the two cannot drift about
            // what a contour costs.
            "bounds": { "x": b.x, "y": b.y, "w": b.w, "h": b.h },
        })
    }
}

/// Copies the top-level keys of `extra` into `into`, which must be an object.
fn merge(into: &mut Value, extra: Value) {
    let (Some(dst), Value::Object(src)) = (into.as_object_mut(), extra) else {
        return;
    };
    dst.extend(src);
}

// ---------------------------------------------------------------------------
// the pixels
// ---------------------------------------------------------------------------

/// Cuts the frame to its masks, in place: black outside them, and dimmed
/// wherever a mask asks for less than full opacity.
///
/// Scanline fill, not a point-in-polygon test per pixel: at 1920x1080 the naive
/// version is two million polygon walks a frame, and this is one walk per row.
/// A pixel is kept if it is inside **any** [`lit_masks`] — masks union, they do
/// not intersect, because several masks on one device are several objects being
/// lit and not one object described twice.
///
/// What each pixel keeps is [`coverage_of`]; `Scanline` computes the same thing
/// a row at a time. **Full coverage is not "multiply by one", it is "do not
/// touch"** — the branch below is what makes every mapping written before H.5b
/// come out byte-identical instead of merely arithmetically equal, and that
/// property is worth a branch per pixel to have as a fact rather than as a
/// rounding argument.
///
/// No masks means no masking. See the module note.
pub fn apply_masks(pixels: &mut [u8], width: u32, height: u32, masks: &[Mask]) {
    apply_masks_with(pixels, width, height, masks, &[], crate::layers::Composition::Overlap);
}

/// [`apply_masks`] with **transient** masks — the 3D masks a device plan
/// carries for this frame but does not persist into `Output::masks` (plan:
/// "A 3D mask is not written into `Output.masks`: the device plan carries its
/// contours as transient masks, and `apply_masks` cuts with persisted ∪
/// transient").
///
/// Transient masks are ordinary contours by the time they land here (the
/// projection already happened upstream), so they simply join the persisted
/// ones before the scanline walk. The one exception is **sectors** mode: there
/// each layer replaces its rectangle, so a device-wide transient contour would
/// fight the composition it belongs to and is skipped — the same way the 3D
/// mask is skipped in the plan's sectors case.
pub fn apply_masks_with(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    masks: &[Mask],
    transient: &[Mask],
    composition: crate::layers::Composition,
) {
    // Persisted ∪ transient, transient last so a projected contour wins an id
    // clash with a stale saved one. Sectors skips the transient half.
    let combined: Vec<Mask>;
    let all: &[Mask] = if transient.is_empty() || composition == crate::layers::Composition::Sectors {
        masks
    } else {
        combined = masks.iter().cloned().chain(transient.iter().cloned()).collect();
        &combined
    };
    let usable = lit_masks(all);
    if usable.is_empty() {
        return;
    }
    let w = width as usize;
    let mut scratch = Scanline::new(width);

    for y in 0..height as usize {
        scratch.fill(all, &usable, y, height);
        let row = y * w * 4;
        for (x, cover) in scratch.cover.iter().enumerate() {
            let p = row + x * 4;
            if *cover >= 1.0 {
                continue;
            }
            if *cover <= 0.0 {
                pixels[p] = 0;
                pixels[p + 1] = 0;
                pixels[p + 2] = 0;
                continue;
            }
            for c in 0..3 {
                pixels[p + c] = (pixels[p + c] as f32 * cover).round() as u8;
            }
        }
    }

    // 22-sept-2026 (tarde): the mask strokes, after the cut. The band is the
    // original coverage minus the coverage of the polygon eroded inward by
    // the stroke's width, so the interior keeps the `image` content and only
    // the rim is filled — the ask's "outline with a material and the main
    // content inside".
    //
    // No material map here: this is the whole-frame path, reached only for a
    // device whose graph carries no live `mask` node, and a material can only
    // be wired to one. A device that has one is cut per layer by the graph and
    // gets its strokes from `engine::render_device`, materials and all.
    apply_mask_strokes(pixels, width, height, all, &HashMap::new());
}

/// Paints every [`MaskStroke`] on the device over the already-cut frame. The
/// band is `cover(polygon) − cover(polygon eroded by width)`; a flat `color`
/// fills it, and a `source` material — the pixels of whatever chain is wired
/// to the `mask` node's `stroke` port, resolved by the caller and keyed here
/// by **mask id** — is sampled instead when the map carries one.
///
/// **Public because the two render paths meet here.** A device with no live
/// `mask` node is cut whole-frame by [`apply_masks`], which calls this with an
/// empty map (there is no graph port to wire a material to). A device *with*
/// one is cut per layer by the graph and never reaches [`apply_masks`] — so
/// `engine::render_device` calls this directly, with the materials
/// [`crate::paint::RunOutput::materials`] just produced. Same band, same
/// clamp, one implementation.
pub fn apply_mask_strokes(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    masks: &[Mask],
    materials: &HashMap<MaskId, StrokeMaterial>,
) {
    let stroked: Vec<&Mask> = masks
        .iter()
        .filter(|m| {
            m.closed()
                && m.stroke
                    .as_ref()
                    .is_some_and(|s| s.width > 0.0)
        })
        .collect();
    if stroked.is_empty() {
        return;
    }
    let w = width as usize;
    let mut outer = Scanline::new(width);
    let mut inner = Scanline::new(width);
    for mask in &stroked {
        let stroke = mask.stroke.as_ref().expect("filtered to stroked");
        let material = materials.get(&mask.id);
        let bounds = mask.bounds_in(masks);
        let _eroded = erode(&mask.points, stroke.width.min(MASK_STROKE_MAX), width, height);
        let eroded_mask = eroded_mask_of(mask, stroke, width, height);
        for y in 0..height as usize {
            outer.fill(masks, &[mask], y, height);
            inner.fill(masks, &[&eroded_mask], y, height);
            let row = y * w * 4;
            let color = stroke.color.unwrap_or([1.0, 1.0, 1.0, 1.0]);
            for x in 0..w {
                // In the outer shape but not the eroded one: the rim.
                let band = (outer.cover[x] - inner.cover[x]).clamp(0.0, 1.0);
                if band <= 0.0 {
                    continue;
                }
                // `source` material when the caller wired one, flat `color`
                // otherwise — the fallback the ask names *"or material filling
                // via a image […] wired to the mask"*: a stroked mask always
                // shows its rim, wired material or not.
                let src = match material {
                    Some(m) => m.at(bounds, x as u32, y as u32, width, height),
                    None => color,
                };
                let a = src[3];
                let p = row + x * 4;
                for c in 0..3 {
                    let under = pixels[p + c] as f32;
                    let over = src[c] * 255.0;
                    let mixed = under * (1.0 - band * a) + over * (band * a);
                    pixels[p + c] = mixed.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// The polygon a stroke's inner edge is read from: `mask` eroded by the
/// stroke width.
fn eroded_mask_of(mask: &Mask, stroke: &MaskStroke, width: u32, height: u32) -> Mask {
    let eroded = erode(&mask.points, stroke.width.min(MASK_STROKE_MAX), width, height);
    Mask {
        // Empty on purpose: `Scanline::fill` resolves a target's own
        // polygon by identity first, id second, and this clone shares
        // neither — so the eroded points get walked directly instead
        // of silently resolving back to the original's row. No mask
        // in `all` carries an empty id (`Outputs::load` fills blanks
        // in), and `nesting`/`resolved_at` read `parent`, not the
        // mask's own id, so resolution is unaffected.
        id: String::new(),
        name: mask.name.clone(),
        points: eroded,
        parent: mask.parent.clone(),
        alpha: mask.alpha,
        compose: None,
        stroke: None,
    }
}

// ---------------------------------------------------------------------------
// G3: the same cuts as GPU passes. The polygon walk stays on the CPU (the one
// `Scanline`, so edges agree to the pixel with `apply_masks_with`) but yields a
// coverage map instead of touching the frame; the map uploads once, is cached
// while the masks stand still, and the frame never reads back.
// ---------------------------------------------------------------------------

fn hash_of(parts: &[&dyn std::fmt::Debug]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        format!("{p:?}").hash(&mut h);
    }
    h.finish()
}

/// Coverage of one contour `clip` (resolved against `all`) on rows
/// `y0..y1` of a `w x h` device, as bytes; rows outside stay `0`. What a
/// contour-bound layer is cut by in the GPU compose shader.
pub fn clip_cover_bytes(all: &[Mask], clip: &Mask, w: u32, h: u32, y0: u32, y1: u32) -> Vec<u8> {
    let mut out = vec![0u8; w as usize * h as usize];
    let mut scan = Scanline::new(w);
    for y in y0..y1.min(h) {
        scan.fill(all, &[clip], y as usize, h);
        let row = y as usize * w as usize;
        for (x, c) in scan.cover.iter().enumerate() {
            out[row + x] = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    out
}

/// The cache key of [`clip_cover_bytes`]'s inputs.
pub fn clip_cover_key(all: &[Mask], clip: &Mask, w: u32, h: u32, y0: u32, y1: u32) -> u64 {
    hash_of(&[&"clip", &all, &clip, &w, &h, &y0, &y1])
}

/// [`apply_masks_with`] and, with `cut` off, the strokes alone, on a texture:
/// the device-wide cut to the contours and the stroke bands over it.
/// `materials` are the wired stroke materials (empty on the whole-frame
/// path). `None` when this painter cannot shade on the GPU; the caller then
/// reads back and uses the CPU functions.
#[allow(clippy::too_many_arguments)]
pub fn apply_masks_gpu(
    paint: &mut dyn crate::paint::Paint,
    frame: &crate::gpu_frame::GpuFrame,
    masks: &[Mask],
    transient: &[Mask],
    composition: crate::layers::Composition,
    materials: &HashMap<MaskId, StrokeMaterial>,
    cut: bool,
) -> Option<crate::gpu_frame::GpuFrame> {
    use crate::gpu_frame::Shade;
    let (w, h) = (frame.width(), frame.height());
    let combined: Vec<Mask>;
    let all: &[Mask] = if transient.is_empty() || composition == crate::layers::Composition::Sectors {
        masks
    } else {
        combined = masks.iter().cloned().chain(transient.iter().cloned()).collect();
        &combined
    };
    let mut cur = frame.clone();
    if cut {
        let usable = lit_masks(all);
        if !usable.is_empty() {
            let key = hash_of(&[&"cut", &all, &w, &h]);
            let cover = paint.cover_gpu(key, w, h, &mut || {
                let mut out = vec![0u8; w as usize * h as usize];
                let mut scan = Scanline::new(w);
                for y in 0..h as usize {
                    scan.fill(all, &usable, y, h);
                    for (x, c) in scan.cover.iter().enumerate() {
                        out[y * w as usize + x] = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    }
                }
                out
            })?;
            cur = paint.shade_gpu(&cur, &Shade::Cut(&cover))?;
        }
    }
    let mut outer = Scanline::new(w);
    let mut inner = Scanline::new(w);
    for mask in all.iter().filter(|m| m.closed() && m.stroke.as_ref().is_some_and(|s| s.width > 0.0)) {
        let stroke = mask.stroke.as_ref().expect("filtered to stroked");
        let eroded = eroded_mask_of(mask, stroke, w, h);
        let band = |outer: &mut Scanline, inner: &mut Scanline| {
            let mut out = vec![0f32; w as usize * h as usize];
            for y in 0..h as usize {
                outer.fill(all, &[mask], y, h);
                inner.fill(all, &[&eroded], y, h);
                for x in 0..w as usize {
                    out[y * w as usize + x] = (outer.cover[x] - inner.cover[x]).clamp(0.0, 1.0);
                }
            }
            out
        };
        match materials.get(&mask.id) {
            Some(m) => {
                let band = band(&mut outer, &mut inner);
                let bounds = mask.bounds_in(all);
                let mut rgba = vec![0u8; w as usize * h as usize * 4];
                for y in 0..h {
                    for x in 0..w {
                        let b = band[(y * w + x) as usize];
                        if b <= 0.0 {
                            continue;
                        }
                        let src = m.at(bounds, x, y, w, h);
                        let i = ((y * w + x) * 4) as usize;
                        for c in 0..3 {
                            rgba[i + c] = (src[c] * 255.0).round().clamp(0.0, 255.0) as u8;
                        }
                        rgba[i + 3] = (b * src[3] * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
                cur = paint.shade_gpu(&cur, &Shade::Over { rgba: &rgba })?;
            }
            None => {
                let color = stroke.color.unwrap_or([1.0, 1.0, 1.0, 1.0]);
                let key = hash_of(&[&"stroke", &all, &mask.id, &w, &h]);
                let cover = paint.cover_gpu(key, w, h, &mut || {
                    band(&mut outer, &mut inner).iter().map(|b| (b * 255.0 + 0.5) as u8).collect()
                })?;
                cur = paint.shade_gpu(&cur, &Shade::Tint { rgb: [color[0], color[1], color[2]], k: color[3], cover: Some(&cover) })?;
            }
        }
    }
    Some(cur)
}

/// A polygon offset inward by `width` device pixels: each edge is pushed
/// along its inward normal and adjacent offset lines are re-intersected, so
/// corners stay sharp. A polygon under three points is returned unchanged
/// (nothing to erode), and a shape that vanishes under erosion (width wider
/// than the shape) comes back empty — the whole mask then reads as its
/// stroke, which is the honest answer for a band wider than the object.
fn erode(points: &[[f32; 2]], width: f32, w: u32, h: u32) -> Vec<[f32; 2]> {
    if points.len() < 3 || width <= 0.0 {
        return points.to_vec();
    }
    let (fw, fh) = (w as f32, h as f32);
    // to device px
    let px: Vec<[f32; 2]> = points.iter().map(|p| [p[0] * fw, p[1] * fh]).collect();
    // Signed area decides which side is "inside"; screen y grows down.
    let area: f32 = (0..px.len())
        .map(|i| {
            let a = px[i];
            let b = px[(i + 1) % px.len()];
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let n = px.len();
    // The offset line for edge i: direction and a point pushed inward.
    let mut lines: Vec<([f32; 2], [f32; 2])> = Vec::with_capacity(n);
    for i in 0..n {
        let a = px[i];
        let b = px[(i + 1) % n];
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if len < 1e-6 {
            continue;
        }
        let dir = [d[0] / len, d[1] / len];
        // Inward normal (screen space, y down): rotate the direction by 90°
        // and flip by the winding sign so it points into the shape.
        let normal = [-dir[1] * sign, dir[0] * sign];
        lines.push((dir, [a[0] + normal[0] * width, a[1] + normal[1] * width]));
    }
    if lines.len() < 3 {
        return points.to_vec();
    }
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(lines.len());
    for i in 0..lines.len() {
        let (d1, p1) = lines[(i + lines.len() - 1) % lines.len()];
        let (d2, p2) = lines[i];
        // Solve p1 + t d1 = p2 + s d2 for the corner where the two offset
        // edges meet.
        let denom = d1[0] * d2[1] - d1[1] * d2[0];
        if denom.abs() < 1e-6 {
            out.push(p2);
            continue;
        }
        let t = ((p2[0] - p1[0]) * d2[1] - (p2[1] - p1[1]) * d2[0]) / denom;
        out.push([p1[0] + d1[0] * t, p1[1] + d1[1] * t]);
    }
    // back to 0..1
    out.into_iter().map(|p| [p[0] / fw, p[1] / fh]).collect()
}

/// One row's worth of "is this column inside any of these polygons", and the
/// buffers to work it out without allocating per row.
///
/// Scanline, not a point-in-polygon test per pixel: at 1920x1080 the naive
/// version is two million polygon walks a frame, and this is one walk per row.
/// Shared by [`apply_masks`] — which cuts the finished frame — and by
/// `compose.rs`, which cuts one source to one contour. They must agree to the
/// pixel or a source bound to a contour would be trimmed twice, by two slightly
/// different edges, and show a black hairline around the object it is lighting.
pub struct Scanline {
    /// How much of each column survives, `0.0`..`1.0` — [`coverage_of`] for
    /// the row [`Scanline::row`] names.
    pub cover: Vec<f32>,
    /// Which device row `cover` currently describes, for callers that walk a
    /// rectangle rather than the whole frame and so cannot tell from the loop
    /// alone whether they have crossed into a new row.
    pub row: Option<u32>,
    width: u32,
    crossings: Vec<f32>,
    /// `raw[i * width .. (i + 1) * width]` is mask `i`'s **own** polygon on
    /// this row, before `parent` and `compose` are applied. Kept on the
    /// struct for the same reason `crossings` is: one allocation per tile,
    /// not one per row.
    raw: Vec<bool>,
    /// The nesting depth of whichever mask currently owns each column of
    /// `cover`. `coverage_of` resolves "deepest wins" by looking at every
    /// mask at once; a row is filled one target at a time, so it has to
    /// remember what won so far to know whether the next one beats it.
    depth_of: Vec<u8>,
}

impl Scanline {
    pub fn new(width: u32) -> Scanline {
        Scanline {
            cover: vec![0.0; width as usize],
            row: None,
            width,
            crossings: Vec::new(),
            raw: Vec::new(),
            depth_of: vec![0; width as usize],
        }
    }

    /// Fills `cover` for device row `y`: [`coverage_of`] over `targets`,
    /// resolved against `all`.
    ///
    /// `all` is the resolution context — every mask on the device, because a
    /// `parent` or a `compose` operand can name any of them. `targets` is
    /// what actually gets unioned: [`lit_masks`] for the device-wide cut, and
    /// the single bound contour for `compose.rs`'s per-source clip.
    ///
    /// # How the tree gets resolved without a walk per pixel
    ///
    /// Two passes, and the split is the whole point. First every mask's own
    /// polygon is walked **once for this row** into `raw` — that is the
    /// expensive part and its cost is unchanged from before H.5b, one walk
    /// per mask per row. Then `parent`/`compose` are resolved per column as a
    /// boolean combination of rows that have already been walked, which is a
    /// handful of array reads at a nesting depth that is one or two in
    /// practice and capped at [`MAX_MASK_DEPTH`] in the worst case.
    ///
    /// [`resolved_at`] is written to be read directly against
    /// [`Mask::contains_at`] — same branches, same order, same degradations
    /// for a dangling id — so that the row version and the point version
    /// cannot drift apart in meaning. `masks_agree_row_by_row_with_the_point_wise_reference`
    /// holds them to it.
    ///
    /// # Why this is on the CPU and not in a shader
    ///
    /// There is no mask shader to put it in. Masks are cut on the CPU at both
    /// ends of this pipeline — here over the finished frame, and in
    /// `compose.rs` over one source — and the note on this type says why that
    /// matters: *the two must agree to the pixel or a source bound to a
    /// contour shows a black hairline*. A GPU path for the boolean tree would
    /// be a second implementation of the same geometry, which is precisely
    /// the way that hairline gets built. If masks ever need to move per frame
    /// faster than this can serve them, the thing to move to the GPU is both
    /// cuts together, not the tree on its own.
    pub fn fill(&mut self, all: &[Mask], targets: &[&Mask], y: usize, height: u32) {
        self.cover.iter_mut().for_each(|v| *v = 0.0);
        self.depth_of.iter_mut().for_each(|v| *v = 0);
        // Sampled at the centre of the row. On the edge of a polygon the
        // even-odd rule is ambiguous, and a half-pixel offset is what stops a
        // horizontal edge landing exactly on a row and filling the wrong side
        // of it.
        let sy = (y as f32 + 0.5) / height as f32;
        let w = self.width as usize;

        self.raw.clear();
        self.raw.resize(all.len() * w, false);
        for (i, mask) in all.iter().enumerate() {
            polygon_row(
                &mask.points,
                sy,
                self.width,
                &mut self.raw[i * w..(i + 1) * w],
                &mut self.crossings,
            );
        }

        // Each target's own row, without walking its polygon a second time
        // when `all` already holds it.
        //
        // **By identity first, by id second.** `apply_masks` passes borrows
        // straight out of `all`, so identity settles it and cannot be fooled;
        // `compose.rs` passes a clone, which identity misses and the id
        // catches (ids are unique per device —
        // `two_contours_on_one_device_can_never_share_an_id`). Doing it the
        // other way round would break on the one place ids are *not*
        // guaranteed unique: a hand-built list in a test, where two masks
        // sharing an id would both resolve to the first one's shape and the
        // second would silently vanish.
        let mut spare: Vec<bool> = Vec::new();
        for target in targets {
            if !target.closed() {
                continue;
            }
            let found = all
                .iter()
                .position(|m| std::ptr::eq(m, *target))
                .or_else(|| all.iter().position(|m| m.id == target.id));
            let own: &[bool] = match found {
                Some(i) => &self.raw[i * w..(i + 1) * w],
                None => {
                    spare.clear();
                    spare.resize(w, false);
                    polygon_row(
                        &target.points,
                        sy,
                        self.width,
                        &mut spare,
                        &mut self.crossings,
                    );
                    &spare
                }
            };
            let depth = target.nesting(all);
            for x in 0..w {
                if !resolved_at(all, &self.raw, w, target, own, x, 0) {
                    continue;
                }
                // Deepest wins, ties by max — `coverage_of`'s rule, and the
                // reason this is not a plain `max` over the whole row.
                let cur = self.cover[x];
                if depth > self.depth_of[x] || (depth == self.depth_of[x] && target.alpha > cur) {
                    self.cover[x] = target.alpha;
                    self.depth_of[x] = depth;
                }
            }
        }
    }
}

/// Marks the columns of `out` that row `sy` crosses inside `points`, using
/// `crossings` as scratch. Lifted out of [`Scanline::fill`] unchanged so the
/// per-mask row and the per-target row are the same walk and not two.
fn polygon_row(
    points: &[[f32; 2]],
    sy: f32,
    width: u32,
    out: &mut [bool],
    crossings: &mut Vec<f32>,
) {
    out.iter_mut().for_each(|v| *v = false);
    let w = out.len();
    crossings.clear();
    let n = points.len();
    // Under three points is not a shape — the same call `Mask::closed` and
    // `point_in_polygon` both make.
    if n < 3 {
        return;
    }
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        // Half-open in y: a vertex shared by two edges is counted once,
        // which is what keeps a polygon with a horizontal-ish spike
        // from leaking a row.
        if (a[1] <= sy) != (b[1] <= sy) {
            let t = (sy - a[1]) / (b[1] - a[1]);
            crossings.push(a[0] + t * (b[0] - a[0]));
        }
    }
    if crossings.len() < 2 {
        return;
    }
    crossings.sort_by(f32::total_cmp);
    for span in crossings.chunks_exact(2) {
        let x0 = ((span[0] * width as f32).ceil().max(0.0) as usize).min(w);
        let x1 = ((span[1] * width as f32).ceil().max(0.0) as usize).min(w);
        out[x0..x1].iter_mut().for_each(|v| *v = true);
    }
}

/// The column-space twin of [`Mask::contains_at`]: same branches, same order,
/// same degradations — a dangling `compose` operand contributes nothing, a
/// dangling `parent` is treated as absent — reading a row already walked into
/// `raw` instead of walking the polygon again. Read the two side by side; a
/// change to one is a change to both.
///
/// `own` is `mask`'s own polygon row, passed in rather than looked up so a
/// caller holding a mask that is not in `all` still resolves.
fn resolved_at(
    all: &[Mask],
    raw: &[bool],
    w: usize,
    mask: &Mask,
    own: &[bool],
    x: usize,
    depth: u8,
) -> bool {
    if depth > MAX_MASK_DEPTH {
        return false;
    }
    let inside = match &mask.compose {
        Some(Compose { op, a, b }) => {
            let a = named_at(all, raw, w, a, x, depth + 1).unwrap_or(false);
            let b = named_at(all, raw, w, b, x, depth + 1).unwrap_or(false);
            match op {
                BoolOp::Union => a || b,
                BoolOp::Intersect => a && b,
                BoolOp::Subtract => a && !b,
            }
        }
        None => own[x],
    };
    match &mask.parent {
        Some(pid) => inside && named_at(all, raw, w, pid, x, depth + 1).unwrap_or(true),
        None => inside,
    }
}

/// [`resolved_at`] for a mask named by id, or `None` when no mask in `all`
/// has that id — which is what lets the caller pick the degradation, exactly
/// as `contains_at`'s two `unwrap_or`s do.
fn named_at(all: &[Mask], raw: &[bool], w: usize, id: &str, x: usize, depth: u8) -> Option<bool> {
    let i = all.iter().position(|m| m.id == id)?;
    Some(resolved_at(
        all,
        raw,
        w,
        &all[i],
        &raw[i * w..(i + 1) * w],
        x,
        depth,
    ))
}

/// The colour mask number `i` of a device is outlined in when
/// `POST /mask/project` is asked for the whole device (no `mask_id`) —
/// S.B's *"with a different colour each"*. Twelve well-separated hues,
/// cycling; the `test_mask_grid` tool builds its legend from this same
/// function, so the picture and the caption cannot disagree.
pub fn mask_color(i: usize) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 12] = [
        [255, 64, 64],
        [64, 255, 64],
        [64, 96, 255],
        [255, 255, 64],
        [255, 64, 255],
        [64, 255, 255],
        [255, 160, 64],
        [160, 64, 255],
        [64, 255, 160],
        [255, 64, 160],
        [160, 255, 64],
        [64, 160, 255],
    ];
    PALETTE[i % PALETTE.len()]
}

/// How many cells the calibration grid is divided into, across.
pub const GRID_DIVISIONS: u32 = 16;

/// Draws the calibration grid and the mask outlines over the frame.
///
/// The grid is what the ask asks for — *"project a grid onto the whole device"* — and it goes in the frame rather than in the browser so it lands
/// on the real surface through the real projector. Every fourth line is
/// brighter, so a point can be named out loud across a room ("third heavy line,
/// two up") while somebody else drags it.
pub fn draw_calibration(pixels: &mut [u8], width: u32, height: u32, masks: &[Mask]) {
    let (w, h) = (width as usize, height as usize);
    let cell_x = (width as f32 / GRID_DIVISIONS as f32).max(1.0);
    // Square cells, so the grid is a grid and not a stack of letterboxes: the
    // vertical count follows from the horizontal one and the aspect.
    let rows = ((height as f32 / cell_x).round() as u32).max(1);
    let cell_y = height as f32 / rows as f32;

    let mut plot = |x: isize, y: isize, c: [u8; 3]| {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            return;
        }
        let p = (y as usize * w + x as usize) * 4;
        pixels[p] = c[0];
        pixels[p + 1] = c[1];
        pixels[p + 2] = c[2];
    };

    let faint = [70u8, 70, 74];
    let heavy = [190u8, 60, 55];

    for i in 0..=GRID_DIVISIONS {
        let x = (i as f32 * cell_x).round() as isize;
        let c = if i % 4 == 0 { heavy } else { faint };
        for y in 0..h as isize {
            plot(x, y, c);
        }
    }
    for j in 0..=rows {
        let y = (j as f32 * cell_y).round() as isize;
        let c = if j % 4 == 0 { heavy } else { faint };
        for x in 0..w as isize {
            plot(x, y, c);
        }
    }

    // The masks themselves: the outline so the contour can be judged against
    // the object, and a box at each key point so it can be found and dragged.
    for mask in masks {
        let n = mask.points.len();
        for i in 0..n {
            let a = mask.points[i];
            let b = mask.points[(i + 1) % n];
            if n >= 3 || i + 1 < n {
                line(
                    &mut plot,
                    (a[0] * width as f32) as isize,
                    (a[1] * height as f32) as isize,
                    (b[0] * width as f32) as isize,
                    (b[1] * height as f32) as isize,
                    [255, 230, 220],
                );
            }
        }
        for p in &mask.points {
            let (cx, cy) = (
                (p[0] * width as f32) as isize,
                (p[1] * height as f32) as isize,
            );
            for dy in -4isize..=4 {
                for dx in -4isize..=4 {
                    let edge = dx.abs() == 4 || dy.abs() == 4;
                    plot(
                        cx + dx,
                        cy + dy,
                        if edge { [255, 255, 255] } else { [255, 60, 50] },
                    );
                }
            }
        }
    }
}

/// Bresenham, so a mask outline is one continuous line rather than a dotted one
/// on the diagonals.
fn line(
    plot: &mut impl FnMut(isize, isize, [u8; 3]),
    x0: isize,
    y0: isize,
    x1: isize,
    y1: isize,
    c: [u8; 3],
) {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut x, mut y, mut err) = (x0, y0, dx + dy);
    loop {
        plot(x, y, c);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two devices each contouring a first object used to both call it
    /// `mask-1`, and one source routed to both was cut to the pillar on one
    /// and to the cup on the other. An assigned id is now new everywhere; a
    /// chosen one (the same object on two projectors) is kept.
    #[test]
    fn an_assigned_mask_id_is_new_on_every_device_and_a_chosen_one_is_kept() {
        let mut outs = Outputs { list: vec![Output::new("a", "A", 64, 64), Output::new("b", "B", 64, 64)] };
        let shape = |id: &str| Mask { id: id.into(), points: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], ..Mask::default() };
        outs.list[0].masks = vec![shape(""), shape("panel")];
        outs.list[1].masks = vec![shape(""), shape("panel")];
        outs.name_masks();
        let ids = |o: &Output| o.masks.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        let (a, b) = (ids(&outs.list[0]), ids(&outs.list[1]));
        assert_ne!(a[0], b[0], "{a:?} {b:?}");
        assert_eq!((a[1].as_str(), b[1].as_str()), ("panel", "panel"), "one object, two projectors");
    }

    fn white(w: u32, h: u32) -> Vec<u8> {
        vec![255u8; (w * h * 4) as usize]
    }

    fn lit(pixels: &[u8], w: u32, x: u32, y: u32) -> bool {
        let p = ((y * w + x) * 4) as usize;
        pixels[p] > 0
    }

    #[test]
    fn no_masks_leaves_the_frame_alone() {
        // An output with nothing contoured is a plain screen, not a black one.
        let (w, h) = (32, 18);
        let mut px = white(w, h);
        apply_masks(&mut px, w, h, &[]);
        assert!(px.iter().all(|v| *v == 255));
    }

    #[test]
    fn a_transient_mask_cuts_alongside_the_persisted_ones_but_is_skipped_in_sectors() {
        // The 3D-mask contract: a mask the plan carries for this frame only is
        // cut with the persisted ones — and, in sectors mode, left out so it
        // cannot fight the composition.
        let (w, h) = (100, 100);
        let transient = Mask {
            id: "3d".into(),
            name: "wall".into(),
            points: vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
            ..Default::default()
        };

        let mut overlap = white(w, h);
        apply_masks_with(
            &mut overlap,
            w,
            h,
            &[],
            std::slice::from_ref(&transient),
            crate::layers::Composition::Overlap,
        );
        assert!(lit(&overlap, w, 50, 50), "the transient contour cut its inside away");
        assert!(!lit(&overlap, w, 1, 1), "the transient contour left its outside lit");

        let mut sectors = white(w, h);
        apply_masks_with(
            &mut sectors,
            w,
            h,
            &[],
            std::slice::from_ref(&transient),
            crate::layers::Composition::Sectors,
        );
        assert!(sectors.iter().all(|v| *v == 255), "sectors must skip the 3D contour");
    }

    #[test]
    fn an_unfinished_mask_is_not_a_mask_yet() {
        // Two key points placed is an operator mid-contour. Treating it as a
        // shape would black out their screen while they were still working.
        let (w, h) = (32, 18);
        let mut px = white(w, h);
        let half = Mask {
            id: "t".into(),
            name: "wip".into(),
            points: vec![[0.1, 0.1], [0.9, 0.1]],
            ..Default::default()
        };
        apply_masks(&mut px, w, h, &[half]);
        assert!(px.iter().all(|v| *v == 255));
    }

    #[test]
    fn a_rectangle_keeps_what_is_inside_it_and_blacks_what_is_not() {
        let (w, h) = (100, 100);
        let mut px = white(w, h);
        let square = Mask {
            id: "t".into(),
            name: "pillar".into(),
            points: vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
            ..Default::default()
        };
        apply_masks(&mut px, w, h, &[square]);
        assert!(lit(&px, w, 50, 50), "the middle of the mask went dark");
        assert!(lit(&px, w, 26, 26), "just inside the corner went dark");
        assert!(!lit(&px, w, 5, 5), "outside the mask stayed lit");
        assert!(!lit(&px, w, 95, 50), "outside the mask stayed lit");
        assert!(!lit(&px, w, 50, 95), "outside the mask stayed lit");
    }

    #[test]
    fn several_masks_on_one_device_add_up_rather_than_cancelling() {
        // "several masks within the same device" — two objects lit,
        // not the overlap of two descriptions of one.
        let (w, h) = (100, 100);
        let mut px = white(w, h);
        let left = Mask {
            id: "t".into(),
            name: "l".into(),
            points: vec![[0.05, 0.4], [0.35, 0.4], [0.35, 0.6], [0.05, 0.6]],
            ..Default::default()
        };
        let right = Mask {
            id: "t".into(),
            name: "r".into(),
            points: vec![[0.65, 0.4], [0.95, 0.4], [0.95, 0.6], [0.65, 0.6]],
            ..Default::default()
        };
        apply_masks(&mut px, w, h, &[left, right]);
        assert!(lit(&px, w, 20, 50), "the left mask went dark");
        assert!(lit(&px, w, 80, 50), "the right mask went dark");
        assert!(!lit(&px, w, 50, 50), "the gap between them stayed lit");
    }

    #[test]
    fn a_concave_contour_is_followed_rather_than_filled_in() {
        // The point of key points: an object is rarely a rectangle. An L that
        // came out as its bounding box would light the wall beside the pillar.
        let (w, h) = (100, 100);
        let mut px = white(w, h);
        let ell = Mask {
            id: "t".into(),
            name: "L".into(),
            points: vec![
                [0.1, 0.1],
                [0.4, 0.1],
                [0.4, 0.6],
                [0.9, 0.6],
                [0.9, 0.9],
                [0.1, 0.9],
            ],
            ..Default::default()
        };
        apply_masks(&mut px, w, h, &[ell]);
        assert!(lit(&px, w, 25, 30), "the upright of the L went dark");
        assert!(lit(&px, w, 70, 75), "the foot of the L went dark");
        assert!(!lit(&px, w, 70, 30), "the notch of the L was filled in");
    }

    #[test]
    fn a_mask_is_the_same_shape_at_any_resolution() {
        // Points are normalised precisely so a projector swapped for a
        // different one keeps its mapping. This is that promise.
        let square = Mask {
            id: "t".into(),
            name: "s".into(),
            points: vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
            ..Default::default()
        };
        for (w, h) in [(64u32, 36u32), (1280, 720), (1920, 1080)] {
            let mut px = white(w, h);
            apply_masks(&mut px, w, h, std::slice::from_ref(&square));
            assert!(lit(&px, w, w / 2, h / 2), "{w}x{h}: the middle went dark");
            assert!(!lit(&px, w, 1, 1), "{w}x{h}: the corner stayed lit");
        }
    }

    #[test]
    fn the_calibration_grid_draws_inside_the_frame_and_nowhere_else() {
        // It writes by hand into a pixel buffer, so the only real risk is
        // writing past the end of it. Key points near the edge are the case
        // that would do it — the marker is nine pixels across.
        let (w, h) = (64, 36);
        let mut px = vec![0u8; (w * h * 4) as usize];
        let edge = Mask {
            id: "t".into(),
            name: "edge".into(),
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            ..Default::default()
        };
        draw_calibration(&mut px, w, h, &[edge]);
        assert!(px.iter().any(|v| *v > 0), "the grid drew nothing at all");
    }

    #[test]
    fn a_contour_reports_the_rectangle_a_source_bound_to_it_is_drawn_at() {
        // The doorway case: a tall narrow contour in the middle of a projection
        // must not cost a full frame per source per frame.
        let door = Mask {
            id: "d".into(),
            name: "puerta".into(),
            points: vec![[0.4, 0.3], [0.6, 0.28], [0.62, 0.95], [0.38, 0.9]],
            ..Default::default()
        };
        let b = door.bounds();
        assert!(
            (b.x - 0.38).abs() < 1e-5 && (b.y - 0.28).abs() < 1e-5,
            "{b:?}"
        );
        assert!(
            (b.w - 0.24).abs() < 1e-5 && (b.h - 0.67).abs() < 1e-5,
            "{b:?}"
        );
        // Under a fifth of the frame, which is the whole point.
        assert!(b.w * b.h < 0.2, "a doorway came out at {}", b.w * b.h);
    }

    #[test]
    fn an_unfinished_contour_asks_for_the_whole_device_rather_than_nothing() {
        // Otherwise a source bound to a contour that is being drawn is rendered
        // at a degenerate box and reads as a broken binding.
        let wip = Mask {
            id: "w".into(),
            name: "w".into(),
            points: vec![[0.4, 0.4]],
            ..Default::default()
        };
        assert!(wip.bounds().is_full());
    }

    #[test]
    fn a_mapping_written_before_ids_existed_gets_them_on_the_way_through() {
        // `data/outputs.json` on a real machine has contours with no id, and a
        // source can only ever hold an id. Assigned in draw order, so the id an
        // operator sees against the second contour they drew is `mask-2`.
        let mut outputs = Outputs {
            list: vec![Output {
                masks: vec![
                    Mask {
                        id: String::new(),
                        name: "pilar".into(),
                        ..Default::default()
                    },
                    Mask {
                        id: String::new(),
                        name: "puerta".into(),
                        ..Default::default()
                    },
                ],
                ..Output::new("A", "a", 1920, 1080)
            }],
        };
        outputs.name_masks();
        let ids: Vec<&str> = outputs.list[0]
            .masks
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["mask-1", "mask-2"]);
    }

    #[test]
    fn two_contours_on_one_device_can_never_share_an_id() {
        // Only reachable through a hand-edited file, and one contour shadowing
        // another sends a source to the wrong object in the room.
        let mut outputs = Outputs {
            list: vec![Output {
                masks: vec![
                    Mask {
                        id: "mask-1".into(),
                        name: "a".into(),
                        ..Default::default()
                    },
                    Mask {
                        id: "mask-1".into(),
                        name: "b".into(),
                        ..Default::default()
                    },
                    Mask {
                        id: "mask-2".into(),
                        name: "c".into(),
                        ..Default::default()
                    },
                ],
                ..Output::new("A", "a", 1920, 1080)
            }],
        };
        outputs.name_masks();
        let ids: Vec<String> = outputs.list[0].masks.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            3,
            "two contours kept the same id: {ids:?}"
        );
        assert_eq!(ids[0], "mask-1", "the first one should keep the id it had");
    }

    #[test]
    fn a_contour_is_found_by_id_and_not_by_position() {
        // The reason ids exist at all: deleting an earlier contour must not
        // move every later binding one object to the left.
        let mut out = Output::new("A", "a", 1920, 1080);
        out.masks = vec![
            Mask {
                id: "mask-1".into(),
                name: "pilar".into(),
                ..Default::default()
            },
            Mask {
                id: "mask-2".into(),
                name: "puerta".into(),
                ..Default::default()
            },
        ];
        out.masks.remove(0);
        assert_eq!(out.mask("mask-2").map(|m| m.name.as_str()), Some("puerta"));
        assert!(out.mask("mask-1").is_none());
    }

    #[test]
    fn outputs_round_trip_through_their_file() {
        let mut outputs = Outputs {
            list: vec![Output::new("TEST-0", "test", 1280, 720)],
        };
        outputs.list[0].masks.push(Mask {
            id: "t".into(),
            name: "pillar".into(),
            points: vec![[0.1, 0.1], [0.5, 0.2], [0.4, 0.9]],
            ..Default::default()
        });
        let json = serde_json::to_string(&outputs.list).expect("serialise");
        let back: Vec<Output> = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, outputs.list);
        // A file written before `calibrating` existed still has to load.
        let old = r#"[{"id":"A","label":"a","width":800,"height":600,"masks":[]}]"#;
        let parsed: Vec<Output> = serde_json::from_str(old).expect("legacy outputs still load");
        assert!(!parsed[0].calibrating);
    }

    /// S.R: a file saved before `node`/`composed_on` existed has neither key
    /// at all, not `null` — it must still load as "shown and composed
    /// locally", the single-machine behaviour every such file already had.
    #[test]
    fn every_saved_file_reads_as_local_and_each() {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-outputs-load-legacy-test-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("outputs.json");
        metal_vfs::write(
            &path,
            r#"[{"id":"A","label":"a","width":800,"height":600,"masks":[]}]"#,
        )
        .expect("write scratch file");

        let outputs = Outputs::load_from(&path);
        let out = &outputs.list[0];
        assert_eq!(out.node, None);
        assert_eq!(out.composed_on, None);
        assert_eq!(out.composer("laptop"), "laptop");
        assert_eq!(out.shown_on("laptop"), "laptop");

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The chain `composer` resolves: `composed_on` wins when set, `node` is
    /// the fallback, and `local` is what is left when neither device says
    /// otherwise.
    #[test]
    fn composer_resolves_composed_on_then_node_then_local() {
        let mut out = Output::new("A", "a", 1920, 1080);
        assert_eq!(
            out.composer("laptop"),
            "laptop",
            "neither set: falls back to local"
        );

        out.node = Some("pi".to_string());
        assert_eq!(
            out.composer("laptop"),
            "pi",
            "no composed_on: falls back to node"
        );

        out.composed_on = Some("laptop".to_string());
        assert_eq!(
            out.composer("laptop"),
            "laptop",
            "composed_on wins over node"
        );
    }

    /// S.R's `op: "cede"` writes exactly this expression into the device —
    /// mirroring `call::outputs::tests::node_and_composed_on_write_a_string_and_clear_on_null_or_absent`.
    /// A string sets the field; `null` and an absent key both clear it.
    #[test]
    fn ceded_to_writes_a_string_and_clears_on_null_or_absent() {
        let mut out = Output::new("A", "a", 1920, 1080);

        out.ceded_to = json!({ "to": "resolume" })["to"]
            .as_str()
            .map(str::to_string);
        assert_eq!(out.ceded_to.as_deref(), Some("resolume"));
        out.ceded_to = json!({ "to": Value::Null })["to"]
            .as_str()
            .map(str::to_string);
        assert_eq!(out.ceded_to, None, "an explicit null clears it");
        out.ceded_to = Some("resolume".to_string());
        out.ceded_to = json!({})["to"].as_str().map(str::to_string);
        assert_eq!(out.ceded_to, None, "an absent key clears it just like null");
    }

    /// The save/load round trip, not just serde on a struct in memory — the
    /// same scratch-path pattern as `a_fan_out_layer_survives_a_save_and_reload_through_outputs_json`,
    /// through `Outputs::save_to`/`Outputs::load_from`. Covers `ceded_to`
    /// alongside `node`/`composed_on` rather than duplicating a whole new
    /// round-trip test for it.
    #[test]
    fn node_and_composed_on_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-outputs-node-roundtrip-test-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("outputs.json");

        let mut outputs = Outputs {
            list: vec![Output::new("device-a", "a", 640, 360)],
        };
        outputs.list[0].node = Some("pi".to_string());
        outputs.list[0].composed_on = Some("laptop".to_string());
        outputs.list[0].ceded_to = Some("resolume".to_string());
        let sources = std::collections::HashMap::new();
        outputs
            .save_to(&sources, &[], &path)
            .expect("save must succeed");

        let reloaded = Outputs::load_from(&path);
        assert_eq!(reloaded.list[0].node.as_deref(), Some("pi"));
        assert_eq!(reloaded.list[0].composed_on.as_deref(), Some("laptop"));
        assert_eq!(reloaded.list[0].ceded_to.as_deref(), Some("resolume"));

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_screen_with_no_node_is_saved_under_this_machines_name() {
        // A null `node` in a file is read by a peer as the peer, so the save
        // writes the local name for it. Only the written file is checked; the
        // in-RAM `node` stays None.
        set_local_node("casa");
        let dir = std::env::temp_dir().join(format!(
            "fw-output-stamp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("outputs.json");
        let mut outputs = Outputs { list: vec![Output::new("main", "principal", 640, 360)] };
        outputs.save_to(&std::collections::HashMap::new(), &[], &path).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v[0]["node"], "casa");
        assert_eq!(Outputs::load_from(&path).list[0].shown_on("casa"), "casa");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scratch `outputs.json` in exactly the shape 17.1 left on disk — a
    /// device's own JSON object with `layers` still nested inside it, each
    /// layer with no `outputs` key at all (it predates the field). This is
    /// `load_devices`'s whole reason to exist: the design notes PARTE IV §17.1/§17.4's
    /// real gap was not "where does `Device` live" (17.1 already answered
    /// that) but "a `Layer` still lives inside exactly one `Device`'s own
    /// `Vec`, so it still cannot feed two destinations" — this is the load
    /// path for data written before that second half existed.
    #[test]
    fn a_pre_outputs_field_file_migrates_every_layer_to_its_original_device() {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-load-devices-test-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("outputs.json");
        metal_vfs::write(
            &path,
            r#"[
            {
                "id": "output-a", "label": "a", "width": 1920, "height": 1080, "masks": [],
                "device": {
                    "layers": [
                        {"id":"l0-0","name":"fondo","model":3,"opacity":1.0,"z":0,
                         "blend":"normal","sector":{"x":0,"y":0,"w":1,"h":1},
                         "active":true,"solo":false,"base":null,"library":null,
                         "feed":"model","picked":[],"mask":null}
                    ],
                    "composition": "overlap", "mode": {"kind": "own"}
                }
            },
            {
                "id": "output-b", "label": "b", "width": 1280, "height": 720, "masks": [],
                "device": { "layers": [], "composition": "overlap", "mode": {"kind": "automix"} }
            }
        ]"#,
        )
        .unwrap();

        let (sources, layers, collisions) = load_devices_from(&path);
        assert_eq!(collisions, 0, "nothing here should collide");
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].id, "l0-0");
        assert_eq!(
            layers[0].outputs,
            vec!["output-a".to_string()],
            "a migrated layer must be routed to exactly the device it was nested under"
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(sources["output-a"].mode, crate::layers::DeviceMode::Own);
        assert_eq!(sources["output-b"].mode, crate::layers::DeviceMode::Automix);

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// Before a global pool existed, an id only had to be unique against the
    /// device it was created on (`POST /layers` op `"add"`'s generator
    /// filtered by `stack.layers`, one device's own `Vec`) — so two different
    /// devices' layers can coincidentally share an id in real pre-migration
    /// data. `load_devices` must keep both (renaming the second) rather than
    /// silently dropping one, and say so.
    #[test]
    fn load_devices_renames_a_colliding_id_instead_of_dropping_either_layer() {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-load-devices-collision-test-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("outputs.json");
        let one_layer = |name: &str| {
            format!(
                r#"{{"id":"same-id","name":"{name}","model":0,"opacity":1.0,"z":0,
                     "blend":"normal","sector":{{"x":0,"y":0,"w":1,"h":1}},
                     "active":true,"solo":false,"base":null,"library":null,
                     "feed":"model","picked":[],"mask":null}}"#
            )
        };
        metal_vfs::write(
            &path,
            format!(
                r#"[
                {{"id":"output-a","label":"a","width":1920,"height":1080,"masks":[],
                  "device":{{"layers":[{}],"composition":"overlap","mode":{{"kind":"own"}}}}}},
                {{"id":"output-b","label":"b","width":1920,"height":1080,"masks":[],
                  "device":{{"layers":[{}],"composition":"overlap","mode":{{"kind":"own"}}}}}}
            ]"#,
                one_layer("uno"),
                one_layer("dos")
            ),
        )
        .unwrap();

        let (_sources, layers, collisions) = load_devices_from(&path);
        assert_eq!(
            collisions, 1,
            "one collision should have been resolved, and counted"
        );
        assert_eq!(
            layers.len(),
            2,
            "both layers must survive — neither is dropped"
        );
        let ids: std::collections::HashSet<&str> = layers.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids.len(), 2, "the two layers must not still share an id");
        assert!(ids.contains("same-id"), "the first one keeps the id it had");
        assert!(
            ids.iter().any(|id| id.starts_with("same-id-dup")),
            "the second one is renamed rather than lost: {ids:?}"
        );
        // Each keeps the routing to the device it was actually found under.
        let uno = layers.iter().find(|l| l.name == "uno").unwrap();
        let dos = layers.iter().find(|l| l.name == "dos").unwrap();
        assert_eq!(uno.outputs, vec!["output-a".to_string()]);
        assert_eq!(dos.outputs, vec!["output-b".to_string()]);

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The other end of the fan-out story: a file this version of the app
    /// already wrote, with one layer legitimately routed to two devices, must
    /// come back as one layer with both ids in `outputs` — not two layers,
    /// and not renamed. `Outputs::save` is what produces this shape (see its
    /// own comment on why the same layer appears twice in the file); this is
    /// the read path proving the round trip does not multiply it.
    #[test]
    fn load_devices_folds_a_saved_fan_out_layer_back_into_one() {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-load-devices-fanout-test-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("outputs.json");
        let shared = |outputs: &str| {
            format!(
                r#"{{"id":"shared","name":"shared","model":0,"opacity":1.0,"z":0,
                     "blend":"normal","sector":{{"x":0,"y":0,"w":1,"h":1}},
                     "active":true,"solo":false,"base":null,"library":null,
                     "feed":"model","picked":[],"mask":null,"outputs":{outputs}}}"#
            )
        };
        metal_vfs::write(
            &path,
            format!(
                r#"[
                {{"id":"output-a","label":"a","width":1920,"height":1080,"masks":[],
                  "device":{{"layers":[{}],"composition":"overlap","mode":{{"kind":"own"}}}}}},
                {{"id":"output-b","label":"b","width":1920,"height":1080,"masks":[],
                  "device":{{"layers":[{}],"composition":"overlap","mode":{{"kind":"own"}}}}}}
            ]"#,
                shared(r#"["output-a","output-b"]"#),
                shared(r#"["output-a","output-b"]"#)
            ),
        )
        .unwrap();

        let (_sources, layers, collisions) = load_devices_from(&path);
        assert_eq!(collisions, 0, "an already-routed layer is not a collision");
        assert_eq!(
            layers.len(),
            1,
            "the two on-disk copies must fold back into one layer"
        );
        assert_eq!(
            layers[0].outputs,
            vec!["output-a".to_string(), "output-b".to_string()]
        );
    }

    /// The full round trip end to end, through the real functions rather than
    /// hand-written JSON: create a layer routed to two devices, save it,
    /// read it back — it must still have both. This is the property
    /// the design notes PARTE IV §17.1/§17.4 exists to make possible at all, so it gets
    /// its own test that goes through `Outputs::save` and `load_devices`
    /// together rather than trusting each in isolation.
    #[test]
    fn a_fan_out_layer_survives_a_save_and_reload_through_outputs_json() {
        let dir = std::env::temp_dir().join(format!(
            "metal-dash-fanout-save-reload-test-{}-{}",
            std::process::id(),
            web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("outputs.json");

        let mut outputs = Outputs {
            list: vec![
                Output::new("device-a", "a", 640, 360),
                Output::new("device-b", "b", 640, 360),
            ],
        };
        let mut sources = std::collections::HashMap::new();
        sources.insert("device-a".to_string(), crate::layers::Device::default());
        sources.insert("device-b".to_string(), crate::layers::Device::default());
        let mut l = crate::layers::Layer::new("shared", "shared", 2);
        l.outputs = vec!["device-a".to_string(), "device-b".to_string()];
        let pool = vec![l];

        outputs
            .save_to(&sources, &pool, &path)
            .expect("save must succeed");

        let text = metal_vfs::read_to_string(&path).expect("outputs.json must exist");
        // The projection: the same layer id appears once per device it
        // routes to — see `Outputs::save`'s own comment on why that is
        // correct rather than a bug, not an accidental duplicate.
        assert_eq!(
            text.matches("\"id\": \"shared\"").count(),
            2,
            "a fan-out layer must be projected once per device it routes to"
        );

        let (_reloaded_sources, reloaded_layers, collisions) = load_devices_from(&path);
        assert_eq!(
            collisions, 0,
            "an intentionally fanned-out layer is not a collision"
        );
        assert_eq!(
            reloaded_layers.len(),
            1,
            "must fold back into one layer, not two"
        );
        assert_eq!(
            reloaded_layers[0].outputs,
            vec!["device-a".to_string(), "device-b".to_string()],
            "both routes must survive the save/reload round trip"
        );

        let _ = metal_vfs::remove_dir_all(&dir);
    }

    // -------------------------------------------------------------------
    // H.5b / H.10 — nesting and booleans, the shared `Mask` design
    // -------------------------------------------------------------------

    fn rect(id: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> Mask {
        Mask {
            id: id.into(),
            name: id.into(),
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            ..Default::default()
        }
    }

    /// A mask whose shape *is* the boolean of two others — no `points` of its
    /// own, which is the real shape of a composed mask and what makes
    /// `bounds()` alone unable to answer for it.
    fn composed(id: &str, op: BoolOp, a: &str, b: &str) -> Mask {
        Mask {
            id: id.into(),
            name: id.into(),
            compose: Some(Compose {
                op,
                a: a.into(),
                b: b.into(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_plain_polygon_mask_still_evaluates_and_serialises_exactly_as_before() {
        // The backward-compat case H.5b/H.10 must not disturb: no `parent`,
        // no `compose`, `alpha` defaulted to 1.0 — a calibration mask from
        // before this pass.
        let square = rect("t", 0.25, 0.25, 0.75, 0.75);
        assert_eq!(square.parent, None);
        assert_eq!(square.alpha, 1.0);
        assert_eq!(square.compose, None);
        assert!(
            square.contains(&[], 0.5, 0.5),
            "the middle of the square must be inside"
        );
        assert!(
            !square.contains(&[], 0.1, 0.1),
            "the corner outside the square must stay outside"
        );
        assert_eq!(square.coverage(&[], 0.5, 0.5), 1.0);
        assert_eq!(square.coverage(&[], 0.1, 0.1), 0.0);

        // Round-trips through JSON exactly as a calibration mask always has.
        let json = serde_json::to_value(&square).unwrap();
        let back: Mask = serde_json::from_value(json).unwrap();
        assert_eq!(back, square);
    }

    #[test]
    fn a_mask_saved_before_h5b_still_deserialises_with_todays_defaults() {
        // A hand-written fixture mimicking `data/outputs.json` before this
        // pass: no `parent`, `alpha`, or `compose` keys at all.
        let old = r#"{"id":"t","name":"pillar","points":[[0.1,0.1],[0.5,0.2],[0.4,0.9]]}"#;
        let m: Mask = serde_json::from_str(old).expect("a pre-H.5b mask must still deserialise");
        assert_eq!(m.parent, None);
        assert_eq!(m.alpha, 1.0, "alpha must default to fully opaque");
        assert_eq!(m.compose, None);
        assert_eq!(m.points, vec![[0.1, 0.1], [0.5, 0.2], [0.4, 0.9]]);
    }

    #[test]
    fn a_nested_mask_only_reads_as_inside_within_its_parent() {
        // The plant example: an outer silhouette and an inner highlight — the
        // highlight only counts where the outer shape also covers.
        let outer = rect("outer", 0.2, 0.2, 0.8, 0.8);
        let inner = Mask {
            id: "inner".into(),
            name: "inner".into(),
            points: vec![[0.6, 0.6], [0.95, 0.6], [0.95, 0.95], [0.6, 0.95]],
            parent: Some("outer".into()),
            alpha: 0.5,
            compose: None,
            stroke: None,
        };
        let all = vec![outer.clone(), inner.clone()];

        // Inside both outer and inner's own polygon: nested containment holds.
        assert!(
            inner.contains(&all, 0.65, 0.65),
            "inside both shapes must count as inside"
        );
        assert_eq!(
            inner.coverage(&all, 0.65, 0.65),
            0.5,
            "coverage must carry the child's alpha"
        );

        // Inside inner's own polygon but outside the parent: must NOT count,
        // because a child is "this region, but only within the parent".
        assert!(
            !inner.contains(&all, 0.9, 0.9),
            "inner's own polygon reaches past the parent — the overhang must not read as inside"
        );
        assert_eq!(inner.coverage(&all, 0.9, 0.9), 0.0);

        // The outer mask itself is unaffected by having a child.
        assert!(outer.contains(&all, 0.3, 0.3));
    }

    #[test]
    fn a_dangling_parent_id_degrades_to_unconstrained_rather_than_disappearing() {
        // Bad data (a parent that was deleted) must not blank the mask out —
        // same posture as an unfinished polygon staying visible.
        let orphan = Mask {
            id: "o".into(),
            name: "o".into(),
            points: vec![[0.4, 0.4], [0.6, 0.4], [0.6, 0.6], [0.4, 0.6]],
            parent: Some("does-not-exist".into()),
            alpha: 1.0,
            compose: None,
            stroke: None,
        };
        assert!(orphan.contains(&[], 0.5, 0.5));
    }

    #[test]
    fn a_union_mask_is_inside_wherever_either_shape_is() {
        let a = rect("a", 0.0, 0.0, 0.4, 1.0);
        let b = rect("b", 0.6, 0.0, 1.0, 1.0);
        let venn = Mask {
            id: "venn".into(),
            name: "venn".into(),
            points: vec![],
            parent: None,
            alpha: 1.0,
            compose: Some(Compose {
                op: BoolOp::Union,
                a: "a".into(),
                b: "b".into(),
            }),
            stroke: None,
        };
        let all = vec![a, b, venn.clone()];
        assert!(
            venn.contains(&all, 0.2, 0.5),
            "inside only A must count for a union"
        );
        assert!(
            venn.contains(&all, 0.8, 0.5),
            "inside only B must count for a union"
        );
        assert!(
            !venn.contains(&all, 0.5, 0.5),
            "inside neither must not count"
        );
    }

    #[test]
    fn an_intersect_mask_is_inside_only_where_both_shapes_overlap() {
        let a = rect("a", 0.0, 0.0, 0.6, 1.0);
        let b = rect("b", 0.4, 0.0, 1.0, 1.0);
        let venn = Mask {
            id: "venn".into(),
            name: "venn".into(),
            points: vec![],
            parent: None,
            alpha: 1.0,
            compose: Some(Compose {
                op: BoolOp::Intersect,
                a: "a".into(),
                b: "b".into(),
            }),
            stroke: None,
        };
        let all = vec![a, b, venn.clone()];
        assert!(
            venn.contains(&all, 0.5, 0.5),
            "the overlap strip (0.4..0.6) must count"
        );
        assert!(
            !venn.contains(&all, 0.2, 0.5),
            "inside only A (outside the overlap) must not count"
        );
        assert!(
            !venn.contains(&all, 0.8, 0.5),
            "inside only B (outside the overlap) must not count"
        );
    }

    #[test]
    fn a_subtract_mask_is_a_minus_b() {
        let a = rect("a", 0.0, 0.0, 0.6, 1.0);
        let b = rect("b", 0.4, 0.0, 1.0, 1.0);
        let venn = Mask {
            id: "venn".into(),
            name: "venn".into(),
            points: vec![],
            parent: None,
            alpha: 1.0,
            compose: Some(Compose {
                op: BoolOp::Subtract,
                a: "a".into(),
                b: "b".into(),
            }),
            stroke: None,
        };
        let all = vec![a, b, venn.clone()];
        assert!(
            venn.contains(&all, 0.2, 0.5),
            "inside A and outside B must count for A - B"
        );
        assert!(
            !venn.contains(&all, 0.5, 0.5),
            "inside both A and B must not count for A - B"
        );
        assert!(
            !venn.contains(&all, 0.8, 0.5),
            "outside A entirely must not count for A - B"
        );
    }

    #[test]
    fn a_composed_mask_round_trips_through_json_with_its_operands_by_id() {
        let a = rect("a", 0.0, 0.0, 0.5, 1.0);
        let b = rect("b", 0.5, 0.0, 1.0, 1.0);
        let venn = Mask {
            id: "venn".into(),
            name: "venn".into(),
            points: vec![],
            parent: Some("a".into()),
            alpha: 0.75,
            compose: Some(Compose {
                op: BoolOp::Union,
                a: "a".into(),
                b: "b".into(),
            }),
            stroke: None,
        };
        let json = serde_json::to_value(&venn).unwrap();
        let back: Mask = serde_json::from_value(json).unwrap();
        assert_eq!(back, venn);
        // Sanity: `a`/`b` did not change shape by going through the round trip.
        let all = vec![a, b];
        assert!(back.contains(&all, 0.2, 0.5));
    }

    #[test]
    fn a_cyclic_parent_reference_does_not_blow_the_stack() {
        // Bad data only reachable by hand-editing the file: two masks naming
        // each other as parent. `contains` must terminate rather than recurse
        // forever.
        let a = Mask {
            id: "a".into(),
            name: "a".into(),
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            parent: Some("b".into()),
            alpha: 1.0,
            compose: None,
            stroke: None,
        };
        let b = Mask {
            id: "b".into(),
            name: "b".into(),
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            parent: Some("a".into()),
            alpha: 1.0,
            compose: None,
            stroke: None,
        };
        let all = vec![a.clone(), b];
        // Must return rather than hang or panic; the exact answer for a cycle
        // is not load-bearing, only that it terminates.
        let _ = a.contains(&all, 0.5, 0.5);
    }

    // -----------------------------------------------------------------
    // O.1: the mask tree in the render path
    //
    // Everything above this line tests the geometry point by point. These
    // test that `apply_masks` and `compose.rs` actually *use* it — which is
    // the half H.5b left undone, and the half a mapping on a wall can see.
    // -----------------------------------------------------------------

    /// `apply_masks` over a white frame, as a grid of "is this pixel lit".
    fn cut(w: u32, h: u32, masks: &[Mask]) -> Vec<u8> {
        let mut px = white(w, h);
        apply_masks(&mut px, w, h, masks);
        px
    }

    fn value(px: &[u8], w: u32, x: u32, y: u32) -> u8 {
        px[((y * w + x) * 4) as usize]
    }

    #[test]
    fn a_mask_with_no_parent_no_alpha_and_no_compose_cuts_byte_for_byte_what_it_always_did() {
        // The identity O.1 turns on. Nothing about the H.5b fields may cost a
        // single byte on a mapping that does not use them — every show saved
        // before this change has to come out of the projector unchanged.
        //
        // The reference below is the pre-change algorithm transcribed: a
        // ceil-to-ceil span fill per polygon, union across masks, hard
        // black-out outside. It is written out rather than compared against
        // `point_in_polygon` on purpose — the two only coincide when every
        // mask edge lands on a pixel boundary, and pinning identity to
        // *aligned* geometry would leave the interesting case untested. The
        // coordinates here are deliberately off the grid for that reason.
        let (w, h) = (60, 40);
        let masks = vec![
            rect("pillar", 0.11, 0.13, 0.42, 0.87),
            rect("door", 0.63, 0.21, 0.94, 0.68),
            // An operator mid-contour, which the old code also ignored.
            Mask {
                id: "wip".into(),
                name: "wip".into(),
                points: vec![[0.5, 0.5], [0.55, 0.5]],
                ..Default::default()
            },
        ];

        let mut expected = white(w, h);
        for y in 0..h as usize {
            let sy = (y as f32 + 0.5) / h as f32;
            let mut inside = vec![false; w as usize];
            for m in masks.iter().filter(|m| m.closed()) {
                let mut crossings: Vec<f32> = Vec::new();
                let n = m.points.len();
                for i in 0..n {
                    let (a, b) = (m.points[i], m.points[(i + 1) % n]);
                    if (a[1] <= sy) != (b[1] <= sy) {
                        let t = (sy - a[1]) / (b[1] - a[1]);
                        crossings.push(a[0] + t * (b[0] - a[0]));
                    }
                }
                if crossings.len() < 2 {
                    continue;
                }
                crossings.sort_by(f32::total_cmp);
                for span in crossings.chunks_exact(2) {
                    let x0 = ((span[0] * w as f32).ceil().max(0.0) as usize).min(w as usize);
                    let x1 = ((span[1] * w as f32).ceil().max(0.0) as usize).min(w as usize);
                    inside[x0..x1].iter_mut().for_each(|v| *v = true);
                }
            }
            for (x, keep) in inside.iter().enumerate() {
                if !keep {
                    let p = (y * w as usize + x) * 4;
                    expected[p] = 0;
                    expected[p + 1] = 0;
                    expected[p + 2] = 0;
                }
            }
        }

        assert_eq!(
            cut(w, h, &masks),
            expected,
            "H.5b's fields changed a mapping that does not use them"
        );
    }

    #[test]
    fn the_scanline_agrees_with_the_point_wise_reference_on_a_tree_that_uses_every_field() {
        // `Scanline::fill` is a second implementation of `coverage_of`, and
        // the module note is explicit that two implementations of the same
        // geometry is how a black hairline gets built. This is the test that
        // keeps them one algorithm in two shapes rather than two algorithms.
        //
        // **Every edge sits on a pixel boundary, and that is load-bearing.**
        // The row fill claims a span with `ceil` at both ends; the point-wise
        // rule samples the centre of the pixel. Those two agree exactly on
        // the grid and differ by up to one column off it — a pre-existing
        // convention this change did not touch and must not (see the identity
        // test above, which is the one that pins the off-grid behaviour). So
        // the geometry here is aligned, and what the test is actually about
        // is the *tree*: parent, alpha, the boolean, and deepest-wins.
        let (w, h) = (64u32, 64u32);
        let g = |n: u32| n as f32 / 64.0;
        let masks = vec![
            rect("outer", g(8), g(8), g(56), g(56)),
            Mask {
                alpha: 0.5,
                parent: Some("outer".into()),
                ..rect("inner", g(20), g(20), g(44), g(44))
            },
            rect("a", g(10), g(10), g(36), g(36)),
            rect("b", g(28), g(28), g(54), g(54)),
            composed("lens", BoolOp::Intersect, "a", "b"),
        ];

        let got = cut(w, h, &masks);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let want = (255.0 * coverage_of(&masks, fx, fy)).round() as u8;
                assert_eq!(
                    value(&got, w, x, y),
                    want,
                    "scanline and point-wise disagree at ({x}, {y})"
                );
            }
        }
    }
    #[test]
    fn an_intersect_mask_is_the_overlap_and_not_the_union_of_its_operands() {
        // The Venn case of H.10, and the reason `lit_masks` drops operands:
        // if `a` and `b` were still lit alongside `a ∩ b`, the cut would be
        // `a ∪ b` and the operation would mean nothing at all.
        let (w, h) = (100, 100);
        let masks = vec![
            rect("a", 0.1, 0.1, 0.6, 0.6),
            rect("b", 0.4, 0.4, 0.9, 0.9),
            composed("lens", BoolOp::Intersect, "a", "b"),
        ];
        let px = cut(w, h, &masks);

        assert!(lit(&px, w, 50, 50), "the overlap went dark");
        assert!(
            !lit(&px, w, 20, 20),
            "only `a` covers this — an operand is not a thing on the wall"
        );
        assert!(!lit(&px, w, 80, 80), "only `b` covers this");
        assert!(!lit(&px, w, 5, 5), "outside both");
    }

    #[test]
    fn a_subtract_mask_punches_its_second_operand_out_of_its_first() {
        let (w, h) = (100, 100);
        let masks = vec![
            rect("body", 0.1, 0.1, 0.9, 0.9),
            rect("hole", 0.4, 0.4, 0.6, 0.6),
            composed("ring", BoolOp::Subtract, "body", "hole"),
        ];
        let px = cut(w, h, &masks);

        assert!(lit(&px, w, 20, 20), "the ring went dark");
        assert!(!lit(&px, w, 50, 50), "the hole is not a hole");
        assert!(!lit(&px, w, 5, 5), "outside the body");
    }

    #[test]
    fn a_union_mask_lights_both_of_its_operands_and_nothing_else() {
        let (w, h) = (100, 100);
        let masks = vec![
            rect("l", 0.05, 0.4, 0.35, 0.6),
            rect("r", 0.65, 0.4, 0.95, 0.6),
            composed("both", BoolOp::Union, "l", "r"),
        ];
        let px = cut(w, h, &masks);

        assert!(lit(&px, w, 20, 50));
        assert!(lit(&px, w, 80, 50));
        assert!(!lit(&px, w, 50, 50), "the gap between them");
    }

    #[test]
    fn a_nested_mask_dims_its_parent_rather_than_being_swallowed_by_it() {
        // H.5b's own case: *"the outer one defines the silhouette, the inner one
        // is subtracted or added with alpha"*. A plain max over the list would let the
        // opaque parent win everywhere and neither the highlight nor the
        // shadow would be expressible, which is why `coverage_of` resolves
        // deepest-first.
        let (w, h) = (100, 100);
        let masks = vec![
            rect("silhouette", 0.1, 0.1, 0.9, 0.9),
            Mask {
                alpha: 0.25,
                parent: Some("silhouette".into()),
                ..rect("shadow", 0.3, 0.3, 0.7, 0.7)
            },
        ];
        let px = cut(w, h, &masks);

        assert_eq!(value(&px, w, 20, 20), 255, "the silhouette is opaque");
        assert_eq!(
            value(&px, w, 50, 50),
            64,
            "the nested shadow reads at its own alpha"
        );
        assert_eq!(value(&px, w, 5, 5), 0, "outside the silhouette");
    }

    #[test]
    fn a_nested_mask_at_zero_alpha_punches_a_hole_without_a_boolean() {
        let (w, h) = (60, 60);
        let masks = vec![
            rect("body", 0.1, 0.1, 0.9, 0.9),
            Mask {
                alpha: 0.0,
                parent: Some("body".into()),
                ..rect("cut", 0.4, 0.4, 0.6, 0.6)
            },
        ];
        let px = cut(w, h, &masks);
        assert!(lit(&px, w, 12, 12));
        assert!(
            !lit(&px, w, 30, 30),
            "alpha 0.0 inside the body should read as a hole"
        );
    }

    #[test]
    fn a_child_cannot_light_what_its_parent_does_not_cover() {
        let (w, h) = (60, 60);
        let masks = vec![
            rect("window", 0.1, 0.1, 0.5, 0.5),
            // Deliberately sticks out past the parent on the right.
            Mask {
                parent: Some("window".into()),
                ..rect("pane", 0.3, 0.2, 0.9, 0.4)
            },
        ];
        let px = cut(w, h, &masks);
        assert!(lit(&px, w, 24, 18), "inside both");
        assert!(!lit(&px, w, 45, 18), "the child reached past its parent");
    }

    #[test]
    fn a_composed_contour_asks_for_the_rectangle_it_can_use_and_not_the_whole_device() {
        // `bounds_in` is what a source bound to a contour is rendered at.
        // Reporting the whole device for a composed mask is nine tenths of a
        // GPU pass per source per frame thrown away — the same argument this
        // file already makes for the plain case.
        let masks = vec![
            rect("a", 0.1, 0.1, 0.6, 0.6),
            rect("b", 0.4, 0.4, 0.9, 0.9),
            composed("lens", BoolOp::Intersect, "a", "b"),
        ];
        let lens = &masks[2];
        assert_eq!(
            lens.bounds(),
            crate::layers::Sector::default(),
            "no points of its own"
        );

        let b = lens.bounds_in(&masks);
        assert!(
            (b.x - 0.4).abs() < 1e-5 && (b.y - 0.4).abs() < 1e-5,
            "{b:?}"
        );
        assert!(
            (b.w - 0.2).abs() < 1e-5 && (b.h - 0.2).abs() < 1e-5,
            "{b:?}"
        );
    }

    #[test]
    fn a_union_bounds_to_both_operands_and_a_subtract_to_its_first() {
        let masks = vec![
            rect("l", 0.05, 0.4, 0.35, 0.6),
            rect("r", 0.65, 0.4, 0.95, 0.6),
            composed("u", BoolOp::Union, "l", "r"),
            composed("s", BoolOp::Subtract, "l", "r"),
        ];
        let u = masks[2].bounds_in(&masks);
        assert!(
            (u.x - 0.05).abs() < 1e-5 && (u.w - 0.9).abs() < 1e-5,
            "{u:?}"
        );
        let s = masks[3].bounds_in(&masks);
        assert!(
            (s.x - 0.05).abs() < 1e-5 && (s.w - 0.3).abs() < 1e-5,
            "{s:?}"
        );
    }

    #[test]
    fn a_nested_contour_bounds_inside_its_parent() {
        let masks = vec![
            rect("window", 0.1, 0.1, 0.5, 0.5),
            Mask {
                parent: Some("window".into()),
                ..rect("pane", 0.3, 0.2, 0.9, 0.4)
            },
        ];
        let b = masks[1].bounds_in(&masks);
        assert!((b.x - 0.3).abs() < 1e-5, "{b:?}");
        assert!(
            (b.x + b.w - 0.5).abs() < 1e-5,
            "a child cannot ask for pixels past its parent: {b:?}"
        );
    }

    #[test]
    fn a_cyclic_mask_list_terminates_in_the_render_path_too() {
        // The point-wise version already had this test. The row version
        // recurses over the same two fields, so it needs its own — a hang
        // here would be a hung projector, not a failed assertion.
        let (w, h) = (16, 16);
        let masks = vec![
            Mask {
                parent: Some("b".into()),
                ..rect("a", 0.0, 0.0, 1.0, 1.0)
            },
            Mask {
                parent: Some("a".into()),
                ..rect("b", 0.0, 0.0, 1.0, 1.0)
            },
        ];
        let _ = cut(w, h, &masks);
    }

    #[test]
    fn a_dangling_compose_operand_degrades_the_same_way_in_a_row_as_in_a_point() {
        // Grid-aligned for the reason spelled out in the agreement test above.
        let (w, h) = (32u32, 32u32);
        let g = |n: u32| n as f32 / 32.0;
        let masks = vec![
            rect("a", g(4), g(4), g(28), g(28)),
            composed("u", BoolOp::Union, "a", "deleted"),
        ];
        let px = cut(w, h, &masks);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let want = (255.0 * coverage_of(&masks, fx, fy)).round() as u8;
                assert_eq!(value(&px, w, x, y), want, "at ({x}, {y})");
            }
        }
    }
    #[test]
    fn describe_ships_the_h5b_fields_so_the_page_cannot_overwrite_a_composed_mask() {
        let masks = vec![
            rect("a", 0.1, 0.1, 0.6, 0.6),
            rect("b", 0.4, 0.4, 0.9, 0.9),
            Mask {
                alpha: 0.5,
                ..composed("lens", BoolOp::Intersect, "a", "b")
            },
        ];
        let v = masks[2].describe(&masks);
        assert_eq!(v["compose"]["op"], "intersect");
        assert_eq!(v["compose"]["a"], "a");
        assert_eq!(v["alpha"], 0.5);
        assert_eq!(v["parent"], Value::Null);
        // And the bounds it ships are the resolved ones, not the whole device.
        assert!(
            (v["bounds"]["w"].as_f64().unwrap() - 0.2).abs() < 1e-5,
            "{v}"
        );
    }

    // -- L22.12: mask stroke ----------------------------------------------

    fn stroked(id: &str, width: f32, color: Option<[f32; 4]>) -> Mask {
        Mask {
            stroke: Some(MaskStroke {
                width,
                color,
                source: None,
            }),
            ..rect(id, 0.2, 0.2, 0.8, 0.8)
        }
    }

    /// `width: 0` is no stroke at all: the frame is byte-for-byte what a mask
    /// with no `stroke` field produces — every mapping saved before this
    /// feature comes out of the projector unchanged.
    #[test]
    fn a_zero_width_stroke_paints_exactly_what_no_stroke_would() {
        let (w, h) = (40, 40);
        let plain = vec![rect("m", 0.2, 0.2, 0.8, 0.8)];
        let zero = vec![stroked("m", 0.0, Some([1.0, 0.0, 0.0, 1.0]))];
        let mut a = white(w, h);
        apply_masks(&mut a, w, h, &plain);
        let mut b = white(w, h);
        apply_masks(&mut b, w, h, &zero);
        assert_eq!(a, b, "a zero-width stroke must cost zero bytes");
    }

    /// A coloured stroke paints the rim red and leaves the interior white:
    /// the band exists, and the interior is not double-filled.
    #[test]
    fn a_coloured_stroke_paints_the_rim_and_not_the_interior() {
        let (w, h) = (40, 40);
        let masks = vec![stroked("m", 3.0, Some([1.0, 0.0, 0.0, 1.0]))];
        let mut px = white(w, h);
        apply_masks(&mut px, w, h, &masks);
        // The rim, one pixel inside the left edge of the 0.2..0.8 box.
        let rim_x = (0.2 * w as f32).round() as u32 + 1;
        let mid_y = h / 2;
        let p = ((mid_y * w + rim_x) * 4) as usize;
        assert!(px[p] > 200 && px[p + 1] < 60, "the rim reads red: {px:?}", px = &px[p..p + 4]);
        // The interior, at the centre, is untouched white.
        let c = (((h / 2) * w + w / 2) * 4) as usize;
        assert_eq!(&px[c..c + 3], &[255, 255, 255], "interior stays the image");
        // Outside the box is still cut black (the mask is a mask).
        let o = (2 * w as usize) * 4;
        assert_eq!(&px[o..o + 3], &[0, 0, 0], "outside is still black");
    }

    /// A mask that serialises before this feature still deserialises with
    /// `stroke: None`, and `describe` ships the field so the editor cannot
    /// drop it.
    #[test]
    fn a_stroke_round_trips_and_ships_in_describe() {
        let masks = vec![stroked("m", 4.0, Some([0.0, 1.0, 0.0, 1.0]))];
        let json = serde_json::to_string(&masks).expect("serialise");
        let back: Vec<Mask> = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back[0].stroke.as_ref().unwrap().width, 4.0);
        let v = masks[0].describe(&masks);
        assert_eq!(v["stroke"]["width"], 4.0);
        // And a mask saved without the field reads as no stroke.
        let old: Mask =
            serde_json::from_str(r#"{"id":"m","name":"m","points":[[0.1,0.1],[0.5,0.1],[0.5,0.5]]}"#)
                .expect("old mask");
        assert!(old.stroke.is_none());
    }

    /// L22.12b / D7: with a material wired the band samples *its* pixels
    /// instead of `MaskStroke::color` — a flat green stroke becomes the red
    /// material, and the interior stays the image either way.
    #[test]
    fn a_wired_material_fills_the_band_instead_of_the_flat_colour() {
        let (w, h) = (40, 40);
        let masks = vec![stroked("m", 3.0, Some([0.0, 1.0, 0.0, 1.0]))];
        // A 4x4 opaque-red material, standing in for a `gen.*` tile.
        let mat: Vec<u8> = [255u8, 0, 0, 255].repeat(4 * 4);
        let mut materials = HashMap::new();
        materials.insert(
            "m".to_string(),
            StrokeMaterial { pixels: &mat, width: 4, height: 4 },
        );
        let mut px = white(w, h);
        apply_mask_strokes(&mut px, w, h, &masks, &materials);
        let rim_x = (0.2 * w as f32).round() as u32 + 1;
        let mid_y = h / 2;
        let p = ((mid_y * w + rim_x) * 4) as usize;
        assert!(
            px[p] > 200 && px[p + 1] < 60,
            "the band must sample the red material, not the green colour: {:?}",
            &px[p..p + 4]
        );
        let c = (((h / 2) * w + w / 2) * 4) as usize;
        assert_eq!(&px[c..c + 3], &[255, 255, 255], "interior stays the image");
    }

    /// …and with no material for that mask id it is the flat colour, byte for
    /// byte what the stroke painted before this lot — the fallback the ask
    /// names, and the reason a mask with a wire and no rendered tile is
    /// never worse off than a mask with no wire at all.
    #[test]
    fn no_material_falls_back_to_the_flat_colour() {
        let (w, h) = (40, 40);
        let masks = vec![stroked("m", 3.0, Some([0.0, 1.0, 0.0, 1.0]))];
        let mut px = white(w, h);
        apply_mask_strokes(&mut px, w, h, &masks, &HashMap::new());
        let rim_x = (0.2 * w as f32).round() as u32 + 1;
        let mid_y = h / 2;
        let p = ((mid_y * w + rim_x) * 4) as usize;
        assert!(
            px[p + 1] > 200 && px[p] < 60,
            "with no material the band is the flat green: {:?}",
            &px[p..p + 4]
        );
    }
}
