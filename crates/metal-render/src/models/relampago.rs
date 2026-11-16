//! Model 25 — Lightning (`lightning`): a Tesla coil, not a storm — arcs jump
//! outward from one live centre to the edge of its reach, re-lit two or three
//! times, then gone until the centre fires again.
//!
//! The suite had nothing violent in it. Twenty-seven renderers that flow,
//! breathe, drift or advance, and not one that *hits* — and a set with no
//! punctuation in it is an hour of texture. This is the punctuation.
//!
//! # A centre, not two points
//!
//! The first version of this model struck a channel between an `origen` and a
//! `destino` — two arbitrary points, which is what a storm bolt is but is not
//! what a coil arc is. A coil has one electrode; everything it discharges
//! comes *from* that one place and goes *outward*. So `origen`/`destino` are
//! gone and `centro`/`alcance` replace them: every arc this model draws starts
//! at `centro` and reaches `alcance` outward, at an angle that only the seed
//! decides — see `num_arcos` below for what "only the seed decides" buys.
//!
//! `centro` is also lit continuously, not only during a strike — see
//! `brillo_centro` — because a coil's electrode does not disappear between
//! discharges the way an empty sky between storm strikes should. The neon
//! hum of the electrode is the thing that makes this read as a machine with
//! something building in it, rather than lightning that happens to come from
//! one spot.
//!
//! # Several arcs, one seed
//!
//! `num_arcos` arcs fire in the same round, each aimed at its own angle around
//! the centre — evenly spaced and then jittered per-arc from the round's seed,
//! so a coil with three arcs does not draw the same three directions every
//! time it fires. `ramas` (the branch budget) is shared round-robin across the
//! arcs rather than given to each in full, so raising `num_arcos` changes the
//! shape of a discharge without changing its total cost.
//!
//! # It is Rings' round, reused
//!
//! `intervalo` seconds is a round, `floor(time / intervalo)` seeds it, and the
//! whole channel is generated from that seed on every frame. Nothing is carried
//! between frames, so it is a pure function of `(floats, time)` like the rest of
//! the suite: park it for a minute and the frame it comes back on is the frame
//! it would have drawn. The channel is not expensive enough to be worth
//! keeping — sixty-four segments of midpoint displacement is a few hundred
//! flops, against a re-seed that would have to be invalidated, versioned and
//! reasoned about.
//!
//! # Why the re-strikes matter more than the shape
//!
//! Real lightning is not one flash. The channel ionises and is then re-lit two
//! or three times down the *same* path in a fifth of a second, which is why it
//! reads as a flicker rather than a blink. That is what `destellos` is, and it
//! is the single detail that separates this from a white line appearing.
//!
//! So every strike in a round shares one seed and therefore one channel. Only
//! the brightness and which branches are lit change between them. Re-drawing a
//! *different* path each re-strike — which is what generating from
//! `(round, strike)` would do — looks like several bolts in a row and not like
//! one bolt flickering.
//!
//! # The glow is two passes and not a blur
//!
//! `line2d` writes opaque colour with no falloff, so a corona is a second,
//! fatter, dimmer draw underneath the bright one — the slot scaled down on the
//! CPU rather than an alpha, because the pipeline's fragment stage returns
//! alpha 1.0. It is a hard-edged corona and lightning has a hard-edged corona,
//! so the cheap thing and the right thing agree here. `resplandor` at 0 skips
//! the pass entirely.

use std::f32::consts::TAU;

use glam::Vec2;

