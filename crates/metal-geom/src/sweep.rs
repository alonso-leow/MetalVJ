//! Sweep a profile along a path.
//!
//! The profile is placed in a parallel-transport frame that starts from an
//! up vector not parallel to the first tangent. The frame's `u` axis maps the
//! profile's `x` and `v` maps its `y`, with `cross(u, v) == tangent`, so a
//! counter-clockwise profile produces outward-facing walls. Caps use a
//! triangle fan from the profile centroid, which is correct for convex
//! profiles (the intended solid case).

use glam::{Quat, Vec3};

use crate::{Curve2, Curve3, Mesh};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepOptions {
    pub caps: bool,
    pub closed: bool,
    pub segments: usize,
}

impl Default for SweepOptions {
    fn default() -> Self {
        Self {
            caps: true,
            closed: true,
            segments: 32,
        }
    }
}

fn initial_axes(tangent: Vec3) -> (Vec3, Vec3) {
    let up = Vec3::new(crate::UP[0], crate::UP[1], crate::UP[2]);
    let reference = if tangent.dot(up).abs() < 0.99 {
        up
    } else {
        Vec3::X
    };
    let u = reference.cross(tangent).normalize_or_zero();
    let u = if u.length_squared() > 1.0e-12 {
        u
    } else {
        Vec3::X
    };
    let v = tangent.cross(u).normalize_or_zero();
    (u, v)
}

