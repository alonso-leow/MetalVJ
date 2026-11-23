//! The canonical rig — Don Joe, model 12 of the suite: the joints a dance
//! notation can actually say something about.
//!
//! Axes are Y-up, Z-forward (the direction the dancer faces), X to the
//! dancer's left. Units are metres, proportioned for a 1.70 m body; retarget
//! by scaling `rest_offset` per joint, never by editing this table.
//!
//! # Thirty-six joints, cut down from a hundred and forty-two (2026-08-15)
//!
//! The rig used to be a node per bone, everywhere: twenty-four presacral
//! vertebrae, every phalanx of every finger and every toe, five metatarsal
//! heads per foot. The stated reason was that *"a rig that gives a finger three
//! joints and the entire lumbar spine one is not modelling a body, it is
//! modelling what was easy to type."* That reason was right about the
//! asymmetry and wrong about which way to fix it. the operator, cutting it:
//! *"the minimum needed to tell a point from a flex clearly."*
//!
//! The criterion is **what the notation has to be able to tell apart**, and for
//! the extremities that is a short list: pointed against flexed, extended
//! against gathered. Everything past it was resolution nobody could write and
//! no camera could check — the cost was already recorded here as *"two 1080p
//! cameras cannot resolve a toe phalanx"*, which is another way of saying that
//! ninety of these joints were going to be **posed from imagination and
//! recorded as if measured**.
//!
//! So:
//!
//! | region | was | is | what survives |
//! |---|---|---|---|
//! | column | 24 vertebrae | **4** | flex, arch, tilt, twist, and a wave that passes through four hinges |
//! | hand | 24 nodes | **4** | fingers extended against fingers gathered, and a thumb |
//! | foot | 21 nodes | **3** | heel, ball, toe — pointe, demi-pointe, flex |
//!
//! **The four spine joints are named for the level each one stands at**, not
//! for a made-up segment: `vert_l3` is mid-lumbar, `vert_t10` lower thoracic,
//! `vert_t2` upper thoracic (where the clavicles hang, at the manubrium), and
//! `vert_c4` mid-cervical. They are not four of twenty-four with twenty missing
//! — each is the one hinge its region gets, placed at that region's middle,
//! which is where a region bends.
//!
//! Two boundaries kept, and they are kept on their own evidence rather than by
//! inertia:
//!
//! - **Ribs are still absent.** They articulate breathing, not movement.
//! - **The jaw is still here, and it is not there to chew.** `head` and
//!   `head_top` both sit within 6 mm of the skull's own vertical axis, so
//!   turning the head moves *no point on the rig by a measurable distance* —
//!   head yaw would live in a quaternion nobody can triangulate, reproject, or
//!   compare between two annotation passes. The mandible is the one skull
//!   landmark far enough off-axis to make that rotation observable. The
//!   argument is geometric and does not get cheaper on a smaller rig, which is
//!   why cutting ninety joints did not cut this one.

use glam::{Quat, Vec3};

use crate::skeleton::{Joint, JointId, Side, Skeleton};

/// The two things a hand can be told to do: gather the fingers, or move the
/// thumb. Four fingers move as one unit — the distinction the notation needs is
/// extended against gathered, and a per-finger record was a record of what the
/// annotator imagined.
pub const DIGITS: [&str; 2] = ["fingers", "thumb"];

/// The parts of a foot that meet the floor in a flat stance: the calcaneus and
/// the ball. The toe rests on it too and carries nothing, and a default contact
/// set that declares every touching node turns `off_surface` into noise instead
/// of a check.
pub const SOLE: [&str; 2] = ["heel", "ball"];

struct Builder {
    joints: Vec<Joint>,
}

impl Builder {
    fn new() -> Self {
        Self {
            joints: vec![Joint {
                name: "pelvis".into(),
                parent: None,
                side: Side::Center,
                // Chosen so the rest pose puts the sole at exactly y = 0, not
                // near it. Contact checks measure against the floor plane, so a
                // rig that rests a centimetre underground turns every contact
                // test into a test with a built-in bias.
                rest_offset: Vec3::new(0.0, 0.96, 0.0),
                rest_rotation: Quat::IDENTITY,
            }],
        }
    }

    /// Appends a joint under `parent`, returning its id.
    fn add(&mut self, name: &str, parent: JointId, side: Side, offset: Vec3) -> JointId {
        let id = self.joints.len() as JointId;
        self.joints.push(Joint {
            name: name.into(),
            parent: Some(parent),
            side,
            rest_offset: offset,
            rest_rotation: Quat::IDENTITY,
        });
        id
    }

