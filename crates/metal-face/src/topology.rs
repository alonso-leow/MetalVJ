//! The shared face layout: one set of named landmarks every species uses.
//!
//! # The frame
//!
//! Head-local, in units of the head's radius: the skull is roughly the unit
//! sphere, **+y is up, +z points toward the viewer** (the face looks down +z),
//! +x is the creature's own **left** — so "L" in this crate is always +x and
//! the viewer sees it on their right. The head rotation
//! ([`crate::Posed::head`]) is applied to this frame as a whole, so every
//! landmark here is stated as if the head were looking straight at the
//! camera.
//!
//! # What is here, and why in this form
//!
//! Everything is a primitive the shader can build an SDF from without knowing
//! anything about faces: points, balls (a centre and a radius), capsules (two
//! ends and a radius), one ellipsoid, and the eyes' two lid angles.
//!
//! - **Jaw**: two hinges near the ears and the pivot between them. The jaw
//!   rotates about the x axis through the pivot; the hinges exist so a shader
//!   or a debug overlay can draw where it turns.
//! - **Lip ring**: [`LIP_RING`] balls round the mouth, starting at the left
//!   corner and going over the upper lip to the right corner and back under
//!   the lower lip ([`CORNER_L`], [`UPPER_MID`], [`CORNER_R`],
//!   [`LOWER_MID`]). Each ball is a point on the lip's centreline and the
//!   lip's thickness there, so a smooth-union of capsules between neighbours
//!   is a lip. Twelve points is enough to read a pucker as round and a smile
//!   as curved; fewer reads as a polygon.
//! - **Cavity**: an ellipsoid, the inside of the mouth. The tongue lives in
//!   it, and the shader carves it out of the head.
//! - **Teeth**: [`TEETH`] capsules per row, root at the gum, tip at the edge,
//!   ordered left to right like the lips. [`FANGS`] are the canines, the ones
//!   a species lengthens.
//! - **Tongue**: [`TONGUE_SEGMENTS`] capsules chained root → tip, stored as
//!   the joints between them plus a radius per segment.
//! - **Alveolar ridge**: the point just behind the upper front teeth that the
//!   tongue tip reaches for t/d/n/l.
//! - **Eyes, lids, brows, nostrils, cheeks**: one each per side.

use glam::Vec3;

use crate::species::Species;

/// Points round the lip ring.
pub const LIP_RING: usize = 12;
/// The creature's left mouth corner (+x).
pub const CORNER_L: usize = 0;
/// The middle of the upper lip.
pub const UPPER_MID: usize = LIP_RING / 4;
/// The creature's right mouth corner (−x).
pub const CORNER_R: usize = LIP_RING / 2;
/// The middle of the lower lip.
pub const LOWER_MID: usize = 3 * LIP_RING / 4;
/// Teeth per row.
pub const TEETH: usize = 6;
/// Indices of the canines in each row, the teeth a species lengthens.
pub const FANGS: [usize; 2] = [1, TEETH - 2];
/// Capsules in the tongue chain (it has one more joint than this).
pub const TONGUE_SEGMENTS: usize = 4;
/// Points per brow: inner, mid, outer.
pub const BROW_POINTS: usize = 3;
/// Index of the creature's left side in every per-side pair.
pub const L: usize = 0;
/// Index of the creature's right side in every per-side pair.
pub const R: usize = 1;

/// A point with a radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ball {
    pub at: Vec3,
    pub radius: f32,
}

/// A segment with a radius: `a` is the root end, `b` the free end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
}

impl Capsule {
    /// Distance from `p` to the capsule's surface (negative inside).
    pub fn distance(&self, p: Vec3) -> f32 {
        let ab = self.b - self.a;
        let t = ((p - self.a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
        (p - (self.a + ab * t)).length() - self.radius
    }
}

/// An axis-aligned ellipsoid (in the head frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipsoid {
    pub centre: Vec3,
    pub radii: Vec3,
}

impl Ellipsoid {
    /// The ellipsoid's implicit value: < 1 inside, 1 on the surface.
    pub fn level(&self, p: Vec3) -> f32 {
        ((p - self.centre) / self.radii).length_squared()
    }

    pub fn contains(&self, p: Vec3) -> bool {
        self.level(p) <= 1.0
    }
}

/// An eyeball and its two lids.
///
/// Each lid is the eyeball's shell cut by a plane through its centre, tilted
/// about the x axis. The angles are the lid edges' elevation in radians,
/// measured from straight ahead (+z), positive up. The eye is open between
/// `lower_lid` and `upper_lid`; when they are equal it is shut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eye {
    pub centre: Vec3,
    pub radius: f32,
    pub upper_lid: f32,
    pub lower_lid: f32,
}

