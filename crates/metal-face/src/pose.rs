//! The rig proper: controls → moved landmarks → a flat float block.
//!
//! # Order of operations
//!
//! [`pose`] applies the controls in the order a face is built, each step on
//! the result of the last:
//!
//! 1. **Jaw.** `jaw_open` rotates about the x axis through the jaw pivot (down
//!    and slightly back, as a real mandible does) and `jaw_side` shifts it
//!    sideways. The jaw carries the lower lip (corners half-way, upper lip not
//!    at all), the lower teeth and the whole resting tongue, which lies on
//!    the jaw's floor. The cavity's floor drops with the lower lip; its roof
//!    stays with the skull.
//! 2. **Lips.** Stretch, funnel and pucker change the ring's width, how far it
//!    pushes forward and its thickness; `lip_upper_up` / `lip_lower_down`
//!    peel the lips off the teeth; the corners go up or down. `lip_close`
//!    goes last and meets the two lips half-way, so a p/b/m closes the mouth
//!    whatever the jaw is doing — that is what makes PP readable on an open
//!    sung vowel.
//! 3. **Tongue.** `tongue_up` bends the chain toward the alveolar ridge (in
//!    the skull's frame, so it reaches the upper teeth with the jaw open);
//!    `tongue_curl` rolls the last two segments; `tongue_out` slides the chain
//!    forward, past the lips by an amount scaled by the species' tongue
//!    length. Before `tongue_out`, every joint is kept inside the cavity (the
//!    palate is solid), so with `tongue_out` at 0 the chain never leaves it.
//! 4. **Upper face.** Cheeks puff, nostrils flare, brows go up or furrow, and
//!    the lids open (`eye_wide`), narrow (`squint`) or shut (`blink`, which
//!    wins over both because a blink is a blink).
//! 5. **Head.** `head_pitch/yaw/roll` become one rotation, [`Posed::head`].
//!    It is **not** applied to the landmarks: they stay head-local, and the
//!    shader rotates the ray into the head frame once and evaluates the skull
//!    and every face primitive there. One transform per pixel instead of one
//!    per landmark, and the skull and the face can never disagree.
//!
//! # The packed layout
//!
//! [`Posed::pack`] writes a flat `f32` block made of `vec4`s, in the order of
//! [`LAYOUT`], so a WGSL struct of `array<vec4f, N>` fields mirrors it exactly
//! with no padding surprises (std140 and std430 agree on `vec4` arrays):
//!
//! | section | vec4s | each vec4 |
//! |---|---|---|
//! | `jaw` | 2 | (pivot.xyz, open angle rad), (side offset, 0, 0, 0) |
//! | `head` | 3 | columns of the head rotation matrix (xyz, 0) |
//! | `shape` | 3 | (snout, jaw_width, brow_ridge, horns), (scales, eye_spacing, fang_length, tongue_length), (forked 0/1, 0, 0, 0) |
//! | `lips` | `LIP_RING` | (centre.xyz, radius), ring order from the left corner |
//! | `cavity` | 2 | (centre.xyz, 0), (radii.xyz, 0) |
//! | `upper_teeth` | 2 × `TEETH` | per tooth: (root.xyz, radius), (tip.xyz, 0) |
//! | `lower_teeth` | 2 × `TEETH` | as above |
//! | `tongue` | 2 × `TONGUE_SEGMENTS` | per segment, root first: (a.xyz, radius), (b.xyz, 0) |
//! | `eyes` | 4 | per eye, L then R: (centre.xyz, radius), (upper lid, lower lid, 0, 0) |
//! | `brows` | 2 × `BROW_POINTS` | L inner, mid, outer, then R: (p.xyz, 0) |
//! | `nostrils` | 2 | L, R: (centre.xyz, radius) |
//! | `cheeks` | 2 | L, R: (centre.xyz, radius) |
//! | `controls` | ceil(`COUNT`/4) | one clamped float per `FaceControl::ALL` in order, then zero padding |
//! | `rest` | 1 | (mouth.xyz, jaw_pivot.z) from the rest landmark |
//!
//! The WGSL mirror is therefore:
//!
//! ```text
//! struct Face {
//!     jaw: array<vec4f, 2>,
//!     head: array<vec4f, 3>,
//!     shape: array<vec4f, 3>,
//!     lips: array<vec4f, 12>,        // LIP_RING
//!     cavity: array<vec4f, 2>,
//!     upper_teeth: array<vec4f, 12>, // 2 * TEETH
//!     lower_teeth: array<vec4f, 12>,
//!     tongue: array<vec4f, 8>,       // 2 * TONGUE_SEGMENTS
//!     eyes: array<vec4f, 4>,
//!     brows: array<vec4f, 6>,        // 2 * BROW_POINTS
//!     nostrils: array<vec4f, 2>,
//!     cheeks: array<vec4f, 2>,
//!     controls: array<vec4f, 12>,    // ceil(COUNT / 4)
//!     rest: array<vec4f, 1>,
//! }
//! ```
//!
//! The numbers in that sketch are today's values of the constants; the
//! source of truth is [`LAYOUT`], and a test checks `pack` against it. Like
//! the controls, the layout only grows at the end.

