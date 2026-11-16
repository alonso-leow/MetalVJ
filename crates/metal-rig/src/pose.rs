//! Poses and clips: a body as the rotation of each of its joints.
//!
//! A pose is stored as local joint rotations plus a single root translation —
//! not as world-space points. Bone lengths stay fixed by construction, so no
//! value anywhere can stretch a limb, and world coordinates are always derived
//! through [`Pose::world`].
//!
//! # Rotations, and nothing else (2026-08-15)
//!
//! For one day this crate declared **positions**. A given node said where it
//! went, and a mechanism — a chain, a FABRIK solve, a table of remembered
//! batches — worked out the rotations that would put it there. the operator, ending
//! it: *"Move the whole notation to rotations and not position
//! coordinates."*
//!
//! What that mechanism cost is worth writing down once, because it is the
//! reason this file is now a third of its length:
//!
//! - **The zero had to be invented, twice.** A declared position needs a frame
//!   to be a position *in*. First it was the rest pose, which made "lower the
//!   elbow" mean different things with the arm up and the arm down. Then it was
//!   the body's current place, which fixed that and broke reading a tick on its
//!   own — tick 40 no longer meant anything without the thirty-nine before it,
//!   and a [`Batches`]-style memory had to be replayed to know what *go back*
//!   returned to. **A local rotation has neither problem and needs no
//!   invention**: it is already measured against the parent's own frame, so it
//!   transports with the body for free, and it says the same thing on tick 40
//!   whether or not tick 39 was applied.
//! - **The solve was not unique.** An arm reaching a point has a circle of
//!   elbow placements, and FABRIK picked one. The corpus would have recorded a
//!   choice the solver made and the dancer never did.
//! - **Half the rig could not be declared at all.** A joint's rotation places
//!   its children, so under a rotation notation a fingertip says nothing — and
//!   that was the whole argument for positions. On a 36-joint rig it is not a
//!   loss worth a solver: the joints that move a hand are the wrist, the
//!   knuckles and the thumb, and every one of them has children.
//!
//! Rotations are written as **rotation vectors** — the axis a joint turns
//! about, scaled by how far it turns — so a joint is three numbers everywhere:
//! in a curve, in a tick, in a tensor row. See [`to_rotvec`].
//!
//! # No roles
//!
//! There is no *given*, no *carried*, no *silent* and no *reset*. Every node
//! either has a rotation written for it or does not, and a node that does not
//! keeps the one it had. That is a property of writing a sparse map, not a
//! state a node is *in*, and nothing in the pipeline branches on it.