    /// Adds a chain of joints, each parented to the previous one.
    fn chain(&mut self, root: JointId, side: Side, links: &[(&str, Vec3)]) -> JointId {
        let mut parent = root;
        for (name, offset) in links {
            parent = self.add(name, parent, side, *offset);
        }
        parent
    }
}

/// Mirrors an offset across the sagittal plane for the right side.
fn mirror(v: Vec3, side: Side) -> Vec3 {
    match side {
        Side::Right => Vec3::new(-v.x, v.y, v.z),
        _ => v,
    }
}

// ------------------------------------------------------------------- column

/// The four hinges of the vertebral column, and the skull on top of them.
///
/// Each entry is `(clinical level, rise from the joint below, sagittal offset
/// from the joint below)`. The rises sum with the pelvis and the skull to the
/// 1.70 m the rig claims, and they are the real distances between those levels
/// on a spine of that height — the joints are fewer, not shorter.
///
/// **The column is not straight.** A standing spine has lumbar lordosis,
/// thoracic kyphosis and cervical lordosis, and each of the four sits at its
/// own region's apex, which is where a curve is deepest and where a region
/// bends. Stacking them on one vertical line would be a rest pose no body can
/// hold — and the rest pose is the zero every rotation in the corpus is
/// measured from.
const COLUMN: [(&str, f32, f32); 4] = [
    // Mid-lumbar: the hinge for contraction and arch. Forward of the plumb
    // line by the depth of the lordosis.
    ("vert_l3", 0.110, 0.026),
    // Lower thoracic, at the back of the kyphosis.
    ("vert_t10", 0.140, -0.039),
    // Upper thoracic, at the manubrium: what the clavicles hang from.
    ("vert_t2", 0.188, 0.005),
    // Mid-cervical, at the apex of the neck's own lordosis.
    ("vert_c4", 0.090, 0.025),
];

fn add_column(b: &mut Builder, pelvis: JointId) -> JointId {
    let mut parent = pelvis;
    for (name, rise, z) in COLUMN {
        parent = b.add(name, parent, Side::Center, Vec3::new(0.0, rise, z));
    }
    parent
}

// ---------------------------------------------------------------------- limbs

fn side_prefix(side: Side) -> &'static str {
    match side {
        Side::Left => "l",
        Side::Right => "r",
        Side::Center => unreachable!("limbs are lateral"),
    }
}

/// The hand: one joint that gathers the fingers, one that moves the thumb, and
/// a tip on each so both have a length and a point.
///
/// **Four fingers as one node.** `{side}_fingers` sits on the knuckle line —
/// the centroid of the four metacarpal heads — and turning it folds the whole
/// group toward the palm. That is the distinction a dance record has to keep:
/// a hand extended, a hand gathered, and the arc between them. Which finger
/// leads is a distinction no camera in this pipeline can resolve, so writing it
/// down would be writing down the annotator instead of the dancer.
///
/// **The thumb is separate because it is not one of them.** It leaves the
/// wrist toward the anterior edge of the hand and keeps going that way; a hand
/// with the thumb folded into the finger group has no shape for *opposed*, and
/// opposition is the one hand event that reads at stage distance.
///
/// Both run along the arm's own axis, not across it. The arm chain is shoulder
/// → elbow → wrist along X, so a hand built along Z rests at a right angle to
/// its own forearm — a wrist deviates about 20° radially before it stops, so a
/// rest pose holding 90° is a joint outside its range in *every frame ever
/// recorded*, silently, with every rotation keyed on top of it inheriting the
/// error. That is the 2026-08-13 bug and this is where it does not come back.
fn add_hand(b: &mut Builder, wrist: JointId, side: Side) {
    let p = side_prefix(side);

    let fingers = b.add(
        &format!("{p}_fingers"),
        wrist,
        side,
        mirror(Vec3::new(0.069, 0.0, -0.002), side),
    );
    b.add(
        &format!("{p}_finger_tip"),
        fingers,
        side,
        mirror(Vec3::new(0.110, 0.0, 0.0), side),
    );

    let thumb = b.add(
        &format!("{p}_thumb"),
        wrist,
        side,
        mirror(Vec3::new(0.030, -0.012, 0.038), side),
    );
    b.add(
        &format!("{p}_thumb_tip"),
        thumb,
        side,
        mirror(Vec3::new(0.034, -0.012, 0.050), side),
    );
}

