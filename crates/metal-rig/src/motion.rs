//! Authored motion: a movement written as the joints it turns.
//!
//! [`Clip`](crate::pose::Clip) is dense — every joint, every frame — and that
//! is right for motion that was **measured**, because a camera has no opinion
//! about which joints matter. It is wrong for motion that is **authored**,
//! because a choreographer does. An isolation of one arm, written densely, is
//! thirty-six rotations of which thirty-three are the annotator's silence
//! written out as if it were a decision.
//!
//! So a [`Phrase`] writes the joints it is about and says nothing about the
//! rest. **That is not a role system.** There is no *given*, no *carried*, no
//! *silent* — three states a node could be in, with a chain solver and a
//! remembered-batch table behind them, all deleted on 2026-08-15 with the
//! position notation they existed to serve. A joint either has a rotation
//! written for it or does not, and one that does not keeps what it had. Nothing
//! branches on which.
//!
//! **A rotation is three numbers and they are absolute.** `[l_elbow, 0, 1.1,
//! 0]` is that joint turned 1.1 radians about its parent's Y — the same
//! statement whether the arm is hanging or overhead, because a local rotation
//! is measured against the parent's own frame and that frame travels with the
//! body. This is the transportable zero the position notation went through two
//! designs looking for; a rotation had it the whole time.
//!
//! **Silence still is not rest.** A joint the phrase does not mention keeps the
//! pose it already had — from the figure the movement departs from, or from an
//! earlier phrase. Writing "bend the left elbow" therefore bends the left elbow
//! and nothing else, without a single statement about the other thirty-five
//! joints, which is what an isolation *is*.

use std::collections::BTreeMap;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::curve::{Curve, Key};
use crate::figure::Figure;
use crate::pose::{mirror_turn, Clip, Pose};
use crate::skeleton::{mirrored_name, JointId, Skeleton};

/// Keys per second the notation is written on.
///
/// This is not a frame rate and not a sampling rate. A [`Curve`] is continuous
/// in time, so smoothness comes from the interpolation between keys, never from
/// how many of them there are — the grid exists so that two annotation passes
/// of the same phrase are comparable key for key, which is what the reliability
/// gate in `docs/the design notes` §3 measures.
///
/// Eight is chosen against the capture side, not the playback side. A key that
/// falls between two extracted frames is a key placed against an image that
/// does not exist; `metal-ingest` extracts a survey pass at 8 fps for exactly
/// this reason, and records where each frame *really* landed after the camera's
/// own grid rounded the request, so the residual jitter is written down rather
/// than assumed away.
///
/// This is the **body motion rate** — the grid annotated body keys are authored
/// on, sized against the camera's 8 fps survey pass. It is *not* the audio tick
/// rate: it stays 8 Hz deliberately, because a live rig annotated at 8 fps does
/// not gain anything from the audio digest's finer 32 Hz grid, and the audio
/// grid is `metal_schema::TickRate`'s to own. It is a subset of the canonical grid
/// (8 Hz ticks are every fourth 32 Hz tick), which is why the two never need to
/// be converted into each other.
pub const KEY_RATE: f64 = 8.0;

/// The nearest time on the keying grid.
pub fn snap(t: f64) -> f64 {
    (t * KEY_RATE).round() / KEY_RATE
}

/// Whether a key time lands on the grid, to within half a millisecond.
pub fn on_grid(t: f64) -> bool {
    (t - snap(t)).abs() < 5e-4
}

// ---------------------------------------------------------------------------
// Phrase
// ---------------------------------------------------------------------------

/// One movement, written as the joints it turns.
///
/// Joints are keyed by **name**, not by [`JointId`]. Ids are positions in an
/// array, and this rig's array has been rewritten four times — 75 joints, then
/// 140, then 142, then 36 — each time inserting or removing joints in the
/// middle. A file keyed by index would have survived every one of those edits
/// without erroring and meant something different afterwards.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Phrase {
    pub name: String,
    /// Free text for the annotator, not for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Joints whose rotation is written down over time, as a rotation vector
    /// relative to the joint's rest frame — see [`crate::pose::to_rotvec`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub turns: BTreeMap<String, Curve<Vec3>>,
    /// Where the body is, over time. Absent means this phrase does not say —
    /// so an arm gesture cannot accidentally restate where the dancer stands.
    ///
    /// The one thing here that is not a rotation, and it cannot be one: no
    /// amount of turning says the dancer crossed the floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub travel: Option<Curve<Vec3>>,
}

