//! What the notation calls a node, as opposed to where the array keeps it.
//!
//! There are two numbers for every part of the body and they are not the same
//! number. [`JointId`] is an **index**: where the joint sits in
//! [`Skeleton::joints`], which is an implementation detail of the array and
//! has to run 0..n with the parent before the child. [`NodeId`] is a **name**:
//! what a tick of the notation writes down, what the app labels a node with,
//! and what a human reads back a year later.
//!
//! The name carries meaning the index cannot:
//!
//! ```text
//!   negative   on the axis — pelvis, the four spine hinges, the skull, the jaw
//!   even       the dancer's right
//!   odd        the dancer's left
//!   zero       not a node
//!   magnitude  height off the floor, counting up
//! ```
//!
//! Five things fall out of that, and each of them was previously either a
//! table lookup or a bug.
//!
//! **`[0]` stops colliding with the pelvis.** The empty tick has always been
//! written `[0]` and the pelvis has always been joint `0`, so `[0, 0]` said
//! something about the pelvis and `[0]` said *this tick says nothing* — two
//! unrelated statements one character apart, in a notation whose whole point
//! is that saying nothing is a decision. Zero is now not a node, so the empty
//! tick is the only thing that can be spelled with it.
//!
//! **The mirror is arithmetic.** Reflecting a body across the sagittal plane
//! used to need [`Skeleton::mirror`] — a per-rig table built by pairing
//! `l_`/`r_` names. On these ids it is [`mirror`]: flip the low bit of `n − 1`
//! and leave the negatives alone. A track can be mirrored without loading the
//! rig it was baked against, which is what makes mirroring cheap enough to be
//! a corpus operation rather than a preprocessing step.
//!
//! **Laterality is readable without a lookup.** `-14` is on the axis and `37`
//! is on the left, from the number alone, in a diff, in a log line, in a tick
//! printed by a model that has no skeleton loaded.
//!
//! **A left node and its right partner are adjacent**, so the pair sorts
//! together and reads together, where the array puts an entire arm and hand
//! between them.
//!
//! **The magnitude is a height** (changed 2026-08-14). Numbering used to
//! follow the order the rig happened to be *built* in — arms before legs,
//! because that is how `rig::canonical` types them out — which put the
//! clavicles at 1 and 2 and the toes, the part of the body actually touching
//! the floor, somewhere in the nineties. Nothing read off that number. Now
//! both halves of the numbering climb from the ground: `-1` is the pelvis and
//! the axis counts up the column to the jaw at `-8`, and `1`/`2` are the toes
//! with the laterals counting up through the ankles and knees to the hands and
//! the clavicles at `27`/`28`. A node id is now a rough answer to *how far up
//! the body is this*, in a log line, with no rig loaded.
//!
//! The cost, stated plainly: one indirection. Anything that indexes the
//! skeleton needs [`Nodes::joint`] first, and anything that writes the
//! notation needs [`Nodes::id`]. The table is built once per rig and both
//! directions are O(1).

use std::collections::HashMap;

use crate::pose::Pose;
use crate::skeleton::{JointId, Side, Skeleton};

/// A node as the notation names it. Signed, because the sign is the laterality
/// and zero is not a node.
///
/// `i16` for the same reason [`JointId`] is `u16`: the canonical rig reaches
/// ±114 and a hand full of extra joints must not silently wrap.
pub type NodeId = i16;

/// The reflection of a node across the sagittal plane.
///
/// Axial nodes reflect onto themselves — a vertebra has no other side to be
/// on. A lateral node swaps parity within its own pair, which is the whole
/// reason left is odd and right is even rather than, say, low and high.
///
/// This agrees with [`Skeleton::mirror`] on the canonical rig by test, and it
/// is the same answer without the rig.
pub const fn mirror(node: NodeId) -> NodeId {
    if node <= 0 {
        node
    } else {
        // Pair k occupies 2k−1 (left) and 2k (right), so subtracting one puts
        // the pair on an even/odd boundary and the low bit is the side.
        ((node - 1) ^ 1) + 1
    }
}

