//! Monstruo — mythical monster faces that sing, rap, whisper and talk.
//!
//! `the voice design notes` V5. The face is not animated here: every frame the row's
//! rig floats (one per [`metal_face::FaceControl`], named after it) are posed by
//! [`metal_face::pose`] on the CPU and the result is handed to the shader as a
//! packed block of landmarks ([`metal_face::Posed::pack`]). Whatever drives those
//! floats — a lane, a loop template, a slider, or the V4 face solver reading
//! the voice columns — the shader only ever sees landmarks.
//!
//! # One model, one shader per species
//!
//! The `especie` float picks one of [`metal_face::Kind::ALL`]. Each species is
//! its own WGSL file (`shaders/monstruo_<kind>.wgsl`) with its own skull,
//! skin, light and psychedelic field, built on a shared rig file
//! (`shaders/monstruo_rig.wgsl`) that turns the packed landmarks into lips,
//! teeth, tongue, cavity, eyes and lids. So the mouth reads the same on all
//! five, and a species can look as different as it likes. A pipeline is
//! built the first time its species is drawn, never before.
//!
//! # The floats
//!
//! Three blocks, each append-only (plan rule 2):
//!
//! - **look**: `especie` and the knobs every species interprets in its own
//!   way (`trip`, `flujo`, `brillo`, …). Same names on all five, so a lane or
//!   a loop authored against one species still means something on the next.
//! - **rig**: generated from `FaceControl::ALL`, never written by hand.
//!   The 23 speech controls, then the appended expression muscles (FACS
//!   AU1/2/4/5/6/9/10/12/15/20/24/46, per side where a gesture is one-sided).
//!   Preset values for whole expressions are in [`metal_face::EXPRESSIONS`].
//! - the look block ends with `oracle`'s eye floats (`mirada_x`, `mirada_y`,
//!   `vel_escaneo`, `parpado`, `pupila`, `iris`, `giro`) plus the face-only
//!   `ojo_escaneo` and `ojo_tamano`.
//! - colour slots `piel`, `neon`, `boca`, `fondo`, then `oracle`'s eye
//!   colours `iris`, `ojo_brillo` (`oracle`'s `brillo`: the name was taken by
//!   the emission float) and `pupila_color`.

use metal_face::{pose, Controls, FaceControl, Kind, Topology, VEC4S};

use crate::model::{
    color, color_at, background, spec, spec_held, spec_rig, Block, Ctx, FloatSpec, Model, Schema,
    SurfaceSpec,
};
use crate::shared::fullscreen::Fullscreen;
use crate::shared::surface::{MaterialU, PartSet};
use crate::shared::ColorSlotGpu;

/// The parts a picture can be laid on, in the order the shaders bind them:
/// `skin` (and the lids and jaw) and `eyes` (the iris and sclera).
const SURFACES: &[&str] = &["skin", "eyes"];

/// The last `especie` value.
const LAST_KIND: f32 = (Kind::ALL.len() - 1) as f32;

/// The look block. Read by position in [`render`](Monstruo::render) as
/// `look[0..]`, packed in order into `Uniform::look`.
const LOOK: &[FloatSpec] = &[
    // Structural: a different creature, not a stronger one.
    spec_held("species", 0.0, LAST_KIND, 0.0, 0.0),
    // How far the picture leaves the real: domain warp, hue drift, the
    // feedback-free "melt" of skin pattern and background.
    spec("trip", 0.0, 1.0, 0.55),
    // Speed of everything that flows (skin pattern, background, aura).
    spec("flow", 0.0, 1.0, 0.35),
    // Emission: rim light, neon markings, glow.
    spec("shine", 0.0, 1.0, 0.6),
    // The energy field around the head.
    spec("aura", 0.0, 1.0, 0.5),
    // A hit: onset-shaped flash, shockwave, scale flare. Rests at 0.
    spec("pulse", 0.0, 1.0, 0.0),
    // Camera distance: below 1 closer, above 1 further.
    spec("distance", 0.6, 1.8, 1.0),
    // Eye glow.
    spec("eyes", 0.0, 1.0, 0.7),
    // Hue rotation of the whole palette, signed.
    spec("hue", -1.0, 1.0, 0.0),
    // Cost knob: march steps, AO taps, detail octaves. Held: a quality
    // setting, not something the music should swing.
    spec_held("detail", 0.0, 1.0, 0.6, 0.0),
    // ---- 2026-10-07: the eye of `oracle`, on the face (appended, rule 2) ----
    //
    // Same names, ranges and defaults as `oracle`'s floats wherever they mean
    // the same thing (a test reads both schemas): where the eyes look,
    // how fast they scan, how open the lids are, the pupil and iris sizes and
    // the roll. `n_ojos`, `pos_x` and `pos_y` are deliberately absent — a
    // face has two eyes in two sockets of a fixed topology, and moving them
    // out of their sockets is a different creature, not a gesture.
    spec("gaze_x", 0.0, 1.0, 0.5),
    spec("gaze_y", 0.0, 1.0, 0.5),
    spec("speed_scan", 0.05, 4.0, 0.8),
    // 1 open (the rig's own lids), 0 shut. Multiplies the rig, so a blink or
    // a wink still closes an eye the operator left open. Mostly held: the
    // music swinging every eyelid shut reads as a fault.
    spec_held("eyelid", 0.0, 1.0, 1.0, 0.25),
    spec("pupil", 0.0, 1.0, 0.4),
    spec("iris", 0.0, 1.0, 0.55),
    // Rolls each eye about its own gaze axis, mirrored left/right, so a
    // slit pupil and the lid line tilt into a slant (+: outer corners up).
    spec_held("turn", -1.0, 1.0, 0.0, 0.25),
    // Face-only, no `oracle` twin: how far the saccades jump. `oracle`'s eye
    // always scans; a face that has always stared at the camera keeps doing
    // so at the default 0, and `vel_escaneo` is the rate once this is up.
    spec("eye_scan", 0.0, 1.0, 0.0),
    // Face-only: eyeball size, 1 = the species' own. Not `oracle`'s
    // `tamano` (which is the whole eye's size in the frame, 0.2..4): an
    // eyeball four times bigger than its socket is a hole in the head.
    spec_held("eye_size", 0.7, 1.3, 1.0, 0.0),
];

/// Where each eye float sits in [`LOOK`]. Found by name, not by a literal.
fn look_index(name: &str) -> usize {
    LOOK.iter()
        .position(|f| f.name == name)
        .unwrap_or_else(|| panic!("monstruo has no look float {name}"))
}

/// The rig block, one float per face control, generated from the control
/// table so the names, ranges and order cannot drift from `metal-face`.
const RIG: [FloatSpec; FaceControl::COUNT] = {
    let mut out = [spec("", 0.0, 1.0, 0.0); FaceControl::COUNT];
    let mut i = 0;
    while i < FaceControl::COUNT {
        out[i] = spec_rig(FaceControl::ALL[i]);
        i += 1;
    }
    out
};