impl Phrase {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Writes a joint's rotation down.
    pub fn turn(mut self, joint: impl Into<String>, curve: Curve<Vec3>) -> Self {
        self.turns.insert(joint.into(), curve);
        self
    }

    pub fn travelling(mut self, curve: Curve<Vec3>) -> Self {
        self.travel = Some(curve);
        self
    }

    pub fn noted(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Whether this phrase says anything about a joint, by name.
    pub fn writes(&self, joint: &str) -> bool {
        self.turns.contains_key(joint)
    }

    /// Every time this phrase has a key, sorted and deduplicated.
    pub fn key_times(&self) -> Vec<f64> {
        let turns = self.turns.values().flat_map(|c| c.keys().iter().map(|k| k.t));
        let travel = self
            .travel
            .iter()
            .flat_map(|c| c.keys().iter().map(|k| k.t));
        let mut times: Vec<f64> = turns.chain(travel).collect();
        times.sort_by(f64::total_cmp);
        times.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        times
    }

    pub fn span(&self) -> Option<(f64, f64)> {
        let times = self.key_times();
        Some((*times.first()?, *times.last()?))
    }

    /// The same movement on the reflected side of the body.
    ///
    /// The joint names swap and every rotation is conjugated — which on a
    /// rotation vector is two sign flips ([`mirror_turn`]). Travel reflects on
    /// x, because it is a place and not a rotation.
    pub fn mirrored(&self) -> Self {
        let flip = |c: &Curve<Vec3>, f: fn(Vec3) -> Vec3| {
            Curve::from_keys(
                c.keys()
                    .iter()
                    .map(|k| Key::with(k.t, f(k.value), k.interp))
                    .collect(),
            )
        };
        Self {
            name: self.name.clone(),
            note: self.note.clone(),
            turns: self
                .turns
                .iter()
                .map(|(name, curve)| {
                    (
                        mirrored_name(name).unwrap_or_else(|| name.clone()),
                        flip(curve, mirror_turn),
                    )
                })
                .collect(),
            travel: self
                .travel
                .as_ref()
                .map(|c| flip(c, |v| Vec3::new(-v.x, v.y, v.z))),
        }
    }
}

// ---------------------------------------------------------------------------
// Motion
// ---------------------------------------------------------------------------

/// A stack of phrases over a posture.
///
/// Later phrases win per joint, so a whole-body base can be overridden on a
/// limb. The layering is at joint granularity rather than regional, because
/// which joints a movement is about is not a regional question.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Motion {
    pub name: String,
    /// The posture every unwritten joint holds.
    #[serde(default)]
    pub from: Figure,
    #[serde(default, rename = "phrase", skip_serializing_if = "Vec::is_empty")]
    pub phrases: Vec<Phrase>,
}

impl Motion {
    pub fn new(name: impl Into<String>, from: Figure) -> Self {
        Self {
            name: name.into(),
            from,
            phrases: Vec::new(),
        }
    }

    pub fn then(mut self, phrase: Phrase) -> Self {
        self.phrases.push(phrase);
        self
    }

