//! Linear blend skinning: carrying arbitrary geometry along with the rig.
//!
//! This is the bridge out of the rig and into a renderer. A mesh vertex, or the
//! mean of a 3D Gaussian, is bound to a few joints with weights; the joint
//! transforms then move it. It is the standard method for animatable Gaussian
//! avatars (3DGS-Avatar, Human Gaussian Splatting), which deform splats with
//! exactly this and add a learned residual for cloth.
//!
//! Storing poses as **local rotations** is what makes this cheap. A skinning
//! palette is `world_j · inverse_bind_j` per joint, and the world transforms
//! are already one forward pass. Had poses been stored as world-space points,
//! there would be no joint frames to skin with at all.
//!
//! A Gaussian needs two of these operations, not one: its mean is a point, and
//! its orientation is a rotation. Skin the mean and leave the orientation
//! alone and the splat smears — the ellipsoid keeps pointing the way it did in
//! the rest pose while its centre walks away.

use glam::{Affine3A, Mat3, Quat, Vec3};

use crate::pose::Pose;
use crate::skeleton::{JointId, Skeleton};

/// How many joints one point may be bound to. Four is the usual ceiling and
/// is plenty for a body: a knuckle needs two, a shoulder three.
pub const MAX_INFLUENCES: usize = 4;

/// The joints a point follows, and how much.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Binding {
    pub joints: [JointId; MAX_INFLUENCES],
    /// Sums to 1. Unused slots hold zero.
    pub weights: [f32; MAX_INFLUENCES],
}

impl Binding {
    /// Binds rigidly to a single joint.
    pub fn rigid(joint: JointId) -> Self {
        let mut weights = [0.0; MAX_INFLUENCES];
        weights[0] = 1.0;
        Binding {
            joints: [joint; MAX_INFLUENCES],
            weights,
        }
    }

    fn normalized(mut pairs: Vec<(JointId, f32)>) -> Self {
        pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
        pairs.truncate(MAX_INFLUENCES);
        let total: f32 = pairs.iter().map(|(_, w)| w).sum();

        let mut out = Binding {
            joints: [0; MAX_INFLUENCES],
            weights: [0.0; MAX_INFLUENCES],
        };
        if total <= f32::EPSILON {
            out.weights[0] = 1.0;
            return out;
        }
        for (i, (j, w)) in pairs.into_iter().enumerate() {
            out.joints[i] = j;
            out.weights[i] = w / total;
        }
        out
    }
}

/// Rest-pose joint frames and their inverses.
#[derive(Debug, Clone)]
pub struct Skin {
    /// `inverse_bind[j]` takes a point from rest world space into joint `j`'s
    /// local frame.
    pub inverse_bind: Vec<Affine3A>,
    /// Rest-pose world transforms, kept for automatic binding.
    pub rest_world: Vec<Affine3A>,
}

impl Skin {
    pub fn new(skeleton: &Skeleton) -> Self {
        let rest_world = Pose::rest(skeleton).world(skeleton);
        Skin {
            inverse_bind: rest_world.iter().map(|m| m.inverse()).collect(),
            rest_world,
        }
    }

    /// The palette for one pose: what to multiply a rest-space point by.
    pub fn palette_into(&self, world: &[Affine3A], out: &mut Vec<Affine3A>) {
        out.clear();
        out.reserve(world.len());
        for (j, w) in world.iter().enumerate() {
            out.push(*w * self.inverse_bind[j]);
        }
    }

    pub fn palette(&self, world: &[Affine3A]) -> Vec<Affine3A> {
        let mut out = Vec::new();
        self.palette_into(world, &mut out);
        out
    }

    /// Binds a rest-space point to the nearest bones.
    ///
    /// Distance is measured to the bone *segment*, not to the joint, so a
    /// point beside the middle of a forearm binds to the forearm rather than
    /// to whichever end happens to be closer. Weights fall off as `1/d²`,
    /// which gives smooth blends across a joint without any training.
    ///
    /// Good enough to hang a splat on a skeleton and see it move. Published
    /// avatar work learns per-Gaussian weights instead, and that is a real
    /// improvement — but it needs the capture rig this bootstraps.
    pub fn bind_nearest(&self, skeleton: &Skeleton, point: Vec3) -> Binding {
        let mut pairs: Vec<(JointId, f32)> = Vec::new();
        for (j, joint) in skeleton.joints.iter().enumerate() {
            let tip: Vec3 = self.rest_world[j].translation.into();
            let root: Vec3 = match joint.parent {
                Some(p) => self.rest_world[p as usize].translation.into(),
                None => tip,
            };
            let d = distance_to_segment(point, root, tip);
            pairs.push((j as JointId, 1.0 / (d * d + 1e-4)));
        }
        Binding::normalized(pairs)
    }
}