impl Eye {
    /// How open the eye is, in radians of lid separation.
    pub fn opening(&self) -> f32 {
        (self.upper_lid - self.lower_lid).max(0.0)
    }
}

/// The lid edge angles of a neutral open eye.
pub const UPPER_LID_REST: f32 = 0.55;
pub const LOWER_LID_REST: f32 = -0.45;

/// The face's landmarks. See the module docs for the frame and the parts.
#[derive(Clone, Debug, PartialEq)]
pub struct Topology {
    /// The proportions these landmarks were built from (the shader reads the
    /// skull-only ones from here).
    pub species: Species,
    pub jaw_hinge: [Vec3; 2],
    pub jaw_pivot: Vec3,
    /// The centre of the lip ring at rest.
    pub mouth: Vec3,
    pub lips: [Ball; LIP_RING],
    pub cavity: Ellipsoid,
    pub upper_teeth: [Capsule; TEETH],
    pub lower_teeth: [Capsule; TEETH],
    /// Joints of the tongue chain, root first.
    pub tongue: [Vec3; TONGUE_SEGMENTS + 1],
    /// Radius of each tongue segment, root first.
    pub tongue_radius: [f32; TONGUE_SEGMENTS],
    /// Where the tongue tip goes for t/d/n/l: just behind the upper front
    /// teeth.
    pub alveolar_ridge: Vec3,
    pub eyes: [Eye; 2],
    /// Per side, inner → mid → outer.
    pub brows: [[Vec3; BROW_POINTS]; 2],
    pub nostrils: [Ball; 2],
    pub cheeks: [Ball; 2],
}

/// The angle of lip ring point `i` about the mouth centre, from +x toward +y:
/// 0 at the left corner, π/2 at the upper middle, π at the right corner, 3π/2
/// at the lower middle (counter-clockwise as the viewer sees it).
pub fn lip_angle(i: usize) -> f32 {
    i as f32 * std::f32::consts::TAU / LIP_RING as f32
}

/// Whether ring point `i` is on the upper lip (corners excluded).
pub fn is_upper_lip(i: usize) -> bool {
    i > CORNER_L && i < CORNER_R
}

/// Whether ring point `i` is on the lower lip (corners excluded).
pub fn is_lower_lip(i: usize) -> bool {
    i > CORNER_R
}

/// The lower-lip point facing upper-lip point `i` across the mouth.
pub fn lip_partner(i: usize) -> usize {
    LIP_RING - i
}

impl Topology {
    /// The neutral face: [`Species::NEUTRAL`], a roughly human layout.
    pub fn neutral() -> Self {
        Self::for_species(&Species::NEUTRAL)
    }