/// The foot: heel, ball, toe.
///
/// Three nodes, and between them and the ankle they carry every distinction
/// the notation was cut down to keep. The ankle points and flexes; the ball is
/// where the foot folds for demi-pointe and where the toes extend from; the toe
/// is the point. Pointe is the ankle down and the toe out, flex is the ankle up
/// and the toe back, demi-pointe is the ball bent with the heel off the floor —
/// three shapes, and each one is a different set of numbers on these joints.
///
/// Every offset is measured from the ankle and drops 0.070 m to put the sole
/// flat on the floor, which is what makes `Contact::standing` exact rather than
/// approximately right.
fn add_foot(b: &mut Builder, ankle: JointId, side: Side) {
    const SOLE_DROP: f32 = -0.070;
    let p = side_prefix(side);

    b.add(
        &format!("{p}_heel"),
        ankle,
        side,
        mirror(Vec3::new(0.0, SOLE_DROP, -0.055), side),
    );
    let ball = b.add(
        &format!("{p}_ball"),
        ankle,
        side,
        mirror(Vec3::new(0.0, SOLE_DROP, 0.116), side),
    );
    b.add(&format!("{p}_toe"), ball, side, Vec3::new(0.0, 0.0, 0.084));
}

fn add_arm(b: &mut Builder, chest: JointId, side: Side) {
    let p = side_prefix(side);
    let clavicle = b.add(
        &format!("{p}_clavicle"),
        chest,
        side,
        mirror(Vec3::new(0.035, 0.022, 0.028), side),
    );
    let wrist = b.chain(
        clavicle,
        side,
        &[
            (&format!("{p}_shoulder"), mirror(Vec3::new(0.14, 0.0, 0.0), side)),
            (&format!("{p}_elbow"), mirror(Vec3::new(0.28, 0.0, 0.0), side)),
            (&format!("{p}_wrist"), mirror(Vec3::new(0.26, 0.0, 0.0), side)),
        ],
    );
    add_hand(b, wrist, side);
}

fn add_leg(b: &mut Builder, pelvis: JointId, side: Side) {
    let p = side_prefix(side);
    let ankle = b.chain(
        pelvis,
        side,
        &[
            (&format!("{p}_hip"), mirror(Vec3::new(0.09, -0.06, 0.0), side)),
            (&format!("{p}_knee"), Vec3::new(0.0, -0.42, 0.0)),
            (&format!("{p}_ankle"), Vec3::new(0.0, -0.41, 0.0)),
        ],
    );
    add_foot(b, ankle, side);
}