fn distance_to_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq <= 1e-12 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Moves a rest-space point into the posed body.
pub fn skin_point(palette: &[Affine3A], binding: &Binding, point: Vec3) -> Vec3 {
    let mut out = Vec3::ZERO;
    for i in 0..MAX_INFLUENCES {
        let w = binding.weights[i];
        if w > 0.0 {
            out += w * palette[binding.joints[i] as usize].transform_point3(point);
        }
    }
    out
}

/// Moves a rest-space orientation into the posed body — the other half of
/// deforming a Gaussian.
///
/// Quaternions are blended rather than the matrices averaged, because averaging
/// rotation matrices does not produce a rotation. Each is flipped into the same
/// hemisphere first; without that, two rotations that are close in space but
/// opposite in sign cancel and the splat collapses.
pub fn skin_rotation(palette: &[Affine3A], binding: &Binding, rotation: Quat) -> Quat {
    let mut acc = glam::Vec4::ZERO;
    let mut reference: Option<Quat> = None;

    for i in 0..MAX_INFLUENCES {
        let w = binding.weights[i];
        if w <= 0.0 {
            continue;
        }
        let m = Mat3::from(palette[binding.joints[i] as usize].matrix3);
        let q = Quat::from_mat3(&orthonormalize(m));
        let q = match reference {
            None => {
                reference = Some(q);
                q
            }
            Some(r) if r.dot(q) < 0.0 => -q,
            _ => q,
        };
        acc += glam::Vec4::new(q.x, q.y, q.z, q.w) * w;
    }

    if acc.length_squared() <= 1e-12 {
        return rotation;
    }
    let blended = Quat::from_xyzw(acc.x, acc.y, acc.z, acc.w).normalize();
    blended * rotation
}

