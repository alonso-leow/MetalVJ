//! Reference postures: the poses a rig is checked against, not danced in.
//!
//! Every one of these is a claim about the body that can be measured and
//! therefore tested. The anatomical position is the sharpest: it fixes the
//! palms facing forward, which pins down a rotation the rest pose leaves
//! entirely free — and a rig whose hands are built on the wrong axis cannot
//! reach it without bending a wrist past its limit. That is how the 87°
//! deviation in the first canonical rig was found.
//!
//! They are also the only poses in the system with an answer known in advance.
//! A reconstruction of a dancer standing still can be compared against
//! [`Figure::Standing`]; nothing else here offers ground truth for free.

use std::f32::consts::FRAC_PI_2;

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::pose::Pose;
use crate::skeleton::{Side, Skeleton};

/// How far the arms hang from vertical when standing at ease, radians. Not
/// zero: a body with its arms welded to its sides reads as a mannequin, and
/// the gap is what the shoulder actually does.
const ARM_SPLAY: f32 = 0.12;

/// Hip adduction that brings the feet together for the anatomical position.
const FEET_TOGETHER: f32 = 0.05;

/// Half the depth of a torso at the hip, metres.
///
/// The rig is a mid-line skeleton with no volume, so a body lying on its back
/// has *nothing* at floor level — its spine floats one half-torso up and the
/// skin that is actually touching is not represented at all. This constant
/// stands in for that missing thickness so the supine figure sits at a
/// believable height. It is a patch over a real gap: see
/// `docs/CORRECTIONS.md` on joint radius and contact.
pub const TORSO_HALF_DEPTH: f32 = 0.10;

/// A named posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Figure {
    /// The rig as built: arms out sideways, palms down. A construction
    /// artefact, kept because binding and retargeting are defined against it.
    Rest,
    /// Standing at ease — arms hanging, palms toward the thighs, fingers
    /// relaxed. The default a movement departs from, because it is the posture
    /// a take starts and ends in.
    #[default]
    Standing,
    /// The anatomical position: upright, feet together, arms at the sides,
    /// **palms forward**, fingers extended. The reference every anatomy text
    /// describes position from.
    Anatomical,
    /// Lying on the back, face up, head upstage.
    Supine,
}

impl Figure {
    pub const ALL: [Figure; 4] = [
        Figure::Rest,
        Figure::Standing,
        Figure::Anatomical,
        Figure::Supine,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Figure::Rest => "rest",
            Figure::Standing => "standing",
            Figure::Anatomical => "anatomical",
            Figure::Supine => "supine",
        }
    }

    /// What the posture is for, in one line.
    pub fn purpose(self) -> &'static str {
        match self {
            Figure::Rest => "the rig as built; bind and retarget reference",
            Figure::Standing => "at ease; the neutral a take starts and ends in",
            Figure::Anatomical => "palms forward; fixes the axial rotation the rest pose leaves free",
            Figure::Supine => "on the floor; the case contact has to survive",
        }
    }

    /// Builds the posture on `skeleton`.
    pub fn pose(self, skeleton: &Skeleton) -> Pose {
        let mut p = Pose::rest(skeleton);
        match self {
            Figure::Rest => return p,
            Figure::Standing => {
                set(skeleton, &mut p, "l_shoulder", Quat::from_rotation_z(-(FRAC_PI_2 - ARM_SPLAY)));
                // A hanging forearm carries the hand slightly in front of the
                // body, never behind it.
                set(skeleton, &mut p, "l_elbow", Quat::from_rotation_y(-0.18));
                curl_hand(skeleton, &mut p, "l", 0.34, 0.40);
            }
            Figure::Anatomical => {
                // Two rotations, and the order is the whole point. Supinate
                // first about the arm's own axis so the palm leads, then swing
                // the arm down. Composed the other way the palm ends facing
                // the thigh, which is the standing posture, not this one.
                set(
                    skeleton,
                    &mut p,
                    "l_shoulder",
                    Quat::from_rotation_z(-FRAC_PI_2) * Quat::from_rotation_x(-FRAC_PI_2),
                );
                set(skeleton, &mut p, "l_hip", Quat::from_rotation_z(-FEET_TOGETHER));
            }
            Figure::Supine => {
                // Rotating the root lays the whole body down in one operation
                // — the payoff of storing a pose as local rotations under a
                // single root rather than as world points.
                set(skeleton, &mut p, "pelvis", Quat::from_rotation_x(-FRAC_PI_2));
                set(skeleton, &mut p, "l_shoulder", Quat::from_rotation_z(-(FRAC_PI_2 - 0.22)));
                curl_hand(skeleton, &mut p, "l", 0.24, 0.45);
            }
        }
        let mut p = symmetric(skeleton, p);
        match self {
            // The supine body rests on its back, which the rig does not have.
            Figure::Supine => p.root.y = TORSO_HALF_DEPTH,
            _ => settle(skeleton, &mut p, 0.0),
        }
        p
    }
}

