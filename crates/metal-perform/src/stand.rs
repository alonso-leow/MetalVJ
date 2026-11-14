//! The stand-in painter — the suite, drawn on the CPU, for hosts with no GPU.
//!
//! Every model becomes one of six cheap procedural figures, keyed off its
//! index. It is not what the model looks like and it does not pretend to be;
//! `Paint::real()` returns false and the page prints it next to the digest's
//! own warning. What it *is* is a real picture that is different per model,
//! moves with the digest, honours the shared effects and costs a few
//! milliseconds on one core — which is exactly enough to review a layout with.
//!
//! # What it takes from the real thing rather than inventing
//!
//! Three things, and taking them is the difference between a review surface and
//! a screensaver:
//!
//! - **The float row.** Not the defaults — the row `drive::row` produced for
//!   this model at this instant, mapped the same way it would be for the real
//!   renderer. So turning the drive knob down visibly stills the picture, and
//!   the monitor's float bars agree with what is on screen.
//! - **The colour slot**, if the model has one. So the palette button changes
//!   the colour of the picture, because harmony really is driving it.
//! - **The shared effects.** Shake, light leak, mirrors and the trail all run
//!   here, as coordinate and colour operations rather than as passes. Four
//!   footer knobs that did nothing would read as a broken app.
//!
//! What it cannot take is the models themselves, which are shaders.
//!
//! # Why six figures and not seventeen
//!
//! Because seventeen bad imitations is seventeen chances to be quietly wrong
//! about what a model looks like, and somebody reviewing a layout would start
//! believing them. Six obviously-synthetic figures, assigned round-robin, are
//! legible as *"these are placeholders, and that one is a different one from
//! this one"* — which is all a layer stack needs from them.

use metal_schema::{Block, Manifest, Post, Schema};

use crate::paint::Paint;

/// `data/schema.json`, baked in.
///
/// Embedded rather than read from disk because this build exists to be copied
/// to a host that has the binary and nothing else. `metal-render`'s
/// `tests/schema.rs` is what keeps the file honest against the renderers, and
/// this is that file.
const SCHEMA: &str = include_str!("../../../data/schema.json");

/// The eighteenth model is planned, not built, in both painters — see
/// `main.rs`'s `UNBUILT`. Nothing here needs to know that; it draws whatever
/// the manifest lists.
pub struct Stand {
    ids: Vec<&'static str>,
    names: Vec<&'static str>,
    schemas: Vec<&'static Schema>,
    /// Which entries are transmissions. Carried through from the manifest
    /// because this painter has no renderers to ask — and it is the painter
    /// that most needs it: the surface that says a transmission has no folder
    /// yet is exactly what somebody reviewing the app on a phone is looking at.
    footage: Vec<bool>,
    /// Which entries draw their background via the shared `Backdrop` pass —
    /// see `metal_render::model::Model::backdrop`. Carried through from the
    /// manifest for the same reason `footage` is: this painter has no
    /// renderers to ask.
    backdrop: Vec<bool>,
}

impl Stand {
    pub fn new() -> Stand {
        let suite = Manifest::parse(SCHEMA)
            .expect("the baked-in schema.json parses")
            .into_suite();
        let mut ids = Vec::new();
        let mut names = Vec::new();
        let mut schemas = Vec::new();
        let mut footage = Vec::new();
        let mut backdrop = Vec::new();
        for e in suite {
            // Leaked for the same reason the schemas are, and at the same
            // moment: a suite is fixed for the life of the process.
            ids.push(&*Box::leak(e.id.into_boxed_str()));
            names.push(&*Box::leak(e.name.into_boxed_str()));
            schemas.push(e.schema);
            footage.push(e.footage);
            backdrop.push(e.backdrop);
        }
        Stand {
            ids,
            names,
            schemas,
            footage,
            backdrop,
        }
    }

    /// Draws one model into a fresh RGBA buffer.
    fn draw(&self, index: usize, floats: &[f32], time: f32, w: u32, h: u32) -> Vec<u8> {
        let mut px = vec![0u8; (w as usize) * (h as usize) * 4];
        self.draw_into(&mut px, w, w, h, 0, 0, index, floats, time);
        px
    }

