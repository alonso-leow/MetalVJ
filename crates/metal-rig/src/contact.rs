//! Contact: where the body touches the world.
//!
//! Standing, the two feet are on the floor and nothing else is. That is a
//! default, not a rule — in floor work a shoulder, a spine, a forearm, the back
//! of a hand can all be bearing weight, and a notation that can only describe
//! feet cannot describe half of contemporary dance.
//!
//! **Contact is a different kind of channel from style and difficulty.**
//! Those two are *declared*: the performer says what they intended, and no
//! measurement can contradict them. Contact is *observed*: a joint is either
//! at the surface or it is not, and the geometry can be asked. Two
//! consequences follow, and both are why this lives in `metal-rig` beside
//! the pose rather than beside the declared channels it used to sit with:
//!
//! 1. **It is genuinely per joint.** Declared style/difficulty are not —
//!    there is no idiom in an elbow independent of the forearm, which is why
//!    those bind to regions. But an elbow can be on the floor while the
//!    wrist is not, so contact is the one channel where per-node is the
//!    honest granularity rather than false precision.
//! 2. **It is a free correctness check**, in exactly the way bone lengths
//!    already are. A joint labelled in contact that reconstructs 40 mm above
//!    the floor means the triangulation is wrong, and it can be caught without
//!    ground truth. See [`Contact::off_surface`] and [`Contact::sliding`] —
//!    the second is how foot-skate is found, which is the classic artefact of
//!    a reconstruction that looks fine frame by frame.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::curve::Curve;
use crate::pose::{Clip, Pose};
use crate::skeleton::{JointId, Mirror, Skeleton};

/// The name of a surface. One stage has a floor; it may also have a rostrum, a
/// wall, or a chair, and a joint says which one it is touching.
pub type SurfaceId = String;

/// The default surface every stage has.
pub const FLOOR: &str = "floor";

/// A horizontal plane the body can rest on, in stage coordinates.
///
/// Deliberately only a height: a raked stage or a prop with a slope is a real
/// thing and not yet a thing here, and a plane that pretends to more generality
/// than it has would be worse than one that says what it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub name: SurfaceId,
    /// Metres above the stage floor. The floor itself is `0.0`.
    pub height: f32,
}

impl Surface {
    pub fn floor() -> Self {
        Self {
            name: FLOOR.to_owned(),
            height: 0.0,
        }
    }
}

/// Whether one joint is touching a surface, over time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub joint: JointId,
    #[serde(default = "default_surface")]
    pub surface: SurfaceId,
    pub touching: Curve<bool>,
    /// Share of the body's weight borne here, `0..=1`, when it is known.
    ///
    /// Separate from contact because touching and bearing are different: a
    /// fingertip steadying a balance is in contact and carrying almost nothing,
    /// and a handstand is the same joint carrying everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<Curve<f32>>,
}

fn default_surface() -> SurfaceId {
    FLOOR.to_owned()
}

impl Track {
    /// A joint that is touching the floor for the whole clip.
    pub fn always(joint: JointId) -> Self {
        Self {
            joint,
            surface: default_surface(),
            touching: Curve::constant(true),
            load: None,
        }
    }

    pub fn touching_at(&self, t: f64) -> bool {
        self.touching.sample(t).unwrap_or(false)
    }

    pub fn load_at(&self, t: f64) -> f32 {
        self.load
            .as_ref()
            .and_then(|c| c.sample(t))
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    }
}

/// Every contact in one clip.
///
/// Sparse: joints that never touch anything are absent, because in most
/// choreography that is 130 of the rig's 142 joints, and storing a false curve
/// for each of them would bury the record in a claim about nothing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    /// Sorted by joint, so a serialized record diffs cleanly and two records
    /// built in different orders compare equal.
    #[serde(default, rename = "track")]
    tracks: Vec<Track>,
}

impl Contact {
    pub fn new() -> Self {
        Self::default()
    }

    /// The default: standing, both soles down, nothing else touching.
    ///
    /// This is where every take starts and what most of them stay as. Anything
    /// else — a roll, a slide, a lift — is authored by adding joints to it.
    ///
    /// The sole is the heel and the ball
    /// ([`crate::rig::SOLE`]), not every node that happens to rest on the
    /// floor. The toe touches in a flat stance and carries nothing, and a default
    /// that declares them makes `off_surface` report a dozen joints the moment
    /// a dancer rocks onto the ball of one foot — which is the check crying
    /// wolf, not a reconstruction going wrong.
    pub fn standing(skeleton: &Skeleton) -> Self {
        let mut contact = Self::new();
        for side in ["l", "r"] {
            for part in crate::rig::SOLE {
                if let Some(id) = skeleton.find(&format!("{side}_{part}")) {
                    contact.set(Track::always(id));
                }
            }
        }
        contact
    }