/// Gram-Schmidt, because a skinning matrix may carry scale and
/// `Quat::from_mat3` expects a pure rotation.
fn orthonormalize(m: Mat3) -> Mat3 {
    let x = m.x_axis.normalize_or_zero();
    let y = (m.y_axis - x * x.dot(m.y_axis)).normalize_or_zero();
    let z = x.cross(y);
    if x.length_squared() < 0.5 || y.length_squared() < 0.5 {
        return Mat3::IDENTITY;
    }
    Mat3::from_cols(x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig;

    #[test]
    fn the_rest_pose_moves_nothing() {
        let s = rig::canonical();
        let skin = Skin::new(&s);
        let palette = skin.palette(&Pose::rest(&s).world(&s));

        let point = Vec3::new(0.2, 1.3, 0.05);
        let binding = skin.bind_nearest(&s, point);
        let moved = skin_point(&palette, &binding, point);
        assert!(
            moved.distance(point) < 1e-4,
            "rest pose displaced a point by {}",
            moved.distance(point)
        );
        // And the orientation is untouched.
        let q = Quat::from_rotation_x(0.7);
        assert!(skin_rotation(&palette, &binding, q).dot(q).abs() > 0.9999);
    }

    #[test]
    fn a_point_on_the_forearm_binds_to_the_forearm() {
        let s = rig::canonical();
        let skin = Skin::new(&s);
        let elbow = s.find("l_elbow").unwrap();
        let wrist = s.find("l_wrist").unwrap();

        // Halfway down the forearm, a few centimetres off the bone.
        let mid = (Vec3::from(skin.rest_world[elbow as usize].translation)
            + Vec3::from(skin.rest_world[wrist as usize].translation))
            * 0.5
            + Vec3::new(0.0, 0.03, 0.0);

        let b = skin.bind_nearest(&s, mid);
        // The wrist joint owns the forearm segment, since a bone is named for
        // its far end.
        assert_eq!(b.joints[0], wrist, "bound to {:?}", b.joints);
        assert!(b.weights[0] > 0.4);
        assert!((b.weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn skinned_geometry_follows_the_joint_that_moves_it() {
        let s = rig::canonical();
        let skin = Skin::new(&s);
        let elbow = s.find("l_elbow").unwrap();
        let wrist = s.find("l_wrist").unwrap();

        let on_forearm = (Vec3::from(skin.rest_world[elbow as usize].translation)
            + Vec3::from(skin.rest_world[wrist as usize].translation))
            * 0.5;
        let on_head = Vec3::from(skin.rest_world[s.find("head").unwrap() as usize].translation);

        let arm_binding = skin.bind_nearest(&s, on_forearm);
        let head_binding = skin.bind_nearest(&s, on_head);

        let mut posed = Pose::rest(&s);
        posed.local[elbow as usize] = Quat::from_rotation_y(1.0);
        let palette = skin.palette(&posed.world(&s));

        let arm_moved = skin_point(&palette, &arm_binding, on_forearm).distance(on_forearm);
        let head_moved = skin_point(&palette, &head_binding, on_head).distance(on_head);

        assert!(arm_moved > 0.05, "the forearm point should travel");
        assert!(head_moved < 1e-4, "the head must not move with the elbow");
    }

    #[test]
    fn a_gaussians_orientation_turns_with_its_bone() {
        // The failure this guards against: skinning the mean but not the
        // orientation, which leaves splats smeared along their old axes.
        let s = rig::canonical();
        let skin = Skin::new(&s);
        let elbow = s.find("l_elbow").unwrap();

        let mut posed = Pose::rest(&s);
        posed.local[elbow as usize] = Quat::from_rotation_y(1.2);
        let palette = skin.palette(&posed.world(&s));

        let binding = Binding::rigid(s.find("l_wrist").unwrap());
        let turned = skin_rotation(&palette, &binding, Quat::IDENTITY);
        let angle = turned.to_axis_angle().1;
        assert!(
            (angle - 1.2).abs() < 1e-3,
            "orientation turned by {angle}, expected 1.2"
        );
    }

    #[test]
    fn blending_opposite_sign_quaternions_does_not_collapse() {
        let s = rig::canonical();
        let skin = Skin::new(&s);
        let mut posed = Pose::rest(&s);
        // A large rotation, where the two influencing joints can end up on
        // opposite hemispheres.
        posed.local[s.find("l_shoulder").unwrap() as usize] = Quat::from_rotation_z(3.0);
        let palette = skin.palette(&posed.world(&s));

        let mut binding = Binding::rigid(s.find("l_elbow").unwrap());
        binding.joints[1] = s.find("vert_t2").unwrap();
        binding.weights = [0.5, 0.5, 0.0, 0.0];

        let q = skin_rotation(&palette, &binding, Quat::IDENTITY);
        assert!(q.is_normalized(), "blend produced {q:?}");
    }

    #[test]
    fn weights_always_sum_to_one() {
        let s = rig::canonical();
        let skin = Skin::new(&s);
        for p in [
            Vec3::new(0.0, 1.6, 0.0),
            Vec3::new(0.5, 0.9, 0.2),
            Vec3::new(-0.3, 0.1, -0.1),
            Vec3::new(9.0, 9.0, 9.0), // far outside the body
        ] {
            let b = skin.bind_nearest(&s, p);
            assert!((b.weights.iter().sum::<f32>() - 1.0).abs() < 1e-5, "at {p}");
            assert!(b.weights.iter().all(|w| *w >= 0.0));
        }
    }

    #[test]
    fn the_palette_can_be_rebuilt_without_allocating() {
        let s = rig::canonical();
        let skin = Skin::new(&s);
        let mut world = Vec::new();
        let mut palette = Vec::new();
        let pose = Pose::rest(&s);

        pose.world_into(&s, &mut world);
        skin.palette_into(&world, &mut palette);
        let capacity = palette.capacity();

        for _ in 0..8 {
            pose.world_into(&s, &mut world);
            skin.palette_into(&world, &mut palette);
        }
        assert_eq!(palette.capacity(), capacity, "palette reallocated");
        assert_eq!(palette.len(), s.len());
    }
}