    /// Draws model `index` into a `w x h` rectangle at `(ox, oy)` of a buffer
    /// whose row stride is `stride` pixels.
    ///
    /// One function for both entry points, so a tile of the rack and the same
    /// model in the preview cannot drift apart.
    #[allow(clippy::too_many_arguments)]
    fn draw_into(
        &self,
        px: &mut [u8],
        stride: u32,
        w: u32,
        h: u32,
        ox: u32,
        oy: u32,
        index: usize,
        floats: &[f32],
        time: f32,
    ) {
        // Rows come off sockets and older files (`a_short_row_does_not_
        // panic_the_painter`); `paint.rs` guards `read_post` the same way —
        // shorter than the effects block reads as "no effects", the CPU
        // stand-in defaults, not a panic.
        let post = if floats.len() >= metal_schema::EFFECTS_WIDTH {
            metal_schema::read_post(floats)
        } else {
            metal_schema::Post::default()
        };
        let figure = index % FIGURES;
        let sig = Signature::of(self.schemas.get(index).copied(), floats, index);
        // The trail is a second sample a little way back in time, mixed in —
        // not a frame history. It reads as a trail on a moving figure, which is
        // what the knob is for, and it costs one extra evaluation rather than a
        // texture per model.
        let ghost = post.ghost.clamp(0.0, 1.0);
        let lag = time - post.ghost_tau();
        // The advanced block (`fx.grade` and the six after it) — parsed once
        // here, applied per pixel below. Neutral is the identity, so an
        // ungraded frame is byte-identical to before this existed.
        let adv = AdvancedFx::of(&post);

        for y in 0..h {
            for x in 0..w {
                // Normalised to -1..1 with the aspect folded in, so a figure is
                // the same shape on a 4:3 projector and a 16:9 wall.
                let aspect = w as f32 / h.max(1) as f32;
                let u = ((x as f32 + 0.5) / w as f32 * 2.0 - 1.0) * aspect;
                let v = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
                let (u, v) = effects(u, v, &post, time);
                // Geometry is a UV transform, so it belongs here — before the
                // field is sampled, the same place a real pass resamples.
                let geo_uv = adv.geometry([u, v]);
                let geo_uv = adv.fm(geo_uv);
                let geo_uv = adv.wavewarp(geo_uv, time);
                let (u, v) = (geo_uv[0], geo_uv[1]);

                let mut a = field(figure, u, v, time, &sig);
                if ghost > 0.0 {
                    let b = field(figure, u, v, lag, &sig);
                    a = a.max(b * ghost);
                }

                // Blur and sharpen have no framebuffer to sample neighbours
                // from here, the way the shader pass does — the stand-in for
                // both is the same field re-sampled at a few nearby taps
                // instead of a rendered pixel, averaged the same shape a box
                // blur averages texels.
                let blur = post.blur.clamp(0.0, 1.0);
                let sharpen = post.sharpen.clamp(0.0, 1.0);
                if blur > 0.0 || sharpen > 0.0 {
                    let r = 0.03 * (blur.max(sharpen));
                    let mut nbr = 0.0;
                    for i in 0..4 {
                        let ang = std::f32::consts::TAU * (i as f32) / 4.0;
                        nbr += field(figure, u + ang.cos() * r, v + ang.sin() * r, time, &sig);
                    }
                    nbr *= 0.25;
                    if blur > 0.0 {
                        a += (nbr - a) * blur;
                    }
                    if sharpen > 0.0 {
                        a = (a + (a - nbr) * sharpen * 2.0).clamp(0.0, 1.0);
                    }
                }

                // The gradient says *which* colour; the field says how much
                // light. Both, because a slot's four stops are often the same
                // colour — every model's own defaults are solid white — and a
                // figure shaded only by picking a stop out of four identical
                // ones is a flat rectangle. Found by this module's own
                // "is this a picture" test, which is exactly what it is for.
                let gain = 0.06 + 0.94 * a;
                let base = sig.shade(a);
                let mut rgb = [base[0] * gain, base[1] * gain, base[2] * gain];
                leak(&mut rgb, u, v, &post, time);
                effects_color(&mut rgb, u, v, &post, time);
                adv.apply(&mut rgb, [u, v]);

                let p = (((oy + y) as usize) * (stride as usize) + (ox + x) as usize) * 4;
                px[p] = to_byte(rgb[0]);
                px[p + 1] = to_byte(rgb[1]);
                px[p + 2] = to_byte(rgb[2]);
                px[p + 3] = 255;
            }
        }
    }
}

impl Default for Stand {
    fn default() -> Stand {
        Stand::new()
    }
}

impl Paint for Stand {
    fn model_count(&self) -> usize {
        self.ids.len()
    }
    fn ids(&self) -> Vec<&'static str> {
        self.ids.clone()
    }
    fn names(&self) -> Vec<&'static str> {
        self.names.clone()
    }
    fn schemas(&self) -> Vec<&'static Schema> {
        self.schemas.clone()
    }

    fn solo(
        &mut self,
        index: usize,
        floats: &[f32],
        time: f32,
        w: u32,
        h: u32,
        // The stand-in draws placeholder shapes on the CPU, with no shared
        // `Backdrop` pass of its own to make transparent — ignored.
        _fondo_alpha: bool,
    ) -> (Vec<u8>, u32, u32) {
        (
            self.draw(
                index.min(self.ids.len().saturating_sub(1)),
                floats,
                time,
                w,
                h,
            ),
            w,
            h,
        )
    }

    /// K: the one effect shader this painter can run for real. `fx.move` is a
    /// plain pixel translation, so the stand-in does it on the CPU rather than
    /// passing the frame through — a host with no adapter still sees the
    /// content move behind its mask, which is the whole point of the node.
    /// Any other shader (a user's `fx.forge`) passes through, the same answer
    /// the trait's default gives.
    fn forge_fx(
        &mut self,
        key: &str,
        _wgsl: &str,
        frame: &(Vec<u8>, u32, u32),
        time: f32,
        ingest: &[f32],
    ) -> Option<Vec<u8>> {
        if key != crate::motion::MOVE_SHADER {
            return None;
        }
        let mut p = [0.0f32; 5];
        for (i, v) in ingest.iter().take(5).enumerate() {
            p[i] = *v;
        }
        Some(crate::motion::translate(frame, p, time).0)
    }

    fn footage(&self) -> Vec<bool> {
        self.footage.clone()
    }

    fn backdrop(&self) -> Vec<bool> {
        self.backdrop.clone()
    }

    /// Never. There is no renderer here to hand footage to, and the page
    /// already says these are stand-ins — a painter that accepted a library and
    /// then drew a figure would be the comforting lie `real()` exists to
    /// prevent.
    fn reel(&mut self, _index: usize) -> Option<&mut dyn metal_reel::Reel> {
        None
    }

    fn real(&self) -> bool {
        false
    }

    fn describe(&self) -> String {
        format!(
            "{} stand-in figures on the CPU — no GPU on this host",
            self.ids.len()
        )
    }
}

