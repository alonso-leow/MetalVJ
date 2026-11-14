//! Skeleton topology: a flat, parent-sorted joint array.
//!
//! Every joint appears after its parent, so forward kinematics is a single
//! forward pass with no recursion and no sorting at runtime.

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Index into [`Skeleton::joints`]. `u16` is deliberate: the rig has been 142
/// joints and the dataset stores millions of frames.
pub type JointId = u16;

pub const ROOT: JointId = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Center,
    Left,
    Right,
}

impl Side {
    /// The side a reflected body part lands on.
    pub fn opposite(self) -> Side {
        match self {
            Side::Center => Side::Center,
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// The name of a joint's reflection, by the rig's `l_`/`r_` convention.
///
/// Returns `None` for names that carry no side, which is the correct answer
/// for `chest` and the wrong answer for a rig that spells its sides some other
/// way — hence [`Mirror::unpaired`], which reports what could not be paired
/// rather than letting a half-mirrored body pass as a whole one.
pub fn mirrored_name(name: &str) -> Option<String> {
    for (from, to) in [
        ("l_", "r_"),
        ("r_", "l_"),
        ("left_", "right_"),
        ("right_", "left_"),
    ] {
        if let Some(rest) = name.strip_prefix(from) {
            return Some(format!("{to}{rest}"));
        }
    }
    None
}

/// Which joint each joint reflects onto, across the sagittal plane.
///
/// Centre joints reflect onto themselves. A lateral joint whose partner is
/// missing also reflects onto itself — the mirror stays usable on an
/// asymmetric rig — but it is listed in [`Mirror::unpaired`], because a body
/// mirrored through a partial map is not a mirror image and nothing should
/// discover that by looking at the result.
#[derive(Debug, Clone)]
pub struct Mirror {
    map: Vec<JointId>,
    unpaired: Vec<JointId>,
}

impl Mirror {
    /// The joint `id` reflects onto.
    pub fn partner(&self, id: JointId) -> JointId {
        self.map.get(id as usize).copied().unwrap_or(id)
    }

    /// Lateral joints with no partner in the rig.
    pub fn unpaired(&self) -> &[JointId] {
        &self.unpaired
    }

    /// True when every lateral joint found its partner.
    pub fn is_exact(&self) -> bool {
        self.unpaired.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Joint {
    pub name: String,
    /// `None` only for the root.
    pub parent: Option<JointId>,
    pub side: Side,
    /// Rest-pose translation relative to the parent, in metres.
    pub rest_offset: Vec3,
    /// Rest-pose rotation relative to the parent.
    pub rest_rotation: Quat,
}

impl Joint {
    /// Bone length is the rest offset magnitude — the distance to the parent.
    pub fn length(&self) -> f32 {
        self.rest_offset.length()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skeleton {
    pub name: String,
    pub joints: Vec<Joint>,
}

impl Skeleton {
    pub fn len(&self) -> usize {
        self.joints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.joints.is_empty()
    }

    pub fn find(&self, name: &str) -> Option<JointId> {
        self.joints
            .iter()
            .position(|j| j.name == name)
            .map(|i| i as JointId)
    }

    /// Every joint's parent must come before it, and only joint 0 may be
    /// parentless. Checked once at load; the FK pass then assumes it.
    pub fn validate(&self) -> Result<(), SkeletonError> {
        if self.joints.is_empty() {
            return Err(SkeletonError::Empty);
        }
        for (i, joint) in self.joints.iter().enumerate() {
            match (i, joint.parent) {
                (0, None) => {}
                (0, Some(_)) => return Err(SkeletonError::RootHasParent),
                (_, None) => return Err(SkeletonError::MultipleRoots(i)),
                (_, Some(p)) if (p as usize) >= i => {
                    return Err(SkeletonError::ParentNotBefore { joint: i, parent: p })
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Joint ids from `id` down to the root, nearest first.
    pub fn ancestors(&self, id: JointId) -> impl Iterator<Item = JointId> + '_ {
        let mut cur = self.joints[id as usize].parent;
        std::iter::from_fn(move || {
            let out = cur?;
            cur = self.joints[out as usize].parent;
            Some(out)
        })
    }

    /// `id` and everything below it, in joint order. Single forward pass,
    /// again relying on parent-before-child ordering.
    pub fn subtree(&self, id: JointId) -> Vec<JointId> {
        let mut inside = vec![false; self.joints.len()];
        inside[id as usize] = true;
        for i in (id as usize + 1)..self.joints.len() {
            if let Some(p) = self.joints[i].parent {
                inside[i] = inside[p as usize];
            }
        }
        inside
            .iter()
            .enumerate()
            .filter(|(_, &b)| b)
            .map(|(i, _)| i as JointId)
            .collect()
    }

    pub fn children(&self, id: JointId) -> impl Iterator<Item = JointId> + '_ {
        self.joints
            .iter()
            .enumerate()
            .filter(move |(_, j)| j.parent == Some(id))
            .map(|(i, _)| i as JointId)
    }

    /// A joint with nothing hanging off it — a fingertip, a toe, the top of the
    /// skull.
    ///
    /// Worth naming because of what it means for the notation: **a joint's
    /// rotation places its children**, so a rotation written on a terminal
    /// moves no point on the body. It is a statement with no geometry in it,
    /// and the one way a rotation notation can be quietly ignored.
    /// `Motion::lint` and the app's proposal endpoint both check it.
    pub fn is_terminal(&self, id: JointId) -> bool {
        self.children(id).next().is_none()
    }

    /// Pairs every joint with its reflection across the sagittal plane.
    ///
    /// Built once and reused: mirroring a clip is this map applied per frame,
    /// and rebuilding it per frame would dominate the cost of the operation.
    pub fn mirror(&self) -> Mirror {
        let mut map: Vec<JointId> = (0..self.joints.len() as JointId).collect();
        let mut unpaired = Vec::new();
        for (i, joint) in self.joints.iter().enumerate() {
            if joint.side == Side::Center {
                continue;
            }
            match mirrored_name(&joint.name).and_then(|n| self.find(&n)) {
                Some(partner) => map[i] = partner,
                None => unpaired.push(i as JointId),
            }
        }
        Mirror { map, unpaired }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkeletonError {
    #[error("skeleton has no joints")]
    Empty,
    #[error("joint 0 must be the root but has a parent")]
    RootHasParent,
    #[error("joint {0} has no parent; only joint 0 may be parentless")]
    MultipleRoots(usize),
    #[error("joint {joint} references parent {parent} that is not earlier in the array")]
    ParentNotBefore { joint: usize, parent: JointId },
}