use glam::{EulerRot, Mat3, Quat, Vec3};

use crate::control::{Controls, FaceControl as C};
use crate::topology::{
    is_lower_lip, is_upper_lip, lip_angle, lip_partner, Ball, Topology, BROW_POINTS, CORNER_R,
    LIP_RING, LOWER_MID, TEETH, TONGUE_SEGMENTS,
};

/// The jaw's angle at `jaw_open = 1`, in radians (≈ 22°). With the hinge
/// near the ear that drops the lower lip by about a third of a head radius on
/// a neutral face — a monster's gape, more on a long snout.
pub const JAW_MAX: f32 = 0.38;
/// Sideways jaw shift at `jaw_side = ±1`, in head radii.
pub const JAW_SIDE_MAX: f32 = 0.06;
/// Head angles at ±1, in radians: pitch, yaw, roll.
pub const HEAD_MAX: [f32; 3] = [0.4, 0.6, 0.3];
/// Lid angle both lids meet at when the eye is shut: a little below centre,
/// so a blink is mostly the upper lid coming down.
pub const LID_CLOSED: f32 = -0.15;

/// How far inside the cavity wall a tongue joint is kept, as a fraction of
/// the ellipsoid's radii.
const INSIDE: f32 = 0.97;

/// The packed sections, in order, with their size in `vec4`s. See the module
/// docs for what each vec4 holds.
pub const LAYOUT: &[(&str, usize)] = &[
    ("jaw", 2),
    ("head", 3),
    ("shape", 3),
    ("lips", LIP_RING),
    ("cavity", 2),
    ("upper_teeth", 2 * TEETH),
    ("lower_teeth", 2 * TEETH),
    ("tongue", 2 * TONGUE_SEGMENTS),
    ("eyes", 4),
    ("brows", 2 * BROW_POINTS),
    ("nostrils", 2),
    ("cheeks", 2),
    ("controls", (C::COUNT + 3) / 4),
    ("rest", 1),
];

/// Total `vec4`s in [`Posed::pack`], derived from [`LAYOUT`].
pub const VEC4S: usize = {
    let mut n = 0;
    let mut i = 0;
    while i < LAYOUT.len() {
        n += LAYOUT[i].1;
        i += 1;
    }
    n
};

/// Total floats in [`Posed::pack`].
pub const PACKED: usize = VEC4S * 4;

/// A posed face: the moved landmarks, still head-local, plus the jaw and head
/// transforms the shader needs for the skull itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Posed {
    pub face: Topology,
    /// The jaw's rotation about the pivot's x axis, radians, + opens.
    pub jaw_angle: f32,
    /// The jaw's sideways shift, head radii.
    pub jaw_side: f32,
    /// The whole head's rotation, head frame → view.
    pub head: Quat,
    /// The raw control strengths `pose` was given (before clamping). `pack`
    /// writes `controls.clamped()`, the values the deformation actually used.
    pub controls: Controls,
}