// ---------------------------------------------------------------------------
// the figures
// ---------------------------------------------------------------------------

const FIGURES: usize = 6;

/// What this model's row says about how it should look and move right now.
struct Signature {
    /// 0..1, from the row's most expressive float. Drives size and count.
    weight: f32,
    /// How fast, from a second float.
    speed: f32,
    /// The four colours of the model's slot, or a grey ramp if it has none.
    ramp: [[f32; 3]; 4],
}

impl Signature {
    fn of(schema: Option<&'static Schema>, floats: &[f32], index: usize) -> Signature {
        let mut weight = 0.5;
        let mut speed = 1.0;
        // A grey ramp keyed off the index, so a model with no colour slot is
        // still distinguishable from its neighbour in the rack.
        let base = 0.35 + (index % 5) as f32 * 0.11;
        let mut ramp = [[base, base, base]; 4];

        if let Some(schema) = schema {
            // Which colour slot the figure is drawn in.
            //
            // **The first one that is not the background**, and this is not a
            // nicety. It used to be whichever slot came last, which was fine
            // until `metal_schema::fondo` (2026-08-21) gave eleven models a
            // background slot at the end of their schema — and a background
            // slot starts *solid black*, so every figure on this painter drew
            // black on black. The whole GPU-free review surface went dark and
            // nothing errored: a rack of correctly-sized, correctly-composed,
            // entirely black tiles.
            //
            // A model whose only slot is its background still uses it — that is
            // a model whose one colour choice is the background, and drawing it
            // grey instead would be inventing a colour it does not have.
            let paint_slot = {
                let mut at = 0usize;
                let mut first_any = None;
                let mut first_named = None;
                for block in schema.blocks {
                    if let Block::ColorSlot(name, _) = block {
                        first_any.get_or_insert(at);
                        if *name != "background" {
                            first_named.get_or_insert(at);
                        }
                    }
                    at += block.width();
                }
                first_named.or(first_any)
            };

            let mut at = 0usize;
            let mut seen = 0usize;
            for block in schema.blocks {
                match block {
                    Block::Floats(specs) => {
                        for (k, spec) in specs.iter().enumerate() {
                            let Some(v) = floats.get(at + k) else {
                                continue;
                            };
                            // Normalised into the float's own declared range —
                            // the same normalisation the monitor's bars use, so
                            // the picture and the bars move together.
                            let span = (spec.max - spec.min).abs().max(1e-6);
                            let unit = ((v - spec.min) / span).clamp(0.0, 1.0);
                            match seen {
                                0 => weight = unit,
                                1 => speed = 0.25 + unit * 2.0,
                                _ => {}
                            }
                            seen += 1;
                        }
                    }
                    Block::ColorSlot(..) => {
                        if paint_slot == Some(at) && floats.len() >= at + 12 {
                            for (i, stop) in ramp.iter_mut().enumerate() {
                                stop[0] = floats[at + i * 3];
                                stop[1] = floats[at + i * 3 + 1];
                                stop[2] = floats[at + i * 3 + 2];
                            }
                        }
                    }
                    Block::Camera(_) => {}
                }
                at += block.width();
            }
        }
        // A model whose chosen slot defaults to black — a parallax world whose
        // first slot is its near-black backdrop, say — would draw black on
        // black on this CPU review surface. The grey ramp keyed off the index
        // keeps the rack legible, the same trade the "no colour slot" case
        // already makes.
        let brightest = ramp.iter().flatten().fold(0.0f32, |m, v| m.max(*v));
        if brightest < 0.02 {
            ramp = [[base, base, base]; 4];
        }
        Signature {
            weight: weight.clamp(0.0, 1.0),
            speed,
            ramp,
        }
    }

    /// The four-stop gradient the renderers' `grad4` draws with, evaluated at
    /// `t` in 0..1.
    fn shade(&self, t: f32) -> [f32; 3] {
        let t = t.clamp(0.0, 1.0) * 3.0;
        let i = (t as usize).min(2);
        let f = t - i as f32;
        let (a, b) = (self.ramp[i], self.ramp[i + 1]);
        [
            a[0] + (b[0] - a[0]) * f,
            a[1] + (b[1] - a[1]) * f,
            a[2] + (b[2] - a[2]) * f,
        ]
    }
}