/// Sweep `profile` (2D, in the path's local frame) along `path`.
pub fn sweep(path: &Curve3, profile: &Curve2, opts: SweepOptions) -> Mesh {
    let segments = opts.segments.max(2);
    let profile_points = profile.flatten(1.0e-4);
    if path.points.len() < 2 || profile_points.len() < 2 {
        return Mesh::new();
    }
    let path = path.resample(segments);
    let n = path.points.len();
    if n < 2 {
        return Mesh::new();
    }
    let profile_closed = opts.closed || profile.closed;
    let m = profile_points.len();

    let mut tangents = Vec::with_capacity(n);
    for i in 0..n {
        let prev = path.points[if i == 0 {
            if path.closed {
                n - 1
            } else {
                0
            }
        } else {
            i - 1
        }];
        let next = path.points[if i + 1 == n {
            if path.closed {
                0
            } else {
                n - 1
            }
        } else {
            i + 1
        }];
        let d = Vec3::from_array(next) - Vec3::from_array(prev);
        tangents.push(d.normalize_or_zero());
    }
    if tangents.iter().all(|t| t.length_squared() < 1.0e-12) {
        return Mesh::new();
    }

    let (mut u, mut v) = initial_axes(tangents[0]);
    let mut frames = Vec::with_capacity(n);
    let mut last_t = tangents[0];
    for (i, &t) in tangents.iter().enumerate() {
        if i > 0 && t.length_squared() > 1.0e-12 {
            let q = Quat::from_rotation_arc(last_t, t);
            u = (q * u).normalize_or_zero();
            v = (q * v).normalize_or_zero();
            last_t = t;
        }
        frames.push((u, v, last_t));
    }

    let mut mesh = Mesh::new();
    let rings = n;
    let mut ring_uvs = Vec::with_capacity(rings);
    for (i, (u, v, _)) in frames.iter().enumerate() {
        let origin = Vec3::from_array(path.points[i]);
        let t_frac = i as f32 / (rings - 1).max(1) as f32;
        let mut uvs = Vec::with_capacity(m);
        for (j, p) in profile_points.iter().enumerate() {
            let p = origin + *u * p[0] + *v * p[1];
            mesh.positions.push(p.to_array());
            let s = if m > 1 {
                j as f32 / if profile_closed { m } else { m - 1 } as f32
            } else {
                0.0
            };
            uvs.push([s, t_frac]);
        }
        ring_uvs.push(uvs);
    }
    for uvs in &ring_uvs {
        mesh.uvs.extend_from_slice(uvs);
    }

    let loops = if profile_closed { m } else { m - 1 };
    let connect = if path.closed { rings } else { rings - 1 };
    for i in 0..connect {
        let i0 = i;
        let i1 = (i + 1) % rings;
        for j in 0..loops {
            let j1 = (j + 1) % m;
            let a = (i0 * m + j) as u32;
            let b = (i0 * m + j1) as u32;
            let c = (i1 * m + j) as u32;
            let d = (i1 * m + j1) as u32;
            mesh.indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }

    let caps = opts.caps && !path.closed && profile_closed;
    if caps {
        let c0 = mesh.positions.len() as u32;
        let centroid0 = centroid(&profile_points);
        let (u0, v0, t0) = frames[0];
        let p0 = Vec3::from_array(path.points[0]) + u0 * centroid0[0] + v0 * centroid0[1];
        mesh.positions.push(p0.to_array());
        mesh.uvs.push([0.5, 0.0]);
        for j in 0..m {
            let j1 = (j + 1) % m;
            mesh.indices
                .extend_from_slice(&[c0, (j1) as u32, (j) as u32]);
        }
        let _ = t0;

        let c1 = mesh.positions.len() as u32;
        let (u1, v1, _) = frames[rings - 1];
        let p1 = Vec3::from_array(path.points[rings - 1]) + u1 * centroid0[0] + v1 * centroid0[1];
        mesh.positions.push(p1.to_array());
        mesh.uvs.push([0.5, 1.0]);
        let base = ((rings - 1) * m) as u32;
        for j in 0..m {
            let j1 = (j + 1) % m;
            mesh.indices
                .extend_from_slice(&[c1, base + j as u32, base + j1 as u32]);
        }
    }

    mesh.recompute_normals();
    mesh
}

fn centroid(points: &[[f32; 2]]) -> [f32; 2] {
    let mut c = [0.0f32; 2];
    for p in points {
        c[0] += p[0];
        c[1] += p[1];
    }
    let n = points.len().max(1) as f32;
    [c[0] / n, c[1] / n]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SplineKind;

    fn square() -> Curve2 {
        Curve2 {
            points: vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]],
            closed: true,
            kind: SplineKind::Polyline,
        }
    }

    #[test]
    fn straight_sweep_is_an_extrusion() {
        let path = Curve3 {
            points: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 3.0]],
            closed: false,
            kind: SplineKind::Polyline,
        };
        let profile = square();
        let opts = SweepOptions {
            caps: true,
            closed: true,
            segments: 10,
        };
        let mesh = sweep(&path, &profile, opts);
        let (min, max) = mesh.bbox().unwrap();
        assert!((min[0] + 1.0).abs() < 1.0e-4);
        assert!((max[0] - 1.0).abs() < 1.0e-4);
        assert!((min[1] + 1.0).abs() < 1.0e-4);
        assert!((max[1] - 1.0).abs() < 1.0e-4);
        assert!((min[2] - 0.0).abs() < 1.0e-4);
        assert!((max[2] - 3.0).abs() < 1.0e-4);

        let walls = (10 - 1) * 4 * 2;
        let caps = 2 * 4;
        assert_eq!(mesh.indices.len() / 3, walls + caps);
    }

    #[test]
    fn degenerate_paths_do_not_panic() {
        let path = Curve3 {
            points: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
            closed: false,
            kind: SplineKind::Polyline,
        };
        let mesh = sweep(&path, &square(), SweepOptions::default());
        assert!(mesh.is_empty());
        let empty = Curve3::default();
        assert!(sweep(&empty, &square(), SweepOptions::default()).is_empty());
    }

    #[test]
    fn open_profile_has_no_caps() {
        let path = Curve3 {
            points: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            closed: false,
            kind: SplineKind::Polyline,
        };
        let profile = Curve2 {
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            closed: false,
            kind: SplineKind::Polyline,
        };
        let mesh = sweep(
            &path,
            &profile,
            SweepOptions {
                caps: true,
                closed: false,
                segments: 4,
            },
        );
        assert_eq!(mesh.indices.len() / 3, 3 * 2 * 2);
    }
}