/// Which side of the body a node is on, from the number alone.
pub const fn side(node: NodeId) -> Side {
    if node <= 0 {
        Side::Center
    } else if node % 2 == 0 {
        Side::Right
    } else {
        Side::Left
    }
}

/// True for the column and the skull: the nodes that have no partner.
pub const fn is_axial(node: NodeId) -> bool {
    node < 0
}

/// The two numberings of one rig, in both directions.
///
/// Built once and kept. Rebuilding it per tick would put a hash map allocation
/// inside the resolver, which is the one loop in this crate that runs per
/// frame per node.
#[derive(Debug, Clone)]
pub struct Nodes {
    /// Indexed by [`JointId`]; never contains zero.
    to_node: Vec<NodeId>,
    from_node: HashMap<NodeId, JointId>,
}

/// Rest-pose height is compared on a tenth-of-a-millimetre grid.
///
/// Every joint of a foot below the ankle sits on the floor by construction —
/// heel, ball and toe — so a raw `f32` compare would order them on whatever
/// rounding the offsets happened to accumulate. Quantizing first makes the tie
/// a real tie, resolved by the rule below rather than by the last bit of a
/// float.
const HEIGHT_GRID: f32 = 1e-4;

impl Nodes {
    /// Numbers a rig **by height off the floor**, both halves counting up.
    ///
    /// The axis runs downward from −1 in skeleton order, which is already the
    /// order it stands in: `-1` is the pelvis, the lowest node on the column,
    /// and the magnitude climbs as the column climbs to the jaw at −8.
    ///
    /// Lateral nodes are ordered the same way and paired as they go, so pair
    /// *k* is `2k−1` on the left and `2k` on the right. Three keys, in order:
    ///
    /// 1. **Rest-pose height**, lowest first, on a 0.1 mm grid.
    /// 2. **Depth in the chain**, deepest first, which is what settles the
    ///    tie the foot creates. Three joints of each foot rest at exactly
    ///    `y = 0`, so "the lowest point of each side" does not name one node
    ///    on its own; among equally low nodes the most peripheral is the one
    ///    a numbering that climbs *inward from the ground* should start at.
    ///    That puts the toes at `1` and `2` and the heel above them.
    /// 3. **Joint index**, so the result is a function of the rig and not of
    ///    the sort's stability.
    ///
    /// The numbering therefore depends on the rest pose, which is a rig
    /// property computed once here and never at runtime. A rig whose rest pose
    /// changed shape would renumber, which is the same class of event as
    /// adding a joint: baked artifacts are regenerated, not migrated
    /// (`Track::rig`).
    ///
    /// A lateral joint whose mirror image is missing from the rig still gets a
    /// number of the right parity — the numbering stays total on an asymmetric
    /// rig, and its partner slot is simply unoccupied. That is the same
    /// tolerance [`Skeleton::mirror`] has, and [`Nodes::unpaired`] reports it
    /// for the same reason: a half-mirrored body must not pass as a whole one.
    pub fn of(skeleton: &Skeleton) -> Self {
        let mirror_map = skeleton.mirror();
        let mut to_node = vec![0 as NodeId; skeleton.len()];
        let mut axis: NodeId = 0;
        let mut pair: NodeId = 0;

        for (i, joint) in skeleton.joints.iter().enumerate() {
            if joint.side == Side::Center {
                axis += 1;
                to_node[i] = -axis;
            }
        }

        for i in Self::climbing(skeleton) {
            if to_node[i as usize] != 0 {
                continue; // already numbered as some earlier node's partner
            }
            pair += 1;
            let (left, right) = (2 * pair - 1, 2 * pair);
            let (mine, theirs) = match skeleton.joints[i as usize].side {
                Side::Left => (left, right),
                _ => (right, left),
            };
            to_node[i as usize] = mine;
            let partner = mirror_map.partner(i);
            if partner != i {
                to_node[partner as usize] = theirs;
            }
        }

        let from_node = to_node
            .iter()
            .enumerate()
            .map(|(i, &n)| (n, i as JointId))
            .collect();
        Nodes {
            to_node,
            from_node,
        }
    }

