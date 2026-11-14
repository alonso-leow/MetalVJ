//! Synthetic tests of the rig. No counts are written by hand: everything is
//! walked from `FaceControl::ALL`, `Viseme::ALL`, `Kind::ALL` and `LAYOUT`.

use glam::Vec3;

use crate::control::FaceControl as C;
use crate::topology::{
    is_lower_lip, is_upper_lip, CORNER_L, CORNER_R, LIP_RING, LOWER_MID, TONGUE_SEGMENTS,
};
use crate::*;

const EPS: f32 = 1e-5;

/// Every topology the rig must work on: neutral plus every species.
fn topologies() -> Vec<(String, Topology)> {
    let mut out = vec![("neutral".to_string(), Topology::neutral())];
    for k in Kind::ALL {
        out.push((k.name().to_string(), Topology::for_species(&k.species())));
    }
    out
}

fn posed(t: &Topology, pairs: &[(FaceControl, f32)]) -> Posed {
    pose(t, &Controls::with(pairs))
}

fn close(a: Vec3, b: Vec3) -> bool {
    a.distance(b) < EPS
}

#[test]
fn control_names_are_unique_snake_case() {
    let mut seen = std::collections::HashSet::new();
    for (i, c) in FaceControl::ALL.iter().enumerate() {
        let n = c.name();
        assert!(seen.insert(n), "duplicate control name {n}");
        assert!(
            n.starts_with(|ch: char| ch.is_ascii_lowercase())
                && n.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
                && !n.ends_with('_')
                && !n.contains("__"),
            "{n} is not snake_case"
        );
        // The discriminant is the index, and serde agrees with name().
        assert_eq!(c.index(), i);
        assert_eq!(FaceControl::from_name(n), Some(*c));
        assert_eq!(serde_json::to_string(c).unwrap(), format!("\"{n}\""));
        let s = c.spec();
        assert!(s.lo < s.hi && s.lo <= s.rest && s.rest <= s.hi, "{n} range");
        assert!(
            s.spring.attack > 0.0 && s.spring.release > 0.0,
            "{n} spring"
        );
    }
}