/// Builds the canonical rig. Guaranteed to pass [`Skeleton::validate`].
pub fn canonical() -> Skeleton {
    let mut b = Builder::new();

    let chest = add_column(&mut b, crate::skeleton::ROOT);
    let head = b.add("head", chest, Side::Center, Vec3::new(0.0, 0.120, -0.001));
    b.add("head_top", head, Side::Center, Vec3::new(0.0, 0.092, -0.006));
    // The mandibular condyle: just below the head node and well forward of it,
    // which is where the hinge actually sits and — more to the point — far
    // enough off the skull's vertical axis that turning the head moves it. See
    // the module note and the yaw test.
    b.add("jaw", head, Side::Center, Vec3::new(0.0, -0.018, 0.048));

    // The clavicle articulates with the manubrium, which sits at the T2 level.
    let manubrium = b
        .joints
        .iter()
        .position(|j| j.name == "vert_t2")
        .expect("the column carries a T2 hinge") as JointId;
    add_arm(&mut b, manubrium, Side::Left);
    add_arm(&mut b, manubrium, Side::Right);
    add_leg(&mut b, crate::skeleton::ROOT, Side::Left);
    add_leg(&mut b, crate::skeleton::ROOT, Side::Right);

    Skeleton {
        // Still `koreo.canonical` after the 2026-08-17 rename, on purpose. This
        // is not prose, it is the rig identifier written into every saved track
        // and checked on load. Renaming it would make every existing track fail
        // validation against the rig it was actually authored on, to buy a word.
        name: "koreo.canonical".into(),
        joints: b.joints,
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;
    use crate::pose::Pose;

    fn points(s: &Skeleton) -> Vec<Vec3> {
        Pose::rest(s).points(s)
    }

    #[test]
    fn canonical_rig_is_well_formed() {
        let s = canonical();
        s.validate().expect("canonical rig must validate");
    }

    /// The size of the thing, pinned. Thirty-six is not a round number chosen
    /// for looks — it is eight on the axis and fourteen per side — and a rig
    /// that grew a joint back without anyone deciding to is a rig whose tick
    /// width changed underneath a trained model.
    #[test]
    fn the_rig_is_thirty_six_joints_and_every_one_of_them_is_writable() {
        let s = canonical();
        assert_eq!(s.len(), 36);
        let axial = s.joints.iter().filter(|j| j.side == Side::Center).count();
        assert_eq!(axial, 8, "pelvis, four hinges, head, head_top, jaw");
        for side in ["l", "r"] {
            let count = s
                .joints
                .iter()
                .filter(|j| j.name.starts_with(&format!("{side}_")))
                .count();
            assert_eq!(count, 14, "{side}: arm 4, hand 4, leg 3, foot 3");
        }
    }

    /// The column is four hinges and they are one unbroken chain from the
    /// pelvis to the skull — not four of twenty-four with twenty missing.
    #[test]
    fn the_column_is_four_hinges_in_one_chain() {
        let s = canonical();
        for (name, _, _) in COLUMN {
            assert!(s.find(name).is_some(), "missing {name}");
        }
        let neck = s.find("vert_c4").unwrap();
        assert_eq!(s.ancestors(neck).count(), 4, "the column is not one chain");
        // And nothing from the old twenty-four survived as a stray.
        for gone in ["vert_l5", "vert_t7", "vert_c1", "vert_c7", "vert_t12"] {
            assert!(s.find(gone).is_none(), "{gone} outlived the cut");
        }
    }

    #[test]
    fn the_column_climbs_from_the_pelvis_to_the_skull() {
        let s = canonical();
        let pts = points(&s);
        let y = |n: &str| pts[s.find(n).unwrap() as usize].y;
        for pair in [
            ("pelvis", "vert_l3"),
            ("vert_l3", "vert_t10"),
            ("vert_t10", "vert_t2"),
            ("vert_t2", "vert_c4"),
            ("vert_c4", "head"),
            ("head", "head_top"),
        ] {
            assert!(y(pair.1) > y(pair.0), "{} is not above {}", pair.1, pair.0);
        }
    }

    /// Lordosis, kyphosis, lordosis — on four joints instead of twenty-four,
    /// and each one still at its own region's apex. A column stacked on one
    /// vertical line is a rest pose no body holds, and the rest pose is the
    /// zero every rotation downstream is measured from.
    #[test]
    fn the_spine_carries_its_three_sagittal_curves() {
        let s = canonical();
        let pts = points(&s);
        let z = |n: &str| pts[s.find(n).unwrap() as usize].z;
        assert!(z("vert_l3") > 0.015, "no lumbar lordosis");
        assert!(z("vert_t10") < -0.010, "no thoracic kyphosis");
        assert!(z("vert_c4") > 0.008, "no cervical lordosis");
        // The skull comes back to the plumb line, or the curves would stack
        // into a lean rather than cancelling.
        assert!(z("head_top").abs() < 0.015, "the head is off the plumb line");
    }

    /// The distinction the hand was cut down to keep. Turning one node folds
    /// the fingers toward the palm and moves nothing else.
    #[test]
    fn one_node_gathers_the_fingers_and_another_moves_the_thumb() {
        let s = canonical();
        let before = points(&s);
        let at = |p: &[Vec3], n: &str| p[s.find(n).expect(n) as usize];

        let mut gathered = Pose::rest(&s);
        gathered.local[s.find("l_fingers").unwrap() as usize] = Quat::from_rotation_z(-1.2);
        let after = gathered.points(&s);
        let moved = |p: &[Vec3], n: &str| at(&before, n).distance(at(p, n));

        assert!(moved(&after, "l_finger_tip") > 0.08, "the fingers did not gather");
        assert!(moved(&after, "l_thumb_tip") < 1e-6, "the thumb came along");
        assert!(moved(&after, "l_wrist") < 1e-6, "the wrist moved");
        assert!(moved(&after, "r_finger_tip") < 1e-6, "the other hand moved");

        let mut opposed = Pose::rest(&s);
        opposed.local[s.find("l_thumb").unwrap() as usize] = Quat::from_rotation_y(0.8);
        let after = opposed.points(&s);
        assert!(moved(&after, "l_thumb_tip") > 0.02, "the thumb barely travelled");
        assert!(moved(&after, "l_finger_tip") < 1e-6, "the fingers came along");
    }

    /// A thumb is opposed, not a fifth finger. It has to stand clear of the
    /// finger group and sit anterior of it, which is the direction opposition
    /// actually happens in.
    #[test]
    fn the_thumb_does_not_lie_along_the_fingers() {
        let s = canonical();
        let pts = points(&s);
        let at = |n: &str| pts[s.find(n).expect(n) as usize];

        for side in ["l", "r"] {
            let p = |n: &str| at(&format!("{side}_{n}"));
            let closest = [p("fingers"), p("finger_tip")]
                .iter()
                .flat_map(|f| [p("thumb").distance(*f), p("thumb_tip").distance(*f)])
                .fold(f32::INFINITY, f32::min);
            assert!(
                closest > 0.025,
                "{side} thumb passes within {:.1} mm of the fingers",
                closest * 1000.0
            );

            let wrist = p("wrist");
            let thumb_dir = (p("thumb_tip") - wrist).normalize();
            let finger_dir = (p("finger_tip") - wrist).normalize();
            let angle = thumb_dir.dot(finger_dir).clamp(-1.0, 1.0).acos().to_degrees();
            assert!(
                (30.0..90.0).contains(&angle),
                "{side} thumb sits {angle:.0}° off the fingers"
            );
            assert!(
                p("thumb_tip").z > p("fingers").z,
                "{side} thumb is not anterior of the knuckles"
            );
        }
    }

    #[test]
    fn hands_mirror_across_the_sagittal_plane() {
        let s = canonical();
        let l = s.find("l_fingers").unwrap();
        let r = s.find("r_fingers").unwrap();
        let (lo, ro) = (
            s.joints[l as usize].rest_offset,
            s.joints[r as usize].rest_offset,
        );
        assert_eq!(lo.x, -ro.x);
        assert_eq!((lo.y, lo.z), (ro.y, ro.z));
    }

    /// The rest pose must be a pose the body can actually hold. A hand built on
    /// a different axis from its arm puts ~90° of radial deviation into every
    /// frame ever recorded, silently, because nothing downstream checks the
    /// rest pose against a joint limit.
    #[test]
    fn the_hand_continues_the_forearm_rather_than_turning_off_it() {
        let s = canonical();
        let pts = points(&s);
        for side in ["l", "r"] {
            let p = |n: &str| pts[s.find(&format!("{side}_{n}")).unwrap() as usize];
            let forearm = (p("wrist") - p("elbow")).normalize();
            let hand = (p("finger_tip") - p("wrist")).normalize();
            let deviation = forearm.dot(hand).clamp(-1.0, 1.0).acos().to_degrees();
            assert!(
                deviation < 20.0,
                "{side} hand sits {deviation:.1}° off its forearm; a wrist reaches about 20°"
            );
        }
    }

    /// Both hands reach outward. Mirroring the offsets is what makes this true
    /// on the right — a hand chained along a fixed +X would grow back through
    /// the arm.
    #[test]
    fn both_hands_extend_away_from_the_body() {
        let s = canonical();
        let pts = points(&s);
        let reach = |side: &str, n: &str| pts[s.find(&format!("{side}_{n}")).unwrap() as usize].x;
        assert!(reach("l", "finger_tip") > reach("l", "wrist"));
        assert!(reach("r", "finger_tip") < reach("r", "wrist"));
    }

    #[test]
    fn fingertips_are_descendants_of_the_wrist() {
        let s = canonical();
        let wrist = s.find("l_wrist").unwrap();
        for n in ["l_finger_tip", "l_thumb_tip"] {
            let tip = s.find(n).unwrap();
            assert!(s.ancestors(tip).any(|a| a == wrist), "{n} is not on the wrist");
        }
    }

    /// A flat foot: heel, ball and toe all exactly on the floor rather than
    /// near it.
    #[test]
    fn the_sole_lies_flat_on_the_floor() {
        let s = canonical();
        let pts = points(&s);
        for side in ["l", "r"] {
            for part in SOLE.iter().chain(["toe"].iter()) {
                let y = pts[s.find(&format!("{side}_{part}")).unwrap() as usize].y;
                assert!(y.abs() < 1e-6, "{side}_{part} rests at y = {y}");
            }
        }
    }

    #[test]
    fn the_foot_points_forward_and_is_a_foots_length() {
        let s = canonical();
        let pts = points(&s);
        for side in ["l", "r"] {
            let at = |n: &str| pts[s.find(&format!("{side}_{n}")).unwrap() as usize];
            let length = at("toe").z - at("heel").z;
            assert!(
                (0.23..0.28).contains(&length),
                "{side} foot is {length:.3} m long"
            );
            assert!(at("toe").z > at("ball").z);
            assert!(at("ball").z > at("heel").z);
        }
    }

    /// Pointe, flex and demi-pointe are three different sets of numbers on the
    /// ankle and the ball — which is the whole reason those two joints are the
    /// ones that survived the cut. Measured toe against heel rather than
    /// against the floor, because where the floor is depends on the root and
    /// this is a claim about the shape of the foot.
    #[test]
    fn the_foot_tells_a_point_from_a_flex() {
        let s = canonical();
        let (toe, heel, ball) = (
            s.find("l_toe").unwrap() as usize,
            s.find("l_heel").unwrap() as usize,
            s.find("l_ball").unwrap() as usize,
        );
        let shape = |ankle: f32, ball: f32| {
            let mut p = Pose::rest(&s);
            p.local[s.find("l_ankle").unwrap() as usize] = Quat::from_rotation_x(ankle);
            p.local[s.find("l_ball").unwrap() as usize] = Quat::from_rotation_x(ball);
            p.points(&s)
        };
        // Plantarflexion is +X here: the toes go down and away.
        let flat = shape(0.0, 0.0);
        let pointed = shape(0.9, 0.3);
        let flexed = shape(-0.6, -0.4);
        let demi = shape(0.8, -0.8);
        let rake = |p: &[Vec3]| p[toe].y - p[heel].y;

        assert!(rake(&flat).abs() < 1e-6, "a flat foot is level");
        assert!(rake(&pointed) < -0.15, "the point did not drop the toe: {:.3}", rake(&pointed));
        assert!(rake(&flexed) > 0.12, "the flex did not lift the toe: {:.3}", rake(&flexed));
        // Demi-pointe is the shape neither of the other two makes: the heel
        // comes up while the toe stays flat on the ground under the ball, which
        // takes the ankle and the ball turning opposite ways.
        assert!(demi[heel].y - demi[ball].y > 0.10, "the heel never lifted");
        assert!((demi[toe].y - demi[ball].y).abs() < 1e-5, "the toe left the ground");
    }

    #[test]
    fn the_rig_is_a_body_shaped_body() {
        let s = canonical();
        let pts = points(&s);
        let at = |n: &str| pts[s.find(n).unwrap() as usize];
        let height = at("head_top").y;
        assert!(
            (1.68..1.72).contains(&height),
            "the rig is {height:.3} m, not the 1.70 m it claims"
        );
        // Shoulders where a shoulder is, not on top of the skull.
        let shoulder = at("l_shoulder").y;
        assert!((0.80..0.86).contains(&(shoulder / height)));
    }

    /// The skull carries orientation and silhouette. The jaw is here for the
    /// orientation half of that and nothing else; the eyes are not, and a rig
    /// that grew a face would be a rig that grew one by accident.
    #[test]
    fn the_skull_has_the_jaw_and_no_other_face_joints() {
        let s = canonical();
        assert!(s.find("jaw").is_some(), "the jaw carries head yaw");
        for face in ["mandible", "l_eye", "r_eye", "tongue"] {
            assert!(s.find(face).is_none(), "{face} is not a movement joint");
        }
        // head, head_top, jaw.
        assert_eq!(s.subtree(s.find("head").unwrap()).len(), 3);
    }

    /// Why the jaw survived a cut that took ninety joints. `head` and
    /// `head_top` sit on the skull's own vertical axis to within 6 mm, so a
    /// head that turns to look stage-left moves them by less than a
    /// triangulation error — the rotation would be in the record and out of the
    /// geometry, which means nothing observed could check it and two annotation
    /// passes could not be compared on it.
    #[test]
    fn head_yaw_is_only_visible_because_of_the_jaw() {
        let s = canonical();
        let head = s.find("head").unwrap();
        let mut turned = Pose::rest(&s);
        turned.local[head as usize] = Quat::from_rotation_y(FRAC_PI_2);

        let (before, after) = (points(&s), turned.points(&s));
        let moved = |n: &str| {
            let i = s.find(n).unwrap() as usize;
            before[i].distance(after[i])
        };
        assert!(
            moved("head_top") < 0.010,
            "head_top moved {:.4} m — the axis is no longer an axis",
            moved("head_top")
        );
        assert!(
            moved("jaw") > 0.050,
            "the jaw moved {:.4} m; a quarter turn of the head has to be measurable",
            moved("jaw")
        );
    }
}