/// Sets one joint's local rotation by name. A name absent from the rig is
/// skipped, so a figure stays buildable on a reduced skeleton.
fn set(skeleton: &Skeleton, pose: &mut Pose, joint: &str, q: Quat) {
    if let Some(id) = skeleton.find(joint) {
        pose.local[id as usize] = q;
    }
}

/// Gathers the fingers of one hand toward the palm.
///
/// `curl` is how far the finger group folds, in radians; `thumb` is the
/// fraction of that the thumb takes, because a thumb opposes rather than folds
/// and a relaxed hand does not curl it as far.
///
/// Two joints, where this used to set thirty. The gathering of a relaxed hand
/// is one number now, which is the point of the cut: the shape a record can
/// honestly claim is *how far gathered*, not which phalanx of which finger.
fn curl_hand(skeleton: &Skeleton, pose: &mut Pose, side: &str, curl: f32, thumb: f32) {
    // Curling folds the fingers toward the palm, which faces -Y in rest, so for
    // a group running along X it is a rotation about Z.
    set(
        skeleton,
        pose,
        &format!("{side}_fingers"),
        Quat::from_rotation_z(-curl),
    );
    // The thumb runs along its own axis — mostly +Z — and rotating *it* about Z
    // would spin the digit about its own length instead of folding it. It folds
    // about X, the axis perpendicular to both its own direction and the palm.
    set(
        skeleton,
        pose,
        &format!("{side}_thumb"),
        Quat::from_rotation_x(curl * thumb),
    );
}

/// Completes a pose declared on the left and centre by reflecting it onto the
/// right side.
///
/// Reuses [`Pose::mirrored`] rather than negating angles by hand: the mirror
/// conjugates every rotation, and a figure written twice by hand is a figure
/// that will eventually disagree with itself on one axis.
fn symmetric(skeleton: &Skeleton, mut pose: Pose) -> Pose {
    let reflected = pose.mirrored(&skeleton.mirror());
    for (i, joint) in skeleton.joints.iter().enumerate() {
        if joint.side == Side::Right {
            pose.local[i] = reflected.local[i];
        }
    }
    pose
}

/// Drops or lifts a pose until its lowest joint rests on `height`.
///
/// Any rotation at the hip or ankle changes how far the body reaches down, by
/// millimetres. Not correcting for it leaves a figure hovering or buried, and
/// the contact channel measures exactly that distance — so an uncorrected
/// figure poisons the one check that costs nothing.
pub fn settle(skeleton: &Skeleton, pose: &mut Pose, height: f32) {
    let lowest = pose
        .points(skeleton)
        .into_iter()
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min);
    if lowest.is_finite() {
        pose.root.y += height - lowest;
    }
}