#[test]
fn controls_round_trip_through_serde_by_name() {
    let c = Viseme::Aa.pose();
    let json = serde_json::to_string(&c).unwrap();
    let back: Controls = serde_json::from_str(&json).unwrap();
    assert_eq!(back, c);
    // A missing control loads at rest, an unknown one is ignored.
    let partial: Controls =
        serde_json::from_str(r#"{"jaw_open":0.5,"from_the_future":1}"#).unwrap();
    assert_eq!(partial, Controls::with(&[(C::JawOpen, 0.5)]));
}

#[test]
fn every_viseme_pose_is_inside_control_ranges() {
    let mut names = std::collections::HashSet::new();
    for v in Viseme::ALL {
        assert!(names.insert(v.name()), "duplicate viseme {}", v.name());
        assert_eq!(
            serde_json::to_string(v).unwrap(),
            format!("\"{}\"", v.name())
        );
        // The table itself, not only the clamped pose, must be in range.
        for &(c, x) in v.moves() {
            let s = c.spec();
            assert!(x >= s.lo && x <= s.hi, "{} {} = {x}", v.name(), c.name());
        }
        for (c, x) in v.pose().iter() {
            let s = c.spec();
            assert!(x >= s.lo && x <= s.hi, "{} {}", v.name(), c.name());
        }
    }
    assert_eq!(Viseme::Sil.pose(), Controls::rest());
    // Blends stay in range too, even over-weighted ones.
    let all: Vec<_> = Viseme::ALL.iter().map(|&v| (v, 3.0)).collect();
    for (c, x) in blend(&all).iter() {
        let s = c.spec();
        assert!(x >= s.lo && x <= s.hi, "blend {}", c.name());
    }
    assert_eq!(blend(&[(Viseme::O, 1.0)]), Viseme::O.pose());
    assert_eq!(blend(&[]), Controls::rest());
    let half = blend(&[(Viseme::Aa, 0.5)]);
    assert!((half[C::JawOpen] - 0.5 * Viseme::Aa.pose()[C::JawOpen]).abs() < EPS);
}

#[test]
fn rest_controls_give_the_neutral_topology() {
    for (name, t) in topologies() {
        let p = pose(&t, &Controls::rest());
        assert_eq!(p.face, t, "{name}");
        assert_eq!(p.jaw_angle, 0.0);
        assert_eq!(p.jaw_side, 0.0);
        assert!(p.head.angle_between(glam::Quat::IDENTITY) < EPS);
    }
    assert_eq!(
        Topology::neutral(),
        Topology::for_species(&Species::NEUTRAL)
    );
}

#[test]
fn jaw_open_moves_lower_lip_and_teeth_down_and_keeps_upper_lip() {
    for (name, t) in topologies() {
        let p = posed(&t, &[(C::JawOpen, 1.0)]);
        for i in 0..LIP_RING {
            if is_upper_lip(i) {
                assert!(
                    close(p.face.lips[i].at, t.lips[i].at),
                    "{name} upper lip {i}"
                );
            }
            if is_lower_lip(i) {
                assert!(
                    p.face.lips[i].at.y < t.lips[i].at.y - 0.1,
                    "{name} lower lip {i}"
                );
            }
        }
        for (a, b) in p.face.lower_teeth.iter().zip(&t.lower_teeth) {
            assert!(a.b.y < b.b.y - 0.1, "{name} lower tooth");
        }
        for (a, b) in p.face.upper_teeth.iter().zip(&t.upper_teeth) {
            assert_eq!(a, b, "{name} upper teeth are skull");
        }
        assert!(p.face.lip_gap() > t.lip_gap() + 0.1, "{name} mouth opened");
        assert!(p.face.cavity.radii.y > t.cavity.radii.y);
    }
}

#[test]
fn pp_closes_the_lips() {
    for (name, t) in topologies() {
        let p = pose(&t, &Viseme::PP.pose());
        assert!(p.face.lip_gap() < 1e-4, "{name}: gap {}", p.face.lip_gap());
        // Closing works even over a wide open jaw (a p inside a sung vowel).
        let p = posed(&t, &[(C::JawOpen, 0.85), (C::LipClose, 1.0)]);
        assert!(p.face.lip_gap() < 1e-4, "{name} over open jaw");
    }
}

#[test]
fn aa_opens_the_mouth_wider_than_i() {
    for (name, t) in topologies() {
        let aa = pose(&t, &Viseme::Aa.pose()).face.lip_gap();
        let i = pose(&t, &Viseme::I.pose()).face.lip_gap();
        assert!(aa > i + 0.05, "{name}: aa {aa} vs I {i}");
    }
}

#[test]
fn o_and_u_round_the_lips() {
    for (name, t) in topologies() {
        let rest = t.mouth_width();
        for v in [Viseme::O, Viseme::U] {
            let p = pose(&t, &v.pose());
            let w = p.face.mouth_width();
            assert!(w < 0.8 * rest, "{name} {}: {w} vs {rest}", v.name());
            // Rounded lips push forward.
            assert!(p.face.lips[CORNER_L].at.z > t.lips[CORNER_L].at.z);
        }
        let i = pose(&t, &Viseme::I.pose()).face.mouth_width();
        assert!(i > rest, "{name}: I spreads");
    }
}

#[test]
fn tongue_stays_inside_the_cavity_unless_tongue_out() {
    let inside = |p: &Posed| p.face.tongue.iter().all(|&j| p.face.cavity.contains(j));
    for (name, t) in topologies() {
        for v in Viseme::ALL {
            let c = v.pose();
            if c[C::TongueOut] == 0.0 {
                {
                    let p = pose(&t, &c);
                    assert!(
                        inside(&p),
                        "{name} {}: {:?}",
                        v.name(),
                        p.face
                            .tongue
                            .iter()
                            .map(|&j| p.face.cavity.level(j))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        // Every extreme of every tongue-carrying control, with tongue_out 0.
        for jaw in [0.0, 0.5, 1.0] {
            for up in [0.0, 0.5, 1.0] {
                for curl in [-1.0, 0.0, 1.0] {
                    for side in [-1.0, 0.0, 1.0] {
                        let p = posed(
                            &t,
                            &[
                                (C::JawOpen, jaw),
                                (C::TongueUp, up),
                                (C::TongueCurl, curl),
                                (C::JawSide, side),
                            ],
                        );
                        assert!(
                            inside(&p),
                            "{name} jaw {jaw} up {up} curl {curl} side {side}: {:?}",
                            p.face
                                .tongue
                                .iter()
                                .map(|&j| p.face.cavity.level(j))
                                .collect::<Vec<_>>()
                        );
                    }
                }
            }
        }
        let out = posed(&t, &[(C::TongueOut, 1.0), (C::JawOpen, 0.5)]);
        let tip = out.face.tongue_tip();
        assert!(
            !out.face.cavity.contains(tip),
            "{name} tongue_out leaves the cavity"
        );
        assert!(
            tip.z > out.face.lips[LOWER_MID].at.z,
            "{name} past the lips"
        );
    }
}

#[test]
fn tongue_up_reaches_the_upper_teeth() {
    for (name, t) in topologies() {
        for c in [Viseme::DD.pose(), Controls::with(&[(C::TongueUp, 1.0)])] {
            let p = pose(&t, &c);
            let tip = p.face.tongue_tip();
            let tip_r = p.face.tongue_radius[TONGUE_SEGMENTS - 1];
            let nearest = p
                .face
                .upper_teeth
                .iter()
                .map(|tooth| tooth.distance(tip))
                .fold(f32::INFINITY, f32::min);
            assert!(
                nearest < tip_r + 0.02,
                "{name}: tip {nearest} from the upper teeth"
            );
            let rest_tip = pose(&t, &Controls::rest()).face.tongue_tip();
            let rest_nearest = t
                .upper_teeth
                .iter()
                .map(|tooth| tooth.distance(rest_tip))
                .fold(f32::INFINITY, f32::min);
            assert!(rest_nearest > nearest + 0.03, "{name}: at rest it is lower");
        }
    }
}

#[test]
fn blink_closes_both_lids() {
    for (name, t) in topologies() {
        for extra in [&[][..], &[(C::EyeWide, 1.0)], &[(C::Squint, 1.0)]] {
            let mut pairs = vec![(C::Blink, 1.0)];
            pairs.extend_from_slice(extra);
            let p = posed(&t, &pairs);
            for e in &p.face.eyes {
                assert!(e.opening() < EPS, "{name}: opening {}", e.opening());
            }
        }
        for e in &t.eyes {
            assert!(e.opening() > 0.5, "{name} open at rest");
        }
        let wide = posed(&t, &[(C::EyeWide, 1.0)]);
        let squint = posed(&t, &[(C::Squint, 1.0)]);
        assert!(wide.face.eyes[0].opening() > t.eyes[0].opening());
        assert!(squint.face.eyes[0].opening() < t.eyes[0].opening());
    }
}

#[test]
fn species_keep_the_mouth_landmarks_on_the_mouth() {
    for (name, t) in topologies() {
        for c in [Controls::rest(), Viseme::Aa.pose(), Viseme::U.pose()] {
            let f = pose(&t, &c).face;
            let n = LIP_RING as f32;
            let centroid = f.lips.iter().map(|b| b.at).sum::<Vec3>() / n;
            let cav = f.cavity;
            // The ring surrounds the cavity's opening: centred on it in x,
            // within its height, and in front of its centre.
            assert!(
                (centroid.x - cav.centre.x).abs() < 0.25 * cav.radii.x,
                "{name} x"
            );
            assert!((centroid.y - cav.centre.y).abs() < cav.radii.y, "{name} y");
            assert!(centroid.z > cav.centre.z, "{name} z");
            assert!(f.lips[CORNER_L].at.x > cav.centre.x && f.lips[CORNER_R].at.x < cav.centre.x);
            // The whole mouth is below the eyes and the nose.
            let eye_low = f
                .eyes
                .iter()
                .map(|e| e.centre.y - e.radius)
                .fold(f32::INFINITY, f32::min);
            let nose_low = f
                .nostrils
                .iter()
                .map(|n| n.at.y - n.radius)
                .fold(f32::INFINITY, f32::min);
            for b in &f.lips {
                assert!(b.at.y + b.radius < eye_low, "{name} lips under the eyes");
                assert!(b.at.y < nose_low, "{name} lips under the nose");
            }
            for tooth in f.upper_teeth.iter().chain(&f.lower_teeth) {
                assert!(tooth.a.y < eye_low && tooth.b.y < eye_low, "{name} teeth");
            }
        }
    }
}

#[test]
fn springs_converge_without_overshoot() {
    let dts = [1.0 / 240.0, 1.0 / 60.0, 1.0 / 32.0, 1.0 / 8.0];
    for &dt in &dts {
        for &c in FaceControl::ALL {
            let s = c.spec();
            for goal in [s.lo, s.hi] {
                if goal == s.rest {
                    continue;
                }
                let target = Controls::with(&[(c, goal)]);
                let mut springs = Springs::new();
                let mut prev = s.rest;
                let steps = (6.0 / dt) as usize;
                for _ in 0..steps {
                    let x = springs.step(&target, dt)[c];
                    // Monotone toward the goal, never past it.
                    assert!(
                        (x - prev) * (goal - s.rest) >= -EPS,
                        "{} dt {dt} not monotone",
                        c.name()
                    );
                    assert!(
                        (x - s.rest).abs() <= (goal - s.rest).abs() + EPS,
                        "{} overshoot",
                        c.name()
                    );
                    prev = x;
                }
                assert!(
                    (prev - goal).abs() < 1e-3,
                    "{} did not converge: {prev}",
                    c.name()
                );
                // And back to rest the same way.
                let rest = Controls::rest();
                for _ in 0..steps {
                    let x = springs.step(&rest, dt)[c];
                    assert!(
                        (x - s.rest) * (goal - s.rest) >= -EPS,
                        "{} release overshoot",
                        c.name()
                    );
                }
                assert!(
                    (springs.value()[c] - s.rest).abs() < 1e-3,
                    "{} release",
                    c.name()
                );
            }
        }
    }
}

#[test]
fn lips_close_faster_than_the_jaw() {
    let target = Controls::with(&[(C::LipClose, 1.0), (C::JawOpen, 1.0)]);
    let mut springs = Springs::new();
    let dt = 1.0 / 240.0;
    for _ in 0..(0.06 / dt) as usize {
        springs.step(&target, dt);
    }
    let v = springs.value();
    assert!(
        v[C::LipClose] > 0.8,
        "a p/b/m closure lands within ~60 ms: {}",
        v[C::LipClose]
    );
    assert!(
        v[C::LipClose] > v[C::JawOpen] + 0.2,
        "{} vs {}",
        v[C::LipClose],
        v[C::JawOpen]
    );
    // And the tongue tip is the fastest of all.
    let tip = C::TongueUp.spec().spring.attack;
    for &c in FaceControl::ALL {
        assert!(
            c.spec().spring.attack <= tip,
            "{} faster than the tongue tip",
            c.name()
        );
    }
}

#[test]
fn whisper_caps_the_jaw() {
    let mut springs = Springs::with_style(Style::WHISPER);
    let target = Viseme::Aa.pose();
    let mut max_jaw: f32 = 0.0;
    for _ in 0..200 {
        max_jaw = max_jaw.max(springs.step(&target, 1.0 / 60.0)[C::JawOpen]);
    }
    assert!(max_jaw <= springs::WHISPER_JAW + EPS, "jaw {max_jaw}");
    assert!(springs.value()[C::Squint] > 0.3, "whisper squints");
    // Speaking the same vowel opens wider.
    let mut speak = Springs::new();
    for _ in 0..200 {
        speak.step(&target, 1.0 / 60.0);
    }
    assert!(speak.value()[C::JawOpen] > max_jaw + 0.3);
}

#[test]
fn styles_change_the_springs_as_documented() {
    let at = |style: Style, c: FaceControl| {
        let mut s = Springs::with_style(style);
        let target = Controls::with(&[(c, 1.0)]);
        for _ in 0..12 {
            s.step(&target, 1.0 / 240.0);
        }
        s.value()[c]
    };
    assert!(at(Style::RAP, C::JawOpen) > at(Style::SPEAK, C::JawOpen));
    assert!(at(Style::SING, C::JawOpen) < at(Style::SPEAK, C::JawOpen));
    assert_eq!(at(Style::RAP, C::Blink), at(Style::SPEAK, C::Blink));
    let zero = Style {
        sing: 0.0,
        rap: 0.0,
        speak: 0.0,
        whisper: 0.0,
    };
    assert_eq!(zero.shares(), Style::SPEAK.shares());
}

#[test]
fn pack_length_matches_the_documented_layout() {
    for (name, t) in topologies() {
        let p = pose(&t, &Viseme::O.pose());
        let (flat, sections) = p.pack_sections();
        assert_eq!(flat.len(), PACKED, "{name}");
        assert_eq!(flat.len() % 4, 0);
        assert_eq!(sections.as_slice(), LAYOUT, "{name} sections in order");
        assert_eq!(VEC4S, LAYOUT.iter().map(|s| s.1).sum::<usize>());
        assert!(flat.iter().all(|x| x.is_finite()), "{name}");
        assert_eq!(p.pack(), flat);
        // Spot-check the documented positions: jaw angle is float 3, and the
        // lips section starts with the left corner and its radius.
        assert_eq!(flat[3], p.jaw_angle);
        let lips = LAYOUT
            .iter()
            .take_while(|s| s.0 != "lips")
            .map(|s| s.1)
            .sum::<usize>()
            * 4;
        let corner = p.face.lips[CORNER_L];
        assert_eq!(
            &flat[lips..lips + 4],
            &[corner.at.x, corner.at.y, corner.at.z, corner.radius]
        );
    }
}

#[test]
fn new_sections_are_appended_and_sum_to_vec4s() {
    assert_eq!(LAYOUT[LAYOUT.len() - 2].0, "controls");
    assert_eq!(LAYOUT[LAYOUT.len() - 1].0, "rest");
    assert_eq!(
        LAYOUT.iter().find(|s| s.0 == "controls").unwrap().1,
        (FaceControl::COUNT + 3) / 4,
        "controls vec4 count is derived from COUNT"
    );
    assert_eq!(VEC4S, LAYOUT.iter().map(|s| s.1).sum::<usize>());
    assert_eq!(PACKED, VEC4S * 4);
}

#[test]
fn pack_carries_the_clamped_controls_in_all_order() {
    let t = Topology::neutral();
    let c = Controls::with(&[
        (C::JawOpen, 0.5),
        (C::Squint, 0.7),
        (C::BrowFurrow, 0.3),
        (C::JawSide, 2.0),
    ]);
    let p = pose(&t, &c);
    let flat = p.pack();
    let offset = LAYOUT
        .iter()
        .take_while(|s| s.0 != "controls")
        .map(|s| s.1)
        .sum::<usize>()
        * 4;
    let clamped = c.clamped();
    for (i, &fc) in FaceControl::ALL.iter().enumerate() {
        assert_eq!(flat[offset + i], clamped.get(fc), "{}", fc.name());
    }
    for i in FaceControl::COUNT..((FaceControl::COUNT + 3) / 4 * 4) {
        assert_eq!(flat[offset + i], 0.0, "control padding {i}");
    }
}

#[test]
fn pack_carries_the_rest_mouth_and_jaw_seam() {
    for (name, t) in topologies() {
        let p = pose(&t, &Viseme::U.pose());
        let flat = p.pack();
        let offset = LAYOUT
            .iter()
            .take_while(|s| s.0 != "rest")
            .map(|s| s.1)
            .sum::<usize>()
            * 4;
        assert_eq!(
            &flat[offset..offset + 4],
            &[t.mouth.x, t.mouth.y, t.mouth.z, t.jaw_pivot.z],
            "{name}"
        );
    }
}

#[test]
fn head_controls_only_turn_the_head() {
    let t = Topology::neutral();
    let p = posed(
        &t,
        &[(C::HeadPitch, 1.0), (C::HeadYaw, -1.0), (C::HeadRoll, 0.5)],
    );
    assert_eq!(p.face, t, "landmarks stay head-local");
    assert!(p.head.angle_between(glam::Quat::IDENTITY) > 0.3);
    // + pitch looks down: the face's forward axis tips toward −y.
    let nod = posed(&t, &[(C::HeadPitch, 1.0)]);
    assert!(nod.to_view(Vec3::Z).y < 0.0);
}

#[test]
fn species_floats_name_every_species() {
    for (i, k) in Kind::ALL.iter().enumerate() {
        assert_eq!(Kind::from_float(i as f32), *k);
        assert_eq!(
            serde_json::to_string(k).unwrap(),
            format!("\"{}\"", k.name())
        );
    }
    assert_eq!(Kind::from_float(-3.0), Kind::ALL[0]);
    assert_eq!(Kind::from_float(1e9), *Kind::ALL.last().unwrap());
    assert_eq!(Kind::from_float(f32::NAN), Kind::ALL[0]);
}

// ---- the expression muscles (2026-10-07) ----------------------------------

/// The landmarks of a posed face as one flat list, so "did anything move"
/// and "is this the mirror of that" are one comparison each.
fn landmarks(p: &Posed) -> Vec<Vec3> {
    let f = &p.face;
    let mut v: Vec<Vec3> = f.lips.iter().map(|b| b.at + Vec3::splat(b.radius)).collect();
    for e in &f.eyes {
        v.push(e.centre);
        v.push(Vec3::new(e.upper_lid, e.lower_lid, e.radius));
    }
    for b in &f.brows {
        v.extend_from_slice(b);
    }
    for b in f.nostrils.iter().chain(&f.cheeks) {
        v.push(b.at);
        v.push(Vec3::splat(b.radius));
    }
    v
}

/// The controls appended after the 23 speech controls: everything from
/// `brow_inner_up` on, found by name rather than by a literal index.
fn expression_controls() -> &'static [FaceControl] {
    let from = FaceControl::ALL
        .iter()
        .position(|c| c.name() == "brow_inner_up")
        .expect("the expression block starts at brow_inner_up");
    &FaceControl::ALL[from..]
}

#[test]
fn every_expression_control_moves_the_face_on_every_species() {
    assert!(!expression_controls().is_empty());
    for (name, t) in topologies() {
        let rest = landmarks(&posed(&t, &[]));
        for &c in expression_controls() {
            let moved = landmarks(&posed(&t, &[(c, c.spec().hi)]));
            let most = rest
                .iter()
                .zip(&moved)
                .map(|(a, b)| a.distance(*b))
                .fold(0.0f32, f32::max);
            assert!(
                most > 0.01,
                "{name}: {} at full moves the face only {most}",
                c.name()
            );
        }
    }
}

#[test]
fn expression_controls_are_one_sided_and_rest_at_zero() {
    for &c in expression_controls() {
        let s = c.spec();
        assert_eq!((s.lo, s.hi, s.rest), (0.0, 1.0, 0.0), "{}", c.name());
    }
}

/// Every `_l` control has an `_r` twin, and the twin's pose is the mirror
/// image (x → −x, left ↔ right) of the left one's — while the left one,
/// alone, leaves the right side's brows, eye, cheek and nostril at rest.
#[test]
fn per_side_controls_are_mirror_twins_and_leave_the_other_side_alone() {
    use crate::topology::{L, R};
    let lefts: Vec<FaceControl> = FaceControl::ALL
        .iter()
        .copied()
        .filter(|c| c.name().ends_with("_l"))
        .collect();
    assert!(lefts.len() >= 8, "the per-side muscles exist");
    let t = Topology::neutral();
    let rest = posed(&t, &[]);
    let mirror = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
    for l in lefts {
        let rname = format!("{}_r", l.name().trim_end_matches("_l"));
        let r = FaceControl::from_name(&rname).unwrap_or_else(|| panic!("{} has no {rname}", l.name()));
        let pl = posed(&t, &[(l, 1.0)]);
        let pr = posed(&t, &[(r, 1.0)]);

        // The untouched side stays at rest.
        let f = &pl.face;
        for k in 0..crate::topology::BROW_POINTS {
            assert!(close(f.brows[R][k], rest.face.brows[R][k]), "{}: right brow moved", l.name());
        }
        assert_eq!(f.eyes[R], rest.face.eyes[R], "{}: right eye moved", l.name());
        assert_eq!(f.cheeks[R], rest.face.cheeks[R], "{}: right cheek moved", l.name());
        assert_eq!(f.nostrils[R], rest.face.nostrils[R], "{}: right nostril moved", l.name());
        assert!(close(f.lips[CORNER_R].at, rest.face.lips[CORNER_R].at), "{}: right corner moved", l.name());

        // And the right twin is its mirror image.
        for s in [L, R] {
            let o = 1 - s;
            for k in 0..crate::topology::BROW_POINTS {
                assert!(close(pr.face.brows[o][k], mirror(pl.face.brows[s][k])), "{rname}: brows");
            }
            assert!(close(pr.face.cheeks[o].at, mirror(pl.face.cheeks[s].at)), "{rname}: cheeks");
            assert!(close(pr.face.nostrils[o].at, mirror(pl.face.nostrils[s].at)), "{rname}: nostrils");
            let (a, b) = (pr.face.eyes[o], pl.face.eyes[s]);
            assert!((a.upper_lid - b.upper_lid).abs() < EPS && (a.lower_lid - b.lower_lid).abs() < EPS, "{rname}: lids");
        }
        for i in 0..LIP_RING {
            // Ring point i mirrors to the point at angle π − θ.
            let j = (LIP_RING + LIP_RING / 2 - i) % LIP_RING;
            assert!(close(pr.face.lips[j].at, mirror(pl.face.lips[i].at)), "{rname}: lip {i}");
        }
    }
}

#[test]
fn a_wink_shuts_one_eye_only() {
    let t = Topology::neutral();
    let p = posed(&t, &[(C::BlinkL, 1.0)]);
    let [l, r] = p.face.eyes;
    assert!(l.opening() < EPS, "the winking eye is shut");
    assert!(r.opening() > 0.5, "the other eye stays open");
}

#[test]
fn a_one_sided_smile_lifts_one_corner() {
    for (name, t) in topologies() {
        let rest = posed(&t, &[]);
        let p = posed(&t, &[(C::SmileL, 1.0)]);
        let dl = p.face.lips[CORNER_L].at.y - rest.face.lips[CORNER_L].at.y;
        let dr = p.face.lips[CORNER_R].at.y - rest.face.lips[CORNER_R].at.y;
        assert!(dl > 0.06, "{name}: the left corner rose only {dl}");
        assert!(dr.abs() < EPS, "{name}: the right corner moved {dr}");
    }
}

#[test]
fn per_side_controls_add_to_their_symmetric_sibling_and_clamp() {
    let t = Topology::neutral();
    let both = posed(&t, &[(C::Blink, 1.0), (C::BlinkL, 1.0)]);
    let one = posed(&t, &[(C::Blink, 1.0)]);
    assert_eq!(both.face.eyes, one.face.eyes, "blink + blink_l never closes past shut");
}

#[test]
fn lip_press_thins_and_closes_the_lips() {
    let t = Topology::neutral();
    let rest = posed(&t, &[(C::JawOpen, 0.3)]);
    let p = posed(&t, &[(C::JawOpen, 0.3), (C::LipPress, 1.0)]);
    assert!(p.face.lip_gap() < rest.face.lip_gap() * 0.1, "pressed lips meet");
    assert!(p.face.lips[crate::topology::UPPER_MID].radius < rest.face.lips[crate::topology::UPPER_MID].radius * 0.7);
}

#[test]
fn every_expression_preset_is_in_range_and_moves_the_face() {
    let t = Topology::neutral();
    let rest = landmarks(&posed(&t, &[]));
    let mut names = std::collections::HashSet::new();
    for e in EXPRESSIONS {
        assert!(names.insert(e.name), "{} twice", e.name);
        for &(c, v) in e.controls {
            assert_eq!(c.spec().clamp(v), v, "{}: {} out of range", e.name, c.name());
        }
        let moved = landmarks(&pose(&t, &e.controls()));
        assert_ne!(rest, moved, "{} does nothing", e.name);
    }
    for want in ["rage", "terror", "sneer", "wink", "smirk", "surprise"] {
        assert!(Expression::by_name(want).is_some(), "no {want} preset");
    }
}