/// Scales, mid, light, highlight — a cold-to-hot skin, recoloured per species.
const SKIN: [f32; 12] = [
    0.02, 0.05, 0.10, //
    0.05, 0.35, 0.45, //
    0.30, 0.85, 0.55, //
    0.95, 0.95, 0.70,
];
/// The psychedelic markings and rim: magenta → cyan → acid → white-hot.
const NEON: [f32; 12] = [
    1.00, 0.10, 0.75, //
    0.10, 0.85, 1.00, //
    0.75, 1.00, 0.10, //
    1.00, 0.95, 0.90,
];
/// The black of the eye, `oracle`'s own default for its `pupila_color`.
const PUPIL_BLACK: [f32; 12] = crate::model::solid4(0.01, 0.01, 0.015);

/// Mouth: cavity shadow → gums → tongue → teeth. The cavity and gums are kept
/// dark and the tongue a saturated red (the blue pulled out), so the shader's
/// shared `mouth_*_base` helpers can never paint the tongue pink-on-pink with
/// the cavity.
const MOUTH: [f32; 12] = [
    0.01, 0.00, 0.008, //
    0.26, 0.02, 0.03, //
    0.78, 0.05, 0.04, //
    0.95, 0.92, 0.82,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(LOOK),
        Block::Floats(&RIG),
        color_at("skin", &SKIN),
        color_at("neon", &NEON),
        color_at("mouth", &MOUTH),
        background(),
        // `oracle`'s eye colours (appended). The iris slot *tints* each
        // species' own iris (white = the species as designed); `ojo_brillo`
        // is `oracle`'s `brillo`, renamed because `brillo` is already this
        // model's emission float; `pupila_color` replaces the black of the
        // pupil. `oracle`'s `piel` is this model's existing `piel`.
        color("iris"),
        color("ojo_brillo"),
        color_at("pupil_color", &PUPIL_BLACK),
        crate::model::SIGNAL_FLOATS_BLOCK,
        crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

/// Mirrors `struct U` in `monstruo_rig.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    face: [[f32; 4]; VEC4S],
    /// (aspect, time, especie, trip)
    a: [f32; 4],
    /// (flujo, brillo, aura, pulso)
    b: [f32; 4],
    /// (distancia, ojos, matiz, detalle)
    c: [f32; 4],
    skin: ColorSlotGpu,
    neon: ColorSlotGpu,
    mouth: ColorSlotGpu,
    background: ColorSlotGpu,
    /// (mirada_x, mirada_y, vel_escaneo, parpado)
    d: [f32; 4],
    /// (pupila, iris, giro, ojo_escaneo)
    e: [f32; 4],
    iris: ColorSlotGpu,
    ojo_brillo: ColorSlotGpu,
    pupil_color: ColorSlotGpu,
    /// `skin`'s material: picture setting and `mat_*`, see `material.wgsl`.
    mat_skin: MaterialU,
    /// `eyes`'s material, as `mat_skin`.
    mat_eyes: MaterialU,
}

/// The WGSL for one species: the shared rig file, then the species' own.
fn wgsl(kind: Kind) -> &'static str {
    macro_rules! with_rig {
        ($file:literal) => {
            concat!(
                include_str!("../../shaders/surface.wgsl"),
                "\n",
                include_str!("../../shaders/material.wgsl"),
                "\n",
                include_str!("../../shaders/monstruo_rig.wgsl"),
                "\n",
                include_str!(concat!("../../shaders/", $file))
            )
        };
    }
    match kind {
        Kind::Dragon => with_rig!("monstruo_dragon.wgsl"),
        Kind::Minotauro => with_rig!("monstruo_minotauro.wgsl"),
        Kind::Gorgona => with_rig!("monstruo_gorgona.wgsl"),
        Kind::Oni => with_rig!("monstruo_oni.wgsl"),
        Kind::Quetzalcoatl => with_rig!("monstruo_quetzalcoatl.wgsl"),
    }
}

pub struct Monstruo {
    /// One pass per species, by `Kind::ALL` index, built lazily.
    passes: Vec<Option<Fullscreen>>,
    /// The topology of the species last drawn, so a held `especie` does not
    /// rebuild it every frame.
    topology: Option<(Kind, Topology)>,
    /// `skin` and `eyes`, in `SURFACES` order: their pictures and their
    /// materials, one `MaterialU` per part appended to `Uniform`.
    surfaces: PartSet<2>,
}

impl Monstruo {
    pub fn new() -> Monstruo {
        Monstruo {
            passes: Kind::ALL.iter().map(|_| None).collect(),
            topology: None,
            surfaces: PartSet::new(SURFACES),
        }
    }
}

impl Default for Monstruo {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
/// The packed face for a row: species from `especie`, controls from the rig
/// block. Public to the crate so the tests can check the shader's input
/// without a GPU.
pub(crate) fn packed(topology: &Topology, rig: &[f32]) -> [[f32; 4]; VEC4S] {
    packed_eyes(topology, rig, Eyes::default())
}

/// How far gorgona's lip ring sits from the shared topology's: thinner,
/// narrower lips for this one species, without touching
/// [`metal_face::Topology::for_species`] (and its tests) at all. Applied after
/// `pose`, around the lips' own centre, so the opening and the jaw seam
/// still land exactly where the rig put them.
const GORGONA_LIP_SCALE: f32 = 0.74;

/// The look floats that change eye *landmarks* rather than shading: the
/// lids and the eyeball size, applied after the rig so every species (and
/// every lid, socket and brow that reads `eye_lids`/`eye_radius`) agrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Eyes {
    /// `parpado`: 1 the rig's own lids, 0 shut.
    pub open: f32,
    /// `ojo_tamano`: eyeball radius scale.
    pub size: f32,
}

impl Default for Eyes {
    fn default() -> Self {
        Eyes { open: 1.0, size: 1.0 }
    }
}

/// A bull's bite: a blunt, nearly flush dental pad up top (cattle have no
/// upper incisors) and a row of short, even, blunt lower incisors — no fang
/// taper, no odd sizes, nothing floating clear of the gum.
fn apply_bovine_teeth(face: &mut Topology) {
    for t in &mut face.upper_teeth {
        // Wide enough to overlap its neighbours into one continuous pad,
        // short enough to sit nearly flush with the gum.
        let dir = (t.b - t.a).normalize_or_zero();
        t.b = t.a + dir * 0.016;
        t.radius = 0.07;
    }
    for t in &mut face.lower_teeth {
        // Wide, blunt, evenly sized blocks that touch their neighbours —
        // a visible row, not dots.
        let dir = (t.b - t.a).normalize_or_zero();
        t.b = t.a + dir * 0.05;
        t.radius = 0.055;
    }
    // A broad, blunt bovine tongue: close to uniform width along its
    // length instead of the shared chain's sharp taper, so it reads as
    // rounded and flattened rather than a pointed, ribbed cone.
    face.tongue_radius = [0.072, 0.07, 0.066, 0.056];
}

