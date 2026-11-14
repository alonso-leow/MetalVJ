//! The per-tick notation: what one eighth of a second of a take says.
//!
//! [`Motion`] is the authored form — sparse, continuous, keyed wherever the
//! movement actually has a key. This module is the *baked* form, one record per
//! tick of the 8 Hz grid ([`KEY_RATE`]), and it exists because a model emits
//! ticks, not curves. The two are not competing representations: a [`Track`] is
//! regenerated from its `Motion` whenever anything upstream changes, the way a
//! `Clip` is.
//!
//! Written down, a tick with movement in it is
//!
//! ```text
//! [node, x, y, z], [node, x, y, z], [travel, x, y, z]
//! ```
//!
//! and a tick with nothing in it is `[0]`.
//!
//! **Every group is a rotation** (2026-08-15, the operator: *"move the whole
//! notation to rotations and not position coordinates"*). `x, y, z` is the axis
//! the joint turns about scaled by how far it turns
//! ([`crate::pose::to_rotvec`]), measured against the joint's rest frame. There
//! is exactly one kind of group, so a tick is a list of one kind of thing.
//!
//! What went, and it went all at once:
//!
//! - **The roles.** `[node, 0]` was *carried* — in the movement, resolved by a
//!   chain solver, no numbers of its own. `[node, 0, 0, 0]` was *reset* — back
//!   to a remembered origin. Absence was *silent*. Three states, a fourth
//!   logit, a solver and a per-take memory, all to work around the fact that a
//!   declared **position** needs a mechanism to become a body. A rotation is
//!   already the body.
//! - **The relative zero.** A position needs a frame to be a position in, and
//!   the frame had to travel with the dancer, so a tick became a *step* and
//!   stopped being readable on its own. A local rotation is measured against
//!   its parent — the frame travels for free — so `[node, x, y, z]` says the
//!   same thing on tick 40 as on tick 1.
//! - **The special zero.** `[node, 0, 0, 0]` is no longer a reset and no longer
//!   forbidden: it is a joint at its rest rotation, which is a perfectly
//!   ordinary thing to write and the natural way to spell *straighten it*.
//!   Silence is still different, and still means *hold whatever it had*.
//!
//! **A tensor is dense, and it is dense with rotations.** The row a model emits
//! is not this tick — it is the **pose** this tick leaves ([`Pose::tensor_row`]),
//! every joint's rotation whether the tick mentioned it or not, plus where the
//! body is. No mask, no role logits, no loss that has to be told which columns
//! to read. A sparse tick is an *authoring* economy; a training row is a body.
//!
//! Ids, not names, and that is not a contradiction of [`crate::motion::Phrase`]'s
//! rule. A phrase is authored and has to survive the rig being edited
//! underneath it, so it keys by name. A track is derived and is thrown away and
//! rebuilt when that happens, so it keys by id and writes the rig it was baked
//! against into [`Track::rig`] — which is what makes the id safe rather than
//! merely convenient.
//!
//! **The id is the notation's [`NodeId`], not the array's [`JointId`]**
//! (2026-08-13). Negative on the axis, even on the right, odd on the left, and
//! never zero — see [`crate::node`]. One thing that buys is visible right here:
//! `[0]` used to be both *the empty tick* and *the pelvis*, since the pelvis is
//! joint 0. Zero is no longer a node, so the empty tick owns it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::motion::{Motion, KEY_RATE};
use crate::node::{NodeId, Nodes};
use crate::pose::Pose;
use crate::skeleton::{JointId, Skeleton};

/// Numbers per joint in the dense tensor: one rotation vector.
pub const SLOTS_PER_JOINT: usize = 3;

/// One tick of the 8 Hz grid.
///
/// Nodes absent from `turns` hold whatever rotation they had, which is why an
/// empty frame is a legal and frequent thing to write rather than a gap in the
/// record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub turns: BTreeMap<NodeId, Vec3>,
    /// Where the body is. Absent means this tick does not say, so an arm
    /// gesture cannot accidentally restate where the dancer stands — the same
    /// rule [`crate::motion::Phrase::travel`] follows.
    ///
    /// The one number here that is not a rotation, and the one that cannot be:
    /// no amount of turning says the dancer crossed the floor, and `CORPUS.md`'s
    /// subset 2 is *entirely* about crossing the floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub travel: Option<Vec3>,
}