use crate::model::{color_at, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::backdrop::Backdrop;
use crate::shared::line2d::{Line2dPipeline, Strip, Vertex};
use crate::shared::ColorSlotGpu;

/// The most subdivisions a channel may have — `2^NIVEL_MAX` segments. Seven is
/// 128 segments across a frame, which is finer than the line is thick.
pub const LEVELS_MAX: u32 = 7;
/// The most branches. Bounded so the vertex buffer for one frame has a size
/// that can be stated rather than discovered.
pub const BRANCHES_MAX: usize = 12;
/// The most simultaneous arcs a discharge may have. Bounded for the same
/// reason as `RAMAS_MAX` — a frame's vertex count should be a number this
/// module states, not one the caller has to discover by driving `num_arcos`
/// as high as it goes.
pub const ARCS_MAX: usize = 6;

const FLOATS: &[FloatSpec] = &[
    // The clock. Structural for Rings' reason: a round length moved by the
    // music would restart the channel mid-strike.
    spec_held("interval", 0.15, 6.0, 1.1, 0.25),
    spec("duration", 0.02, 0.8, 0.13),
    spec_held("flashes", 1.0, 5.0, 3.0, 0.2),
    // The electrode. Every arc starts here — see the module header on why
    // this replaced `origen`/`destino`.
    spec("center_x", -1.6, 1.6, 0.0),
    spec("center_y", -1.2, 1.2, 0.0),
    spec("reach", 0.3, 1.8, 1.1),
    // Structural: how many arcs fire is what the discharge *is*, the same way
    // `nivel` and `ramas` below are — a signal swinging it would restructure
    // the coil rather than intensify it.
    spec_held("num_arcs", 1.0, ARCS_MAX as f32, 3.0, 0.2),
    spec("dispersion", 0.0, 0.6, 0.17),
    // Structural: the number of subdivisions is what the channel *is*, and a
    // signal swinging it would re-cut the bolt rather than move it.
    spec_held("level", 3.0, LEVELS_MAX as f32, 6.0, 0.15),
    spec_held("branches", 0.0, BRANCHES_MAX as f32, 5.0, 0.2),
    spec("branch_long", 0.05, 0.8, 0.3),
    spec("fork", 0.0, 1.0, 0.6),
    spec("thickness", 0.002, 0.03, 0.009),
    spec("glare", 0.0, 1.0, 0.5),
    // How brightly the electrode hums when nothing is discharging. At 0 the
    // centre is only visible while an arc is lit, which is the point this
    // model moved away from — see the module header.
    spec("shine_center", 0.0, 1.0, 0.35),
];

/// Hot core, blue-white, violet, deep blue. Linear values, so they read darker
/// written down than they arrive on screen — the targets encode sRGB on write.
///
/// Not white for the usual reason and one extra: the gradient is walked along
/// the *length* of the channel, so four white stops give a uniform white line
/// and the bolt loses the thing that makes it look hot at one end.
const BOLT: [f32; 12] = [
    1.00, 1.00, 1.00, //
    0.62, 0.76, 1.00, //
    0.22, 0.24, 0.85, //
    0.03, 0.04, 0.22,
];

/// **The one `fondo` in the suite that does not open black**, and the reason is
/// this model and not taste.
///
/// Every other renderer draws something in every frame, so its background is
/// the thing behind the subject and black is a fine place to start. This one is
/// empty most of the time on purpose — the darkness between strikes is what
/// makes a strike an event — so for the majority of any round the background
/// *is* the picture. Opening it black means opening the model on a black
/// rectangle that flashes occasionally, which is not what a storm looks like
/// and is not even obviously working.
///
/// It is also what the render gate found. `tests/render.rs` samples a couple of
/// instants and requires a frame that is not all black; at the defaults both of
/// its instants fall between strikes. The gate was right and the model was
/// wrong: a lit bolt is not the only thing this model owes the frame.
///
/// Linear values, so about 0.10 to 0.19 once the target encodes them — a dark
/// sky rather than an absence.
const SKY: [f32; 12] = [
    0.008, 0.010, 0.020, //
    0.014, 0.018, 0.034, //
    0.020, 0.024, 0.045, //
    0.030, 0.034, 0.060,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color_at("ray", &BOLT), color_at("background", &SKY), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

// ---------------------------------------------------------------------------
// the strike
// ---------------------------------------------------------------------------

fn hash(x: u32) -> f32 {
    let mut h = x.wrapping_add(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

/// Which round the clock is in, and how far into it.
pub fn round_at(interval: f32, time: f32) -> (u32, f32) {
    let r = interval.max(0.05);
    let t = time.max(0.0);
    let index = (t / r).floor().clamp(0.0, 2.0e9);
    (index as u32, t - index * r)
}

/// How bright the channel is right now, 0 if nothing is lit.
///
/// The re-strikes sit one after another with a gap of `duracion * 0.7` between
/// them, so a round is `destellos` flashes inside its first fraction of a
/// second and then darkness until the next round. The darkness is the point:
/// most of a round is nothing happening, which is what makes the strike an
/// event rather than an animation.
pub fn brightness(seed: u32, local: f32, duration: f32, flashes: u32) -> f32 {
    let d = duration.clamp(0.01, 0.8);
    let gap = d * 1.7;
    for k in 0..flashes.max(1) {
        let start = k as f32 * gap;
        let phase = (local - start) / d;
        if (0.0..1.0).contains(&phase) {
            // A hard attack and a square-law decay: up in a fraction of the
            // window and out over the rest of it.
            let attack = (phase / 0.08).min(1.0);
            let decay = (1.0 - phase) * (1.0 - phase);
            // Each re-strike weaker than a fresh one, and never all the same,
            // because evenly-matched flashes read as a machine.
            let amp = if k == 0 {
                1.0
            } else {
                0.35 + 0.5 * hash(seed ^ k.wrapping_mul(0x51ED_2701))
            };
            return (attack * decay * amp).clamp(0.0, 1.0);
        }
    }
    0.0
}

/// The channel: midpoint displacement from `from` to `to`.
///
/// Each pass splits every segment and pushes the new midpoint sideways by a
/// share of **that segment's own length**. The share itself is constant, which
/// looks wrong written down and is the whole thing: segments halve every pass,
/// so a constant share is a displacement that halves every pass, which is the
/// 1/f self-similarity that makes a coastline a coastline.
///
/// Halving the share *as well* is the mistake this had in it first, and it is
/// worth leaving on the record because the result renders perfectly and looks
/// like the wrong subject. Displacement then falls by four each pass instead of
/// two, fine detail dies within three levels, and the bolt comes out as a
/// smooth curve — an elegant arc across the frame that nobody would call
/// lightning. There is no gate that catches that.
///
/// The endpoints are never displaced, so a channel always starts and ends
/// exactly where it was aimed. `origen`/`destino` mean what they say and can be
/// pointed at a mask.
pub fn channel(seed: u32, from: Vec2, to: Vec2, dispersion: f32, levels: u32) -> Vec<Vec2> {
    let levels = levels.clamp(1, LEVELS_MAX);
    let mut pts = vec![from, to];
    let amount = dispersion.clamp(0.0, 0.6);

    for pass in 0..levels {
        let mut next = Vec::with_capacity(pts.len() * 2 - 1);
        for i in 0..pts.len() - 1 {
            let (a, b) = (pts[i], pts[i + 1]);
            let mid = (a + b) * 0.5;
            let along = b - a;
            let normal = Vec2::new(-along.y, along.x).normalize_or_zero();
            let key = seed
                ^ (pass.wrapping_mul(0x9E37_79B9))
                ^ ((i as u32).wrapping_mul(0x27D4_EB2F));
            let push = (hash(key) - 0.5) * 2.0 * amount * along.length();
            next.push(a);
            next.push(mid + normal * push);
        }
        next.push(*pts.last().expect("a channel always has two ends"));
        pts = next;
    }

    pts
}

/// Where a branch leaves the channel, and where it goes.
///
/// Branches fork off the main channel between a fifth and four fifths of the
/// way along — never at the ends, because a branch at the strike point reads as
/// a second bolt and a branch at the origin reads as a mistake.
pub fn branch_from(
    seed: u32,
    k: usize,
    main: &[Vec2],
    length: f32,
    fork: f32,
) -> (Vec2, Vec2) {
    let key = seed ^ ((k as u32 + 1).wrapping_mul(0x85EB_CA6B));
    let share = 0.2 + 0.6 * hash(key);
    let at = ((main.len() - 1) as f32 * share) as usize;
    let from = main[at];

    // The direction the channel is going where the branch leaves it, turned
    // aside. A branch that ignored the local direction would fork backwards up
    // its own bolt.
    let ahead = main[(at + 1).min(main.len() - 1)];
    let behind = main[at.saturating_sub(1)];
    let dir = (ahead - behind).normalize_or_zero();
    let side = if hash(key ^ 0x1234_5678) < 0.5 { -1.0 } else { 1.0 };
    let turn = (0.35 + 0.75 * hash(key ^ 0xABCD)) * side * fork.clamp(0.0, 1.0);
    let (s, c) = turn.sin_cos();
    let out = Vec2::new(dir.x * c - dir.y * s, dir.x * s + dir.y * c);

    let span = (main[main.len() - 1] - main[0]).length().max(0.1);
    (from, from + out * span * length.clamp(0.05, 0.8))
}

// ---------------------------------------------------------------------------
// the GPU side
// ---------------------------------------------------------------------------

/// Turns a path into a triangle strip of a given half-width, tapering to a
/// point at its far end so a branch ends in a tip rather than a stub.
fn strip_of(pts: &[Vec2], half: f32, taper: bool, out: &mut Vec<Vertex>) -> Strip {
    let start = out.len() as u32;
    for (i, p) in pts.iter().enumerate() {
        let s = i as f32 / (pts.len() - 1).max(1) as f32;
        let ahead = pts[(i + 1).min(pts.len() - 1)];
        let behind = pts[i.saturating_sub(1)];
        let tangent = (ahead - behind).normalize_or_zero();
        let normal = Vec2::new(-tangent.y, tangent.x);
        let w = if taper { half * (1.0 - s * 0.85).max(0.05) } else { half };
        out.push(Vertex::new(*p + normal * w, s));
        out.push(Vertex::new(*p - normal * w, s));
    }
    Strip { start, len: out.len() as u32 - start }
}

pub struct Relampago {
    pipeline: Built<Line2dPipeline>,
    backdrop: Built<Backdrop>,
}

impl Relampago {
    pub fn new() -> Relampago {
        Relampago { pipeline: Built::default(), backdrop: Built::default() }
    }
}

impl Model for Relampago {
    fn id(&self) -> &'static str {
        "lightning"
    }

    fn name(&self) -> &'static str {
        "Lightning"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn backdrop(&self) -> bool {
        true
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let interval = floats[0];
        let duration = floats[1];
        let flashes = floats[2].round().clamp(1.0, 5.0) as u32;
        let center = Vec2::new(floats[3], floats[4]);
        let reach = floats[5].max(0.05);
        let num_arcs = floats[6].round().clamp(1.0, ARCS_MAX as f32) as usize;
        let dispersion = floats[7];
        let levels = floats[8].round().clamp(1.0, LEVELS_MAX as f32) as u32;
        let branches = floats[9].round().clamp(0.0, BRANCHES_MAX as f32) as usize;
        let branch_length = floats[10];
        let fork = floats[11];
        let thickness = floats[12].max(0.0005);
        let glow = floats[13].clamp(0.0, 1.0);
        let center_glow = floats[14].clamp(0.0, 1.0);
        let bolt = ColorSlotGpu::from_floats(&floats[15..27]);
        let background = ColorSlotGpu::from_floats(&floats[27..39]);

        // The background is always drawn, strike or no strike, and it is the
        // pass that clears — `line2d` loads.
        self.backdrop
            .get(format, || Backdrop::new(ctx.device, format))
            .draw(ctx, target, background, 0.0, ctx.fondo_alpha);

        let (round, local) = round_at(interval, ctx.time);
        let seed = round.wrapping_mul(0x9E37_79B9).wrapping_add(101);
        let lit = brightness(seed, local, duration, flashes);

        let pipeline = self
            .pipeline
            .get(format, || Line2dPipeline::new(ctx.device, format));

        // The electrode. Drawn every frame — strike or no strike, which is the
        // whole difference between a coil and a storm — as a short, almost
        // degenerate segment whose *width* does the work of looking like a
        // point. A fast, small-amplitude hum keeps it from reading as a
        // sticker pasted on the frame; `lit` adds most of a strike's
        // brightness on top of it rather than the discharge and the electrode
        // being two unrelated brightnesses.
        let hum = 0.85 + 0.15 * (ctx.time * 23.0).sin();
        let core_glow = (center_glow * hum + lit * 0.6).clamp(0.0, 1.6);
        if core_glow > 0.002 {
            let jitter = Vec2::new(
                (hash(seed ^ (ctx.time * 47.0) as u32) - 0.5) * 0.01,
                (hash(seed ^ (ctx.time * 71.0) as u32 ^ 0x55) - 0.5) * 0.01,
            );
            let tip = center + jitter + Vec2::new(0.0001, 0.0);
            if glow > 0.01 {
                let mut halo_verts = Vec::new();
                let mut halo_strips = Vec::new();
                halo_strips.push(strip_of(&[center, tip], thickness * 9.0, false, &mut halo_verts));
                let dim = scaled(bolt, core_glow * glow * 0.3);
                pipeline.draw(ctx, target, &halo_verts, &halo_strips, dim);
            }
            let mut core_verts = Vec::new();
            let mut core_strips = Vec::new();
            core_strips.push(strip_of(&[center, tip], thickness * 3.5, false, &mut core_verts));
            pipeline.draw(ctx, target, &core_verts, &core_strips, scaled(bolt, core_glow));
        }

        if lit <= 0.002 {
            // Most of a round is this branch, and that is the design: the
            // darkness between strikes is what makes a strike an event. The
            // electrode above still fired, which is what makes the darkness
            // read as "waiting" rather than "off".
            return;
        }

        // Every arc starts at the electrode and reaches outward — evenly
        // spaced around it, then jittered per arc from the round's seed so a
        // three-arc discharge does not point the same three ways twice.
        let mut vertices = Vec::new();
        let mut strips = Vec::new();
        let mut halo_verts = Vec::new();
        let mut halo_strips = Vec::new();
        let mut mains: Vec<Vec<Vec2>> = Vec::with_capacity(num_arcs);

        for arc in 0..num_arcs {
            let arc_seed = seed ^ (arc as u32 + 1).wrapping_mul(0x2545_F491);
            let base_angle = (arc as f32 / num_arcs as f32) * TAU;
            let jitter_angle = (hash(arc_seed) - 0.5) * (TAU / num_arcs as f32) * 0.8;
            let angle = base_angle + jitter_angle;
            let end = center + Vec2::new(angle.cos(), angle.sin()) * reach;

            let main = channel(arc_seed, center, end, dispersion, levels);
            strips.push(strip_of(&main, thickness * 0.5, false, &mut vertices));
            if glow > 0.01 {
                halo_strips.push(strip_of(&main, thickness * 2.4, false, &mut halo_verts));
            }
            mains.push(main);
        }

        // Branches are shared round-robin across the arcs rather than given
        // to each arc in full — see the module header — so `ramas` bounds the
        // total regardless of `num_arcos`.
        for k in 0..branches {
            let arc = k % num_arcs;
            let arc_seed = seed ^ (arc as u32 + 1).wrapping_mul(0x2545_F491);
            let main = &mains[arc];

            // Not every branch is lit on every re-strike. Keyed on the round
            // only — see the module header on why the channel must not change
            // between the flashes of one round — but on which flash it is, so
            // the bolt flickers its own limbs rather than being a still image
            // dimmed and brightened.
            let flash = (local / (duration.max(0.01) * 1.7)).floor() as u32;
            if hash(arc_seed ^ (k as u32).wrapping_mul(0x2545_F491) ^ flash.wrapping_mul(7)) > 0.65 {
                continue;
            }
            let (a, b) = branch_from(arc_seed, k, main, branch_length, fork);
            let sub = channel(
                arc_seed ^ (k as u32 + 1).wrapping_mul(0xC2B2_AE35),
                a,
                b,
                dispersion,
                levels.saturating_sub(2).max(1),
            );
            strips.push(strip_of(&sub, thickness * 0.45, true, &mut vertices));
        }

        // The corona first, underneath — see the module header on why this is
        // a second draw and not a blur.
        if glow > 0.01 {
            let dim = scaled(bolt, lit * glow * 0.22);
            pipeline.draw(ctx, target, &halo_verts, &halo_strips, dim);
        }

        pipeline.draw(ctx, target, &vertices, &strips, scaled(bolt, lit));
    }
}

/// The slot with every stop scaled — how a strike fades, given that `line2d`'s
/// fragment stage returns alpha 1.0 and there is nothing to fade *with*.
fn scaled(slot: ColorSlotGpu, by: f32) -> ColorSlotGpu {
    let mut out = slot;
    for c in out.c.iter_mut() {
        c[0] *= by;
        c[1] *= by;
        c[2] *= by;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Vec<f32> {
        SCHEMA.defaults()
    }

    #[test]
    fn the_schema_and_the_slices_render_reads_are_the_same_width() {
        // 15 floats + two colour slots. `render` slices `[15..27]` and `[27..39]`.
        assert_eq!(SCHEMA.width(), 39 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 39 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn the_sky_is_never_black_because_most_of_the_time_it_is_the_whole_picture() {
        // Stated here rather than left to `tests/render.rs` to discover again.
        // This model spends most of every round drawing nothing but its
        // background, so a black background is a black frame — which the render
        // gate reads, correctly, as a broken renderer.
        let f = row();
        let background = &f[27..39];
        assert_eq!(background.len(), 12);
        assert!(
            background.iter().any(|&c| c > 0.004),
            "the sky opens black, and between strikes that is the entire frame"
        );
    }

    #[test]
    fn arcs_jitter_apart_from_evenly_spaced_and_differ_between_rounds() {
        // The property the module header claims: `num_arcos` arcs start from
        // evenly spaced base angles around the electrode, each nudged by its
        // own hash, so a coil with three arcs does not point the same three
        // ways every time it fires.
        let num_arcs = 3usize;
        let angles_for = |seed: u32| -> Vec<f32> {
            (0..num_arcs)
                .map(|arc| {
                    let arc_seed = seed ^ (arc as u32 + 1).wrapping_mul(0x2545_F491);
                    let base = (arc as f32 / num_arcs as f32) * TAU;
                    let jitter = (hash(arc_seed) - 0.5) * (TAU / num_arcs as f32) * 0.8;
                    base + jitter
                })
                .collect()
        };
        let bases: Vec<f32> = (0..num_arcs).map(|arc| (arc as f32 / num_arcs as f32) * TAU).collect();
        assert_eq!(bases, vec![0.0, TAU / 3.0, TAU * 2.0 / 3.0]);

        let a = angles_for(11);
        let b = angles_for(29);
        assert_ne!(a, b, "two different seeds pointed the coil's arcs the same way");
    }

    #[test]
    fn a_channel_starts_and_ends_exactly_where_it_was_aimed() {
        // `origen`/`destino` are coordinates an operator points at a thing in
        // the room. A displacement that moved the endpoints would make them
        // approximate, and there would be no way to strike a fixed mark.
        let from = Vec2::new(-0.4, 1.1);
        let to = Vec2::new(0.6, -1.0);
        for levels in 1..=LEVELS_MAX {
            for &d in &[0.0f32, 0.17, 0.6] {
                let pts = channel(7, from, to, d, levels);
                assert_eq!(pts[0], from);
                assert_eq!(*pts.last().unwrap(), to);
                assert_eq!(pts.len(), (1 << levels) + 1, "wrong segment count at nivel {levels}");
            }
        }
    }

    #[test]
    fn no_dispersion_leaves_a_straight_line() {
        let from = Vec2::new(0.0, 1.0);
        let to = Vec2::new(0.0, -1.0);
        for p in channel(3, from, to, 0.0, 6) {
            assert!(p.x.abs() < 1e-5, "a channel with no dispersion bent to x={}", p.x);
        }
    }

    #[test]
    fn the_widest_dispersion_still_keeps_the_bolt_near_the_frame() {
        // The failure mode this model has that nothing else catches: a
        // displacement that compounds throws the channel off screen, and the
        // frame is a backdrop with a strike happening somewhere outside it.
        // Renders fine, deterministic, not black, and no lightning.
        let from = Vec2::new(0.0, 1.2);
        let to = Vec2::new(0.0, -1.2);
        for seed in 0..64u32 {
            for p in channel(seed, from, to, 0.6, LEVELS_MAX) {
                assert!(
                    p.x.abs() < 3.0 && p.y.abs() < 3.0,
                    "seed {seed} threw the channel to {p:?}"
                );
            }
        }
    }

    #[test]
    fn most_of_a_round_is_dark_and_some_of_it_is_not() {
        // Both halves matter. A bolt that is always lit is a line, and one that
        // is never lit is a black frame — and the render gate reads the second
        // as a failure only by luck, since the backdrop is not quite black.
        let f = row();
        let (interval, duration, flashes) = (f[0], f[1], f[2] as u32);
        let mut lit = 0;
        let mut dark = 0;
        let mut peak = 0.0f32;
        let mut local = 0.0;
        while local < interval {
            let b = brightness(11, local, duration, flashes);
            assert!(b.is_finite() && (0.0..=1.0).contains(&b));
            if b > 0.01 {
                lit += 1;
            } else {
                dark += 1;
            }
            peak = peak.max(b);
            local += 0.004;
        }
        assert!(peak > 0.8, "the strike never reached full brightness: {peak}");
        assert!(lit > 0, "the bolt never struck at all");
        assert!(dark > lit * 2, "the bolt was lit for {lit} of {} samples", lit + dark);
    }

    #[test]
    fn every_re_strike_of_one_round_is_the_same_channel() {
        // The detail that makes it read as lightning rather than as several
        // bolts in a row. Both flashes are generated from the round's seed, so
        // this is a check that nothing ever starts mixing the flash index in.
        let f = row();
        let (round_a, _) = round_at(f[0], 3.0);
        let seed = round_a.wrapping_mul(0x9E37_79B9).wrapping_add(101);
        let a = channel(seed, Vec2::new(0.0, 1.1), Vec2::new(0.1, -1.05), 0.17, 6);
        let b = channel(seed, Vec2::new(0.0, 1.1), Vec2::new(0.1, -1.05), 0.17, 6);
        assert_eq!(a, b);
    }

    #[test]
    fn the_next_round_strikes_somewhere_else() {
        let a_seed = round_at(1.1, 0.2).0.wrapping_mul(0x9E37_79B9).wrapping_add(101);
        let b_seed = round_at(1.1, 1.3).0.wrapping_mul(0x9E37_79B9).wrapping_add(101);
        let ends = (Vec2::new(0.0, 1.1), Vec2::new(0.1, -1.05));
        let a = channel(a_seed, ends.0, ends.1, 0.17, 6);
        let b = channel(b_seed, ends.0, ends.1, 0.17, 6);
        assert_ne!(a, b, "two rounds drew the same bolt");
    }

    #[test]
    fn a_branch_leaves_the_channel_from_somewhere_in_its_middle() {
        // Never at the ends: a branch at the strike point reads as a second
        // bolt, and one at the origin reads as a mistake.
        let main = channel(5, Vec2::new(0.0, 1.1), Vec2::new(0.0, -1.05), 0.17, 6);
        for k in 0..BRANCHES_MAX {
            let (from, to) = branch_from(5, k, &main, 0.3, 0.6);
            assert!(from != main[0] && from != *main.last().unwrap());
            assert!(from.distance(to) > 1e-3, "branch {k} went nowhere");
            assert!(to.x.is_finite() && to.y.is_finite());
        }
    }

    #[test]
    fn asking_for_the_same_instant_twice_gives_the_same_bolt() {
        let f = row();
        let (r1, l1) = round_at(f[0], 4.03);
        let (r2, l2) = round_at(f[0], 4.03);
        assert_eq!(r1, r2);
        assert_eq!(l1, l2);
        let seed = r1.wrapping_mul(0x9E37_79B9).wrapping_add(101);
        assert_eq!(
            brightness(seed, l1, f[1], 3),
            brightness(seed, l2, f[1], 3)
        );
    }
}