/// World-space direction the palm faces, from three points on the hand.
///
/// The palm is a plane, and the outward normal of that plane is what "facing
/// forward" means — measurable, unlike the eye's opinion of a stick figure.
///
/// The plane used to be taken across the knuckles, from the index and little
/// metacarpal heads. Those joints are gone, and the **thumb** is what replaces
/// them: it is the one part of the hand far enough off the finger axis to give
/// the plane a second direction, which is the same argument that keeps the jaw
/// in the rig. Reading the wrist's own quaternion would be easier and would
/// stop this being a *measurement*.
pub fn palm_normal(skeleton: &Skeleton, pose: &Pose, side: &str) -> Option<Vec3> {
    let pts = pose.points(skeleton);
    let at = |n: &str| {
        skeleton
            .find(&format!("{side}_{n}"))
            .map(|id| pts[id as usize])
    };
    let (wrist, fingers, thumb) = (at("wrist")?, at("finger_tip")?, at("thumb_tip")?);
    let along = fingers - wrist;
    let across = thumb - wrist;
    let n = along.cross(across).normalize_or_zero();
    // Cross products have two answers; anchor on the side of the hand the palm
    // is on, which in rest is -Y for both hands.
    let sign = if side == "l" { 1.0 } else { -1.0 };
    Some(n * sign)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig;

    fn at(s: &Skeleton, p: &Pose, name: &str) -> Vec3 {
        p.points(s)[s.find(name).expect(name) as usize]
    }

    #[test]
    fn every_figure_keeps_the_body_it_was_given() {
        let s = rig::canonical();
        let rest = Pose::rest(&s);
        for f in Figure::ALL {
            let p = f.pose(&s);
            let (a, b) = (rest.points(&s), p.points(&s));
            for (i, joint) in s.joints.iter().enumerate() {
                let Some(parent) = joint.parent else { continue };
                let before = a[i].distance(a[parent as usize]);
                let after = b[i].distance(b[parent as usize]);
                assert!(
                    (before - after).abs() < 1e-5,
                    "{}: {} changed length in {}",
                    f.name(),
                    joint.name,
                    f.name()
                );
            }
        }
    }

    #[test]
    fn the_upright_figures_stand_on_the_floor() {
        let s = rig::canonical();
        for f in [Figure::Rest, Figure::Standing, Figure::Anatomical] {
            let p = f.pose(&s);
            let lowest = p.points(&s).into_iter().map(|v| v.y).fold(f32::INFINITY, f32::min);
            assert!(lowest.abs() < 1e-5, "{} floats at {lowest}", f.name());
            // And it is a foot doing the touching, not a fingertip.
            for side in ["l", "r"] {
                assert!(at(&s, &p, &format!("{side}_heel")).y.abs() < 0.02);
                assert!(at(&s, &p, &format!("{side}_toe")).y.abs() < 0.02);
            }
        }
    }

    #[test]
    fn standing_hangs_the_arms_with_the_palms_facing_the_thighs() {
        let s = rig::canonical();
        let p = Figure::Standing.pose(&s);
        for (side, inward) in [("l", -1.0f32), ("r", 1.0)] {
            let wrist = at(&s, &p, &format!("{side}_wrist"));
            let shoulder = at(&s, &p, &format!("{side}_shoulder"));
            assert!(wrist.y < shoulder.y - 0.4, "{side} arm is not hanging");
            // Palm toward the midline: inward is -X on the left, +X on the right.
            let n = palm_normal(&s, &p, side).unwrap();
            assert!(
                n.x * inward > 0.7,
                "{side} palm points {n:?}, expected toward the body"
            );
        }
    }

    #[test]
    fn the_anatomical_position_turns_both_palms_forward() {
        let s = rig::canonical();
        let p = Figure::Anatomical.pose(&s);
        for side in ["l", "r"] {
            let n = palm_normal(&s, &p, side).unwrap();
            assert!(
                n.z > 0.9,
                "{side} palm points {n:?}, not forward — this is the whole figure"
            );
            // Forward-facing palms with the arm still hanging, not raised.
            let (wrist, shoulder) = (
                at(&s, &p, &format!("{side}_wrist")),
                at(&s, &p, &format!("{side}_shoulder")),
            );
            assert!((wrist.y - (shoulder.y - 0.54)).abs() < 0.02);
            // Fingers extended, pointing down at the floor.
            let tip = at(&s, &p, &format!("{side}_finger_tip"));
            assert!(tip.y < wrist.y - 0.15, "{side} fingers are not extended");
        }
    }

    #[test]
    fn the_anatomical_position_brings_the_feet_together() {
        let s = rig::canonical();
        let (rest, anat) = (Figure::Rest.pose(&s), Figure::Anatomical.pose(&s));
        let gap = |p: &Pose| (at(&s, p, "l_ankle").x - at(&s, p, "r_ankle").x).abs();
        assert!(gap(&anat) < gap(&rest), "the feet did not come together");
        assert!(gap(&anat) > 0.02, "the ankles are overlapping, not together");
    }

    #[test]
    fn supine_lays_the_body_down_face_up() {
        let s = rig::canonical();
        let p = Figure::Supine.pose(&s);
        let (head, pelvis, chest) = (
            at(&s, &p, "head_top"),
            at(&s, &p, "pelvis"),
            at(&s, &p, "vert_t2"),
        );
        // Horizontal: the whole spine sits within a few centimetres of one height.
        assert!((head.y - pelvis.y).abs() < 0.05, "the body is not lying flat");
        // Long axis along Z, head upstage.
        assert!(head.z < pelvis.z - 0.6, "the body did not lie down along Z");
        // Face up: the chest's forward axis now points at the ceiling.
        let front = p.world(&s)[s.find("vert_t2").unwrap() as usize]
            .transform_vector3(Vec3::Z)
            .normalize();
        assert!(front.y > 0.9, "the body is prone, not supine: chest faces {front:?}");
        assert!(chest.y > 0.0, "the torso is underground");
    }

    /// The measurement that will be compared against a reconstruction. If the
    /// figure is not a plausible human height, nothing downstream can be.
    #[test]
    fn the_upright_figures_are_the_height_the_rig_claims() {
        let s = rig::canonical();
        for f in [Figure::Standing, Figure::Anatomical] {
            let h = at(&s, &f.pose(&s), "head_top").y;
            assert!((1.60..1.80).contains(&h), "{} is {h:.3} m tall", f.name());
        }
    }

    #[test]
    fn figures_are_left_right_symmetric() {
        let s = rig::canonical();
        let m = s.mirror();
        for f in Figure::ALL {
            let p = f.pose(&s);
            let (before, after) = (p.points(&s), p.mirrored(&m).points(&s));
            for j in 0..s.len() {
                let (want, got) = (before[j], after[m.partner(j as u16) as usize]);
                assert!(
                    (got.x + want.x).abs() < 1e-4
                        && (got.y - want.y).abs() < 1e-4
                        && (got.z - want.z).abs() < 1e-4,
                    "{} is asymmetric at {}",
                    f.name(),
                    s.joints[j].name
                );
            }
        }
    }
}