impl Frame {
    /// Nothing declared: the `[0]` tick.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty() && self.travel.is_none()
    }

    /// Writes a rotation down for a node.
    ///
    /// A zero is kept as a zero. It says *this joint is at its rest rotation*,
    /// which is a statement — the straightened elbow, the released spine — and
    /// is not what silence says.
    pub fn turn(mut self, node: NodeId, by: Vec3) -> Self {
        self.turns.insert(node, by);
        self
    }

    pub fn travelling(mut self, to: Vec3) -> Self {
        self.travel = Some(to);
        self
    }

    /// What this tick says about a node, if anything.
    pub fn get(&self, node: NodeId) -> Option<Vec3> {
        self.turns.get(&node).copied()
    }

    /// `(written, unwritten)`, given a rig of `joints` nodes.
    ///
    /// One number and its complement, where there used to be a three-way audit.
    /// The second is the one worth reading: a tick that writes three joints is
    /// an isolation, and one that writes all of them either moved everything or
    /// was produced by something filling in boxes.
    pub fn counts(&self, joints: usize) -> (usize, usize) {
        (self.turns.len(), joints.saturating_sub(self.turns.len()))
    }

    /// Writes the tick in the bracket notation.
    ///
    /// Groups come out in **reading order** — down the axis from the pelvis
    /// first, then the limbs in left/right pairs — rather than in the map's own
    /// order, which sorts −8 before −1 and would print a tick starting at the
    /// jaw. The map is still the source of truth; this is only the order the
    /// line is laid out in, and [`Frame::parse`] does not depend on it.
    pub fn write(&self) -> String {
        if self.is_empty() {
            return "[0]".into();
        }
        let mut order: Vec<(&NodeId, &Vec3)> = self.turns.iter().collect();
        order.sort_by_key(|(n, _)| (**n > 0, n.abs()));
        let mut out = String::new();
        for (node, v) in order {
            if !out.is_empty() {
                out.push_str(", ");
            }
            let _ = write!(out, "[{node}, {:.4}, {:.4}, {:.4}]", v.x, v.y, v.z);
        }
        if let Some(t) = self.travel {
            let _ = write!(out, ", [travel, {:.4}, {:.4}, {:.4}]", t.x, t.y, t.z);
        }
        out
    }

    /// Reads a tick back. The inverse of [`Frame::write`] up to the four
    /// decimal places it prints.
    pub fn parse(text: &str) -> Result<Frame, FrameError> {
        let mut frame = Frame::default();
        let trimmed = text.trim();
        if trimmed == "[0]" || trimmed.is_empty() {
            return Ok(frame);
        }
        for group in trimmed.split('[').skip(1) {
            let body = group
                .split(']')
                .next()
                .ok_or_else(|| FrameError::Malformed(group.into()))?;
            let fields: Vec<&str> = body.split(',').map(str::trim).collect();
            let number = |s: &str| s.parse::<f32>().map_err(|_| FrameError::Malformed(s.into()));
            match fields.as_slice() {
                ["travel", x, y, z] => {
                    frame.travel = Some(Vec3::new(number(x)?, number(y)?, number(z)?));
                }
                [node, x, y, z] => {
                    let v = Vec3::new(number(x)?, number(y)?, number(z)?);
                    frame.turns.insert(parse_id(node)?, v);
                }
                _ => return Err(FrameError::Malformed(body.into())),
            }
        }
        Ok(frame)
    }

    /// Applies the tick to a pose that already holds what was underneath.
    ///
    /// Travel first, because travel places the body and everything hangs off
    /// that placement. Then one rotation per named node, in any order — no two
    /// of them interact, which is why there is no resolver here to share with
    /// the authored form any more.
    ///
    /// Nodes this tick does not name are not touched: they hold, and forward
    /// kinematics carries them along if something above them turned.
    ///
    /// `nodes` is built once per rig by the caller, the same trade
    /// [`Skeleton::mirror`] makes and for the same reason: this runs per tick
    /// and must not rebuild a table inside the loop.
    pub fn apply(&self, nodes: &Nodes, out: &mut Pose) {
        if let Some(t) = self.travel {
            out.root = t;
        }
        for (node, by) in &self.turns {
            if let Some(joint) = nodes.joint(*node) {
                out.turn(joint, *by);
            }
        }
    }
}