/// Poses `topology` with `controls`. Pure; see the module docs for the order.
pub fn pose(topology: &Topology, controls: &Controls) -> Posed {
    let c = controls.clamped();
    let v = |k: C| c.get(k);
    let rest = topology;
    let mut f = topology.clone();

    // 1. Jaw.
    let jaw_angle = v(C::JawOpen) * JAW_MAX;
    let jaw_side = v(C::JawSide) * JAW_SIDE_MAX;
    let rot = Quat::from_rotation_x(jaw_angle);
    let pivot = rest.jaw_pivot;
    // Exactly the identity at rest, not a round trip through the pivot, so
    // rest controls reproduce the topology bit for bit.
    let jaw_moves = jaw_angle != 0.0 || jaw_side != 0.0;
    let jaw = |p: Vec3| {
        if jaw_moves {
            pivot + rot * (p - pivot) + Vec3::X * jaw_side
        } else {
            p
        }
    };

    for (i, lip) in f.lips.iter_mut().enumerate() {
        let carry = lip_carry(i);
        lip.at = lip.at.lerp(jaw(lip.at), carry);
    }
    for t in &mut f.lower_teeth {
        t.a = jaw(t.a);
        t.b = jaw(t.b);
    }
    for j in &mut f.tongue {
        *j = jaw(*j);
    }
    // The cavity is the space between the skull's roof and the jaw's floor,
    // so it opens like the jaw at half the angle: its centre turns half-way
    // about the pivot (down and back, following what the jaw carries) and it
    // grows taller by half the lower lip's drop.
    let drop = f.lips[LOWER_MID].at.y - rest.lips[LOWER_MID].at.y;
    if jaw_moves {
        let half = Quat::from_rotation_x(0.5 * jaw_angle);
        f.cavity.centre = pivot + half * (rest.cavity.centre - pivot) + Vec3::X * (0.5 * jaw_side);
        f.cavity.radii.y += 0.5 * (-drop).max(0.0);
    }

    // 2. Lips.
    let stretch = v(C::LipStretch);
    let funnel = v(C::LipFunnel);
    let pucker = v(C::LipPucker);
    let upper_up = v(C::LipUpperUp);
    let lower_down = v(C::LipLowerDown);
    let corner = v(C::LipCornerUp) - v(C::LipCornerDown);
    let press = v(C::LipPress);
    let width = (1.0 + 0.25 * stretch - 0.25 * funnel - 0.4 * pucker - 0.12 * press).max(0.35);
    let thick = (1.0 + 0.3 * funnel + 0.35 * pucker - 0.2 * stretch - 0.45 * press).max(0.35);
    // The per-side muscles, (left, right). Each adds to its symmetric
    // sibling, clamped, so the sum never exceeds what one control could do.
    // The species' expression gain scales how far a muscle *moves* the
    // landmarks (not whether a lid shuts: wide/blink below read raw).
    let g = rest.species.expression;
    let pair = |l: C, r: C| (g * v(l), g * v(r));
    let up_side = pair(C::LipUpperUpL, C::LipUpperUpR);
    let sneer = pair(C::NoseWrinkleL, C::NoseWrinkleR);
    let smile_side = pair(C::SmileL, C::SmileR);
    let frown_side = pair(C::FrownL, C::FrownR);
    let stretch_side = pair(C::LipStretchL, C::LipStretchR);
    let raise = pair(C::CheekRaiseL, C::CheekRaiseR);
    for (i, lip) in f.lips.iter_mut().enumerate() {
        let (sn, cs) = lip_angle(i).sin_cos();
        let mid = sn.abs();
        let cornerness = cs * cs;
        let cx = jaw_side * lip_carry(i);
        let p = &mut lip.at;
        p.x = cx + (p.x - cx) * width;
        p.z += (0.05 * funnel + 0.08 * pucker) * (0.5 + 0.5 * mid) - 0.03 * stretch * cornerness;
        // Funnel opens the lips round; pucker presses them.
        p.y += sn * (0.025 * funnel - 0.015 * pucker);
        if is_upper_lip(i) {
            p.y += 0.06 * upper_up * mid;
        }
        if is_lower_lip(i) {
            p.y -= 0.06 * lower_down * mid;
        }
        p.y += 0.05 * corner * cornerness;
        lip.radius *= thick;

        // Expression muscles. `wl`/`wr` say how much of point i is the left
        // or the right corner (cs² on its own side, 0 on the other), `hl`/`hr`
        // how much of it is the left or right half of the lip (the fang
        // position, cs ≈ ±0.5, gets the most of a one-sided snarl).
        let (wl, wr) = (cs.max(0.0).powi(2), cs.min(0.0).powi(2));
        let (hl, hr) = ((0.5 + cs).clamp(0.0, 1.0), (0.5 - cs).clamp(0.0, 1.0));
        let by_side = |(l, r): (f32, f32), (a, b): (f32, f32)| l * a + r * b;
        let sx = cs.signum();
        let smile = by_side(smile_side, (wl, wr));
        let frown = by_side(frown_side, (wl, wr));
        let st = by_side(stretch_side, (wl, wr));
        let lift = by_side(raise, (wl, wr));
        // AU12 goes up, out and back along the cheek; AU15 straight down
        // and a little in; AU20 straight out toward the ear, flat.
        p.y += 0.085 * smile - 0.085 * frown + 0.02 * lift;
        p.x += sx * (0.035 * smile + 0.075 * st - 0.01 * frown);
        p.z -= 0.05 * smile + 0.035 * st;
        if is_upper_lip(i) {
            let snarl = by_side(up_side, (hl, hr)) + 0.6 * by_side(sneer, (hl, hr));
            p.y += 0.075 * snarl.min(g) * (0.35 + 0.65 * mid);
            p.z += 0.012 * snarl.min(g);
        }
        if is_lower_lip(i) {
            // The lower lip goes with a frown's corners: the pout's chin.
            p.y -= 0.02 * by_side(frown_side, (hl, hr)) * mid;
        }
        // Pressed lips roll in toward the teeth.
        p.z -= 0.02 * press;
    }
    let close = v(C::LipClose).max(press);
    if close > 0.0 {
        for i in 1..CORNER_R {
            let j = lip_partner(i);
            let (u, l) = (f.lips[i], f.lips[j]);
            let gap = (u.at.y - u.radius) - (l.at.y + l.radius);
            if gap > 0.0 {
                let meet = (l.at.y + l.radius) + 0.5 * gap;
                f.lips[i].at.y = lerp(u.at.y, meet + u.radius, close);
                f.lips[j].at.y = lerp(l.at.y, meet - l.radius, close);
            }
        }
    }

    // 3. Tongue.
    let up = v(C::TongueUp);
    if up > 0.0 {
        let root = f.tongue[0];
        let target = rest.alveolar_ridge;
        let ctrl = (root + target) * 0.5 + Vec3::Y * 0.05;
        for k in 1..=TONGUE_SEGMENTS {
            let t = k as f32 / TONGUE_SEGMENTS as f32;
            let s = 1.0 - t;
            let bez = root * (s * s) + ctrl * (2.0 * s * t) + target * (t * t);
            f.tongue[k] = f.tongue[k].lerp(bez, up);
        }
    }
    let curl = v(C::TongueCurl);
    if curl != 0.0 {
        // Negative x rotation lifts points in front of the pivot: + curls up
        // and back (retroflex). Down has far less room — the tip already
        // rests on the floor of the mouth — so it bends a third as far.
        let bend = if curl > 0.0 { 0.8 } else { 0.3 };
        let q = Quat::from_rotation_x(-bend * curl);
        for pivot_joint in TONGUE_SEGMENTS - 2..TONGUE_SEGMENTS {
            let o = f.tongue[pivot_joint];
            for j in pivot_joint + 1..=TONGUE_SEGMENTS {
                f.tongue[j] = o + q * (f.tongue[j] - o);
            }
        }
    }
    // The palate and the floor are solid: whatever the up/curl mix asked
    // for, a joint that ended outside the cavity is pulled back onto its
    // wall. Only `tongue_out`, below, may take the tongue past it.
    for j in &mut f.tongue {
        let level = f.cavity.level(*j);
        if level > INSIDE * INSIDE {
            *j = f.cavity.centre + (*j - f.cavity.centre) * (INSIDE / level.sqrt());
        }
    }
    let out = v(C::TongueOut);
    if out > 0.0 {
        let reach = 0.42 * rest.species.tongue_length;
        for (k, j) in f.tongue.iter_mut().enumerate() {
            let t = k as f32 / TONGUE_SEGMENTS as f32;
            j.z += reach * out * (0.25 + 0.75 * t);
            j.y -= 0.04 * out * t;
        }
    }

    // 4. Upper face.
    let puff = v(C::CheekPuff);
    let smile = v(C::LipCornerUp);
    let squint = v(C::Squint);
    let of_side = |side: usize, (l, r): (f32, f32)| if side == crate::topology::L { l } else { r };
    for (side, ch) in f.cheeks.iter_mut().enumerate() {
        let sx = side_sign(side);
        ch.radius *= 1.0 + 0.35 * puff;
        ch.at.x += sx * 0.04 * puff;
        ch.at.z += 0.03 * puff;
        ch.at.y += 0.03 * smile + 0.02 * squint;
        // AU6 bunches the cheek up under the eye; AU12 and AU9 drag it.
        let lift = of_side(side, raise);
        let grin = of_side(side, smile_side);
        let wrinkle = of_side(side, sneer);
        ch.at.y += 0.075 * lift + 0.04 * grin + 0.035 * wrinkle;
        ch.at.z += 0.03 * lift + 0.02 * grin;
        ch.at.x += sx * 0.02 * grin;
        ch.radius *= 1.0 + 0.12 * lift + 0.08 * grin;
    }
    let flare = v(C::NostrilFlare);
    for (side, n) in f.nostrils.iter_mut().enumerate() {
        n.radius *= 1.0 + 0.6 * flare;
        n.at.x += side_sign(side) * 0.015 * flare;
        // AU9: the wing hikes up and back, and the nostril opens with it.
        let wrinkle = of_side(side, sneer);
        n.at.y += 0.045 * wrinkle;
        n.at.z -= 0.012 * wrinkle;
        n.at.x += side_sign(side) * 0.01 * wrinkle;
        n.radius *= 1.0 + 0.25 * wrinkle;
    }
    let brow_up = v(C::BrowUp);
    let furrow = v(C::BrowFurrow);
    let inner_up = g * v(C::BrowInnerUp);
    let outer_up = pair(C::BrowOuterUpL, C::BrowOuterUpR);
    let brow_down = pair(C::BrowDownL, C::BrowDownR);
    for (side, brow) in f.brows.iter_mut().enumerate() {
        let sx = side_sign(side);
        let outer = of_side(side, outer_up);
        let down = of_side(side, brow_down);
        let wrinkle = of_side(side, sneer);
        for (k, p) in brow.iter_mut().enumerate() {
            // inner, mid, outer
            let lift = [1.0, 0.9, 0.6][k];
            let pull = [1.0, 0.6, 0.0][k];
            p.y += 0.08 * brow_up * lift - 0.05 * furrow * pull;
            p.x -= sx * 0.04 * furrow * pull;
            // AU1 tilts the brow (inner up, outer a hair down); AU2 lifts
            // the tail; AU4 drags the whole brow down, in and forward.
            p.y += 0.1 * inner_up * [1.0, 0.45, -0.1][k];
            p.x -= sx * 0.015 * inner_up * [1.0, 0.4, 0.0][k];
            p.y += 0.1 * outer * [0.15, 0.6, 1.0][k];
            p.y -= 0.085 * down * [1.0, 0.75, 0.4][k];
            p.x -= sx * 0.05 * down * [1.0, 0.55, 0.1][k];
            p.z += 0.025 * down * [1.0, 0.6, 0.2][k];
            // The nose wrinkler pulls the inner brow down with it.
            p.y -= 0.035 * wrinkle * [1.0, 0.4, 0.0][k];
        }
    }
    let wide = v(C::EyeWide);
    let blink = v(C::Blink);
    let wide_side = (v(C::EyeWideL), v(C::EyeWideR));
    let blink_side = (v(C::BlinkL), v(C::BlinkR));
    for (side, eye) in f.eyes.iter_mut().enumerate() {
        let wide = (wide + of_side(side, wide_side)).min(1.0);
        let blink = (blink + of_side(side, blink_side)).min(1.0);
        // AU6/AU9 push the lower lid up from below; AU4 weighs the upper
        // lid down from above (the glare).
        let lower_push = 0.3 * of_side(side, raise) + 0.15 * of_side(side, sneer);
        let lower_push = lower_push.min(0.45);
        let upper_weigh = (0.18 * of_side(side, brow_down)).min(0.3);
        let mut upper = eye.upper_lid + 0.35 * wide - 0.15 * squint - upper_weigh;
        let mut lower = eye.lower_lid - 0.1 * wide + 0.35 * squint + lower_push;
        if lower > upper {
            let m = 0.5 * (upper + lower);
            upper = m;
            lower = m;
        }
        eye.upper_lid = lerp(upper, LID_CLOSED, blink);
        eye.lower_lid = lerp(lower, LID_CLOSED, blink);
    }

    // 5. Head.
    let head = Quat::from_euler(
        EulerRot::YXZ,
        v(C::HeadYaw) * HEAD_MAX[1],
        v(C::HeadPitch) * HEAD_MAX[0],
        v(C::HeadRoll) * HEAD_MAX[2],
    );

    Posed {
        face: f,
        jaw_angle,
        jaw_side,
        head,
        controls: *controls,
    }
}