    /// The same landmarks, moved and scaled by a species' proportions. The
    /// identity of every landmark (its index, its meaning) never changes.
    pub fn for_species(sp: &Species) -> Self {
        let w = sp.jaw_width;
        let s = sp.snout;
        // The muzzle pushes the whole mouth forward and slightly down, and
        // deepens the inside by the same proportion, so a long snout gets a
        // long mouth rather than a flat one stuck on the end of a nose.
        let depth = 1.0 + 0.6 * s;
        let mouth = Vec3::new(0.0, -0.45 - 0.05 * s, 0.88 + 0.45 * s);

        // Narrower and fuller than the first cut: a smaller opening (the
        // complaint was "too big") with lips plump enough to bury the carve
        // seam under their own (now skin-toned) surface instead of leaving a
        // ring of bare cavity wall showing around the hole.
        let half_width = 0.21 * w;
        let lips = std::array::from_fn(|i| {
            let th = lip_angle(i);
            let (sn, cs) = th.sin_cos();
            let x = half_width * cs;
            let upper = sn > 1e-4;
            let h = if upper { 0.04 } else { 0.045 };
            let mid_r = if upper { 0.052 } else { 0.058 };
            let corner_r = 0.042;
            Ball {
                at: Vec3::new(
                    x,
                    mouth.y + h * sn,
                    mouth.z - 0.6 * (x / w.max(0.1)).powi(2),
                ),
                radius: corner_r + (mid_r - corner_r) * sn.abs(),
            }
        });

        // The cavity itself (tongue room, teeth arch depth) is left at its
        // original size — only the visible opening shrinks — so the tongue
        // and teeth placement math downstream keeps its invariants.
        let cavity = Ellipsoid {
            centre: Vec3::new(0.0, mouth.y - 0.06, mouth.z - 0.28 * depth),
            radii: Vec3::new(0.24 * w, 0.12, 0.33 * depth),
        };

        // Teeth stand along an arch behind the lips, left to right.
        let tooth_x = |i: usize| {
            let u = i as f32 / (TEETH - 1) as f32; // 0 at the left, 1 at the right
            (0.22 - 0.44 * u) * w
        };
        let arch_z = |x: f32| mouth.z - 0.06 - 0.8 * (x / w.max(0.1)).powi(2);
        let fang = |i: usize| FANGS.contains(&i);
        let upper_teeth = std::array::from_fn(|i| {
            let x = tooth_x(i);
            let z = arch_z(x);
            let extra = if fang(i) { 0.15 * sp.fang_length } else { 0.0 };
            Capsule {
                a: Vec3::new(x, mouth.y + 0.06, z),
                b: Vec3::new(x, mouth.y - extra, z),
                radius: if fang(i) { 0.03 } else { 0.025 },
            }
        });
        let lower_teeth = std::array::from_fn(|i| {
            let x = tooth_x(i) * 0.92;
            let z = arch_z(x) - 0.02;
            let extra = if fang(i) { 0.10 * sp.fang_length } else { 0.0 };
            Capsule {
                a: Vec3::new(x, mouth.y - 0.09, z),
                b: Vec3::new(x, mouth.y - 0.035 + extra, z),
                radius: if fang(i) { 0.028 } else { 0.022 },
            }
        });

        // The resting tongue lies on the floor of the mouth, its tip just
        // behind the lower front teeth. Offsets scale with the cavity depth,
        // so it fits every snout; the species' tongue length shows as reach.
        let tongue_rest = [
            (-0.12, -0.46),
            (-0.10, -0.38),
            (-0.08, -0.27),
            (-0.075, -0.17),
            (-0.07, -0.10),
        ];
        let tongue = tongue_rest.map(|(dy, dz)| Vec3::new(0.0, mouth.y + dy, mouth.z + dz * depth));
        let thick = 0.75 + 0.25 * w;
        let tongue_radius = [0.08, 0.065, 0.05, 0.035].map(|r| r * thick);

        let alveolar_ridge = Vec3::new(0.0, mouth.y + 0.035, mouth.z - 0.10);

        let ridge = sp.brow_ridge;
        let eye_x = 0.33 * sp.eye_spacing;
        let eye = |side: f32| Eye {
            centre: Vec3::new(side * eye_x, 0.18, 0.80 - 0.03 * ridge),
            radius: 0.16,
            upper_lid: UPPER_LID_REST,
            lower_lid: LOWER_LID_REST,
        };
        let brow = |side: f32| {
            [(0.14, 0.88), (0.32, 0.86), (0.48, 0.78)].map(|(x, z)| {
                Vec3::new(
                    side * x * sp.eye_spacing,
                    0.40 + 0.05 * ridge,
                    z + 0.08 * ridge,
                )
            })
        };
        let nostril = |side: f32| Ball {
            at: Vec3::new(side * 0.07, -0.22 - 0.03 * s, 0.97 + 0.45 * s),
            radius: 0.035,
        };
        let cheek = |side: f32| Ball {
            at: Vec3::new(side * 0.45 * w, -0.25, 0.78),
            radius: 0.2,
        };
        let hinge = |side: f32| Vec3::new(side * 0.72 * w.max(1.0), -0.30, -0.05);

        Topology {
            species: *sp,
            jaw_hinge: [hinge(1.0), hinge(-1.0)],
            jaw_pivot: Vec3::new(0.0, -0.30, -0.05),
            mouth,
            lips,
            cavity,
            upper_teeth,
            lower_teeth,
            tongue,
            tongue_radius,
            alveolar_ridge,
            eyes: [eye(1.0), eye(-1.0)],
            brows: [brow(1.0), brow(-1.0)],
            nostrils: [nostril(1.0), nostril(-1.0)],
            cheeks: [cheek(1.0), cheek(-1.0)],
        }
    }

    /// Tongue segment `i` as a capsule, root first.
    pub fn tongue_capsule(&self, i: usize) -> Capsule {
        Capsule {
            a: self.tongue[i],
            b: self.tongue[i + 1],
            radius: self.tongue_radius[i],
        }
    }

    /// The tongue tip's joint.
    pub fn tongue_tip(&self) -> Vec3 {
        self.tongue[TONGUE_SEGMENTS]
    }

    /// The vertical opening between the lips at their middle, surface to
    /// surface; 0 when they touch or press.
    pub fn lip_gap(&self) -> f32 {
        let u = self.lips[UPPER_MID];
        let l = self.lips[LOWER_MID];
        ((u.at.y - u.radius) - (l.at.y + l.radius)).max(0.0)
    }

    /// Distance between the two mouth corners.
    pub fn mouth_width(&self) -> f32 {
        self.lips[CORNER_L].at.distance(self.lips[CORNER_R].at)
    }
}