/// Reads a node id, and refuses zero.
///
/// Zero parses perfectly well as a number and is not a node, so accepting it
/// would turn a typo into a silent write to whatever the array's joint 0
/// happens to be. It is the one id worth an error of its own.
fn parse_id(s: &str) -> Result<NodeId, FrameError> {
    match s.parse::<NodeId>() {
        Ok(0) => Err(FrameError::ZeroIsNotANode),
        Ok(n) => Ok(n),
        Err(_) => Err(FrameError::UnknownNode(s.into())),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrameError {
    Malformed(String),
    UnknownNode(String),
    ZeroIsNotANode,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Malformed(s) => write!(f, "malformed group: {s}"),
            FrameError::UnknownNode(s) => write!(f, "not a node id: {s}"),
            FrameError::ZeroIsNotANode => {
                write!(f, "0 is not a node; [0] on its own is the empty tick")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// the dense form
// ---------------------------------------------------------------------------

/// Width of one row of the dense tensor, for a rig of `joints` nodes.
///
/// `joints × 3 + 3`: a rotation vector per joint, then where the body is. On
/// the canonical rig that is **111 floats**, down from 997 — the cut is the rig
/// (142 → 36) and the end of the role block (7 numbers per joint → 3).
pub fn tensor_width(joints: usize) -> usize {
    joints * SLOTS_PER_JOINT + 3
}

impl Pose {
    /// Writes the pose into a flat `f32` row:
    ///
    /// ```text
    /// [ turns : joints × 3 ]  each joint's rotation vector
    /// [ root  :          3 ]  where the body is
    /// ```
    ///
    /// **Rows are positional, in rig order.** A dense row needs no names, so
    /// the notation's signed ids do not appear in the tensor at all — slot *j*
    /// is joint *j*, which keeps this indexing arithmetic rather than a hash
    /// lookup per joint per tick. The consequence worth knowing: mirroring a
    /// tensor is a fixed column permutation (the left limb block swapped with
    /// the right) plus the sign flip on y and z, not a re-read of the ids.
    ///
    /// Every number in the row is data. There is no mask and nothing to mask —
    /// the row is a body, and every joint of a body is somewhere. That is the
    /// difference between training on a pose and training on an annotation: the
    /// old row carried role logits so the loss could be told which of its own
    /// numbers were the annotator's silence, and a rotation notation has no
    /// silence to write out.
    pub fn tensor_row(&self, out: &mut [f32]) {
        let joints = self.local.len();
        assert_eq!(out.len(), tensor_width(joints), "wrong row width");
        for j in 0..joints {
            let v = self.turned(j as JointId);
            out[j * 3..j * 3 + 3].copy_from_slice(&[v.x, v.y, v.z]);
        }
        out[joints * 3..].copy_from_slice(&[self.root.x, self.root.y, self.root.z]);
    }

    /// Reads a pose back out of a tensor row.
    pub fn from_tensor_row(skeleton: &Skeleton, row: &[f32]) -> Pose {
        let joints = skeleton.len();
        assert_eq!(row.len(), tensor_width(joints), "wrong row width");
        let mut pose = Pose::rest(skeleton);
        for j in 0..joints {
            pose.turn(
                j as JointId,
                Vec3::new(row[j * 3], row[j * 3 + 1], row[j * 3 + 2]),
            );
        }
        pose.root = Vec3::new(row[joints * 3], row[joints * 3 + 1], row[joints * 3 + 2]);
        pose
    }
}

/// A whole take, on the grid: the thing a model is trained to emit.
///
/// One [`Frame`] per tick, in order, plus the one thing that is true of the
/// take rather than of any tick: which rig the ids mean.
///
/// **Serialized as JSON, not TOML**, and not by preference: `turns` is keyed by
/// [`NodeId`], and TOML has no integer-keyed tables, so `toml::to_string` on a
/// track fails with *"map key was not a string"*. Discovered by
/// `fw-studio`'s save test rather than by reading the spec.
///
/// The constraint points the same way the design already did. A
/// [`crate::motion::Phrase`] is authored, keyed by joint *name*, and
/// hand-editable — TOML is right for it. A track is baked: hundreds of ticks by
/// thirty-six nodes, written by a tool and read by a tool. Nobody opens one to
/// change a number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    /// The rig these ids index. Baked artifacts are regenerated, not migrated;
    /// this is here so a stale one is *detected* rather than silently
    /// reinterpreted.
    pub rig: String,
    pub joints: usize,
    #[serde(default = "default_rate")]
    pub rate: f64,
    #[serde(default, rename = "frame")]
    pub frames: Vec<Frame>,
}

fn default_rate() -> f64 {
    KEY_RATE
}

impl Track {
    /// Bakes an authored motion onto the grid.
    ///
    /// A joint counts as written at this tick only while its curve's own span
    /// covers the tick, and only when the rotation actually *changed* — past
    /// the end of a curve, and on ticks in the middle of it where nothing
    /// moves, the value holds, which is what silence already means. Restating
    /// it every tick would be writing the annotator's last statement out again
    /// until the take ended.
    ///
    /// A curve and a tick say the same kind of thing now, so this is a sample
    /// and a comparison. It used to be a conversion — curves were absolute
    /// positions and ticks were steps — and the seam between them was where the
    /// two forms could disagree about what a declaration meant.
    pub fn bake(motion: &Motion, skeleton: &Skeleton) -> Track {
        let nodes = Nodes::of(skeleton);
        let (start, end) = motion.span().unwrap_or((0.0, 0.0));
        let ticks = ((end - start) * KEY_RATE).round().max(0.0) as usize + 1;
        let mut frames = Vec::with_capacity(ticks);
        // What each joint was last written as, so a tick only speaks when
        // something changed.
        let mut was: BTreeMap<NodeId, Vec3> = BTreeMap::new();
        let departure = motion.from.pose(skeleton);

        for i in 0..ticks {
            let t = start + i as f64 / KEY_RATE;
            let mut frame = Frame::default();
            for phrase in &motion.phrases {
                for (name, curve) in &phrase.turns {
                    let (Some(id), Some(a), Some(b)) =
                        (skeleton.find(name), curve.start(), curve.end())
                    else {
                        continue;
                    };
                    if t + 1e-9 < a || t > b + 1e-9 {
                        continue;
                    }
                    if let Some(v) = curve.sample(t) {
                        let node = nodes.id(id);
                        let before = was.insert(node, v).unwrap_or_else(|| departure.turned(id));
                        if v != before {
                            frame.turns.insert(node, v);
                        }
                    }
                }
                if let Some(travel) = &phrase.travel {
                    if let Some(root) = travel.sample(t) {
                        frame.travel = Some(root);
                    }
                }
            }
            frames.push(frame);
        }

        Track {
            name: motion.name.clone(),
            rig: skeleton.name.clone(),
            joints: skeleton.len(),
            rate: KEY_RATE,
            frames,
        }
    }

    /// Applies tick `i` to `out`, which must already hold the pose the ticks
    /// before it left. Silence is what makes this cumulative: nothing is reset
    /// between ticks, so a joint nobody has mentioned since tick 0 still holds
    /// tick 0's rotation.
    pub fn pose_into(&self, nodes: &Nodes, i: usize, out: &mut Pose) {
        if let Some(frame) = self.frames.get(i) {
            frame.apply(nodes, out);
        }
    }

    /// The pose at tick `upto`, replayed from tick 0 onto `out`, which must
    /// hold the posture the take departs from.
    ///
    /// A replay because the record is sparse, not because a tick is relative:
    /// tick 40 says exactly what it says, and the replay is only how the joints
    /// tick 40 declines to mention get their values. Under the position
    /// notation this was the whole cost of a transportable zero and there was
    /// no shortcut to be had; here a dense track could be read at any tick
    /// directly.
    pub fn pose_through(&self, nodes: &Nodes, upto: usize, out: &mut Pose) {
        for i in 0..=upto.min(self.frames.len().saturating_sub(1)) {
            self.pose_into(nodes, i, out);
        }
    }

    /// The whole track as one dense matrix, row per tick, ready to be written
    /// out as the training target beside its audio matrix.
    ///
    /// The rows are **poses, not ticks**: the take is replayed and each row is
    /// the body at that instant. A sparse tick cannot become a dense row on its
    /// own — the joints it does not mention have rotations, and *hold* is not a
    /// number. Writing zeros under them would say every unmentioned joint is
    /// straight, which is the exact confusion the old role block existed to
    /// prevent, and replaying is how it is prevented instead.
    pub fn tensor(&self, skeleton: &Skeleton, nodes: &Nodes, from: &Pose) -> Vec<f32> {
        let width = tensor_width(self.joints);
        let mut out = vec![0.0; width * self.frames.len()];
        let mut pose = from.clone();
        debug_assert_eq!(pose.local.len(), skeleton.len());
        for (i, frame) in self.frames.iter().enumerate() {
            frame.apply(nodes, &mut pose);
            pose.tensor_row(&mut out[i * width..(i + 1) * width]);
        }
        out
    }

    /// Ticks that declare nothing at all — the `[0]` rows.
    ///
    /// Worth reading before training on a take: a track that is mostly empty is
    /// either a genuinely still passage or an annotation that was never
    /// finished, and the two want opposite responses.
    pub fn empty_ticks(&self) -> usize {
        self.frames.iter().filter(|f| f.is_empty()).count()
    }

    pub fn write(&self) -> String {
        let mut out = String::new();
        for frame in &self.frames {
            out.push_str(&frame.write());
            out.push('\n');
        }
        out
    }

    /// The invariants a track holds that its *type* cannot.
    ///
    /// Two rules. **Zero is not a node** — [`Frame::parse`] refuses it in the
    /// text form, but serde will happily deserialize `"0"` as a map key, so a
    /// track that arrived as JSON has never been asked. And **no rotation is
    /// written the long way round**, which is the encoding invariant
    /// [`crate::pose::to_rotvec`] guarantees on the way out and nothing
    /// guarantees on the way in.
    pub fn validate(&self) -> Result<(), TrackError> {
        for (i, frame) in self.frames.iter().enumerate() {
            if frame.turns.contains_key(&0) {
                return Err(TrackError::ZeroNode { tick: i });
            }
            for (node, v) in &frame.turns {
                if v.length() > std::f32::consts::PI + 1e-4 {
                    return Err(TrackError::TheLongWayRound {
                        tick: i,
                        node: *node,
                        radians: v.length(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TrackError {
    #[error("node 0 is not a node (tick {tick})")]
    ZeroNode { tick: usize },
    #[error("node {node} turns {radians:.2} rad at tick {tick}, the long way round")]
    TheLongWayRound {
        tick: usize,
        node: NodeId,
        radians: f32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::{Curve, Key};
    use crate::figure::Figure;
    use crate::motion::Phrase;
    use crate::rig;
    use crate::skeleton::{Joint, Side};
    use glam::Quat;

    /// A joint that starts unturned and ends at `to`.
    fn swing(from: f64, to: f64, by: Vec3) -> Curve<Vec3> {
        Curve::from_keys(vec![Key::new(from, Vec3::ZERO), Key::new(to, by)])
    }

    /// Four joints, one of each kind: an axial root, a lateral pair, and a
    /// second axial node. Numbered −1, 1, 2, −2 — small enough to write a
    /// tensor row out by hand and check every number in it.
    fn tiny() -> Skeleton {
        let joint = |name: &str, parent: Option<u16>, side| Joint {
            name: name.into(),
            parent,
            side,
            rest_offset: Vec3::Y,
            rest_rotation: Quat::IDENTITY,
        };
        Skeleton {
            name: "tiny".into(),
            joints: vec![
                joint("pelvis", None, Side::Center),
                joint("l_hip", Some(0), Side::Left),
                joint("r_hip", Some(0), Side::Right),
                joint("vert_l3", Some(0), Side::Center),
            ],
        }
    }

    #[test]
    fn an_empty_tick_is_a_single_zero() {
        let frame = Frame::default();
        assert!(frame.is_empty());
        assert_eq!(frame.write(), "[0]");
        assert_eq!(Frame::parse("[0]").unwrap(), frame);
    }

    #[test]
    fn a_tick_round_trips_through_the_bracket_notation() {
        let frame = Frame::default()
            .turn(12, Vec3::new(0.25, -0.5, 0.0))
            .turn(13, Vec3::new(0.0, 0.125, 0.0));
        let text = frame.write();
        assert_eq!(text, "[12, 0.2500, -0.5000, 0.0000], [13, 0.0000, 0.1250, 0.0000]");
        assert_eq!(Frame::parse(&text).unwrap(), frame);
        assert_eq!(frame.counts(36), (2, 34));
    }

    /// The zero is a value again. Under the old notation `[node, 0, 0, 0]` was
    /// the reset — a fourth role with a remembered origin behind it — and a
    /// zero-valued declaration was canonicalised away. A zero rotation is a
    /// joint at its rest rotation, which is what *straighten it* means, and it
    /// is not what silence means.
    #[test]
    fn a_zero_rotation_is_a_statement_and_silence_is_a_different_one() {
        let straight = Frame::default().turn(9, Vec3::ZERO);
        let silent = Frame::default();
        assert_ne!(straight, silent);
        assert!(!straight.is_empty(), "writing a zero is not writing nothing");
        assert_eq!(straight.write(), "[9, 0.0000, 0.0000, 0.0000]");
        assert_eq!(Frame::parse(&straight.write()).unwrap(), straight);
        assert_eq!(straight.get(9), Some(Vec3::ZERO));
        assert_eq!(silent.get(9), None);

        // And they do different things to a body: the zero straightens the
        // joint, the silence leaves it bent.
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        let elbow = s.find("l_elbow").unwrap();
        let bent = {
            let mut p = Pose::rest(&s);
            p.turn(elbow, Vec3::new(0.0, 1.0, 0.0));
            p
        };
        let (mut a, mut b) = (bent.clone(), bent.clone());
        Frame::default().turn(nodes.id(elbow), Vec3::ZERO).apply(&nodes, &mut a);
        silent.apply(&nodes, &mut b);
        assert!(a.turned(elbow).length() < 1e-6, "the zero did not straighten it");
        assert!((b.turned(elbow).y - 1.0).abs() < 1e-6, "silence rewrote it");
    }

    /// The axis is written with a minus in front of it and read back with one.
    /// Groups come out down the body — the column from the pelvis up, then the
    /// limbs — not in the map's order, which would start the line at the jaw.
    #[test]
    fn the_axis_is_negative_and_a_tick_reads_down_the_body() {
        let frame = Frame::default()
            .turn(7, Vec3::ZERO)
            .turn(-6, Vec3::new(0.1, 0.0, 0.0))
            .turn(-1, Vec3::ZERO);
        assert!(frame.write().starts_with("[-1,"), "{}", frame.write());
        assert!(frame.write().contains("[-6, 0.1000"), "{}", frame.write());
        assert!(frame.write().ends_with("[7, 0.0000, 0.0000, 0.0000]"), "{}", frame.write());
        assert_eq!(Frame::parse(&frame.write()).unwrap(), frame);
    }

    /// The collision the signed ids exist to remove: `[0]` is the empty tick
    /// and nothing else, so a zero inside a group is an error rather than a
    /// silent write to whatever sits at index 0.
    #[test]
    fn zero_is_the_empty_tick_and_never_a_node() {
        assert!(Frame::parse("[0]").unwrap().is_empty());
        assert_eq!(
            Frame::parse("[0, 0.1, 0.0, 0.0]"),
            Err(FrameError::ZeroIsNotANode)
        );
        assert!(matches!(Frame::parse("[3, 0]"), Err(FrameError::Malformed(_))));
    }

    #[test]
    fn travel_round_trips_and_stays_optional() {
        let mut frame = Frame::default().turn(3, Vec3::X);
        assert!(!frame.write().contains("travel"));
        frame.travel = Some(Vec3::new(1.0, 0.96, -0.5));
        assert_eq!(Frame::parse(&frame.write()).unwrap(), frame);
    }

    // ---------------------------------------------------------- the dense form

    /// The number the operator asked for, pinned. A row is a rotation per joint and
    /// the three that say where the body is — 111, where it was 997.
    #[test]
    fn a_row_is_three_numbers_a_joint_and_nothing_else() {
        let s = rig::canonical();
        assert_eq!(tensor_width(s.len()), 36 * 3 + 3);
        assert_eq!(tensor_width(s.len()), 111);
        assert_eq!(SLOTS_PER_JOINT, 3, "there is one kind of number in a row");
    }

    #[test]
    fn a_pose_round_trips_through_a_tensor_row() {
        let s = tiny();
        let mut pose = Pose::rest(&s);
        pose.turn(1, Vec3::new(0.5, 0.25, -0.125));
        pose.turn(3, Vec3::new(0.0, 0.75, 0.0));
        pose.root = Vec3::new(0.0, 0.96, 1.5);

        let mut row = vec![0.0; tensor_width(s.len())];
        pose.tensor_row(&mut row);
        // Four joints, three numbers each, then the root.
        let block = |j: usize| Vec3::new(row[j * 3], row[j * 3 + 1], row[j * 3 + 2]);
        assert_eq!(block(0), Vec3::ZERO, "the pelvis is unturned");
        assert!(block(1).abs_diff_eq(Vec3::new(0.5, 0.25, -0.125), 1e-6));
        assert_eq!(block(2), Vec3::ZERO);
        assert!(block(3).abs_diff_eq(Vec3::new(0.0, 0.75, 0.0), 1e-6));
        assert_eq!(&row[12..], &[0.0, 0.96, 1.5]);

        let back = Pose::from_tensor_row(&s, &row);
        for (a, b) in back.local.iter().zip(&pose.local) {
            assert!(a.abs_diff_eq(*b, 1e-6));
        }
        assert_eq!(back.root, pose.root);
    }

    /// The row is positional and the notation's ids are not in it, so the two
    /// numberings have to agree about which column belongs to which joint —
    /// this is the seam where a silent off-by-one would land the left hand's
    /// rotation under the right hand's.
    #[test]
    fn the_tensor_column_and_the_node_id_mean_the_same_joint() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        let elbow = s.find("l_elbow").unwrap();
        let mut pose = Pose::rest(&s);
        Frame::default()
            .turn(nodes.id(elbow), Vec3::new(0.0, 0.0, 0.7))
            .apply(&nodes, &mut pose);

        let mut row = vec![0.0; tensor_width(s.len())];
        pose.tensor_row(&mut row);
        let column = elbow as usize;
        assert!((row[column * 3 + 2] - 0.7).abs() < 1e-6, "l_elbow's z is not in its column");
        assert_eq!(&row[column * 3..column * 3 + 2], &[0.0, 0.0]);
    }

    /// A dense row is a body, and a body has no gaps in it. This is the test
    /// that the role block is really gone: nothing in a row says *nobody
    /// looked*, because there is nothing for it to say that about.
    #[test]
    fn a_row_carries_no_mask_and_a_silent_joint_still_has_a_rotation() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        let mut pose = Figure::Standing.pose(&s);
        let shoulder = s.find("l_shoulder").unwrap();
        let before = pose.turned(shoulder);
        assert!(before.length() > 0.1, "Standing turns the shoulder; this test needs that");

        // A tick that says nothing about the shoulder.
        Frame::default()
            .turn(nodes.id(s.find("l_elbow").unwrap()), Vec3::new(0.0, 0.5, 0.0))
            .apply(&nodes, &mut pose);
        let mut row = vec![0.0; tensor_width(s.len())];
        pose.tensor_row(&mut row);

        let col = shoulder as usize * 3;
        let wrote = Vec3::new(row[col], row[col + 1], row[col + 2]);
        assert!(
            wrote.abs_diff_eq(before, 1e-6),
            "the unmentioned shoulder came out as {wrote:?}, not the {before:?} it is at"
        );
        assert!(wrote.length() > 0.1, "silence was written out as a zero rotation");
    }

    // --------------------------------------------------------------- baking

    #[test]
    fn baking_puts_one_frame_on_every_eighth_of_a_second() {
        let s = rig::canonical();
        let motion = Motion::new("swing", Figure::Standing)
            .then(Phrase::new("arc").turn("l_shoulder", swing(0.0, 1.0, Vec3::new(0.0, 0.0, 0.8))));
        let track = Track::bake(&motion, &s);

        assert_eq!(track.rate, KEY_RATE);
        assert_eq!(track.joints, s.len());
        assert_eq!(track.rig, "koreo.canonical");
        assert_eq!(track.frames.len(), 9, "one second at 8 Hz is nine ticks");
        let shoulder = Nodes::of(&s).id(s.find("l_shoulder").unwrap());
        for frame in &track.frames {
            assert!(frame.get(shoulder).is_some());
            assert_eq!(frame.counts(s.len()).0, 1, "one joint, every tick");
        }
    }

    /// An isolation, baked. One joint is spoken for and the other thirty-five
    /// are not — the sparse record surviving the trip onto a fixed grid, which
    /// is the reason the grid does not make the notation dense.
    ///
    /// **Tick 0 is empty**, and that is the sparse rule applying to time as
    /// well as to the body: the curve opens at the rotation `Standing` already
    /// holds the wrist at, so the first tick has nothing to say. A notation
    /// that wrote it anyway would be restating the figure as though it were a
    /// decision.
    #[test]
    fn baking_an_isolation_leaves_the_rest_of_the_body_silent() {
        let s = rig::canonical();
        let motion = Motion::new("wave", Figure::Standing)
            .then(Phrase::new("shake").turn("l_wrist", swing(0.0, 0.5, Vec3::new(0.0, 0.4, 0.0))));

        let track = Track::bake(&motion, &s);
        let nodes = Nodes::of(&s);
        let r_wrist = nodes.id(s.find("r_wrist").unwrap());
        assert!(track.frames[0].is_empty(), "the curve opens where the figure already is");
        for frame in &track.frames[1..] {
            assert_eq!(frame.counts(s.len()), (1, 35));
            assert_eq!(frame.get(r_wrist), None);
        }
    }

    /// Silence past the end of a curve, rather than the last key restated
    /// forever. A phrase that finishes halfway through a take stops speaking.
    #[test]
    fn a_curve_that_ends_early_goes_silent_rather_than_repeating_itself() {
        let s = rig::canonical();
        let up = Vec3::new(0.0, 0.0, 0.5);
        let motion = Motion::new("half", Figure::Standing).then(
            Phrase::new("p")
                .turn("l_shoulder", swing(0.0, 0.5, up))
                .turn("r_shoulder", swing(0.0, 1.0, -up)),
        );
        let track = Track::bake(&motion, &s);
        let nodes = Nodes::of(&s);
        let (l, r) = (
            nodes.id(s.find("l_shoulder").unwrap()),
            nodes.id(s.find("r_shoulder").unwrap()),
        );
        assert_eq!(r, l + 1, "a pair is two adjacent numbers");

        assert!(track.frames[4].get(l).is_some(), "0.5 s is the last key");
        assert_eq!(track.frames[5].get(l), None);
        assert!(track.frames[8].get(r).is_some(), "the other curve runs on");
    }

    /// The bridge back: a baked track drives the rig, which is what the app is
    /// for. Compared against `Motion::pose_at` at the same instant, since the
    /// notation is only worth anything if the two agree — and now they agree
    /// *exactly*, because baking is a sample rather than a conversion. The
    /// position notation could only promise a centimetre here.
    #[test]
    fn a_baked_track_poses_the_body_the_way_the_motion_does() {
        let s = rig::canonical();
        let motion = Motion::new("arc", Figure::Standing).then(
            Phrase::new("p")
                .turn("l_shoulder", swing(0.0, 1.0, Vec3::new(0.1, 0.2, 0.9)))
                .turn("l_elbow", swing(0.0, 1.0, Vec3::new(0.0, 1.0, 0.0))),
        );
        let track = Track::bake(&motion, &s);

        let mut from_track = Figure::Standing.pose(&s);
        track.pose_through(&Nodes::of(&s), 5, &mut from_track);
        let direct = motion.pose_at(&s, 5.0 / KEY_RATE);

        for j in 0..s.len() {
            assert!(
                from_track.local[j].abs_diff_eq(direct.local[j], 1e-6),
                "{} differs between the track and the motion",
                s.joints[j].name
            );
        }
    }

    /// A tick reads on its own, which the position notation gave up and
    /// documented as the price of a transportable zero. Tick 5 applied to the
    /// bare figure puts the joints it names exactly where tick 5 says.
    #[test]
    fn a_single_tick_still_answers_where_the_joints_it_names_are() {
        let s = rig::canonical();
        let motion = Motion::new("arc", Figure::Standing)
            .then(Phrase::new("p").turn("l_shoulder", swing(0.0, 1.0, Vec3::new(0.1, 0.4, 0.2))));
        let track = Track::bake(&motion, &s);
        let nodes = Nodes::of(&s);
        let shoulder = s.find("l_shoulder").unwrap();

        let mut alone = Figure::Standing.pose(&s);
        track.pose_into(&nodes, 5, &mut alone);
        let mut walked = Figure::Standing.pose(&s);
        track.pose_through(&nodes, 5, &mut walked);

        assert!(
            alone.local[shoulder as usize].abs_diff_eq(walked.local[shoulder as usize], 1e-6),
            "one tick and five ticks disagree about the joint they both name"
        );
    }

    /// The tensor is the take replayed, not the ticks copied — the joints a
    /// tick declines to mention still have rotations, and the row has to carry
    /// the ones they actually have.
    #[test]
    fn the_tensor_is_one_row_per_tick_and_every_row_is_a_whole_body() {
        let s = rig::canonical();
        let nodes = Nodes::of(&s);
        let motion = Motion::new("swing", Figure::Standing)
            .then(Phrase::new("arc").turn("l_elbow", swing(0.0, 1.0, Vec3::new(0.0, 1.0, 0.0))));
        let track = Track::bake(&motion, &s);

        let from = Figure::Standing.pose(&s);
        let width = tensor_width(s.len());
        let rows = track.tensor(&s, &nodes, &from);
        assert_eq!(rows.len(), width * track.frames.len());

        // The shoulder is never mentioned by any tick and is not zero in any
        // row, because Standing has it hanging.
        let col = s.find("l_shoulder").unwrap() as usize * 3;
        let want = from.turned(s.find("l_shoulder").unwrap());
        for i in 0..track.frames.len() {
            let at = i * width + col;
            let got = Vec3::new(rows[at], rows[at + 1], rows[at + 2]);
            assert!(got.abs_diff_eq(want, 1e-6), "row {i} lost the standing shoulder");
        }
        // And the elbow does change, row to row.
        let col = s.find("l_elbow").unwrap() as usize * 3;
        assert!((rows[col + 1] - rows[(track.frames.len() - 1) * width + col + 1]).abs() > 0.5);
    }

    #[test]
    fn a_still_take_is_all_empty_ticks() {
        let s = rig::canonical();
        let motion = Motion::new("hold", Figure::Standing).then(
            Phrase::new("p").travelling(Curve::from_keys(vec![
                Key::new(0.0, Vec3::new(0.0, 0.96, 0.0)),
                Key::new(0.25, Vec3::new(0.0, 0.96, 0.0)),
            ])),
        );
        let mut track = Track::bake(&motion, &s);
        assert_eq!(track.empty_ticks(), 0, "travel is a statement");

        track.frames = vec![Frame::default(); 8];
        assert_eq!(track.empty_ticks(), 8);
        assert_eq!(track.write().lines().collect::<Vec<_>>(), vec!["[0]"; 8]);
    }

    /// The format constraint, pinned where the type is rather than where it
    /// bit. A track has integer keys; TOML has no integer-keyed tables.
    #[test]
    fn a_track_round_trips_through_json_and_cannot_go_through_toml() {
        let s = rig::canonical();
        let motion = Motion::new("swing", Figure::Standing)
            .then(Phrase::new("arc").turn("l_shoulder", swing(0.0, 0.5, Vec3::new(0.0, 0.0, 0.6))));
        let track = Track::bake(&motion, &s);

        let json = serde_json::to_string(&track).expect("json");
        let back: Track = serde_json::from_str(&json).expect("json back");
        assert_eq!(back.frames.len(), track.frames.len());
        assert_eq!(back.joints, 36);
        assert_eq!(back.frames[1].counts(36), (1, 35));
        assert!(!json.contains("carried"), "the roles are gone from the wire too");
        assert!(!json.contains("given"), "{json}");

        assert!(
            toml::to_string(&track).is_err(),
            "TOML gained integer keys; the file format note on Track is now wrong"
        );
    }

    /// serde will deserialize `"0"` as a map key without complaint, so the rule
    /// the text form enforces has to be enforced again on the way in.
    #[test]
    fn a_track_with_a_zero_node_is_refused() {
        let s = rig::canonical();
        let motion = Motion::new("swing", Figure::Standing)
            .then(Phrase::new("arc").turn("l_shoulder", swing(0.0, 0.5, Vec3::new(0.0, 0.0, 0.6))));
        let mut track = Track::bake(&motion, &s);
        assert!(track.validate().is_ok());

        track.frames[0].turns.insert(0, Vec3::ZERO);
        assert_eq!(track.validate(), Err(TrackError::ZeroNode { tick: 0 }));
        track.frames[0].turns.remove(&0);
        assert!(track.validate().is_ok());
    }

    /// The encoding invariant, checked where a hand-written track arrives. A
    /// rotation past π walks the joint the long way round to somewhere the
    /// short way already reaches.
    #[test]
    fn a_track_that_turns_the_long_way_round_is_refused() {
        let s = rig::canonical();
        let motion = Motion::new("swing", Figure::Standing)
            .then(Phrase::new("arc").turn("l_shoulder", swing(0.0, 0.5, Vec3::new(0.0, 0.0, 0.6))));
        let mut track = Track::bake(&motion, &s);
        track.frames[1].turns.insert(5, Vec3::new(0.0, 4.0, 0.0));
        assert!(matches!(
            track.validate(),
            Err(TrackError::TheLongWayRound { tick: 1, node: 5, .. })
        ));
    }
}