    /// Adds a track, replacing any existing one for the same joint.
    pub fn set(&mut self, track: Track) -> &mut Self {
        match self.index_of(track.joint) {
            Ok(i) => self.tracks[i] = track,
            Err(i) => self.tracks.insert(i, track),
        }
        self
    }

    pub fn remove(&mut self, joint: JointId) -> Option<Track> {
        self.index_of(joint).ok().map(|i| self.tracks.remove(i))
    }

    pub fn track(&self, joint: JointId) -> Option<&Track> {
        self.index_of(joint).ok().map(|i| &self.tracks[i])
    }

    pub fn track_mut(&mut self, joint: JointId) -> Option<&mut Track> {
        self.index_of(joint).ok().map(|i| &mut self.tracks[i])
    }

    fn index_of(&self, joint: JointId) -> Result<usize, usize> {
        self.tracks.binary_search_by_key(&joint, |c| c.joint)
    }

    pub fn joints(&self) -> impl Iterator<Item = JointId> + '_ {
        self.tracks.iter().map(|c| c.joint)
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Whether one joint is touching anything at `t`.
    pub fn touching(&self, joint: JointId, t: f64) -> bool {
        self.track(joint).is_some_and(|c| c.touching_at(t))
    }

    /// Every joint touching something at `t`, in joint order.
    pub fn touching_at(&self, t: f64) -> Vec<JointId> {
        self.tracks
            .iter()
            .filter(|c| c.touching_at(t))
            .map(|c| c.joint)
            .collect()
    }

    /// Contact reflected across the sagittal plane.
    ///
    /// Required whenever a pose is mirrored: a mirrored dancer with unmirrored
    /// contact is standing on the wrong foot, and every check below would then
    /// confirm it.
    pub fn mirrored(&self, mirror: &Mirror) -> Self {
        let mut out = Self::new();
        for track in &self.tracks {
            out.set(Track {
                joint: mirror.partner(track.joint),
                ..track.clone()
            });
        }
        out
    }

    /// Joints claimed to be touching that are not where the surface is.
    ///
    /// Returns the signed gap in metres — positive is floating, negative is
    /// through the floor — for every joint that misses by more than
    /// `tolerance`. An empty result is a real check passed, not an absent one.
    pub fn off_surface(
        &self,
        pose: &Pose,
        skeleton: &Skeleton,
        surfaces: &[Surface],
        t: f64,
        tolerance: f32,
    ) -> Vec<(JointId, f32)> {
        let points = pose.points(skeleton);
        let mut out = Vec::new();
        for track in &self.tracks {
            if !track.touching_at(t) {
                continue;
            }
            let Some(surface) = surfaces.iter().find(|s| s.name == track.surface) else {
                continue;
            };
            let gap = points[track.joint as usize].y - surface.height;
            if gap.abs() > tolerance {
                out.push((track.joint, gap));
            }
        }
        out
    }

    /// Joints that slide while claiming to be planted.
    ///
    /// A joint in contact across two consecutive frames should not travel
    /// horizontally. When it does, either the contact label is wrong or the
    /// reconstruction is — and this is the check that finds foot-skate, which
    /// no single frame can reveal because every individual frame looks fine.
    ///
    /// Returns the worst horizontal travel per joint, in metres, over any pair
    /// of consecutive frames where it was continuously in contact.
    pub fn sliding(
        &self,
        clip: &Clip,
        skeleton: &Skeleton,
        tolerance: f32,
    ) -> Vec<(JointId, f32)> {
        let mut worst: BTreeMap<JointId, f32> = BTreeMap::new();
        for i in 1..clip.frames.len() {
            let (t0, t1) = ((i - 1) as f64 / clip.fps, i as f64 / clip.fps);
            let (a, b) = (
                clip.frames[i - 1].points(skeleton),
                clip.frames[i].points(skeleton),
            );
            for track in &self.tracks {
                if !(track.touching_at(t0) && track.touching_at(t1)) {
                    continue;
                }
                let (p, q) = (a[track.joint as usize], b[track.joint as usize]);
                let travel = ((q.x - p.x).powi(2) + (q.z - p.z).powi(2)).sqrt();
                let entry = worst.entry(track.joint).or_insert(0.0);
                *entry = entry.max(travel);
            }
        }
        worst
            .into_iter()
            .filter(|(_, travel)| *travel > tolerance)
            .collect()
    }