/// Clamps every tooth (both the capsule's `a` and `b`, so the whole tooth
/// moves, not just one end) so it never reaches past the *posed* lip ring
/// — this frame's, after any species lip_scale or jaw/expression move, not
/// the rest topology's. Without this a species whose lips were narrowed
/// (gorgona) or whose teeth were widened (minotauro's bovine reshape) can
/// show a tooth outside the lip ring: past a corner (x), below the lower
/// lip or above the upper one (y), or in front of the lip surface (z).
/// `clamp_teeth_to_lips`'s x-only version missed exactly this: a lip_scale
/// that also moves the lips up and in (gorgona's does) leaves the teeth at
/// their old, lower y, under the now-higher lower lip.
fn clamp_teeth_to_lips(face: &mut Topology) {
    use metal_face::topology::{CORNER_L, CORNER_R, LOWER_MID, UPPER_MID};
    let corner_l = face.lips[CORNER_L].at.x;
    let corner_r = face.lips[CORNER_R].at.x;
    let cx = 0.5 * (corner_l + corner_r);
    // A small inset so a tooth's own surface sits inside the lip's, not
    // exactly tangent to it.
    let hw = 0.5 * (corner_l - corner_r).abs() - 0.012;
    let corner_l_z = face.lips[CORNER_L].at.z;
    let corner_r_z = face.lips[CORNER_R].at.z;
    let upper = face.lips[UPPER_MID];
    let lower = face.lips[LOWER_MID];
    // Inner surfaces: the upper lip's underside, the lower lip's topside.
    let y_hi = upper.at.y - upper.radius * 0.4 - 0.006;
    let y_lo = lower.at.y + lower.radius * 0.4 + 0.006;
    // Behind the frontmost lip surface, never poking past it toward camera.
    let z_max = corner_l_z.min(corner_r_z).min(upper.at.z).min(lower.at.z) - 0.01;
    // A uniform x-scale around the mouth's centre, not a per-tooth clamp:
    // clamping each tooth to the same bound individually can pull an outer
    // tooth in past its neighbour (which wasn't touching the bound and so
    // never moved), opening a gap and leaving the outer tooth reading as a
    // stray dot apart from the row. Scaling the whole row together keeps
    // every tooth's spacing relative to the others, just smaller.
    let mut reach = 0.0f32;
    for t in face.upper_teeth.iter().chain(face.lower_teeth.iter()) {
        reach = reach.max((t.a.x - cx).abs() + t.radius);
        reach = reach.max((t.b.x - cx).abs() + t.radius);
    }
    let x_scale = if reach > hw && reach > 1e-5 { hw / reach } else { 1.0 };
    for t in face.upper_teeth.iter_mut().chain(face.lower_teeth.iter_mut()) {
        t.a.x = cx + (t.a.x - cx) * x_scale;
        t.b.x = cx + (t.b.x - cx) * x_scale;
        t.radius *= x_scale;
        for p in [&mut t.a, &mut t.b] {
            if y_lo <= y_hi {
                p.y = p.y.clamp(y_lo, y_hi);
            }
            p.z = p.z.min(z_max);
        }
    }
}

#[cfg(test)]
pub(crate) fn packed_eyes(topology: &Topology, rig: &[f32], eyes: Eyes) -> [[f32; 4]; VEC4S] {
    packed_eyes_scaled(topology, rig, eyes, 1.0, false)
}

pub(crate) fn packed_eyes_scaled(
    topology: &Topology,
    rig: &[f32],
    eyes: Eyes,
    lip_scale: f32,
    bovine_teeth: bool,
) -> [[f32; 4]; VEC4S] {
    let mut controls = Controls::rest();
    for (i, v) in rig.iter().enumerate().take(FaceControl::COUNT) {
        controls.0[i] = *v;
    }
    let mut posed = pose(topology, &controls);
    if lip_scale != 1.0 {
        let n = posed.face.lips.len() as f32;
        let cx: f32 = posed.face.lips.iter().map(|b| b.at.x).sum::<f32>() / n;
        let cy: f32 = posed.face.lips.iter().map(|b| b.at.y).sum::<f32>() / n;
        let cz: f32 = posed.face.lips.iter().map(|b| b.at.z).sum::<f32>() / n;
        let centre = glam::Vec3::new(cx, cy, cz);
        for b in &mut posed.face.lips {
            b.at = centre + (b.at - centre) * lip_scale;
            b.radius *= lip_scale;
        }
        // The teeth, the cavity and (through the lips themselves) the mouth
        // hole must move with the lips, not just shrink to fit inside them
        // afterward — otherwise a scale that also pulls the lips up and in
        // (as gorgona's does) leaves the teeth sitting where the lips used
        // to be, below the new lower lip, on the chin. Same centre, same
        // factor, so everything stays nested exactly as it was before the
        // scale.
        for t in posed.face.upper_teeth.iter_mut().chain(posed.face.lower_teeth.iter_mut()) {
            t.a = centre + (t.a - centre) * lip_scale;
            t.b = centre + (t.b - centre) * lip_scale;
            t.radius *= lip_scale;
        }
        posed.face.cavity.centre = centre + (posed.face.cavity.centre - centre) * lip_scale;
        posed.face.cavity.radii *= lip_scale;
    }
    if bovine_teeth {
        apply_bovine_teeth(&mut posed.face);
    }
    // Whatever shrank or reshaped the lips (a species' lip_scale) or the
    // teeth (bovine_teeth) above, the teeth must never reach past the
    // lip ring's own corners — this frame's, posed, not the rest ones —
    // or they read as sitting outside the mouth. A lateral clamp against
    // the posed corners, after any of the above, holds for every
    // expression and every species, not just the one that triggered it.
    clamp_teeth_to_lips(&mut posed.face);
    let shut = 1.0 - eyes.open.clamp(0.0, 1.0);
    let size = eyes.size.clamp(0.7, 1.3);
    for e in &mut posed.face.eyes {
        if shut > 0.0 {
            e.upper_lid += (metal_face::pose::LID_CLOSED - e.upper_lid) * shut;
            e.lower_lid += (metal_face::pose::LID_CLOSED - e.lower_lid) * shut;
        }
        e.radius *= size;
    }
    let flat = posed.pack();
    let mut out = [[0.0; 4]; VEC4S];
    for (i, v4) in out.iter_mut().enumerate() {
        v4.copy_from_slice(&flat[i * 4..i * 4 + 4]);
    }
    out
}