/// The scalar field each figure is a level set of, in 0..1.
fn field(figure: usize, u: f32, v: f32, t: f32, sig: &Signature) -> f32 {
    let r = (u * u + v * v).sqrt();
    let a = v.atan2(u);
    let phase = t * sig.speed;
    let n = 2.0 + sig.weight * 10.0;

    let raw = match figure {
        // Concentric rings, breathing.
        0 => ((r * n * 3.0 - phase * 2.0).sin() * 0.5 + 0.5) * (1.0 - r * 0.55),
        // Vertical bars, sliding.
        1 => ((u * n * 2.0 + phase).sin() * 0.5 + 0.5) * (1.0 - v.abs() * 0.35),
        // A diagonal sweep.
        2 => ((u + v) * n * 1.5 - phase * 1.7).sin() * 0.5 + 0.5,
        // A grid of cells, pulsing out of phase across the frame.
        3 => {
            let gx = (u * n).sin();
            let gy = (v * n).sin();
            (gx * gy).abs() * (0.6 + 0.4 * (phase * 1.3).sin())
        }
        // Radial spokes.
        4 => {
            ((a * (2.0 + sig.weight * 8.0).round() + phase).sin() * 0.5 + 0.5)
                * (1.0 - (r - 0.5).abs()).clamp(0.0, 1.0)
        }
        // Plasma: three interfering waves, the cheapest thing that reads as
        // organic rather than as a test card.
        _ => {
            let s = (u * n + phase).sin()
                + (v * n * 0.8 - phase * 1.1).sin()
                + ((u + v) * n * 0.6 + phase * 0.7).sin();
            s / 3.0 * 0.5 + 0.5
        }
    };
    raw.clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// the shared effects
// ---------------------------------------------------------------------------

/// Shake and the mirrors, as coordinate operations.
///
/// The real chain does these as passes over a rendered frame. Doing them to the
/// coordinates instead is not the same image, but it is the same *control*: the
/// knob moves the picture the direction it says it does, by roughly the amount
/// it says. That is what a knob has to do to be reviewable.
fn effects(mut u: f32, mut v: f32, post: &Post, time: f32) -> (f32, f32) {
    let shake = post.shake.clamp(0.0, 1.0);
    if shake > 0.0 {
        // Two frequencies that do not divide each other, so it reads as a
        // camera being knocked rather than as a wobble.
        u += (time * 37.0).sin() * 0.06 * shake;
        v += (time * 23.0).cos() * 0.06 * shake;
    }
    let mirrors = post.kaleido;
    if mirrors >= 2.0 {
        // The same fold the Kaleidoscope model draws with: wrap the angle into
        // one wedge and mirror alternate wedges so the seams meet.
        let r = (u * u + v * v).sqrt();
        let wedge = std::f32::consts::TAU / mirrors;
        let mut a = v.atan2(u).rem_euclid(wedge * 2.0);
        if a > wedge {
            a = wedge * 2.0 - a;
        }
        u = a.cos() * r;
        v = a.sin() * r;
    }
    let zoom = post.zoom.clamp(-1.0, 1.0);
    if zoom != 0.0 {
        // Same sign convention as `effect_zoom` in the real pass: positive
        // punches in, negative pulls out.
        let scale = 1.0 - zoom * 0.5;
        u *= scale;
        v *= scale;
    }
    let glitch = post.glitch.clamp(0.0, 1.0);
    if glitch > 0.0 {
        // A row-quantised horizontal jitter that changes a few times a
        // second — reads as a glitch rather than a wobble because it holds
        // still between changes instead of drifting continuously.
        let row = (v * 18.0).floor();
        let step = (time * 6.0).floor();
        let jitter = ((row * 12.9898 + step * 78.233).sin() * 43758.5453).fract();
        u += (jitter - 0.5) * 0.3 * glitch;
    }
    (u, v)
}

/// The light leak: a moving warm wash across one corner.
fn leak(rgb: &mut [f32; 3], u: f32, v: f32, post: &Post, time: f32) {
    let amount = post.light_leak.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return;
    }
    let sweep = ((u * 0.7 + v * 0.3 + (time * 0.31).sin() * 1.4) * 1.1)
        .cos()
        .max(0.0);
    let colour = [
        post.leak_color.first().copied().unwrap_or(1.0),
        post.leak_color.get(1).copied().unwrap_or(0.9),
        post.leak_color.get(2).copied().unwrap_or(0.6),
    ];
    for c in 0..3 {
        rgb[c] += colour[c] * sweep * sweep * amount * 0.55;
    }
}

/// The four picks that read as a colour change rather than a coordinate
/// warp — same stand-in contract as `effects`/`leak`: roughly the direction
/// and amount the knob claims, not the real pass.
fn effects_color(rgb: &mut [f32; 3], u: f32, v: f32, post: &Post, time: f32) {
    let crt = post.crt.clamp(0.0, 1.0);
    if crt > 0.0 {
        let scan = (v * 220.0).sin() * 0.5 + 0.5;
        let vignette = (1.0 - (u * u + v * v) * 0.35).max(0.0);
        let mixed = (1.0 - crt) + crt * scan.max(0.35) * vignette;
        for c in rgb.iter_mut() {
            *c *= mixed;
        }
    }
    let posterize = post.posterize.clamp(0.0, 1.0);
    if posterize > 0.0 {
        let levels = 10.0 - posterize * 7.0;
        for c in rgb.iter_mut() {
            let stepped = (*c * levels).round() / levels;
            *c = *c * (1.0 - posterize) + stepped * posterize;
        }
    }
    let fire = post.fire.clamp(0.0, 1.0);
    if fire > 0.0 {
        let flicker =
            ((time * 9.0 + u * 6.0).sin() * 0.5 + 0.5) * ((time * 5.3 - v * 4.0).sin() * 0.5 + 0.5);
        let heat = (((1.0 - v) * 0.5 + flicker * 0.5).clamp(0.0, 1.0)) * fire;
        rgb[0] += heat * 0.9;
        rgb[1] += heat * 0.35;
        rgb[2] += heat * 0.05;
    }
    let lorenz = post.lorenz.clamp(0.0, 1.0);
    if lorenz > 0.0 {
        // A cheap stand-in for the attractor's restless drift, not the
        // attractor itself — three offset sines walking through colour.
        let t = time * 0.6;
        rgb[0] += ((t + u * 3.0).sin() * 0.5 + 0.5) * 0.4 * lorenz;
        rgb[1] += ((t * 1.3 + v * 3.0).sin() * 0.5 + 0.5) * 0.4 * lorenz;
        rgb[2] += ((t * 0.7 + (u + v) * 3.0).sin() * 0.5 + 0.5) * 0.4 * lorenz;
    }
    let vignette = post.vignette.clamp(0.0, 1.0);
    if vignette > 0.0 {
        let falloff = (1.0 - (u * u + v * v) * (0.5 + vignette)).clamp(0.0, 1.0);
        for c in rgb.iter_mut() {
            *c *= 1.0 - vignette * (1.0 - falloff);
        }
    }
    let grain = post.grain.clamp(0.0, 1.0);
    if grain > 0.0 {
        let n = ((u * 12.9898 + v * 78.233 + time * 43.0).sin() * 43758.5453).fract() - 0.5;
        for c in rgb.iter_mut() {
            *c += n * grain * 0.5;
        }
    }
}