    /// Every lateral joint, lowest first: the order the numbering walks. See
    /// [`Nodes::of`] for what the three keys are and why the second one is
    /// there.
    fn climbing(skeleton: &Skeleton) -> Vec<JointId> {
        let height = Pose::rest(skeleton).points(skeleton);
        let mut lateral: Vec<JointId> = (0..skeleton.len() as JointId)
            .filter(|&i| skeleton.joints[i as usize].side != Side::Center)
            .collect();
        lateral.sort_by_key(|&i| {
            let y = (height[i as usize].y / HEIGHT_GRID).round() as i32;
            let depth = skeleton.ancestors(i).count() as i32;
            (y, -depth, i)
        });
        lateral
    }

    /// What the notation calls this joint.
    ///
    /// Panics on a joint that is not in the rig this table was built from,
    /// which is a programming error rather than a data error — the joint came
    /// from a different skeleton.
    pub fn id(&self, joint: JointId) -> NodeId {
        self.to_node[joint as usize]
    }

    /// Where this node lives in the array, or `None` if the rig has no such
    /// node. A track baked against a longer rig reaches this path, and it must
    /// be a miss rather than a panic — [`crate::frame::Track::rig`] exists to
    /// catch that case loudly, one level up.
    pub fn joint(&self, node: NodeId) -> Option<JointId> {
        self.from_node.get(&node).copied()
    }

    /// Every node id, in skeleton order.
    pub fn ids(&self) -> &[NodeId] {
        &self.to_node
    }

    pub fn len(&self) -> usize {
        self.to_node.len()
    }

    pub fn is_empty(&self) -> bool {
        self.to_node.is_empty()
    }

    /// Lateral nodes whose partner number is not occupied by any joint.
    pub fn unpaired(&self) -> Vec<NodeId> {
        self.to_node
            .iter()
            .copied()
            .filter(|&n| n > 0 && !self.from_node.contains_key(&mirror(n)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::Pose;
    use crate::rig;

    #[test]
    fn zero_is_not_a_node() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        assert!(nodes.ids().iter().all(|&n| n != 0));
        assert!(nodes.joint(0).is_none());
    }

    #[test]
    fn the_column_is_negative_and_the_limbs_are_not() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        for (i, joint) in s.joints.iter().enumerate() {
            let n = nodes.id(i as JointId);
            match joint.side {
                Side::Center => assert!(n < 0, "{} is on the axis but numbered {n}", joint.name),
                Side::Left => assert!(n > 0 && n % 2 == 1, "{} is left but numbered {n}", joint.name),
                Side::Right => assert!(n > 0 && n % 2 == 0, "{} is right but numbered {n}", joint.name),
            }
        }
    }

    #[test]
    fn the_pelvis_is_minus_one_and_the_number_climbs_with_the_column() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        let at = |name: &str| nodes.id(s.find(name).unwrap());
        assert_eq!(at("pelvis"), -1);
        assert_eq!(at("vert_l3"), -2);
        assert!(at("vert_c4") < at("vert_t2"), "the neck is further up than the chest");
        assert!(at("jaw") < at("vert_c4"), "the skull is past the top of the column");
        // Contiguous: 8 axial nodes, so -1..=-8 with nothing missing.
        let mut axial: Vec<NodeId> = nodes.ids().iter().copied().filter(|&n| n < 0).collect();
        axial.sort_unstable();
        assert_eq!(axial, (1..=8).rev().map(|n| -n).collect::<Vec<_>>());
    }