    /// Total declared load at `t`. Meaningfully different from `1.0` is worth
    /// a look: a body wholly in the air sums to zero, and a body standing sums
    /// to one, but only where load was actually annotated.
    pub fn total_load(&self, t: f64) -> f32 {
        self.tracks.iter().map(|c| c.load_at(t)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::{Interp, Key};
    use crate::rig;
    use glam::Quat;

    fn intervals(spans: &[(f64, bool)]) -> Curve<bool> {
        Curve::from_keys(
            spans
                .iter()
                .map(|(t, v)| Key::with(*t, *v, Interp::Hold))
                .collect(),
        )
    }

    #[test]
    fn standing_puts_the_soles_down_and_nothing_else() {
        let s = rig::canonical();
        let c = Contact::standing(&s);
        assert_eq!(c.touching_at(0.0).len(), 2 * rig::SOLE.len());
        assert!(c.touching(s.find("l_heel").unwrap(), 0.0));
        assert!(c.touching(s.find("r_ball").unwrap(), 0.0));
        // The toe rests on the floor but is not declared; see `standing`.
        assert!(!c.touching(s.find("l_toe").unwrap(), 0.0));
        assert!(!c.touching(s.find("l_wrist").unwrap(), 0.0));
        assert!(!c.touching(s.find("vert_t10").unwrap(), 0.0));
    }

    #[test]
    fn any_joint_can_take_the_floor() {
        // A shoulder roll: the feet leave, the shoulder and spine arrive.
        let s = rig::canonical();
        let mut c = Contact::standing(&s);
        for id in c.joints().collect::<Vec<_>>() {
            c.track_mut(id).unwrap().touching = intervals(&[(0.0, true), (1.0, false)]);
        }
        for name in ["l_shoulder", "vert_t10"] {
            c.set(
                Track {
                    joint: s.find(name).unwrap(),
                    surface: FLOOR.into(),
                    touching: intervals(&[(0.0, false), (1.0, true)]),
                    load: None,
                },
            );
        }

        assert_eq!(
            c.touching_at(0.0).len(),
            2 * rig::SOLE.len(),
            "standing at the start"
        );
        let rolled = c.touching_at(1.5);
        assert_eq!(rolled.len(), 2, "on the floor at the end");
        assert!(rolled.contains(&s.find("vert_t10").unwrap()));
    }

    #[test]
    fn contact_holds_and_cuts_rather_than_fading() {
        let c = intervals(&[(0.0, false), (2.0, true)]);
        assert!(!c.sample(1.999).unwrap(), "there is no half-contact");
        assert!(c.sample(2.0).unwrap());
    }

    #[test]
    fn a_planted_foot_that_floats_is_caught() {
        let s = rig::canonical();
        let c = Contact::standing(&s);
        let surfaces = [Surface::floor()];

        // The rest pose stands on the floor.
        let resting = Pose::rest(&s);
        assert!(c
            .off_surface(&resting, &s, &surfaces, 0.0, 0.03)
            .is_empty());

        // Lift the whole body 10 cm and the planted feet are a lie.
        let mut floating = resting.clone();
        floating.root.y += 0.10;
        let bad = c.off_surface(&floating, &s, &surfaces, 0.0, 0.03);
        assert_eq!(bad.len(), 2 * rig::SOLE.len(), "{bad:?}");
        assert!(bad.iter().all(|(_, gap)| (*gap - 0.10).abs() < 1e-4));
    }

    #[test]
    fn a_joint_through_the_floor_reports_a_negative_gap() {
        let s = rig::canonical();
        let c = Contact::standing(&s);
        let mut sunk = Pose::rest(&s);
        sunk.root.y -= 0.08;
        let bad = c.off_surface(&sunk, &s, &[Surface::floor()], 0.0, 0.03);
        assert!(bad.iter().all(|(_, gap)| *gap < 0.0), "{bad:?}");
    }

    #[test]
    fn a_joint_that_is_not_touching_is_not_checked() {
        // Mid-jump: nothing is claimed to be down, so nothing can be wrong.
        let s = rig::canonical();
        let mut c = Contact::standing(&s);
        for id in c.joints().collect::<Vec<_>>() {
            c.track_mut(id).unwrap().touching = Curve::constant(false);
        }
        let mut airborne = Pose::rest(&s);
        airborne.root.y += 0.5;
        assert!(c
            .off_surface(&airborne, &s, &[Surface::floor()], 0.0, 0.03)
            .is_empty());
    }

    #[test]
    fn a_planted_foot_that_slides_is_caught() {
        let s = rig::canonical();
        let c = Contact::standing(&s);

        // The whole body translates while both feet claim to be planted.
        let a = Pose::rest(&s);
        let mut b = a.clone();
        b.root.x += 0.20;
        let skating = Clip {
            name: "skate".into(),
            fps: 30.0,
            frames: vec![a.clone(), b],
        };
        let slid = c.sliding(&skating, &s, 0.01);
        assert_eq!(slid.len(), 2 * rig::SOLE.len(), "{slid:?}");
        assert!(slid.iter().all(|(_, d)| (*d - 0.20).abs() < 1e-4));

        // Standing still does not slide.
        let still = Clip {
            name: "still".into(),
            fps: 30.0,
            frames: vec![a.clone(), a],
        };
        assert!(c.sliding(&still, &s, 0.01).is_empty());
    }

    #[test]
    fn lifting_a_foot_before_moving_it_does_not_count_as_sliding() {
        // A real step: the foot is released, travels, and lands. Contact is
        // false across the travel, so no slide is reported.
        let s = rig::canonical();
        let toe = s.find("l_ball").unwrap();
        let mut c = Contact::standing(&s);
        c.track_mut(toe).unwrap().touching =
            intervals(&[(0.0, true), (0.02, false), (0.09, true)]);

        let a = Pose::rest(&s);
        let mut b = a.clone();
        b.root.x += 0.4;
        let clip = Clip {
            name: "step".into(),
            fps: 30.0,
            frames: vec![a.clone(), a.clone(), b.clone(), b],
        };

        let slid = c.sliding(&clip, &s, 0.01);
        assert!(
            !slid.iter().any(|(j, _)| *j == toe),
            "the stepping toe was airborne while it travelled: {slid:?}"
        );
        // The other foot never lifted, so it is correctly reported.
        assert!(slid.iter().any(|(j, _)| *j == s.find("r_ball").unwrap()));
    }

    #[test]
    fn mirroring_moves_contact_to_the_other_side() {
        let s = rig::canonical();
        let m = s.mirror();
        let mut c = Contact::new();
        c.set(Track::always(s.find("l_wrist").unwrap()));

        let mirrored = c.mirrored(&m);
        assert!(mirrored.touching(s.find("r_wrist").unwrap(), 0.0));
        assert!(!mirrored.touching(s.find("l_wrist").unwrap(), 0.0));
        // And a mirrored pose still passes its own contact check.
        let pose = Pose::rest(&s).mirrored(&m);
        let standing = Contact::standing(&s).mirrored(&m);
        assert!(standing
            .off_surface(&pose, &s, &[Surface::floor()], 0.0, 0.03)
            .is_empty());
    }

    #[test]
    fn load_is_separate_from_touching() {
        let s = rig::canonical();
        let mut c = Contact::new();
        let wrist = s.find("l_wrist").unwrap();
        c.set(
            Track {
                joint: wrist,
                surface: FLOOR.into(),
                touching: Curve::constant(true),
                load: Some(Curve::constant(0.05)),
            },
        );
        // Touching, and carrying almost nothing — a steadying fingertip.
        assert!(c.touching(wrist, 0.0));
        assert!((c.total_load(0.0) - 0.05).abs() < 1e-6);
    }

    #[test]
    fn contact_survives_a_round_trip_through_toml() {
        let s = rig::canonical();
        let mut c = Contact::standing(&s);
        c.set(
            Track {
                joint: s.find("l_wrist").unwrap(),
                surface: "rostrum".into(),
                touching: intervals(&[(0.0, false), (1.5, true)]),
                load: Some(Curve::constant(0.4)),
            },
        );
        let text = toml::to_string(&c).unwrap();
        let back: Contact = toml::from_str(&text).unwrap();
        assert_eq!(back, c);
        assert_eq!(
            back.track(s.find("l_wrist").unwrap()).unwrap().surface,
            "rostrum"
        );
    }

    #[test]
    fn contact_does_not_disturb_the_pose_it_describes() {
        // Contact is an observation about a pose, never an edit to one.
        let s = rig::canonical();
        let mut p = Pose::rest(&s);
        p.local[s.find("l_knee").unwrap() as usize] = Quat::from_rotation_x(0.3);
        let before = p.clone();
        let c = Contact::standing(&s);
        let _ = c.off_surface(&p, &s, &[Surface::floor()], 0.0, 0.03);
        assert_eq!(p, before);
    }
}