fn to_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// The advanced block, parsed once per frame out of [`Post::advanced`] — the
/// CPU mirror of the shader's grade/texture/clarity/dehaze/denoise/optics/
/// geometry passes. Built even when every value is neutral (the common case);
/// each stage checks its own amount first, so an ungraded frame costs a few
/// comparisons and no arithmetic.
struct AdvancedFx {
    grade: crate::grade::GradeParams,
    grade_active: bool,
    texture: f32,
    clarity: f32,
    dehaze: f32,
    denoise_lum: f32,
    denoise_color: f32,
    optics: crate::grade::OpticsParams,
    geometry: crate::grade::GeometryParams,
    /// `fx.fm` floats in `FM_FLOATS` order (all zero when absent or off).
    fm: [f32; 11],
    /// `fx.wavewarp` floats in `WAVEWARP_FLOATS` order (all zero when absent or off).
    wavewarp: [f32; 16],
}

/// `fx.fm`'s wave on the CPU, the twin of `fm_wave` in `post.wgsl`
/// (cycles in, -1..1 out).
fn fm_wave(x: f32, shape: f32) -> f32 {
    match shape.round() as i32 {
        1 => 1.0 - 4.0 * ((x + 0.25).rem_euclid(1.0) - 0.5).abs(),
        2 => 2.0 * (x + 0.5).rem_euclid(1.0) - 1.0,
        3 => if x.rem_euclid(1.0) < 0.5 { 1.0 } else { -1.0 },
        _ => (std::f32::consts::TAU * x).sin(),
    }
}

/// One advanced group's floats inside `post.advanced`, by effect id. An
/// unknown id or a short mirror is the empty slice — the same tolerance
/// `read_post` shows a short row.
fn adv_slice<'a>(post: &'a Post, id: &str) -> &'a [f32] {
    match metal_schema::advanced_effect_span(id) {
        Some(r) => {
            let lo = r.start - metal_schema::CHAIN_FLOATS;
            let hi = r.end - metal_schema::CHAIN_FLOATS;
            post.advanced.get(lo..hi).unwrap_or(&[])
        }
        None => &[],
    }
}

fn adv_nth(post: &Post, id: &str, n: usize) -> f32 {
    adv_slice(post, id).get(n).copied().unwrap_or(0.0)
}

impl AdvancedFx {
    fn of(post: &Post) -> AdvancedFx {
        let grade = crate::grade::GradeParams::from_floats(adv_slice(post, "grade"));
        AdvancedFx {
            grade_active: !grade.is_neutral(),
            grade,
            texture: adv_nth(post, "texture", 0),
            clarity: adv_nth(post, "clarity", 0),
            dehaze: adv_nth(post, "dehaze", 0),
            denoise_lum: adv_nth(post, "denoise", 0),
            denoise_color: adv_nth(post, "denoise", 3),
            optics: crate::grade::OpticsParams::from_floats(adv_slice(post, "optics")),
            geometry: crate::grade::GeometryParams::from_floats(adv_slice(post, "geometry")),
            fm: {
                let mut fm = [0.0f32; 11];
                for (d, s) in fm.iter_mut().zip(adv_slice(post, "fm")) {
                    *d = *s;
                }
                fm
            },
            wavewarp: {
                let mut w = [0.0f32; 16];
                for (d, s) in w.iter_mut().zip(adv_slice(post, "wavewarp")) {
                    *d = *s;
                }
                w
            },
        }
    }

    /// `fx.wavewarp` on the CPU stand-in: a single sine layer (the shader's
    /// other shapes, pinning, second layer and edge modes are GPU-only),
    /// displaced perpendicular to `angulo`. Off (`cant == 0`) is the identity.
    fn wavewarp(&self, uv: [f32; 2], time: f32) -> [f32; 2] {
        let [cant, _tipo, long, alto, angle, speed, phase, ..] = self.wavewarp;
        if cant <= 0.0 {
            return uv;
        }
        let a = angle.to_radians();
        let along = (uv[0] * a.cos() + uv[1] * a.sin()) * 0.5;
        let x = along / long.max(0.02) + phase / 360.0 + time * speed;
        let d = (std::f32::consts::TAU * x).sin() * alto * 2.0 * cant;
        [uv[0] - a.sin() * d, uv[1] + a.cos() * d]
    }

    /// `fx.fm` as a UV displacement of the stand-in's -1..1 coordinate: the
    /// same carrier/modulator as the shader, without the `realim` recursion
    /// (this painter has no sampled colour to feed back) or the colour split.
    /// Off (`cant == 0`, which is also the all-zero `fm`) is the identity.
    fn fm(&self, uv: [f32; 2]) -> [f32; 2] {
        let [cant, fc, fmod, index, phase, angle, shape, xc, yc, ..] = self.fm;
        if cant <= 0.0 {
            return uv;
        }
        let a = angle.to_radians();
        let s = (uv[0] * a.cos() + uv[1] * a.sin()) * 0.5;
        let m = fm_wave(fmod * s + phase, shape);
        let car = fm_wave(fc * s + index * 0.159_154_94 * m, shape);
        [uv[0] + xc * car * 0.24 * cant, uv[1] + yc * car * 0.24 * cant]
    }

