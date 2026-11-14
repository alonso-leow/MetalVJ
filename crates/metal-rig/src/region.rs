//! Named groups of joints.
//!
//! Moved into `metal-rig` on 2026-08-13 when `koreo-effort` was deleted.
//! It lived there because an effort layer bound to a region; nothing binds
//! to a region anymore, and it survived the deletion because 140 nodes were
//! too many to pick from a flat list. At 36 that argument is spent — the
//! surviving one is that this is the node tree in `STUDIO.md`, and it is
//! topology rather than notation: which joints are the left hand is a fact
//! about the rig, not a claim about the movement.
//!
//! The named regions are disjoint by construction — `LeftArm` stops at the
//! wrist and `LeftHand` starts after it — so nothing is ever ambiguous about
//! which group owns a joint. `WholeBody` deliberately overlaps everything.

use crate::skeleton::{JointId, Skeleton};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyRegion {
    /// Everything. Not disjoint from the rest, deliberately.
    #[default]
    WholeBody,
    Head,
    Torso,
    LeftArm,
    RightArm,
    LeftHand,
    RightHand,
    LeftLeg,
    RightLeg,
    /// An arbitrary set of joint names, for groups the rig does not name.
    Custom { joints: Vec<String> },
}

impl BodyRegion {
    /// Resolves to concrete joints. Names absent from `skeleton` are skipped,
    /// so a region stays usable on a reduced or retargeted rig.
    pub fn joints(&self, skeleton: &Skeleton) -> Vec<JointId> {
        match self {
            BodyRegion::WholeBody => (0..skeleton.len() as JointId).collect(),
            // The neck moves with the head, so the region starts at the
            // cervical hinge and takes everything above it: the neck and the
            // skull.
            BodyRegion::Head => subtree(skeleton, "vert_c4"),
            // Everything the limbs and the head hang off, defined by what it
            // is not. A hand-written list of the column would be a list that
            // silently disagrees with the rig the first time a hinge is added
            // or removed; this cannot, and the partition test below is what
            // proves the subtraction is exact.
            BodyRegion::Torso => {
                let mut ids = subtree(skeleton, "pelvis");
                for limb in ["vert_c4", "l_clavicle", "r_clavicle", "l_hip", "r_hip"] {
                    let branch = subtree(skeleton, limb);
                    ids.retain(|j| !branch.contains(j));
                }
                ids
            }
            BodyRegion::LeftArm => named(skeleton, &["l_clavicle", "l_shoulder", "l_elbow", "l_wrist"]),
            BodyRegion::RightArm => named(skeleton, &["r_clavicle", "r_shoulder", "r_elbow", "r_wrist"]),
            // The wrist belongs to the arm, so the hand is everything below it.
            BodyRegion::LeftHand => below(skeleton, "l_wrist"),
            BodyRegion::RightHand => below(skeleton, "r_wrist"),
            BodyRegion::LeftLeg => subtree(skeleton, "l_hip"),
            BodyRegion::RightLeg => subtree(skeleton, "r_hip"),
            BodyRegion::Custom { joints } => {
                let names: Vec<&str> = joints.iter().map(String::as_str).collect();
                named(skeleton, &names)
            }
        }
    }

    /// The same region on the reflected side of the body.
    ///
    /// Centre regions map to themselves. A custom set is mirrored name by
    /// name, so `Custom { joints: ["l_wrist"] }` becomes `["r_wrist"]` and a
    /// centre joint in the set stays put.
    pub fn mirrored(&self) -> BodyRegion {
        match self {
            BodyRegion::LeftArm => BodyRegion::RightArm,
            BodyRegion::RightArm => BodyRegion::LeftArm,
            BodyRegion::LeftHand => BodyRegion::RightHand,
            BodyRegion::RightHand => BodyRegion::LeftHand,
            BodyRegion::LeftLeg => BodyRegion::RightLeg,
            BodyRegion::RightLeg => BodyRegion::LeftLeg,
            BodyRegion::WholeBody => BodyRegion::WholeBody,
            BodyRegion::Head => BodyRegion::Head,
            BodyRegion::Torso => BodyRegion::Torso,
            BodyRegion::Custom { joints } => BodyRegion::Custom {
                joints: joints
                    .iter()
                    .map(|n| crate::mirrored_name(n).unwrap_or_else(|| n.clone()))
                    .collect(),
            },
        }
    }

    pub fn name(&self) -> &str {
        match self {
            BodyRegion::WholeBody => "whole_body",
            BodyRegion::Head => "head",
            BodyRegion::Torso => "torso",
            BodyRegion::LeftArm => "left_arm",
            BodyRegion::RightArm => "right_arm",
            BodyRegion::LeftHand => "left_hand",
            BodyRegion::RightHand => "right_hand",
            BodyRegion::LeftLeg => "left_leg",
            BodyRegion::RightLeg => "right_leg",
            BodyRegion::Custom { .. } => "custom",
        }
    }
}

fn named(skeleton: &Skeleton, names: &[&str]) -> Vec<JointId> {
    names.iter().filter_map(|n| skeleton.find(n)).collect()
}

fn subtree(skeleton: &Skeleton, root: &str) -> Vec<JointId> {
    skeleton
        .find(root)
        .map(|id| skeleton.subtree(id))
        .unwrap_or_default()
}