    pub fn span(&self) -> Option<(f64, f64)> {
        self.phrases
            .iter()
            .filter_map(Phrase::span)
            .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)))
    }

    /// The pose at `t`: the base figure, with every phrase applied in order.
    pub fn pose_at(&self, skeleton: &Skeleton, t: f64) -> Pose {
        let mut out = self.from.pose(skeleton);
        self.pose_into(skeleton, t, &mut out);
        out
    }

    /// [`Motion::pose_at`] onto a caller-owned pose, which **must** already
    /// hold the base posture — that is what silence resolves to, and this
    /// method never writes it, so scrubbing a timeline does not rebuild the
    /// figure on every frame.
    ///
    /// Travel first, because travel places the body and everything under it
    /// hangs off that placement.
    ///
    /// A curve and a tick now say the same kind of thing — a rotation, measured
    /// the same way — so there is no conversion here any more. There used to be
    /// one, and it was the seam where the authored form and the baked form
    /// could disagree about what a declaration meant.
    pub fn pose_into(&self, skeleton: &Skeleton, t: f64, out: &mut Pose) {
        for phrase in &self.phrases {
            if let Some(root) = phrase.travel.as_ref().and_then(|c| c.sample(t)) {
                out.root = root;
            }
            let turns: Vec<(JointId, Vec3)> = phrase
                .turns
                .iter()
                .filter_map(|(name, curve)| Some((skeleton.find(name)?, curve.sample(t)?)))
                .collect();
            out.apply(&turns);
        }
    }

    /// Samples onto a fixed grid. The bridge to everything dense: contact
    /// checks, skinning, the viewer, a model's input.
    ///
    /// Note what happens to the rate here — nothing about the stored motion
    /// knows or cares. A rate is chosen at the moment something has to look at
    /// frames, and never before.
    pub fn bake(&self, skeleton: &Skeleton, fps: f64) -> Clip {
        let (start, end) = self.span().unwrap_or((0.0, 0.0));
        let count = ((end - start) * fps).round().max(0.0) as usize + 1;
        let base = self.from.pose(skeleton);
        let mut frames = Vec::with_capacity(count);
        for i in 0..count {
            let mut frame = base.clone();
            self.pose_into(skeleton, start + i as f64 / fps, &mut frame);
            frames.push(frame);
        }
        Clip {
            name: self.name.clone(),
            fps,
            frames,
        }
    }

    /// The joints this motion writes a rotation for, in rig order.
    ///
    /// The coverage gate, and it is one list now rather than a three-way audit.
    /// The number worth reading is how much of the rig is *missing* from it: a
    /// phrase that writes three joints is an isolation, and a phrase that
    /// writes all thirty-six either moved everything or was written by someone
    /// filling in boxes.
    pub fn writes(&self, skeleton: &Skeleton) -> Vec<JointId> {
        let mut ids: Vec<JointId> = self
            .phrases
            .iter()
            .flat_map(|p| p.turns.keys())
            .filter_map(|name| skeleton.find(name))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// The whole motion reflected across the sagittal plane.
    ///
    /// The base figure is carried across unchanged because all four reference
    /// postures are sagittally symmetric by construction — [`Figure`] builds
    /// the left side and fills the right from [`Pose::mirrored`]. A figure that
    /// stopped being symmetric would have to be mirrored here too.
    pub fn mirrored(&self) -> Self {
        Self {
            name: format!("{}.mirrored", self.name),
            from: self.from,
            phrases: self.phrases.iter().map(Phrase::mirrored).collect(),
        }
    }

    /// Checks the motion against the rig and against itself.
    ///
    /// Every issue here is a way for a sparse record to be *quietly* wrong,
    /// which is the price of not writing every joint down: a dense pose that
    /// names a joint that does not exist fails to load, and a sparse one that
    /// does simply omits a limb from the movement.
    ///
    /// Three of the six checks this used to run are gone with the mechanism
    /// they policed — `GivenAndCarried`, `NothingCarriesIt` and
    /// `NothingMovesIt` were all ways of declaring something the chain solver
    /// could not honour, and there is no chain solver. What replaces them is
    /// one check the rotation form makes possible: a rotation written the long
    /// way round.
    pub fn lint(&self, skeleton: &Skeleton) -> Vec<Issue> {
        let mut issues = Vec::new();

        for phrase in &self.phrases {
            let at = |joint: &str| (phrase.name.clone(), joint.to_string());

            for (name, curve) in &phrase.turns {
                let Some(id) = skeleton.find(name) else {
                    let (phrase, joint) = at(name);
                    issues.push(Issue::UnknownJoint { phrase, joint });
                    continue;
                };
                // A joint's rotation places its children, so a terminal has
                // nothing to place. This is the rotation form's own way of
                // being quietly ignored, and it replaces the three checks that
                // policed the chain solver.
                if skeleton.is_terminal(id) {
                    let (phrase, joint) = at(name);
                    issues.push(Issue::MovesNothing { phrase, joint });
                }
                let keys = curve.keys();
                if keys.len() > 1 && keys.iter().all(|k| k.value.abs_diff_eq(keys[0].value, 1e-6)) {
                    let (phrase, joint) = at(name);
                    issues.push(Issue::Motionless { phrase, joint });
                }
                // Past π the rotation vector describes the long way round to a
                // place the short way already reaches, and interpolating
                // through it walks the joint backwards. `to_rotvec` never emits
                // one, so this only ever arrives hand-written.
                if let Some(k) = keys.iter().find(|k| k.value.length() > std::f32::consts::PI + 1e-4)
                {
                    let (phrase, joint) = at(name);
                    issues.push(Issue::TheLongWayRound {
                        phrase,
                        joint,
                        radians: k.value.length(),
                    });
                }
            }

            for t in phrase.key_times() {
                if !on_grid(t) {
                    issues.push(Issue::OffTheGrid {
                        phrase: phrase.name.clone(),
                        t,
                    });
                }
            }
        }
        issues
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Issue {
    /// A name the rig does not have. Silently drops a joint from the movement.
    UnknownJoint { phrase: String, joint: String },
    /// A rotation written on a joint with no children. A joint's rotation
    /// places its children, so this one moves no point on the body.
    MovesNothing { phrase: String, joint: String },
    /// A curve that never leaves its first value. Either a wasted declaration
    /// or a deliberate hold, and the two want different fixes.
    Motionless { phrase: String, joint: String },
    /// A rotation past π: the long way round to somewhere the short way
    /// already reaches.
    TheLongWayRound {
        phrase: String,
        joint: String,
        radians: f32,
    },
    /// A key between grid times, so no extracted frame sits under it.
    OffTheGrid { phrase: String, t: f64 },
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Issue::UnknownJoint { phrase, joint } => {
                write!(f, "{phrase}: no joint named {joint}")
            }
            Issue::MovesNothing { phrase, joint } => write!(
                f,
                "{phrase}: {joint} has nothing hanging off it, so turning it moves nothing"
            ),
            Issue::Motionless { phrase, joint } => {
                write!(f, "{phrase}: {joint} is keyed and never turns")
            }
            Issue::TheLongWayRound {
                phrase,
                joint,
                radians,
            } => write!(
                f,
                "{phrase}: {joint} turns {radians:.2} rad, the long way round"
            ),
            Issue::OffTheGrid { phrase, t } => {
                write!(f, "{phrase}: key at {t:.4}s is off the {KEY_RATE} Hz grid")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::Interp;
    use crate::pose::to_rotvec;
    use crate::rig;
    use glam::Quat;

    /// A joint that starts unturned and ends at `to`.
    fn swing(to: Vec3) -> Curve<Vec3> {
        Curve::from_keys(vec![Key::new(0.0, Vec3::ZERO), Key::new(0.5, to)])
    }

    fn at(s: &Skeleton, p: &Pose, name: &str) -> Vec3 {
        p.points(s)[s.find(name).unwrap() as usize]
    }

    /// The headline case, and the one that got *shorter*. Raising an arm used
    /// to be a declared fingertip plus three carried joints resolved by a
    /// FABRIK solve; it is a rotation on the shoulder, and the arm below it
    /// comes along because forward kinematics is what a skeleton does.
    #[test]
    fn turning_one_joint_carries_everything_below_it() {
        let s = rig::canonical();
        let motion = Motion::new("à la seconde", Figure::Standing).then(
            Phrase::new("open").turn("l_shoulder", swing(Vec3::new(0.0, 0.0, 1.45))),
        );
        assert!(motion.lint(&s).is_empty(), "{:?}", motion.lint(&s));

        let posed = motion.pose_at(&s, 0.5);
        let base = Figure::Standing.pose(&s);
        let moved = |n: &str| at(&s, &base, n).distance(at(&s, &posed, n));

        // The arm went up and everything on it went with it, further the
        // further out it is.
        assert!(moved("l_elbow") > 0.20, "the elbow only moved {:.3} m", moved("l_elbow"));
        assert!(moved("l_finger_tip") > moved("l_wrist"));
        assert!(moved("l_wrist") > moved("l_elbow"));
        // And one joint's rotation is the whole record of it.
        assert_eq!(motion.writes(&s).len(), 1);
    }

    /// The isolation. One joint moves; the rest of the body is not mentioned
    /// and does not move by so much as a float's worth.
    #[test]
    fn an_isolated_arm_leaves_the_rest_of_the_body_exactly_where_it_was() {
        let s = rig::canonical();
        let motion = Motion::new("port de bras", Figure::Standing)
            .then(Phrase::new("lift").turn("l_shoulder", swing(Vec3::new(0.0, 0.0, 0.6))));

        let before = Figure::Standing.pose(&s).points(&s);
        let after = motion.pose_at(&s, 0.5).points(&s);
        let limb = s.subtree(s.find("l_shoulder").unwrap());

        let mut moved = 0;
        for j in 0..s.len() as JointId {
            let d = before[j as usize].distance(after[j as usize]);
            if limb.contains(&j) {
                moved += usize::from(d > 1e-4);
            } else {
                assert!(
                    d < 1e-6,
                    "{} moved {d:.6} m and the phrase never mentions it",
                    s.joints[j as usize].name
                );
            }
        }
        assert!(moved >= 5, "the arm barely moved: {moved} joints");
    }

    /// A wave down the column: four hinges, four rotations, each late on the
    /// one above it. Under the position notation this was one declared vertebra
    /// and twenty-three carried ones resolved by a chain walk, and the shape of
    /// the wave was whatever the solver produced.
    #[test]
    fn a_wave_passes_through_the_column_one_hinge_at_a_time() {
        let s = rig::canonical();
        let ripple = |late: f64| {
            Curve::from_keys(vec![
                Key::new(late, Vec3::ZERO),
                Key::new(late + 0.25, Vec3::new(0.35, 0.0, 0.0)),
                Key::new(late + 0.5, Vec3::ZERO),
            ])
        };
        let motion = Motion::new("cambré", Figure::Standing).then(
            Phrase::new("roll")
                .turn("vert_l3", ripple(0.0))
                .turn("vert_t10", ripple(0.125))
                .turn("vert_t2", ripple(0.25))
                .turn("vert_c4", ripple(0.375)),
        );
        assert!(motion.lint(&s).is_empty(), "{:?}", motion.lint(&s));

        // At 0.25 s the bottom of the column is at full bend and the top has
        // not started, which is what makes it a wave rather than a bow.
        let quarter = motion.pose_at(&s, 0.25);
        let turned = |p: &Pose, n: &str| p.turned(s.find(n).unwrap()).length();
        assert!(turned(&quarter, "vert_l3") > 0.3, "the low back never bent");
        assert!(turned(&quarter, "vert_c4") < 1e-6, "the neck moved too early");
        // And by 0.625 s it has passed: the neck is bent and the low back is
        // back where it started.
        let late = motion.pose_at(&s, 0.625);
        assert!(turned(&late, "vert_c4") > 0.3, "the wave never reached the neck");
        assert!(turned(&late, "vert_l3") < 1e-6, "the low back never released");

        assert_eq!(motion.writes(&s).len(), 4, "four hinges, four rotations");
    }

    /// Silence holds what was underneath. The anatomical palms stay forward
    /// through a phrase that only moves the head — which is the difference
    /// between "not mentioned" and "at rest", and the reason a dense pose
    /// cannot express an isolation.
    #[test]
    fn silence_holds_the_posture_underneath_rather_than_resetting_it() {
        let s = rig::canonical();
        let motion = Motion::new("look", Figure::Anatomical)
            .then(Phrase::new("turn").turn("head", swing(Vec3::new(0.0, 0.6, 0.0))));

        let base = Figure::Anatomical.pose(&s);
        let after = motion.pose_at(&s, 0.5);
        let hand = s.find("l_finger_tip").unwrap() as usize;
        assert!(base.points(&s)[hand].distance(after.points(&s)[hand]) < 1e-6);
        // And it is genuinely the anatomical hand, not the rest hand.
        assert!(Pose::rest(&s).points(&s)[hand].distance(after.points(&s)[hand]) > 0.1);
        let jaw = s.find("jaw").unwrap() as usize;
        assert!(base.points(&s)[jaw].distance(after.points(&s)[jaw]) > 0.02);
    }

    #[test]
    fn a_phrase_that_does_not_travel_leaves_the_body_where_it_stood() {
        let s = rig::canonical();
        let motion = Motion::new("gesture", Figure::Standing)
            .then(Phrase::new("reach").turn("r_shoulder", swing(Vec3::new(0.0, 0.0, -0.5))));
        assert_eq!(motion.pose_at(&s, 0.5).root, Figure::Standing.pose(&s).root);

        let walked = Motion::new("walk", Figure::Standing).then(
            Phrase::new("cross").travelling(Curve::from_keys(vec![
                Key::new(0.0, Vec3::new(0.0, 0.96, 0.0)),
                Key::new(1.0, Vec3::new(1.0, 0.96, 0.0)),
            ])),
        );
        assert!((walked.pose_at(&s, 1.0).root.x - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_later_phrase_overrides_an_earlier_one_joint_by_joint() {
        let s = rig::canonical();
        let motion = Motion::new("stack", Figure::Standing)
            .then(
                Phrase::new("base")
                    .turn("l_elbow", swing(Vec3::new(0.0, 0.4, 0.0)))
                    .turn("r_elbow", swing(Vec3::new(0.0, 0.4, 0.0))),
            )
            .then(Phrase::new("correction").turn("l_elbow", swing(Vec3::new(0.0, 1.2, 0.0))));

        let p = motion.pose_at(&s, 0.5);
        assert!((p.turned(s.find("l_elbow").unwrap()).y - 1.2).abs() < 1e-5);
        assert!((p.turned(s.find("r_elbow").unwrap()).y - 0.4).abs() < 1e-5);
    }

    /// Mirroring is two signs on every rotation and a name swap — and no
    /// re-solve, because there is nothing to solve. A reflected motion is the
    /// reflection of the motion, exactly.
    #[test]
    fn mirroring_swaps_the_sides_and_conjugates_the_rotations() {
        let s = rig::canonical();
        let m = s.mirror();
        let motion = Motion::new("gesture", Figure::Standing)
            .then(Phrase::new("open").turn("l_shoulder", swing(Vec3::new(0.1, 0.4, 0.2))));

        let mirrored = motion.mirrored();
        let flipped = &mirrored.phrases[0].turns["r_shoulder"];
        assert_eq!(flipped.sample(0.5), Some(Vec3::new(0.1, -0.4, -0.2)));
        assert!(!mirrored.phrases[0].turns.contains_key("l_shoulder"));

        let direct = motion.pose_at(&s, 0.5).mirrored(&m).points(&s);
        let through = mirrored.pose_at(&s, 0.5).points(&s);
        for j in 0..s.len() {
            assert!(
                direct[j].distance(through[j]) < 1e-5,
                "{} differs by {:.6} m",
                s.joints[j].name,
                direct[j].distance(through[j])
            );
        }
    }

    #[test]
    fn baking_chooses_a_rate_that_the_stored_motion_never_had() {
        let s = rig::canonical();
        let want = Vec3::new(0.0, 0.0, 1.1);
        let motion = Motion::new("swing", Figure::Standing)
            .then(Phrase::new("arc").turn("l_shoulder", swing(want)));

        for fps in [8.0, 24.0, 30.0, 60.0] {
            let clip = motion.bake(&s, fps);
            assert!((clip.duration() - 0.5).abs() < 1e-9, "{fps} fps");
            assert_eq!(clip.frames.len(), (0.5 * fps) as usize + 1);
            let last = clip.frames.last().unwrap();
            assert!(
                last.turned(s.find("l_shoulder").unwrap()).abs_diff_eq(want, 1e-5),
                "{fps} fps landed on {:?}",
                last.turned(s.find("l_shoulder").unwrap())
            );
        }
    }

    /// A rotation vector interpolates exactly for a turn about one axis, which
    /// is what most keys are — an elbow bends, a knee bends, a spine tilts. It
    /// is an approximation only for a compound turn, and this is the test that
    /// says which of the two is being relied on.
    #[test]
    fn a_turn_about_one_axis_interpolates_exactly() {
        let s = rig::canonical();
        let motion = Motion::new("bend", Figure::Standing)
            .then(Phrase::new("p").turn("l_elbow", swing(Vec3::new(0.0, 1.6, 0.0))));
        let mid = motion.pose_at(&s, 0.25);
        let want = Quat::from_rotation_y(0.8);
        assert!(
            mid.local[s.find("l_elbow").unwrap() as usize].abs_diff_eq(want, 1e-5),
            "halfway through a 1.6 rad bend is not 0.8 rad"
        );
    }

    #[test]
    fn the_coverage_report_is_the_joints_the_motion_speaks_for() {
        let s = rig::canonical();
        let motion = Motion::new("wave", Figure::Standing).then(
            Phrase::new("shake")
                .turn("l_wrist", swing(Vec3::new(0.0, 0.3, 0.0)))
                .turn("l_fingers", swing(Vec3::new(0.0, 0.0, -0.9)))
                .turn("l_thumb", swing(Vec3::new(0.0, 0.4, 0.0))),
        );
        let written = motion.writes(&s);
        assert_eq!(written.len(), 3);
        assert!(written.contains(&s.find("l_fingers").unwrap()));
        assert!(!written.contains(&s.find("r_wrist").unwrap()));
        assert_eq!(s.len() - written.len(), 33, "the rest of the body is unspoken for");
    }

    #[test]
    fn the_lint_catches_every_way_a_sparse_record_goes_quiet() {
        let s = rig::canonical();
        let motion = Motion::new("broken", Figure::Standing).then(
            Phrase::new("p")
                .turn("l_shoulder", swing(Vec3::new(0.0, 0.0, 0.6)))
                .turn("tail", swing(Vec3::X))
                .turn(
                    "r_elbow",
                    Curve::from_keys(vec![Key::new(0.0, Vec3::ZERO), Key::new(0.25, Vec3::ZERO)]),
                )
                .turn("l_knee", swing(Vec3::new(0.0, 4.0, 0.0)))
                .turn("l_finger_tip", swing(Vec3::new(0.0, 0.3, 0.0))),
        );

        let issues = motion.lint(&s);
        let has = |want: &Issue| issues.contains(want);
        assert!(has(&Issue::UnknownJoint {
            phrase: "p".into(),
            joint: "tail".into()
        }));
        // A fingertip has nothing hanging off it, so turning it does nothing.
        assert!(has(&Issue::MovesNothing {
            phrase: "p".into(),
            joint: "l_finger_tip".into()
        }));
        assert!(!has(&Issue::MovesNothing {
            phrase: "p".into(),
            joint: "l_shoulder".into()
        }));
        assert!(has(&Issue::Motionless {
            phrase: "p".into(),
            joint: "r_elbow".into()
        }));
        assert!(has(&Issue::TheLongWayRound {
            phrase: "p".into(),
            joint: "l_knee".into(),
            radians: 4.0
        }));
        // …and the one that is written properly is not reported.
        assert!(!issues.iter().any(|i| matches!(
            i,
            Issue::Motionless { joint, .. } if joint == "l_shoulder"
        )));
    }

    /// The check the long-way-round lint exists for: `to_rotvec` cannot emit
    /// one, so the encoding is closed under everything the pipeline produces.
    #[test]
    fn nothing_the_pipeline_produces_is_ever_the_long_way_round() {
        for angle in [0.1, 1.5, 3.0, 4.0, 6.0, 7.0] {
            let v = to_rotvec(Quat::from_rotation_y(angle));
            assert!(v.length() <= std::f32::consts::PI + 1e-6, "{angle} rad became {v:?}");
        }
    }

    #[test]
    fn keys_must_land_on_the_keying_grid() {
        let s = rig::canonical();
        let off = Motion::new("drifted", Figure::Standing).then(
            Phrase::new("p").turn(
                "l_shoulder",
                Curve::from_keys(vec![
                    Key::new(0.0, Vec3::ZERO),
                    Key::with(0.137, Vec3::new(0.0, 0.4, 0.0), Interp::Linear),
                ]),
            ),
        );
        assert!(matches!(
            off.lint(&s).as_slice(),
            [Issue::OffTheGrid { t, .. }] if (*t - 0.137).abs() < 1e-9
        ));

        for i in 0..16 {
            assert!(on_grid(i as f64 / KEY_RATE));
        }
        assert!((snap(0.137) - 0.125).abs() < 1e-9);
    }

    #[test]
    fn an_empty_motion_is_the_posture_it_departs_from() {
        let s = rig::canonical();
        let motion = Motion::new("still", Figure::Supine);
        assert_eq!(motion.span(), None);
        assert_eq!(motion.bake(&s, KEY_RATE).frames.len(), 1);
        assert!(motion.writes(&s).is_empty());
    }

    #[test]
    fn a_phrase_round_trips_through_toml_by_name() {
        let s = rig::canonical();
        let motion = Motion::new("gesture", Figure::Standing).then(
            Phrase::new("open")
                .noted("the hand rides the wrist")
                .turn("l_shoulder", swing(Vec3::new(0.0, 0.0, 1.2)))
                .turn("l_elbow", swing(Vec3::new(0.0, 0.7, 0.0))),
        );

        let text = toml::to_string(&motion).expect("serialize");
        assert!(text.contains("l_shoulder"), "{text}");
        // A joint is one entry with one curve on it. There is no second table
        // of carried names beside it any more.
        assert!(!text.contains("carried"), "{text}");
        let back: Motion = toml::from_str(&text).expect("deserialize");
        assert_eq!(back.phrases[0].turns.len(), 2);
        let a = motion.pose_at(&s, 0.375);
        let b = back.pose_at(&s, 0.375);
        for (x, y) in a.local.iter().zip(&b.local) {
            assert!(x.abs_diff_eq(*y, 1e-6));
        }
    }
}