    /// Geometry as a UV transform — the sampling coordinate for an output
    /// `uv` in -1..1. Neutral is the identity.
    fn geometry(&self, uv: [f32; 2]) -> [f32; 2] {
        crate::grade::apply_geometry_uv(uv, &self.geometry)
    }

    /// The per-pixel colour stages, in the order the shader runs them.
    fn apply(&self, rgb: &mut [f32; 3], uv: [f32; 2]) {
        if self.grade_active {
            *rgb = crate::grade::apply_grade(*rgb, &self.grade);
        }
        if self.texture != 0.0 {
            *rgb = crate::grade::apply_texture(*rgb, self.texture);
        }
        if self.clarity != 0.0 {
            *rgb = crate::grade::apply_clarity(*rgb, self.clarity);
        }
        if self.dehaze != 0.0 {
            *rgb = crate::grade::apply_dehaze(*rgb, self.dehaze);
        }
        if self.denoise_lum != 0.0 || self.denoise_color != 0.0 {
            *rgb = crate::grade::apply_denoise(*rgb, self.denoise_lum, self.denoise_color);
        }
        if self.optics.vignetting != 0.0 {
            *rgb = crate::grade::apply_optics(*rgb, uv, &self.optics);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_baked_in_manifest_is_the_suite() {
        // If this build's schema.json were stale, every stand-in row would be
        // the wrong width and the monitor would name floats the model does not
        // have. `metal-render`'s tests/schema.rs is the other half of this.
        let s = Stand::new();
        assert!(
            s.model_count() >= 11,
            "only {} models in the baked manifest",
            s.model_count()
        );
        assert_eq!(s.ids().len(), s.model_count());
        assert_eq!(s.names().len(), s.model_count());
        assert_eq!(s.schemas().len(), s.model_count());
        assert!(s.ids().iter().all(|id| !id.is_empty()));
    }

    #[test]
    fn it_says_it_is_not_real() {
        // The one thing on this path that must not be a comforting lie: the
        // page prints this, and somebody reviewing a layout has to know the
        // figures are not the models.
        assert!(!Stand::new().real());
        assert!(Stand::new().describe().contains("stand-in"));
    }

    #[test]
    fn a_frame_is_the_size_asked_for_and_fully_opaque() {
        let mut s = Stand::new();
        let row = s.schemas()[0].defaults();
        for (w, h) in [(64u32, 36u32), (320, 180), (200, 200)] {
            let (px, pw, ph) = s.solo(0, &row, 1.0, w, h, false);
            assert_eq!((pw, ph), (w, h));
            assert_eq!(px.len(), (w * h * 4) as usize);
            assert!(
                px.chunks_exact(4).all(|p| p[3] == 255),
                "{w}x{h} left a hole in the alpha"
            );
        }
    }

    #[test]
    fn a_frame_is_a_picture_rather_than_a_flat_colour() {
        // The failure that would look like a working app: every model a solid
        // rectangle, laid out correctly, telling you nothing.
        let mut s = Stand::new();
        for i in 0..s.model_count() {
            let row = s.schemas()[i].defaults();
            let (px, _, _) = s.solo(i, &row, 2.5, 96, 54, false);
            let lum: Vec<f32> = px
                .chunks_exact(4)
                .map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
                .collect();
            let lo = lum.iter().cloned().fold(f32::INFINITY, f32::min);
            let hi = lum.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            // A dark-palette model (the parallax worlds' near-black stops) is
            // legitimately dim, so the bar is "not a solid colour" rather than
            // a fixed brightness.
            assert!(hi - lo > 2.0, "model {i} drew a flat field ({lo}..{hi})");
        }
    }

    #[test]
    fn different_models_do_not_all_draw_the_same_thing() {
        // A rack of seventeen identical tiles is worse than no rack: it looks
        // like the app is working.
        let mut s = Stand::new();
        let frames: Vec<Vec<u8>> = (0..FIGURES.min(s.model_count()))
            .map(|i| s.solo(i, &s.schemas()[i].defaults(), 3.0, 48, 27, false).0)
            .collect();
        for i in 0..frames.len() {
            for j in i + 1..frames.len() {
                assert_ne!(
                    frames[i], frames[j],
                    "models {i} and {j} drew the same frame"
                );
            }
        }
    }

    #[test]
    fn the_picture_moves_with_time() {
        let mut s = Stand::new();
        let row = s.schemas()[0].defaults();
        let a = s.solo(0, &row, 1.0, 64, 36, false).0;
        let b = s.solo(0, &row, 1.9, 64, 36, false).0;
        assert_ne!(
            a, b,
            "the stand-in is frozen — a still picture reads as a hung renderer"
        );
    }

    #[test]
    fn the_float_row_reaches_the_picture() {
        // The property that makes the drive knob and the monitor agree with
        // what is on screen. Without it the bars move and the picture does not,
        // which is precisely the confusion the monitor exists to remove.
        let mut s = Stand::new();
        let schema = s.schemas()[0];
        let low = schema.defaults();
        let mut high = low.clone();
        if let Block::Floats(specs) = schema.blocks[0] {
            high[0] = specs[0].max;
        }
        let a = s.solo(0, &low, 4.0, 64, 36, false).0;
        let b = s.solo(0, &high, 4.0, 64, 36, false).0;
        assert_ne!(a, b, "the row did not reach the picture");
    }

    /// The advanced block is *applied* on the CPU path, and its neutral value
    /// is the identity. The grade reaches the picture through the row's tail
    /// exactly the way `paint.rs` bakes a `fijo` `fx.grade`'s resolved
    /// `post.advanced` back into the row before `read_post`.
    #[test]
    fn an_advanced_grade_reaches_the_cpu_picture_and_neutral_is_the_identity() {
        let mut s = Stand::new();
        let neutral = s.schemas()[0].defaults();
        let (a, _, _) = s.solo(0, &neutral, 4.0, 64, 36, false);

        // The neutral grade again: the advanced block must be a no-op, not a
        // near-miss that only shows up as a colour shift.
        let (same, _, _) = s.solo(0, &neutral, 4.0, 64, 36, false);
        assert_eq!(a, same, "a neutral advanced block changed the frame");

        // A real exposure on the grade group, written into the row's advanced
        // tail the way the resolver/painter bake a `fijo` `fx.grade`.
        let mut graded = neutral.clone();
        let at = graded.len() - metal_schema::EFFECTS_WIDTH;
        let span = metal_schema::advanced_effect_span("grade").expect("grade is advanced");
        graded[at + span.start + 2] = 60.0; // grade_exposure
        let (b, _, _) = s.solo(0, &graded, 4.0, 64, 36, false);
        assert_ne!(a, b, "a nonzero grade must change the CPU frame");

        let mean = |px: &[u8]| -> f64 {
            px.chunks_exact(4).map(|p| p[0] as u64).sum::<u64>() as f64 / (px.len() / 4) as f64
        };
        assert!(mean(&b) > mean(&a), "exposure + must brighten the CPU frame");
    }

    /// `fx.fm` on the CPU stand-in: off is the identity, on moves the frame.
    #[test]
    fn an_fm_row_moves_the_cpu_frame_and_a_default_row_does_not() {
        let mut s = Stand::new();
        let row = s.schemas()[0].defaults();
        let (a, _, _) = s.solo(0, &row, 4.0, 64, 36, false);
        let mut on = row.clone();
        let at = on.len() - metal_schema::EFFECTS_WIDTH;
        let cant = metal_schema::effect_index("fx_fm_cant").unwrap();
        on[at + cant] = 0.0;
        assert_eq!(s.solo(0, &on, 4.0, 64, 36, false).0, a, "cant 0 must be the identity");
        on[at + cant] = 0.9;
        assert_ne!(s.solo(0, &on, 4.0, 64, 36, false).0, a, "fm on must move the CPU frame");
    }

    /// `fx.wavewarp` on the CPU stand-in: off is the identity, on moves the frame.
    #[test]
    fn a_wavewarp_row_moves_the_cpu_frame_and_a_default_row_does_not() {
        let mut s = Stand::new();
        let row = s.schemas()[0].defaults();
        let (a, _, _) = s.solo(0, &row, 4.0, 64, 36, false);
        let mut on = row.clone();
        let at = on.len() - metal_schema::EFFECTS_WIDTH;
        let cant = metal_schema::effect_index("fx_wavewarp_cant").unwrap();
        on[at + cant] = 0.0;
        assert_eq!(s.solo(0, &on, 4.0, 64, 36, false).0, a, "cant 0 must be the identity");
        on[at + cant] = 0.9;
        assert_ne!(s.solo(0, &on, 4.0, 64, 36, false).0, a, "wavewarp on must move the CPU frame");
    }

    /// The bug that took the whole review surface dark, kept as a test.
    ///
    /// `metal_schema::fondo` gave eleven models a background slot, at the end of
    /// their schema, starting solid black. `Signature::of` was reading whichever
    /// slot came last, so from that day every figure on this painter was drawn
    /// in black on a black frame — correct sizes, correct composition, correct
    /// layout, nothing in any log, and an entirely black rack.
    ///
    /// So: no model may open as one flat colour, and the one below it is the
    /// same check per model rather than for model 0 alone.
    #[test]
    fn no_model_opens_as_a_black_rectangle() {
        let mut s = Stand::new();
        for i in 0..s.model_count() {
            let row = s.schemas()[i].defaults();
            let px = s.solo(i, &row, 3.0, 48, 27, false).0;
            let lit = px
                .chunks_exact(4)
                .filter(|p| p[0] > 6 || p[1] > 6 || p[2] > 6)
                .count();
            assert!(
                lit > 0,
                "{} drew nothing but black at its own defaults",
                s.ids()[i]
            );
        }
    }

    /// And the reason the fix is "the first slot that is not the background"
    /// rather than "the first slot": a model whose only colour is a background
    /// still has to be drawn in it.
    #[test]
    fn a_model_whose_only_slot_is_its_background_still_uses_it() {
        let mut s = Stand::new();
        let Some(i) = (0..s.model_count()).find(|i| {
            let slots: Vec<&str> = s.schemas()[*i]
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::ColorSlot(name, _) => Some(*name),
                    _ => None,
                })
                .collect();
            slots == ["background"]
        }) else {
            // No such model in the suite today. The branch still exists and is
            // still right; there is just nothing to point it at.
            return;
        };
        let schema = s.schemas()[i];
        let mut at = 0;
        for b in schema.blocks {
            if matches!(b, Block::ColorSlot(..)) {
                break;
            }
            at += b.width();
        }
        let mut row = schema.defaults();
        for k in 0..12 {
            row[at + k] = if k % 3 == 0 { 1.0 } else { 0.0 };
        }
        let px = s.solo(i, &row, 3.0, 48, 27, false).0;
        let red = px
            .chunks_exact(4)
            .filter(|p| p[0] > 40 && p[1] < 40)
            .count();
        assert!(
            red > 0,
            "{} ignored the only colour slot it has",
            s.ids()[i]
        );
    }