use glam::{Affine3A, Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::skeleton::{JointId, Mirror, Skeleton};

/// A rotation, as the axis it turns about scaled by how far it turns.
///
/// Three numbers for a rotation, which is what makes a joint the same width
/// everywhere: a key in a curve, a group in a tick, three columns of a tensor
/// row. A quaternion would be four and would have to be renormalised at every
/// boundary; Euler angles would be three and would gimbal-lock a shoulder.
///
/// The identity is the zero vector, and angles beyond π wrap to their short way
/// round — so a shoulder that turns 370° and one that turns 10° read the same,
/// which for a body is the right answer.
pub fn to_rotvec(q: Quat) -> Vec3 {
    // The double cover: q and -q are one rotation, and only one of them is the
    // short way round. Left unhandled, a curve that crosses w = 0 emits a
    // rotation vector that jumps the long way and trains the model on a
    // discontinuity that is not in the body.
    let q = if q.w < 0.0 { -q } else { q };
    let (axis, angle) = q.normalize().to_axis_angle();
    axis * angle
}

/// Inverse of [`to_rotvec`].
pub fn from_rotvec(v: Vec3) -> Quat {
    let angle = v.length();
    if angle < 1e-9 {
        Quat::IDENTITY
    } else {
        Quat::from_axis_angle(v / angle, angle)
    }
}

/// A rotation reflected across the plane `x = 0`.
pub(crate) fn mirror_rotation(q: Quat) -> Quat {
    Quat::from_xyzw(q.x, -q.y, -q.z, q.w)
}

/// A rotation vector reflected across the plane `x = 0`.
///
/// Two sign flips, and that is the whole of what mirroring a declaration costs
/// now. Under a reflection `M = diag(-1, 1, 1)` a rotation `R` becomes `M R M`,
/// which on a quaternion is `(w, x, y, z) → (w, x, -y, -z)` and on the axis is
/// exactly the same thing with the angle untouched. A forward bend mirrors onto
/// itself; a turn and a side-tilt reverse.
pub const fn mirror_turn(v: Vec3) -> Vec3 {
    Vec3::new(v.x, -v.y, -v.z)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    /// World-space translation of the root joint, in metres.
    pub root: Vec3,
    /// Rotation of each joint relative to its parent's rest frame. Indexed by
    /// [`JointId`]; length always equals the skeleton's joint count.
    pub local: Vec<Quat>,
}

impl Pose {
    /// The rest pose: every joint at its rest transform.
    pub fn rest(skeleton: &Skeleton) -> Self {
        Self {
            root: skeleton.joints[0].rest_offset,
            local: vec![Quat::IDENTITY; skeleton.len()],
        }
    }

    /// Every joint's rest position, measured from the root.
    ///
    /// The rig's own geometry, and no longer a zero anything is measured
    /// against — a rotation's zero is the joint's rest *rotation*, which is the
    /// identity by construction and needs no table. What this is still for is
    /// drawing: a page is *sent* the rest frame rather than left to compute it,
    /// because two implementations of the rig's geometry disagreeing by a
    /// millimetre is a whole corpus authored a millimetre off.
    pub fn rest_frame(skeleton: &Skeleton) -> Vec<Vec3> {
        let rest = Self::rest(skeleton);
        let root = rest.root;
        rest.points(skeleton).into_iter().map(|p| p - root).collect()
    }

    /// Writes one joint's rotation down.
    ///
    /// The whole of what a declaration means, in one line. There is no solve,
    /// no chain and no order dependence: two joints declared in one tick land
    /// the same way whichever is applied first, because neither reads the
    /// other. That is the property the position notation spent
    /// `Pose::claimants` to approximate and never quite had.
    pub fn turn(&mut self, joint: JointId, by: Vec3) {
        self.local[joint as usize] = from_rotvec(by);
    }

    /// What one joint is currently turned by.
    pub fn turned(&self, joint: JointId) -> Vec3 {
        to_rotvec(self.local[joint as usize])
    }

    /// Applies a whole tick's or phrase's worth of declarations.
    ///
    /// Joints absent from `turns` are not touched — they keep the rotation they
    /// had, which is what a sparse record means and the only sense in which
    /// this notation still has silence in it. Forward kinematics carries them
    /// along if something above them turned.
    pub fn apply(&mut self, turns: &[(JointId, Vec3)]) {
        for &(joint, by) in turns {
            self.turn(joint, by);
        }
    }

    /// Resolves every joint to world space in one forward pass.
    ///
    /// Relies on the parent-before-child ordering guaranteed by
    /// [`Skeleton::validate`].
    pub fn world(&self, skeleton: &Skeleton) -> Vec<Affine3A> {
        let mut out = Vec::with_capacity(skeleton.len());
        self.world_into(skeleton, &mut out);
        out
    }

    /// [`Pose::world`] reusing a caller-owned buffer.
    ///
    /// Live capture runs this every frame per subject; allocating a fresh
    /// `Vec` each time is the kind of cost that only shows up as jitter.
    pub fn world_into(&self, skeleton: &Skeleton, out: &mut Vec<Affine3A>) {
        debug_assert_eq!(self.local.len(), skeleton.len());
        out.clear();
        out.reserve(skeleton.len());
        for (i, joint) in skeleton.joints.iter().enumerate() {
            let rotation = joint.rest_rotation * self.local[i];
            let global = match joint.parent {
                // The root's rest offset is replaced by the animated translation.
                None => Affine3A::from_rotation_translation(rotation, self.root),
                Some(p) => {
                    out[p as usize] * Affine3A::from_rotation_translation(rotation, joint.rest_offset)
                }
            };
            out.push(global);
        }
    }

    /// World-space joint positions, discarding orientation.
    pub fn points(&self, skeleton: &Skeleton) -> Vec<Vec3> {
        self.world(skeleton)
            .into_iter()
            .map(|m| m.translation.into())
            .collect()
    }

    /// Reflects the pose across the sagittal plane: left becomes right.
    ///
    /// Two things happen at once, and doing only the first is the classic way
    /// to get a mirror that is subtly wrong. Joints swap with their partners,
    /// *and* every rotation is conjugated by the reflection — see
    /// [`mirror_turn`]. Swapping the joints without conjugating gives a body
    /// whose limbs are on the correct sides and rotating the wrong way.
    ///
    /// Bone lengths are untouched, so a mirrored pose is exactly as valid as
    /// its original — that is the payoff of storing rotations rather than
    /// points.
    pub fn mirrored(&self, mirror: &Mirror) -> Self {
        let mut local = vec![Quat::IDENTITY; self.local.len()];
        for (i, q) in self.local.iter().enumerate() {
            local[mirror.partner(i as JointId) as usize] = mirror_rotation(*q);
        }
        Self {
            root: Vec3::new(-self.root.x, self.root.y, self.root.z),
            local,
        }
    }

    /// Shortest-arc interpolation between two poses. `t` is clamped to `0..=1`.
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            root: self.root.lerp(other.root, t),
            local: self
                .local
                .iter()
                .zip(&other.local)
                .map(|(a, b)| a.slerp(*b, t))
                .collect(),
        }
    }
}