/// How much of the jaw's motion lip ring point `i` follows: none on the upper
/// lip, all on the lower, half at the corners.
fn lip_carry(i: usize) -> f32 {
    if is_upper_lip(i) {
        0.0
    } else if is_lower_lip(i) {
        1.0
    } else {
        0.5
    }
}

/// +1 for the creature's left (+x), −1 for its right.
fn side_sign(side: usize) -> f32 {
    if side == crate::topology::L {
        1.0
    } else {
        -1.0
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Writes `vec4`s and remembers which section each came from, so a test can
/// hold `pack` to [`LAYOUT`].
struct Packer {
    out: Vec<f32>,
    sections: Vec<(&'static str, usize)>,
}

impl Packer {
    fn section(&mut self, name: &'static str) {
        self.sections.push((name, 0));
    }
    fn v4(&mut self, x: f32, y: f32, z: f32, w: f32) {
        self.out.extend_from_slice(&[x, y, z, w]);
        if let Some(s) = self.sections.last_mut() {
            s.1 += 1;
        }
    }
    fn p(&mut self, p: Vec3, w: f32) {
        self.v4(p.x, p.y, p.z, w);
    }
    fn ball(&mut self, b: &Ball) {
        self.p(b.at, b.radius);
    }
}

impl Posed {
    /// The flat float block a WGSL struct mirrors; see the module docs.
    pub fn pack(&self) -> Vec<f32> {
        self.pack_sections().0
    }

    /// The pack plus the sections it actually wrote, in order. Public for
    /// tests and for a debug overlay; the layout itself is [`LAYOUT`].
    pub fn pack_sections(&self) -> (Vec<f32>, Vec<(&'static str, usize)>) {
        let f = &self.face;
        let mut k = Packer {
            out: Vec::with_capacity(PACKED),
            sections: Vec::with_capacity(LAYOUT.len()),
        };
        k.section("jaw");
        k.p(f.jaw_pivot, self.jaw_angle);
        k.v4(self.jaw_side, 0.0, 0.0, 0.0);

        k.section("head");
        let m = Mat3::from_quat(self.head);
        for col in [m.x_axis, m.y_axis, m.z_axis] {
            k.p(col, 0.0);
        }

        k.section("shape");
        let sp = &f.species;
        k.v4(sp.snout, sp.jaw_width, sp.brow_ridge, sp.horns);
        k.v4(sp.scales, sp.eye_spacing, sp.fang_length, sp.tongue_length);
        k.v4(if sp.forked_tongue { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0);

        k.section("lips");
        for b in &f.lips {
            k.ball(b);
        }

        k.section("cavity");
        k.p(f.cavity.centre, 0.0);
        k.p(f.cavity.radii, 0.0);

        for (name, row) in [
            ("upper_teeth", &f.upper_teeth),
            ("lower_teeth", &f.lower_teeth),
        ] {
            k.section(name);
            for t in row {
                k.p(t.a, t.radius);
                k.p(t.b, 0.0);
            }
        }

        k.section("tongue");
        for i in 0..TONGUE_SEGMENTS {
            let t = f.tongue_capsule(i);
            k.p(t.a, t.radius);
            k.p(t.b, 0.0);
        }

        k.section("eyes");
        for e in &f.eyes {
            k.p(e.centre, e.radius);
            k.v4(e.upper_lid, e.lower_lid, 0.0, 0.0);
        }

        k.section("brows");
        for brow in &f.brows {
            for &p in brow {
                k.p(p, 0.0);
            }
        }

        k.section("nostrils");
        for n in &f.nostrils {
            k.ball(n);
        }

        k.section("cheeks");
        for c in &f.cheeks {
            k.ball(c);
        }

        k.section("controls");
        let controls = self.controls.clamped();
        let mut vals: Vec<f32> = C::ALL.iter().map(|&c| controls.get(c)).collect();
        while vals.len() % 4 != 0 {
            vals.push(0.0);
        }
        for chunk in vals.chunks(4) {
            k.v4(chunk[0], chunk[1], chunk[2], chunk[3]);
        }

        k.section("rest");
        k.p(f.mouth, f.jaw_pivot.z);

        (k.out, k.sections)
    }

    /// A head-local point in the view frame (after the head rotation).
    pub fn to_view(&self, p: Vec3) -> Vec3 {
        self.head * p
    }
}