fn below(skeleton: &Skeleton, root: &str) -> Vec<JointId> {
    let Some(id) = skeleton.find(root) else {
        return Vec::new();
    };
    skeleton.subtree(id).into_iter().filter(|&j| j != id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig;

    #[test]
    fn hands_carry_every_digit_and_no_arm_bone() {
        let s = rig::canonical();
        let hand = BodyRegion::LeftHand.joints(&s);
        // The finger group and its tip, the thumb and its tip.
        assert_eq!(hand.len(), 4);
        assert!(!hand.contains(&s.find("l_wrist").unwrap()));
        assert!(hand.contains(&s.find("l_thumb_tip").unwrap()));
    }

    #[test]
    fn legs_carry_the_whole_foot() {
        let s = rig::canonical();
        let leg = BodyRegion::LeftLeg.joints(&s);
        for part in ["l_knee", "l_ankle", "l_heel", "l_ball", "l_toe"] {
            assert!(leg.contains(&s.find(part).unwrap()), "{part} is not in the leg");
        }
        // hip, knee, ankle, then heel, ball, toe.
        assert_eq!(leg.len(), 3 + 3);
    }

    /// The column is a region member by construction, not by a list somebody
    /// has to remember to extend. That property is what let the rig go from
    /// twenty-four vertebrae to four without this file being touched.
    #[test]
    fn every_hinge_lands_in_a_region_without_being_named() {
        let s = rig::canonical();
        let torso = BodyRegion::Torso.joints(&s);
        let head = BodyRegion::Head.joints(&s);
        for hinge in ["vert_l3", "vert_t10", "vert_t2"] {
            assert!(torso.contains(&s.find(hinge).unwrap()), "{hinge} is not in the torso");
        }
        assert!(head.contains(&s.find("vert_c4").unwrap()), "the neck is not in the head");
        // pelvis and three hinges.
        assert_eq!(torso.len(), 4);
        // The neck, the skull, its top, and the jaw.
        assert_eq!(head.len(), 4);
    }

    #[test]
    fn arm_and_hand_are_disjoint_and_together_cover_the_limb() {
        let s = rig::canonical();
        let arm = BodyRegion::LeftArm.joints(&s);
        let hand = BodyRegion::LeftHand.joints(&s);
        assert!(arm.iter().all(|j| !hand.contains(j)));
        let whole = s.subtree(s.find("l_clavicle").unwrap());
        assert_eq!(arm.len() + hand.len(), whole.len());
    }

    #[test]
    fn left_and_right_never_overlap() {
        let s = rig::canonical();
        for (l, r) in [
            (BodyRegion::LeftArm, BodyRegion::RightArm),
            (BodyRegion::LeftHand, BodyRegion::RightHand),
            (BodyRegion::LeftLeg, BodyRegion::RightLeg),
        ] {
            let (a, b) = (l.joints(&s), r.joints(&s));
            assert!(a.iter().all(|j| !b.contains(j)), "{} overlaps {}", l.name(), r.name());
            assert_eq!(a.len(), b.len());
        }
    }

    #[test]
    fn the_named_regions_partition_the_whole_body() {
        let s = rig::canonical();
        let mut covered: Vec<JointId> = [
            BodyRegion::Head,
            BodyRegion::Torso,
            BodyRegion::LeftArm,
            BodyRegion::RightArm,
            BodyRegion::LeftHand,
            BodyRegion::RightHand,
            BodyRegion::LeftLeg,
            BodyRegion::RightLeg,
        ]
        .iter()
        .flat_map(|r| r.joints(&s))
        .collect();
        let before = covered.len();
        covered.sort();
        covered.dedup();
        assert_eq!(before, covered.len(), "regions must not overlap");
        assert_eq!(covered.len(), s.len(), "regions must leave no joint uncovered");
    }

    #[test]
    fn mirroring_a_region_swaps_its_side_and_is_reciprocal() {
        for (l, r) in [
            (BodyRegion::LeftArm, BodyRegion::RightArm),
            (BodyRegion::LeftHand, BodyRegion::RightHand),
            (BodyRegion::LeftLeg, BodyRegion::RightLeg),
        ] {
            assert_eq!(l.mirrored(), r);
            assert_eq!(r.mirrored(), l);
            assert_eq!(l.mirrored().mirrored(), l);
        }
        for c in [BodyRegion::WholeBody, BodyRegion::Head, BodyRegion::Torso] {
            assert_eq!(c.mirrored(), c);
        }
    }

    #[test]
    fn mirroring_a_custom_region_swaps_only_its_lateral_joints() {
        let s = rig::canonical();
        let r = BodyRegion::Custom {
            joints: vec!["l_wrist".into(), "vert_t2".into(), "r_knee".into()],
        };
        let m = r.mirrored();
        let BodyRegion::Custom { joints } = &m else {
            panic!("a mirrored custom region must stay custom");
        };
        assert_eq!(joints, &["r_wrist", "vert_t2", "l_knee"]);
        // And it still resolves against the rig.
        assert_eq!(m.joints(&s).len(), 3);
    }

    #[test]
    fn a_missing_joint_is_skipped_rather_than_panicking() {
        let s = rig::canonical();
        let r = BodyRegion::Custom {
            joints: vec!["vert_t2".into(), "tail".into()],
        };
        assert_eq!(r.joints(&s).len(), 1);
    }
}