impl Model for Monstruo {
    fn id(&self) -> &'static str {
        "monster"
    }

    fn render_scale(&self) -> f32 {
        0.6
    }

    fn name(&self) -> &'static str {
        "Monstruo"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn is_3d(&self) -> bool {
        true
    }

    fn surfaces(&self) -> &'static [&'static str] {
        self.surfaces.surfaces()
    }

    fn set_surface(&mut self, part: &str, spec: Option<SurfaceSpec>) {
        self.surfaces.set(part, spec);
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        self.surfaces.sync(ctx.device, ctx.queue);
        let look = &floats[..LOOK.len()];
        let rig = &floats[LOOK.len()..LOOK.len() + RIG.len()];
        let colours = LOOK.len() + RIG.len();
        let slot =
            |k: usize| ColorSlotGpu::from_floats(&floats[colours + 12 * k..colours + 12 * (k + 1)]);

        let kind = Kind::from_float(look[0]);
        let index = Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
        if self.topology.as_ref().map(|(k, _)| *k) != Some(kind) {
            self.topology = Some((kind, Topology::for_species(&kind.species())));
        }
        let topology = &self.topology.as_ref().expect("set above").1;

        let at = |name: &str| look[look_index(name)];
        let eyes = Eyes {
            open: at("eyelid"),
            size: at("eye_size"),
        };
        let uniform = Uniform {
            face: packed_eyes_scaled(
                topology,
                rig,
                eyes,
                if kind == Kind::Gorgona { GORGONA_LIP_SCALE } else { 1.0 },
                kind == Kind::Minotauro,
            ),
            a: [ctx.aspect(), ctx.time, index as f32, look[1]],
            b: [look[2], look[3], look[4], look[5]],
            c: [look[6], look[7], look[8], look[9]],
            skin: slot(0),
            neon: slot(1),
            mouth: slot(2),
            background: slot(3),
            d: [at("gaze_x"), at("gaze_y"), at("speed_scan"), at("eyelid")],
            e: [at("pupil"), at("iris"), at("turn"), at("eye_scan")],
            iris: slot(4),
            ojo_brillo: slot(5),
            pupil_color: slot(6),
            mat_skin: self.surfaces.uniform(0),
            mat_eyes: self.surfaces.uniform(1),
        };
        let pass = self.passes[index].get_or_insert_with(|| {
            Fullscreen::with_textures("monster", wgsl(kind), SURFACES.len())
        });
        pass.draw_textured(ctx, target, format, &uniform, &self.surfaces.bind_pairs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_face::topology::{CORNER_L, CORNER_R, LOWER_MID, TONGUE_SEGMENTS, UPPER_MID};

    #[test]
    fn the_rig_block_is_the_control_table() {
        assert_eq!(RIG.len(), FaceControl::COUNT);
        for (f, c) in RIG.iter().zip(FaceControl::ALL) {
            let s = c.spec();
            assert_eq!(f.name, c.name());
            assert_eq!((f.min, f.max, f.default), (s.lo, s.hi, s.rest));
            // V4's contract: the solver finds a control only through
            // `FloatSpec::rig`, and `drive: 0` keeps `drive::row` from
            // round-robining a rig float.
            assert_eq!(f.rig, Some(*c), "{} must carry its FaceControl", c.name());
            assert_eq!(
                f.drive,
                0.0,
                "{} must never be driven by the digest",
                c.name()
            );
        }
    }

    #[test]
    fn the_schema_is_the_width_render_reads() {
        let colours = 7 * 12;
        assert_eq!(
            SCHEMA.width(),
            LOOK.len()
                + RIG.len()
                + colours
                + crate::model::SIGNAL_WIDTH
                + crate::model::EFFECTS_WIDTH
        );
    }

    #[test]
    fn especie_reaches_every_species_and_no_further() {
        let e = &LOOK[0];
        assert_eq!(e.name, "species");
        assert_eq!(Kind::from_float(e.min), Kind::ALL[0]);
        assert_eq!(Kind::from_float(e.max), *Kind::ALL.last().unwrap());
    }

    #[test]
    fn every_species_has_its_shader() {
        for k in Kind::ALL {
            let src = wgsl(*k);
            assert!(src.contains("fn fs_main"), "{} has no fs_main", k.name());
            assert!(
                src.contains("var<uniform> u: U"),
                "{} lost the rig uniform",
                k.name()
            );
        }
    }

    /// The WGSL mirror of `metal_face::pose::LAYOUT`'s two new sections: a
    /// species reads the raw controls and the constant rest mouth, and the
    /// `Face` array is exactly `VEC4S` vec4s, so the Rust `Uniform` and the
    /// shader can never disagree about where `a`/`b`/`c` begin.
    #[test]
    fn the_shader_mirrors_the_controls_and_rest_sections() {
        let controls = metal_face::LAYOUT
            .iter()
            .find(|s| s.0 == "controls")
            .expect("a controls section")
            .1;
        assert_eq!(controls, (FaceControl::COUNT + 3) / 4);
        for k in Kind::ALL {
            let src = wgsl(*k);
            // Every section of the Rust layout, at its Rust size, in order.
            let mut from = 0;
            for (name, n) in metal_face::LAYOUT {
                let decl = format!("{name}: array<vec4<f32>, {n}>");
                let at = src[from..]
                    .find(&decl)
                    .unwrap_or_else(|| panic!("{}: `{decl}` missing or out of order", k.name()));
                from += at + decl.len();
            }
            assert!(
                src.contains(&format!("controls: array<vec4<f32>, {controls}>")),
                "{}: controls section",
                k.name()
            );
            assert!(
                src.contains("rest: array<vec4<f32>, 1>"),
                "{}: rest section",
                k.name()
            );
            for f in [
                "fn ctl(",
                "fn c_brow_furrow(",
                "fn c_squint(",
                "fn c_cheek_puff(",
                "fn c_nostril_flare(",
                "fn rest_mouth(",
            ] {
                assert!(src.contains(f), "{}: missing {f}", k.name());
            }
        }
    }

    /// The WGSL control indices are the `FaceControl::ALL` discriminants. A
    /// future control appended (or worse, reordered) in `metal-face` breaks this
    /// rather than silently reading the wrong float.
    #[test]
    fn the_shader_control_indices_are_the_enum_discriminants() {
        let index = |c: FaceControl| FaceControl::ALL.iter().position(|x| *x == c).unwrap();
        assert_eq!(index(FaceControl::CheekPuff), 13);
        assert_eq!(index(FaceControl::NostrilFlare), 14);
        assert_eq!(index(FaceControl::BrowUp), 15);
        assert_eq!(index(FaceControl::BrowFurrow), 16);
        assert_eq!(index(FaceControl::Squint), 19);
    }

    /// One camera for all five: a species that still carries its own origin or
    /// focal would clip someone's horns or crest.
    #[test]
    fn every_species_uses_the_one_shared_camera() {
        for k in Kind::ALL {
            let src = wgsl(*k);
            assert!(
                !src.contains("3.9 * k_distancia()"),
                "{} still has the old camera",
                k.name()
            );
            assert!(
                !src.contains("4.6 * k_distancia()"),
                "{} still has its own camera",
                k.name()
            );
            assert!(
                src.contains("cam_origin()"),
                "{} never reaches the shared camera",
                k.name()
            );
        }
    }

    #[test]
    fn the_uniform_is_the_packed_face_plus_the_look() {
        assert_eq!(
            std::mem::size_of::<Uniform>(),
            metal_face::PACKED * 4
                + 5 * 16
                + 7 * std::mem::size_of::<ColorSlotGpu>()
                + 2 * std::mem::size_of::<MaterialU>()
        );
    }

    #[test]
    fn the_jaw_float_moves_the_lower_lip_in_the_packed_block() {
        let topo = Topology::for_species(&Kind::Dragon.species());
        let mut rig: Vec<f32> = RIG.iter().map(|f| f.default).collect();
        let shut = packed(&topo, &rig);
        let jaw = FaceControl::ALL
            .iter()
            .position(|c| *c == FaceControl::JawOpen)
            .unwrap();
        rig[jaw] = 1.0;
        let open = packed(&topo, &rig);
        assert_ne!(shut, open, "jaw_open did not reach the shader's input");
    }

    /// Every species only moves and scales the same landmarks: the mouth
    /// stays a mouth. The parenthetical in V5 says the neutral numbers are
    /// first guesses — this is the check that they are still coherent after
    /// `for_species`, not a claim about how they look.
    #[test]
    fn changing_species_keeps_the_rig_landmarks_on_the_mouth() {
        for &k in Kind::ALL {
            let t = Topology::for_species(&k.species());
            let n = t.lips.len() as f32;
            let cx: f32 = t.lips.iter().map(|b| b.at.x).sum::<f32>() / n;
            let cy: f32 = t.lips.iter().map(|b| b.at.y).sum::<f32>() / n;
            let cz: f32 = t.lips.iter().map(|b| b.at.z).sum::<f32>() / n;
            let d =
                ((cx - t.mouth.x).powi(2) + (cy - t.mouth.y).powi(2) + (cz - t.mouth.z).powi(2))
                    .sqrt();
            assert!(
                d < 0.15,
                "{}: lip ring centre drifted {:0.3} from the mouth",
                k.name(),
                d
            );

            // The ring is a mouth: two corners, an upper lip above a lower.
            assert!(
                t.lips[CORNER_L].at.x > t.lips[CORNER_R].at.x,
                "{}: corners are on the wrong sides",
                k.name()
            );
            assert!(
                t.lips[UPPER_MID].at.y > t.lips[LOWER_MID].at.y,
                "{}: the lips are inverted",
                k.name()
            );
            assert!(t.mouth_width() > 0.1, "{}: the mouth is a point", k.name());

            // The mouth sits in front of the jaw hinge and below the eyes.
            assert!(
                t.cavity.centre.z > t.jaw_pivot.z,
                "{}: the cavity is behind the jaw",
                k.name()
            );
            for eye in &t.eyes {
                assert!(
                    eye.centre.y > t.mouth.y,
                    "{}: an eye is below the mouth",
                    k.name()
                );
            }
            for (i, side) in [1.0f32, -1.0].iter().enumerate() {
                assert!(
                    t.nostrils[i].at.y > t.mouth.y,
                    "{}: a nostril is below the mouth",
                    k.name()
                );
                assert!(
                    side * t.nostrils[i].at.x >= 0.0,
                    "{}: nostrils swapped",
                    k.name()
                );
            }

            // The tongue chain runs root → tip forward through the mouth, and
            // the whole chain is on the jaw so it opens with it.
            assert!(
                t.tongue[TONGUE_SEGMENTS].z > t.tongue[0].z,
                "{}: the tongue points backwards",
                k.name()
            );
            for i in 0..TONGUE_SEGMENTS {
                assert!(
                    t.tongue_capsule(i).radius > 0.0,
                    "{}: a tongue segment has no body",
                    k.name()
                );
            }
        }
    }

    /// The rig floats are not decoration: at rest the mouth is a slit, wide
    /// open it is a gape, and the pixels **in the mouth's own screen region**
    /// change between the two. Golden-free — it checks where the change is,
    /// not what the change is.
    #[test]
    fn jaw_open_changes_the_pixels_in_the_mouth_region() {
        let gpu = crate::gpu::Gpu::new_headless();
        let w = 192u32;
        let h = 192u32;
        let aspect = w as f32 / h as f32;

        let shut = SCHEMA.defaults();
        let mut open = SCHEMA.defaults();
        let jaw = FaceControl::ALL
            .iter()
            .position(|c| *c == FaceControl::JawOpen)
            .unwrap();
        open[LOOK.len() + jaw] = 1.0;

        let topo = Topology::for_species(&Kind::Dragon.species());
        let face = packed(&topo, &shut[LOOK.len()..LOOK.len() + RIG.len()]);
        let (ux, uy) = mouth_uv(&face, aspect);
        let mx = (ux * w as f32) as i32;
        let my = (uy * h as f32) as i32;
        let rx = (0.20 * w as f32) as i32;
        let ry = (0.22 * h as f32) as i32;

        let a = render_rgba(&gpu, &shut, w, h);
        let b = render_rgba(&gpu, &open, w, h);
        assert_eq!(a.len(), b.len());

        let mut changed = 0usize;
        for y in (my - ry).max(0)..=(my + ry).min(h as i32 - 1) {
            for x in (mx - rx).max(0)..=(mx + rx).min(w as i32 - 1) {
                let p = ((y as u32 * w + x as u32) * 4) as usize;
                let d = (0..3)
                    .map(|c| (a[p + c] as i32 - b[p + c] as i32).abs())
                    .max()
                    .unwrap_or(0);
                if d > 8 {
                    changed += 1;
                }
            }
        }
        assert!(
            changed > (rx * ry / 20) as usize,
            "jaw_open moved only {changed} pixels in the mouth region ({mx},{my} ±{rx},{ry})"
        );
    }

    /// The proof sheet: five species at rest, `jaw_open`, `oo`, `ee` and
    /// `tongue_out`, written to `scratchpad/monstruo-proof/`. Ignored so the
    /// normal suite writes nothing; run with `--ignored`. Also prints a
    /// rough ms/frame for one frame on this (software, headless) rasteriser.
    #[test]
    #[ignore = "writes the monstruo proof sheet to scratchpad/monstruo-proof"]
    fn write_proof_sheet() {
        use std::time::Instant;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("scratchpad/monstruo-proof");
        std::fs::create_dir_all(&dir).expect("create proof dir");

        let idx = |c: FaceControl| FaceControl::ALL.iter().position(|x| *x == c).unwrap();
        let set = |f: &mut Vec<f32>, c: FaceControl, v: f32| f[LOOK.len() + idx(c)] = v;

        let expressions: [(&str, &[(FaceControl, f32)]); 5] = [
            ("rest", &[]),
            ("jaw_open", &[(FaceControl::JawOpen, 1.0)]),
            (
                "oo",
                &[
                    (FaceControl::LipFunnel, 1.0),
                    (FaceControl::LipPucker, 0.7),
                    (FaceControl::JawOpen, 0.35),
                ],
            ),
            (
                "ee",
                &[
                    (FaceControl::LipStretch, 1.0),
                    (FaceControl::LipUpperUp, 0.5),
                    (FaceControl::LipLowerDown, 0.5),
                    (FaceControl::JawOpen, 0.25),
                ],
            ),
            (
                "tongue_out",
                &[(FaceControl::JawOpen, 0.85), (FaceControl::TongueOut, 1.0)],
            ),
        ];

        let (w, h) = (320u32, 320u32);
        let gpu = crate::gpu::Gpu::new_headless();
        let mut first_ms = 0.0f64;
        for (ki, kind) in Kind::ALL.iter().enumerate() {
            for (name, pairs) in expressions {
                let mut floats = SCHEMA.defaults();
                floats[0] = ki as f32;
                for &(c, v) in pairs {
                    set(&mut floats, c, v);
                }
                let t0 = Instant::now();
                let rgba = render_rgba(&gpu, &floats, w, h);
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                if ki == 0 && name == "rest" {
                    first_ms = ms;
                }
                let jpg = crate::headless::encode_jpeg(&rgba, w, h);
                let path = dir.join(format!("{}-{name}.jpg", kind.name()));
                std::fs::write(&path, jpg).expect("write proof image");
                eprintln!("proof {} {name} -> {}", kind.name(), path.display());
            }
        }
        eprintln!(
            "monstruo proof: one {w}x{h} frame on the headless/software rasteriser ≈ {first_ms:.1} ms/frame"
        );
    }

    /// Every `const C_*: i32 = N;` in the rig WGSL names a `FaceControl`
    /// whose index is N — read from the shader text, not a second list.
    #[test]
    fn every_wgsl_control_constant_is_its_face_control_index() {
        let src = include_str!("../../shaders/monstruo_rig.wgsl");
        let mut seen = 0;
        for line in src.lines() {
            let Some(rest) = line.trim().strip_prefix("const C_") else { continue };
            let (name, n) = rest.split_once(": i32 = ").expect("const C_X: i32 = N;");
            let n: usize = n.trim_end_matches(';').parse().expect("an index");
            let control = FaceControl::from_name(&name.to_lowercase())
                .unwrap_or_else(|| panic!("C_{name} names no FaceControl"));
            assert_eq!(control.index(), n, "C_{name}");
            seen += 1;
        }
        assert!(seen > 10, "the rig reads the expression controls");
    }

    /// `oracle`'s eye on the face: every float of `oracle` exists on
    /// `monster` with the same range and default, except the ones a face's
    /// fixed two-eye topology has no use for; every colour slot exists under
    /// its own name or its documented rename.
    #[test]
    fn the_face_has_every_oracle_eye_parameter() {
        use crate::model::Block as B;
        const NOT_ON_A_FACE: &[&str] = &["n_eyes", "pos_x", "pos_y", "size"];
        // oracle's `fondo` is its sclera; monstruo's `fondo` is the frame's
        // background and its species have no sclera to colour.
        const COLOUR_RENAMES: &[(&str, Option<&str>)] =
            &[("shine", Some("ojo_brillo")), ("background", None)];
        let floats = |s: &Schema| -> Vec<FloatSpec> {
            s.blocks
                .iter()
                .flat_map(|b| match b {
                    B::Floats(f) => f.to_vec(),
                    _ => Vec::new(),
                })
                .collect()
        };
        let slots = |s: &Schema| -> Vec<&'static str> {
            s.blocks
                .iter()
                .filter_map(|b| match b {
                    B::ColorSlot(n, _) => Some(*n),
                    _ => None,
                })
                .collect()
        };
        let face = floats(&SCHEMA);
        // oracle's own floats: its first float block (the shared signal and
        // effects blocks trail every model and are not the eye).
        let oracle_floats: Vec<FloatSpec> = crate::models::oracle::SCHEMA
            .blocks
            .iter()
            .find_map(|b| match b {
                B::Floats(f) => Some(f.to_vec()),
                _ => None,
            })
            .expect("oracle has floats");
        let mut matched = 0;
        for o in &oracle_floats {
            if NOT_ON_A_FACE.contains(&o.name) {
                continue;
            }
            let f = face
                .iter()
                .find(|f| f.name == o.name)
                .unwrap_or_else(|| panic!("monstruo lacks oracle's {}", o.name));
            assert_eq!((f.min, f.max, f.default), (o.min, o.max, o.default), "{}", o.name);
            assert!(LOOK.iter().any(|l| l.name == o.name), "{} is a look float", o.name);
            matched += 1;
        }
        assert!(matched >= 7, "gaze, scan, lids, pupil, iris, roll");
        let face_slots = slots(&SCHEMA);
        for o in slots(&crate::models::oracle::SCHEMA) {
            let want = COLOUR_RENAMES
                .iter()
                .find(|(n, _)| *n == o)
                .map(|(_, to)| *to)
                .unwrap_or(Some(o));
            if let Some(w) = want {
                assert!(face_slots.contains(&w), "monstruo lacks oracle's colour {o} (as {w})");
            }
        }
    }

    #[test]
    fn parpado_and_ojo_tamano_reach_the_packed_eyes() {
        let topo = Topology::for_species(&Kind::Oni.species());
        let rig: Vec<f32> = RIG.iter().map(|f| f.default).collect();
        let eyes = 2 + 3 + 3 + 12 + 2 + 12 + 12 + 8; // LAYOUT up to `eyes`
        let at = |name: &str| metal_face::LAYOUT.iter().take_while(|s| s.0 != name).map(|s| s.1).sum::<usize>();
        assert_eq!(at("eyes"), eyes);
        let open = packed_eyes(&topo, &rig, Eyes::default());
        let shut = packed_eyes(&topo, &rig, Eyes { open: 0.0, size: 1.0 });
        let big = packed_eyes(&topo, &rig, Eyes { open: 1.0, size: 1.3 });
        for i in 0..2 {
            let lids = shut[eyes + 2 * i + 1];
            assert!((lids[0] - lids[1]).abs() < 1e-6, "parpado 0 shuts eye {i}");
            assert!(open[eyes + 2 * i + 1][0] - open[eyes + 2 * i + 1][1] > 0.5);
            assert!((big[eyes + 2 * i][3] / open[eyes + 2 * i][3] - 1.3).abs() < 1e-5);
        }
    }

    /// The oracle-eye floats act on the picture, in the eyes' own region:
    /// gaze, pupil, iris and roll each change pixels there.
    #[test]
    fn the_eye_floats_change_the_pixels_around_the_eyes() {
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (160u32, 160u32);
        let base = {
            let mut f = SCHEMA.defaults();
            f[look_index("distance")] = 0.6;
            f[0] = 3.0; // the oni: big round eyes
            f
        };
        let a = render_rgba(&gpu, &base, w, h);
        for (name, v) in [("gaze_x", 0.0), ("pupil", 1.0), ("iris", 1.0), ("turn", 1.0)] {
            let mut f = base.clone();
            f[look_index(name)] = v;
            let b = render_rgba(&gpu, &f, w, h);
            // The upper half of the frame, where the eyes are.
            let mut changed = 0;
            for p in (0..(w * h / 2) as usize).map(|i| i * 4) {
                let d = (0..3).map(|c| (a[p + c] as i32 - b[p + c] as i32).abs()).max().unwrap();
                if d > 12 {
                    changed += 1;
                }
            }
            assert!(changed > 40, "{name} changed only {changed} pixels around the eyes");
        }
    }

    /// The face-shot proof: every species neutral, every expression muscle at
    /// full, the expression presets, the oracle-eye floats and combos, and a
    /// 24-frame animated sequence, as PNGs under `FACE_SHOTS` (default
    /// `/tmp/claude-1000/face-shots`). Ignored: run with `--ignored`.
    #[test]
    #[ignore = "writes the face-shot PNGs"]
    fn write_face_shots() {
        let dir = std::path::PathBuf::from(
            std::env::var("FACE_SHOTS").unwrap_or_else(|_| "/tmp/claude-1000/face-shots".into()),
        );
        std::fs::create_dir_all(&dir).expect("create face-shot dir");
        let only = std::env::var("FACE_SHOTS_ONLY").ok();
        let size: u32 = std::env::var("FACE_SHOTS_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(384);
        let gpu = crate::gpu::Gpu::new_headless();
        let rig_at = |c: FaceControl| LOOK.len() + c.index();
        let set_rig = |f: &mut Vec<f32>, pairs: &[(FaceControl, f32)]| {
            for &(c, v) in pairs {
                f[rig_at(c)] = v;
            }
        };
        let set_look = |f: &mut Vec<f32>, pairs: &[(&str, f32)]| {
            for &(n, v) in pairs {
                f[look_index(n)] = v;
            }
        };
        let expr = |n: &str| metal_face::Expression::by_name(n).unwrap().controls;
        let first_new = FaceControl::ALL.iter().position(|c| c.name() == "brow_inner_up").unwrap();

        type Shot = (String, Vec<(FaceControl, f32)>, Vec<(&'static str, f32)>);
        let mut shots: Vec<Shot> = vec![("00-neutral".into(), vec![], vec![])];
        for (i, c) in FaceControl::ALL[first_new..].iter().enumerate() {
            shots.push((format!("10-{i:02}-{}", c.name()), vec![(*c, 1.0)], vec![]));
        }
        for (i, e) in metal_face::EXPRESSIONS.iter().enumerate() {
            shots.push((format!("20-{i:02}-{}", e.name), e.controls.to_vec(), vec![]));
        }
        let eyes: &[(&str, &[(&str, f32)])] = &[
            ("gaze-left", &[("gaze_x", 0.0)]),
            ("gaze-right", &[("gaze_x", 1.0)]),
            ("gaze-up", &[("gaze_y", 1.0)]),
            ("gaze-down", &[("gaze_y", 0.0)]),
            ("parpado-half", &[("eyelid", 0.45)]),
            ("pupila-0", &[("pupil", 0.0)]),
            ("pupila-1", &[("pupil", 1.0)]),
            ("iris-0", &[("iris", 0.0)]),
            ("iris-1", &[("iris", 1.0)]),
            ("giro+1", &[("turn", 1.0)]),
            ("giro-1", &[("turn", -1.0)]),
            ("tamano-1.3", &[("eye_size", 1.3)]),
            ("escaneo", &[("eye_scan", 1.0), ("speed_scan", 2.0)]),
        ];
        for (i, (n, l)) in eyes.iter().enumerate() {
            shots.push((format!("30-{i:02}-{n}"), vec![], l.to_vec()));
        }
        let combos: &[(&str, &str, &[(&str, f32)])] = &[
            ("rage-glare", "rage", &[("turn", 0.6), ("pupil", 0.1), ("iris", 0.35)]),
            ("terror-sideways", "terror", &[("gaze_x", 0.0), ("gaze_y", 0.7), ("pupil", 0.0), ("iris", 0.3)]),
            ("sneer-askance", "sneer", &[("gaze_x", 1.0), ("eyelid", 0.75)]),
            ("surprise-dilated", "surprise", &[("pupil", 1.0), ("iris", 1.0)]),
            ("wink-look-down", "wink", &[("gaze_y", 0.15), ("gaze_x", 0.7)]),
            ("smirk-sly", "smirk", &[("gaze_x", 0.0), ("turn", -0.5), ("eyelid", 0.7)]),
            ("grief-down", "grief", &[("gaze_y", 0.0), ("turn", -0.6)]),
        ];
        for (i, (n, e, l)) in combos.iter().enumerate() {
            shots.push((format!("40-{i:02}-{n}"), expr(e).to_vec(), l.to_vec()));
        }

        // The sequence: neutral → rage → terror → wink → neutral, with the gaze
        // sweeping, 24 frames, time advancing at 24 fps.
        let keys: [&[(FaceControl, f32)]; 5] = [&[], expr("rage"), expr("terror"), expr("wink"), &[]];
        let row_of = |pairs: &[(FaceControl, f32)]| {
            let mut v = vec![0.0f32; FaceControl::COUNT];
            for &(c, x) in pairs {
                v[c.index()] = x;
            }
            v
        };

        for (ki, kind) in Kind::ALL.iter().enumerate() {
            if only.as_deref().is_some_and(|o| o != kind.name()) {
                continue;
            }
            let base = {
                let mut f = SCHEMA.defaults();
                f[0] = ki as f32;
                f[look_index("distance")] = 0.72;
                f
            };
            let write = |name: &str, f: &[f32], time: f32| {
                let rgba = render_rgba_at(&gpu, f, size, size, time);
                let path = dir.join(format!("{}-{name}.png", kind.name()));
                write_png(&path, &rgba, size, size);
            };
            for (name, rig, look) in &shots {
                let mut f = base.clone();
                set_rig(&mut f, rig);
                set_look(&mut f, look);
                write(name, &f, 3.0);
            }
            {
                // The whole head at the default camera, for scale.
                let mut f = SCHEMA.defaults();
                f[0] = ki as f32;
                set_rig(&mut f, expr("rage"));
                write("05-rage-default-camera", &f, 3.0);
            }
            let frames = 24;
            for fi in 0..frames {
                let t = fi as f32 / (frames - 1) as f32 * (keys.len() - 1) as f32;
                let k = (t.floor() as usize).min(keys.len() - 2);
                let u = t - k as f32;
                let s = u * u * (3.0 - 2.0 * u);
                let (a, b) = (row_of(keys[k]), row_of(keys[k + 1]));
                let mut f = base.clone();
                for c in 0..FaceControl::COUNT {
                    f[LOOK.len() + c] = a[c] + (b[c] - a[c]) * s;
                }
                let phase = fi as f32 / frames as f32 * std::f32::consts::TAU;
                set_look(&mut f, &[("gaze_x", 0.5 + 0.5 * phase.sin()), ("gaze_y", 0.5 + 0.3 * phase.cos())]);
                write(&format!("90-seq-{fi:02}"), &f, 3.0 + fi as f32 / 24.0);
            }
            eprintln!("face shots: {} done", kind.name());
        }
    }

    /// The mouth proof: every species × jaw_open 0 / 0.5 / 1 × a few
    /// expressions (the preset's own jaw overridden), as PNGs named
    /// `mouth-<species>-j<jaw>-<expr>.png` under `FACE_SHOTS`.
    #[test]
    #[ignore = "writes the mouth-shot PNGs"]
    fn write_mouth_shots() {
        let dir = std::path::PathBuf::from(
            std::env::var("FACE_SHOTS").unwrap_or_else(|_| "/tmp/claude-1000/face-shots".into()),
        );
        std::fs::create_dir_all(&dir).expect("create dir");
        let only = std::env::var("FACE_SHOTS_ONLY").ok();
        let gpu = crate::gpu::Gpu::new_headless();
        let exprs = ["neutral", "rage", "joy", "surprise", "sneer"];
        for (ki, kind) in Kind::ALL.iter().enumerate() {
            if only.as_deref().is_some_and(|o| o != kind.name()) {
                continue;
            }
            for jaw in [0.0f32, 0.5, 1.0] {
                for (ei, e) in exprs.iter().enumerate() {
                    let mut f = SCHEMA.defaults();
                    f[0] = ki as f32;
                    f[look_index("distance")] = 0.72;
                    if let Some(x) = metal_face::Expression::by_name(e) {
                        for &(c, v) in x.controls {
                            f[LOOK.len() + c.index()] = v;
                        }
                    }
                    f[LOOK.len() + FaceControl::JawOpen.index()] = jaw;
                    let rgba = render_rgba_at(&gpu, &f, 320, 320, 3.0);
                    let path = dir.join(format!("mouth-{}-j{:02}-{ei}{e}.png", kind.name(), (jaw * 10.0) as i32));
                    write_png(&path, &rgba, 320, 320);
                }
            }
        }
    }

    fn write_png(path: &std::path::Path, rgba: &[u8], w: u32, h: u32) {
        let file = std::fs::File::create(path).expect("create png");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().expect("png header").write_image_data(rgba).expect("png data");
    }

    /// Head-local → screen uv through the same camera `rig_camera` uses
    /// (position `(0, -0.12, 4.6)`, focal `1.6/0.62`), at the resting
    /// `distancia` (1.0) and an unrotated head. Used by the mouth-region test
    /// to find the mouth without a golden image.
    fn mouth_uv(face: &[[f32; 4]; VEC4S], aspect: f32) -> (f32, f32) {
        // LAYOUT: jaw(2), head(3), shape(3) → the lip ring starts at vec4 8.
        let lips = 2 + 3 + 3;
        let up = face[lips + UPPER_MID];
        let lo = face[lips + LOWER_MID];
        let m = [
            0.5 * (up[0] + lo[0]),
            0.5 * (up[1] + lo[1]),
            0.5 * (up[2] + lo[2]),
        ];
        let ro = [0.0f32, -0.12, 4.6];
        let v = [m[0] - ro[0], m[1] - ro[1], m[2] - ro[2]];
        let v = normalize(v);
        let px = -1.6 * v[0] / (0.62 * v[2]);
        let py = -1.6 * v[1] / (0.62 * v[2]);
        (0.5 + px / (2.0 * aspect), 0.5 - py / 2.0)
    }

    fn normalize(v: [f32; 3]) -> [f32; 3] {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
        [v[0] / l, v[1] / l, v[2] / l]
    }

    /// A surface picture on `skin` is drawn through the shader: a solid red
    /// 4x4 picture at mix 1 changes the frame and adds red, and taking it
    /// away again (`set_surface(part, None)`) restores the frame exactly.
    #[test]
    fn a_skin_picture_reaches_the_frame_and_clearing_it_restores_it() {
        use crate::model::{Mapping, SurfaceImage, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (160u32, 160u32);
        let floats = {
            let mut f = SCHEMA.defaults();
            f[look_index("distance")] = 0.6;
            f
        };
        let red: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Monstruo::new();
        let mut draw = |model: &mut Monstruo| {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: 3.0,
                width: w,
                height: h,
                fondo_alpha: false, detail: 1.0,
            };
            model.render(&ctx, &floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };
        assert_eq!(model.surfaces(), &["skin", "eyes"]);

        let plain = draw(&mut model);
        model.set_surface(
            "skin",
            Some(SurfaceSpec {
                image: Some(SurfaceImage {
                    pixels: &red,
                    width: 4,
                    height: 4,
                    mix: 1.0,
                    mapping: Mapping::Object,
                    scale: 1.0,
                }),
                material: None,
            }),
        );
        let painted = draw(&mut model);
        assert_ne!(plain, painted, "a skin picture at mix 1 did not change the frame");
        let red_sum = |f: &[u8]| f.chunks(4).map(|p| p[0] as u64).sum::<u64>();
        assert!(
            red_sum(&painted) > red_sum(&plain),
            "the red picture added no red: {} vs {}",
            red_sum(&painted),
            red_sum(&plain)
        );

        model.set_surface("skin", None);
        let restored = draw(&mut model);
        assert_eq!(plain, restored, "clearing the skin picture did not restore the frame");
    }

    /// Every species' shader takes both pictures: each one is built with the
    /// surface bindings, drawn with `skin` and `eyes` painted, and changes the
    /// frame against the unpainted one.
    #[test]
    fn every_species_draws_both_surface_pictures() {
        use crate::model::{Mapping, SurfaceImage, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (96u32, 96u32);
        let stripes: Vec<u8> = (0..16).flat_map(|i| if i % 2 == 0 { [0, 255, 0, 255] } else { [0, 0, 255, 255] }).collect();
        for kind in Kind::ALL {
            let mut floats = SCHEMA.defaults();
            floats[0] = Kind::ALL.iter().position(|k| k == kind).unwrap() as f32;
            floats[look_index("distance")] = 0.6;
            let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
            let mut model = Monstruo::new();
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: 3.0,
                width: w,
                height: h,
                fondo_alpha: false, detail: 1.0,
            };
            model.render(&ctx, &floats, &view, FORMAT);
            let plain = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h);
            for (part, mapping) in [("skin", Mapping::Object), ("eyes", Mapping::Projector)] {
                model.set_surface(
                    part,
                    Some(SurfaceSpec {
                        image: Some(SurfaceImage {
                            pixels: &stripes,
                            width: 4,
                            height: 4,
                            mix: 1.0,
                            mapping,
                            scale: 1.0,
                        }),
                        material: None,
                    }),
                );
            }
            model.render(&ctx, &floats, &view, FORMAT);
            let painted = crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h);
            assert_ne!(plain, painted, "{}: the pictures did not reach the frame", kind.name());
        }
    }

    /// Every kind is a surface: at `mat_mix` 1 it changes the frame, and
    /// clearing the part restores the unpainted frame exactly. Covers the
    /// brief's "frame test per kind" and "clearing restores the frame".
    #[test]
    fn every_material_kind_changes_the_frame_and_clearing_restores_it() {
        use crate::model::{MaterialSpec, SurfaceSpec};
        let gpu = crate::gpu::Gpu::new_headless();
        let (w, h) = (160u32, 160u32);
        let floats = {
            let mut f = SCHEMA.defaults();
            f[look_index("distance")] = 0.6;
            f
        };
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let mut model = Monstruo::new();
        let draw = |model: &mut Monstruo| {
            let ctx = Ctx {
                device: &gpu.device,
                queue: &gpu.queue,
                time: 3.0,
                width: w,
                height: h,
                fondo_alpha: false, detail: 1.0,
            };
            model.render(&ctx, &floats, &view, FORMAT);
            crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
        };
        let plain = draw(&mut model);
        let kinds = ["matte", "concrete", "wood", "leather", "metal", "glass", "matrix"];
        for (kind, name) in kinds.iter().enumerate() {
            model.set_surface(
                "skin",
                Some(SurfaceSpec {
                    image: None,
                    material: Some(MaterialSpec {
                        kind: kind as f32,
                        mix: 1.0,
                        scale: 1.0,
                        ..Default::default()
                    }),
                }),
            );
            let painted = draw(&mut model);
            assert_ne!(plain, painted, "{name} did not change the frame");
            model.set_surface("skin", None);
            let restored = draw(&mut model);
            assert_eq!(plain, restored, "clearing {name} did not restore the frame");
        }
    }

    fn render_rgba(gpu: &crate::gpu::Gpu, floats: &[f32], w: u32, h: u32) -> Vec<u8> {
        render_rgba_at(gpu, floats, w, h, 3.0)
    }

    fn render_rgba_at(gpu: &crate::gpu::Gpu, floats: &[f32], w: u32, h: u32, time: f32) -> Vec<u8> {
        let (texture, view) = crate::gpu::offscreen_target(&gpu.device, w, h, FORMAT);
        let ctx = Ctx {
            device: &gpu.device,
            queue: &gpu.queue,
            time,
            width: w,
            height: h,
            fondo_alpha: false, detail: 1.0,
        };
        let mut model = Monstruo::new();
        model.render(&ctx, floats, &view, FORMAT);
        crate::gpu::read_rgba8(&gpu.device, &gpu.queue, &texture, w, h)
    }

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
}