    /// The property the parity was chosen for: mirroring is arithmetic, and
    /// the arithmetic agrees with the name-based mirror the rig already had.
    #[test]
    fn the_arithmetic_mirror_agrees_with_the_rigs_own_mirror() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        let by_name = s.mirror();
        assert!(by_name.is_exact(), "the canonical rig pairs completely");
        for i in 0..s.len() as JointId {
            let expected = nodes.id(by_name.partner(i));
            assert_eq!(
                mirror(nodes.id(i)),
                expected,
                "{} mirrors onto {}",
                s.joints[i as usize].name,
                s.joints[by_name.partner(i) as usize].name
            );
        }
    }

    /// The other half of the same rule, and the one the renumbering was for.
    /// The lateral numbers climb from the floor too: `1` and `2` are a pair of
    /// toes, and everything on a leg is numbered below everything on an arm
    /// because a leg is lower than an arm.
    #[test]
    fn the_lowest_lateral_nodes_are_the_toes() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        let name = |n: NodeId| s.joints[nodes.joint(n).unwrap() as usize].name.as_str();

        assert_eq!(name(1), "l_toe", "node 1 is {}, not the left toe", name(1));
        assert_eq!(name(2), "r_toe", "2 is 1's partner");
        assert_eq!(mirror(1), 2);

        let at = |n: &str| nodes.id(s.find(n).unwrap());
        assert!(at("l_toe") < at("l_ankle"), "the toe is above the ankle");
        assert!(at("l_ankle") < at("l_knee"));
        assert!(at("l_knee") < at("l_hip"));
        assert!(at("l_hip") < at("l_finger_tip"), "the leg is numbered above the hand");
        assert!(at("l_finger_tip") < at("l_wrist"), "a fingertip hangs below its wrist");
        assert!(at("l_wrist") < at("l_elbow") && at("l_elbow") < at("l_shoulder"));
        assert!(at("l_shoulder") < at("l_clavicle"), "the clavicle is the top of the side");
        // 14 pairs, contiguous, nothing skipped.
        let mut lateral: Vec<NodeId> = nodes.ids().iter().copied().filter(|&n| n > 0).collect();
        lateral.sort_unstable();
        assert_eq!(lateral, (1..=28).collect::<Vec<_>>());
    }

    /// The rest pose puts three joints of each foot at exactly `y = 0`, so the
    /// first key alone does not order them and the second one has to. The grid
    /// is what makes that a real tie rather than a float accident.
    #[test]
    fn nodes_on_the_floor_are_ordered_by_how_far_out_the_limb_they_are() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        let at = |n: &str| nodes.id(s.find(n).unwrap());
        let pts = Pose::rest(&s).points(&s);
        let y = |n: &str| pts[s.find(n).unwrap() as usize].y;

        for part in ["heel", "ball", "toe"] {
            assert!(y(&format!("l_{part}")).abs() < 1e-6, "{part} is not on the floor");
        }
        assert!(
            at("l_toe") < at("l_ball"),
            "the toe is further out than the ball above it and must number first"
        );
        // The heel and the ball are the shallowest nodes on the floor and tie
        // at the same depth, so the index settles them; what is substantive is
        // that the toe numbers below both, and all three below the ankle.
        assert!(at("l_toe") < at("l_heel"), "the toe numbers below the heel");
        assert!(at("l_heel") < at("l_ankle"), "the floor numbers below the ankle above it");
    }

    #[test]
    fn a_pair_is_two_adjacent_numbers() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        let l = nodes.id(s.find("l_elbow").unwrap());
        let r = nodes.id(s.find("r_elbow").unwrap());
        assert_eq!(r, l + 1, "left is odd, right is the even one above it");
        assert_eq!(mirror(l), r);
        assert_eq!(mirror(r), l);
    }

    #[test]
    fn every_node_round_trips_through_both_directions() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        for i in 0..s.len() as JointId {
            assert_eq!(nodes.joint(nodes.id(i)), Some(i));
        }
        assert_eq!(nodes.len(), 36);
        assert!(nodes.unpaired().is_empty());
    }

    #[test]
    fn side_and_axiality_read_off_the_number_alone() {
        assert_eq!(side(-7), Side::Center);
        assert_eq!(side(8), Side::Right);
        assert_eq!(side(7), Side::Left);
        assert!(is_axial(-1));
        assert!(!is_axial(1));
        assert_eq!(mirror(-14), -14, "an axial node reflects onto itself");
    }
}