    #[test]
    fn the_colour_slot_reaches_the_picture() {
        let mut s = Stand::new();
        // Find a model that has a colour slot at all.
        let Some(i) = (0..s.model_count()).find(|i| {
            s.schemas()[*i]
                .blocks
                .iter()
                .any(|b| matches!(b, Block::ColorSlot(..)))
        }) else {
            return;
        };
        let schema = s.schemas()[i];
        let mut at = 0;
        let mut slot = None;
        for b in schema.blocks {
            if matches!(b, Block::ColorSlot(..)) {
                slot = Some(at);
                break;
            }
            at += b.width();
        }
        let slot = slot.expect("found above");
        let white = schema.defaults();
        let mut red = white.clone();
        for k in 0..4 {
            red[slot + k * 3] = 1.0;
            red[slot + k * 3 + 1] = 0.0;
            red[slot + k * 3 + 2] = 0.0;
        }
        let a = s.solo(i, &white, 4.0, 48, 27, false).0;
        let b = s.solo(i, &red, 4.0, 48, 27, false).0;
        assert_ne!(a, b, "the palette button would do nothing on this path");
    }

    #[test]
    fn every_shared_effect_changes_the_picture() {
        // Four knobs in the footer. A knob that does nothing on this build is a
        // reviewer reporting a bug that does not exist.
        //
        // *Some* figure, not every figure, and that is not the gate being
        // weakened. Mirrors fold the angle, so on a rotationally symmetric
        // figure — the concentric rings, which is model 0 — folding it is a
        // genuine no-op, and the real `kaleido` pass has exactly the same
        // property on exactly the same kind of image. Demanding that every
        // effect move every figure would be demanding the stand-in be *less*
        // like the thing it stands in for.
        let mut s = Stand::new();
        for (name, post) in [
            (
                "shake",
                Post {
                    shake: 1.0,
                    ..Post::default()
                },
            ),
            (
                "light_leak",
                Post {
                    light_leak: 1.0,
                    ..Post::default()
                },
            ),
            (
                "ghost",
                Post {
                    ghost: 0.9,
                    ..Post::default()
                },
            ),
            (
                "kaleido",
                Post {
                    kaleido: 6.0,
                    ..Post::default()
                },
            ),
            (
                "crt",
                Post {
                    crt: 1.0,
                    ..Post::default()
                },
            ),
            (
                "glitch",
                Post {
                    glitch: 1.0,
                    ..Post::default()
                },
            ),
            (
                "posterize",
                Post {
                    posterize: 1.0,
                    ..Post::default()
                },
            ),
            (
                "zoom",
                Post {
                    zoom: 1.0,
                    ..Post::default()
                },
            ),
            (
                "fire",
                Post {
                    fire: 1.0,
                    ..Post::default()
                },
            ),
            (
                "lorenz",
                Post {
                    lorenz: 1.0,
                    ..Post::default()
                },
            ),
            (
                "vignette",
                Post {
                    vignette: 1.0,
                    ..Post::default()
                },
            ),
            (
                "blur",
                Post {
                    blur: 1.0,
                    ..Post::default()
                },
            ),
            (
                "sharpen",
                Post {
                    sharpen: 1.0,
                    ..Post::default()
                },
            ),
            (
                "grain",
                Post {
                    grain: 1.0,
                    ..Post::default()
                },
            ),
        ] {
            let moved = (0..FIGURES.min(s.model_count())).any(|i| {
                let plain = s.schemas()[i].defaults();
                let mut driven = plain.clone();
                let at = driven.len() - metal_schema::EFFECTS_WIDTH;
                driven[at] = post.shake;
                driven[at + 1] = post.light_leak;
                driven[at + 2] = post.ghost;
                driven[at + 3] = post.kaleido;
                driven[at + 4] = post.crt;
                driven[at + 5] = post.glitch;
                driven[at + 6] = post.posterize;
                driven[at + 7] = post.zoom;
                driven[at + 8] = post.fire;
                driven[at + 9] = post.lorenz;
                driven[at + 10] = post.vignette;
                driven[at + 11] = post.blur;
                driven[at + 12] = post.sharpen;
                driven[at + 13] = post.grain;
                s.solo(i, &driven, 5.0, 64, 36, false).0 != s.solo(i, &plain, 5.0, 64, 36, false).0
            });
            assert!(moved, "{name} did nothing to any figure");
        }
    }

    #[test]
    fn a_short_row_does_not_panic_the_painter() {
        // Rows come off a socket and out of files written by older builds.
        let mut s = Stand::new();
        for row in [vec![], vec![0.5], vec![f32::NAN, 0.5]] {
            let (px, _, _) = s.solo(0, &row, 1.0, 32, 18, false);
            assert_eq!(px.len(), 32 * 18 * 4);
        }
    }

    #[test]
    fn an_index_past_the_suite_is_clamped_rather_than_panicking() {
        let mut s = Stand::new();
        let (px, _, _) = s.solo(9999, &[], 1.0, 16, 9, false);
        assert_eq!(px.len(), 16 * 9 * 4);
    }
}