/// A fixed-rate sequence of poses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub name: String,
    pub fps: f64,
    pub frames: Vec<Pose>,
}

impl Clip {
    pub fn duration(&self) -> f64 {
        if self.frames.is_empty() {
            0.0
        } else {
            (self.frames.len() - 1) as f64 / self.fps
        }
    }

    /// The whole clip reflected across the sagittal plane.
    pub fn mirrored(&self, mirror: &Mirror) -> Self {
        Self {
            name: format!("{}.mirrored", self.name),
            fps: self.fps,
            frames: self.frames.iter().map(|f| f.mirrored(mirror)).collect(),
        }
    }

    /// Samples at an arbitrary time in seconds, interpolating between frames.
    /// Times outside the clip clamp to the first or last frame.
    pub fn sample(&self, seconds: f64) -> Option<Pose> {
        if self.frames.is_empty() {
            return None;
        }
        let x = (seconds * self.fps).clamp(0.0, (self.frames.len() - 1) as f64);
        let i = x.floor() as usize;
        let Some(next) = self.frames.get(i + 1) else {
            return self.frames.last().cloned();
        };
        Some(self.frames[i].lerp(next, (x - i as f64) as f32))
    }
}

/// Rotation of a single joint over time — the form the timeline editor and the
/// 2D-tracking importer both work in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub joint: JointId,
    pub keys: Vec<(f64, Quat)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig;

    #[test]
    fn rest_pose_stands_on_the_ground() {
        let s = rig::canonical();
        let p = Pose::rest(&s);
        let pts = p.points(&s);
        // The floor contacts sit exactly on the floor, so a contact check has
        // no built-in offset to absorb. See `rig::canonical`.
        for side in ["l", "r"] {
            for part in rig::SOLE {
                let name = format!("{side}_{part}");
                let y = pts[s.find(&name).unwrap() as usize].y;
                assert!(y.abs() < 1e-6, "{name} rests at y = {y}, not on the floor");
            }
        }
    }

    #[test]
    fn fk_preserves_bone_lengths() {
        let s = rig::canonical();
        let mut p = Pose::rest(&s);
        let elbow = s.find("l_elbow").unwrap();
        p.local[elbow as usize] = Quat::from_rotation_y(1.1);
        let pts = p.points(&s);
        let wrist = s.find("l_wrist").unwrap();
        let measured = pts[wrist as usize].distance(pts[elbow as usize]);
        assert!((measured - s.joints[wrist as usize].length()).abs() < 1e-5);
    }

    #[test]
    fn rotating_a_shoulder_moves_the_fingertips() {
        let s = rig::canonical();
        let rest = Pose::rest(&s);
        let mut raised = rest.clone();
        raised.local[s.find("l_shoulder").unwrap() as usize] = Quat::from_rotation_z(0.9);
        let tip = s.find("l_finger_tip").unwrap() as usize;
        assert!(raised.points(&s)[tip].distance(rest.points(&s)[tip]) > 0.1);
    }

    // ------------------------------------------------------- rotation vectors

    #[test]
    fn the_identity_rotation_is_three_zeros() {
        assert_eq!(to_rotvec(Quat::IDENTITY), Vec3::ZERO);
        assert!(from_rotvec(Vec3::ZERO).abs_diff_eq(Quat::IDENTITY, 1e-9));
    }

    #[test]
    fn a_rotation_survives_the_round_trip_through_three_numbers() {
        for q in [
            Quat::from_rotation_z(0.7),
            Quat::from_rotation_x(-1.4),
            Quat::from_euler(glam::EulerRot::XYZ, 0.3, -0.9, 1.1),
        ] {
            let back = from_rotvec(to_rotvec(q));
            assert!(back.abs_diff_eq(q, 1e-5) || back.abs_diff_eq(-q, 1e-5), "{q:?}");
        }
    }

    /// The double cover, which is the one way this encoding goes quietly wrong:
    /// q and -q are the same body and opposite vectors, and a curve crossing
    /// w = 0 would emit a jump the dancer never made.
    #[test]
    fn the_long_way_round_is_never_written() {
        let q = Quat::from_rotation_z(0.4);
        assert!(to_rotvec(q).abs_diff_eq(to_rotvec(-q), 1e-6));
        assert!(to_rotvec(q).length() < std::f32::consts::PI);
    }

    /// A declaration is order-free, which is what the position notation needed
    /// a whole claimant pass to approximate: two joints written in one tick
    /// land the same way whichever is applied first, because neither reads the
    /// other.
    #[test]
    fn two_declarations_in_one_tick_do_not_depend_on_their_order() {
        let s = rig::canonical();
        let (a, b) = (s.find("l_elbow").unwrap(), s.find("vert_t2").unwrap());
        let turns = [(a, Vec3::new(0.0, 0.8, 0.0)), (b, Vec3::new(0.3, 0.0, 0.2))];

        let mut forward = Pose::rest(&s);
        forward.apply(&turns);
        let mut backward = Pose::rest(&s);
        backward.apply(&[turns[1], turns[0]]);
        assert_eq!(forward, backward);
    }

    /// A tick reads on its own. Under the position notation this was false and
    /// documented as the price of a transportable zero; a local rotation is
    /// already relative to the parent's frame, so the price was never owed.
    #[test]
    fn a_declaration_means_the_same_thing_whatever_came_before_it() {
        let s = rig::canonical();
        let elbow = s.find("l_elbow").unwrap();
        let bend = Vec3::new(0.0, 1.1, 0.0);

        let mut fresh = Pose::rest(&s);
        fresh.turn(elbow, bend);

        let mut used = Pose::rest(&s);
        used.turn(elbow, Vec3::new(0.0, -0.4, 0.3));
        used.turn(s.find("l_shoulder").unwrap(), Vec3::new(0.0, 0.0, 0.5));
        used.turn(elbow, bend);

        assert!(used.local[elbow as usize].abs_diff_eq(fresh.local[elbow as usize], 1e-6));
    }

    /// Silence is not a role, it is what a sparse map does: a joint nobody
    /// wrote keeps what it had, and forward kinematics still carries it.
    #[test]
    fn a_joint_nobody_wrote_keeps_what_it_had_and_still_rides_its_parent() {
        let s = rig::canonical();
        let (shoulder, elbow) = (s.find("l_shoulder").unwrap(), s.find("l_elbow").unwrap());
        let mut pose = Pose::rest(&s);
        pose.turn(elbow, Vec3::new(0.0, 1.0, 0.0));
        let before = pose.clone();

        pose.apply(&[(shoulder, Vec3::new(0.0, 0.0, 0.7))]);
        assert_eq!(pose.local[elbow as usize], before.local[elbow as usize], "the elbow was rewritten");
        let tip = s.find("l_finger_tip").unwrap() as usize;
        assert!(
            pose.points(&s)[tip].distance(before.points(&s)[tip]) > 0.1,
            "the hand did not ride the shoulder"
        );
    }

    #[test]
    fn sampling_between_frames_interpolates_the_root() {
        let s = rig::canonical();
        let a = Pose::rest(&s);
        let mut b = a.clone();
        b.root.x += 1.0;
        let clip = Clip {
            name: "slide".into(),
            fps: 2.0,
            frames: vec![a.clone(), b],
        };
        assert_eq!(clip.duration(), 0.5);
        let mid = clip.sample(0.25).unwrap();
        assert!((mid.root.x - (a.root.x + 0.5)).abs() < 1e-6);
    }

    #[test]
    fn mirroring_reflects_every_joint_across_the_sagittal_plane() {
        let s = rig::canonical();
        let m = s.mirror();
        assert!(m.is_exact(), "canonical rig must pair completely");

        // A pose with rotation on all three axes, so a mirror that only swaps
        // joints without conjugating cannot accidentally pass.
        let mut p = Pose::rest(&s);
        p.local[s.find("l_shoulder").unwrap() as usize] = Quat::from_rotation_z(0.9);
        p.local[s.find("r_elbow").unwrap() as usize] = Quat::from_rotation_y(0.7);
        p.local[s.find("vert_l3").unwrap() as usize] = Quat::from_rotation_x(0.3);
        p.local[s.find("l_fingers").unwrap() as usize] = Quat::from_rotation_y(0.5);
        p.root.x = 0.4;

        let (before, after) = (p.points(&s), p.mirrored(&m).points(&s));
        for j in 0..s.len() {
            let want = before[j];
            let got = after[m.partner(j as JointId) as usize];
            assert!(
                (got.x + want.x).abs() < 1e-5
                    && (got.y - want.y).abs() < 1e-5
                    && (got.z - want.z).abs() < 1e-5,
                "{} landed at {got:?}, expected the reflection of {want:?}",
                s.joints[j].name,
            );
        }
    }

    /// The mirror on a rotation vector is two sign flips, and it has to agree
    /// with the quaternion conjugation the pose uses — this is the seam where a
    /// notation-level mirror and a geometry-level mirror could drift apart.
    #[test]
    fn mirroring_a_rotation_vector_agrees_with_conjugating_its_quaternion() {
        for v in [
            Vec3::new(0.4, 0.0, 0.0),
            Vec3::new(0.0, 0.7, 0.0),
            Vec3::new(0.2, -0.5, 0.9),
        ] {
            let through_quat = to_rotvec(mirror_rotation(from_rotvec(v)));
            assert!(
                mirror_turn(v).abs_diff_eq(through_quat, 1e-6),
                "{v:?} mirrors to {:?} one way and {through_quat:?} the other",
                mirror_turn(v)
            );
        }
    }

    #[test]
    fn mirroring_twice_is_the_identity() {
        let s = rig::canonical();
        let m = s.mirror();
        let mut p = Pose::rest(&s);
        p.local[s.find("r_wrist").unwrap() as usize] = Quat::from_rotation_y(1.2);
        p.root.x = -0.3;

        let back = p.mirrored(&m).mirrored(&m);
        assert!((back.root - p.root).length() < 1e-6);
        for (a, b) in back.local.iter().zip(&p.local) {
            assert!(a.abs_diff_eq(*b, 1e-6), "{a:?} != {b:?}");
        }
    }

    #[test]
    fn a_sagittal_movement_mirrors_onto_itself() {
        // Bending forward is the same movement in a mirror; turning is not.
        let s = rig::canonical();
        let m = s.mirror();
        let spine = s.find("vert_l3").unwrap() as usize;

        let mut bend = Pose::rest(&s);
        bend.local[spine] = Quat::from_rotation_x(0.4);
        assert!(bend.mirrored(&m).local[spine].abs_diff_eq(bend.local[spine], 1e-6));

        let mut turn = Pose::rest(&s);
        turn.local[spine] = Quat::from_rotation_y(0.4);
        assert!(turn.mirrored(&m).local[spine].abs_diff_eq(Quat::from_rotation_y(-0.4), 1e-6));
    }

    #[test]
    fn a_mirrored_clip_keeps_its_bone_lengths() {
        let s = rig::canonical();
        let m = s.mirror();
        let mut p = Pose::rest(&s);
        p.local[s.find("l_elbow").unwrap() as usize] = Quat::from_rotation_y(1.1);
        let clip = Clip {
            name: "reach".into(),
            fps: 30.0,
            frames: vec![Pose::rest(&s), p],
        };

        let mirrored = clip.mirrored(&m);
        assert_eq!(mirrored.name, "reach.mirrored");
        let pts = mirrored.frames[1].points(&s);
        let (wrist, elbow) = (
            s.find("r_wrist").unwrap() as usize,
            s.find("r_elbow").unwrap() as usize,
        );
        let measured = pts[wrist].distance(pts[elbow]);
        assert!((measured - s.joints[wrist].length()).abs() < 1e-5);
    }

    /// Bone length is structural and nothing can spend it. Under the position
    /// notation this needed a test per chain length and a reachability case;
    /// under rotations it is true by construction, and the test is here to say
    /// that out loud rather than because it could fail.
    #[test]
    fn no_rotation_anywhere_can_stretch_a_bone() {
        let s = rig::canonical();
        let mut pose = Pose::rest(&s);
        for (i, v) in [
            Vec3::new(1.4, -0.9, 0.3),
            Vec3::new(0.0, 3.1, 0.0),
            Vec3::new(-2.0, 0.5, 2.0),
        ]
        .into_iter()
        .enumerate()
        {
            for j in (i..s.len()).step_by(3) {
                pose.turn(j as JointId, v);
            }
        }
        let pts = pose.points(&s);
        for (i, joint) in s.joints.iter().enumerate() {
            let Some(p) = joint.parent else { continue };
            let got = pts[i].distance(pts[p as usize]);
            assert!(
                (got - joint.length()).abs() < 1e-4,
                "{} is {got:.5} m from its parent, not {:.5}",
                joint.name,
                joint.length()
            );
        }
    }

    #[test]
    fn sampling_past_the_end_clamps() {
        let s = rig::canonical();
        let clip = Clip {
            name: "still".into(),
            fps: 30.0,
            frames: vec![Pose::rest(&s), Pose::rest(&s)],
        };
        assert!(clip.sample(99.0).is_some());
    }
}
