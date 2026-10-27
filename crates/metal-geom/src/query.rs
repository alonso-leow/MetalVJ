//! Ray queries over a triangle BVH.
//!
//! The BVH is a median split on the longest axis of each node's AABB, with
//! leaves of up to four triangles. `raycast` is exact Möller–Trumbore against
//! the candidate triangles (nearest positive hit within `max_distance`).
//! `nearest_point` computes the exact closest point on each triangle and
//! returns the nearest one as a hit whose `distance` is the point distance.

use crate::mesh::Mesh;
use crate::Triangle;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub distance: f32,
    pub at: [f32; 3],
    pub normal: [f32; 3],
    pub triangle: usize,
}

#[derive(Clone, Copy, Debug)]
struct Aabb {
    min: [f32; 3],
    max: [f32; 3],
}

impl Aabb {
    fn empty() -> Self {
        Self {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        }
    }

    fn from_triangle(t: &Triangle) -> Self {
        let mut b = Self { min: t.a, max: t.a };
        for p in [t.b, t.c] {
            for (k, &v) in p.iter().enumerate() {
                b.min[k] = b.min[k].min(v);
                b.max[k] = b.max[k].max(v);
            }
        }
        b
    }

    fn union(&mut self, other: &Self) {
        for k in 0..3 {
            self.min[k] = self.min[k].min(other.min[k]);
            self.max[k] = self.max[k].max(other.max[k]);
        }
    }

    fn longest_axis(&self) -> usize {
        let ext = [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ];
        if ext[0] >= ext[1] && ext[0] >= ext[2] {
            0
        } else if ext[1] >= ext[2] {
            1
        } else {
            2
        }
    }

    fn intersects(&self, origin: [f32; 3], inv_dir: [f32; 3], max_t: f32) -> bool {
        let mut tmin = 0.0f32;
        let mut tmax = max_t;
        for k in 0..3 {
            let o = origin[k];
            let inv = inv_dir[k];
            if inv.is_infinite() {
                if o < self.min[k] || o > self.max[k] {
                    return false;
                }
                continue;
            }
            let mut t0 = (self.min[k] - o) * inv;
            let mut t1 = (self.max[k] - o) * inv;
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            tmin = tmin.max(t0);
            tmax = tmax.min(t1);
            if tmin > tmax {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb,
    start: u32,
    count: u32,
    left: u32,
    right: u32,
}

const LEAF: u32 = u32::MAX;
const LEAF_SIZE: usize = 4;

#[derive(Clone, Debug, Default)]
pub struct Bvh {
    triangles: Vec<Triangle>,
    order: Vec<u32>,
    nodes: Vec<Node>,
}

impl Bvh {
    pub fn build(mesh: &Mesh) -> Self {
        let triangles = mesh.triangles();
        let n = triangles.len();
        if n == 0 {
            return Self::default();
        }
        let centers: Vec<[f32; 3]> = triangles
            .iter()
            .map(|t| {
                [
                    (t.a[0] + t.b[0] + t.c[0]) / 3.0,
                    (t.a[1] + t.b[1] + t.c[1]) / 3.0,
                    (t.a[2] + t.b[2] + t.c[2]) / 3.0,
                ]
            })
            .collect();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut nodes = Vec::new();
        build_node(&triangles, &centers, &mut order, 0, n, &mut nodes);
        Self {
            triangles,
            order,
            nodes,
        }
    }

    pub fn raycast(&self, origin: [f32; 3], dir: [f32; 3], max_distance: f32) -> Option<RayHit> {
        if self.nodes.is_empty() || max_distance < 0.0 {
            return None;
        }
        let d = glam::Vec3::from_array(dir);
        let len = d.length();
        if len <= 1.0e-12 {
            return None;
        }
        let d = d / len;
        let dir = d.to_array();
        let inv = [
            if dir[0].abs() > 1.0e-20 {
                1.0 / dir[0]
            } else {
                f32::INFINITY
            },
            if dir[1].abs() > 1.0e-20 {
                1.0 / dir[1]
            } else {
                f32::INFINITY
            },
            if dir[2].abs() > 1.0e-20 {
                1.0 / dir[2]
            } else {
                f32::INFINITY
            },
        ];
        let o = glam::Vec3::from_array(origin);

        let mut best = max_distance;
        let mut hit: Option<RayHit> = None;
        let mut stack = vec![0u32];
        while let Some(node_idx) = stack.pop() {
            let node = &self.nodes[node_idx as usize];
            if !node.bounds.intersects(origin, inv, best) {
                continue;
            }
            if node.left == LEAF {
                let start = node.start as usize;
                let end = start + node.count as usize;
                for &tri_id in &self.order[start..end] {
                    let tri = &self.triangles[tri_id as usize];
                    if let Some(t) = moller_trumbore(origin, dir, tri) {
                        if t > 1.0e-6 && t <= best {
                            let normal = face_normal(tri);
                            if normal.length_squared() > 1.0e-20 {
                                best = t;
                                hit = Some(RayHit {
                                    distance: t,
                                    at: (o + d * t).to_array(),
                                    normal: normal.normalize().to_array(),
                                    triangle: tri_id as usize,
                                });
                            }
                        }
                    }
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        hit
    }

    /// Exact closest point on the mesh surface.
    pub fn nearest_point(&self, p: [f32; 3]) -> Option<RayHit> {
        if self.triangles.is_empty() {
            return None;
        }
        let point = glam::Vec3::from_array(p);
        let mut best = f32::INFINITY;
        let mut hit = None;
        for (i, tri) in self.triangles.iter().enumerate() {
            let q = closest_on_triangle(point, tri);
            let d = (q - point).length_squared();
            if d < best {
                best = d;
                let normal = face_normal(tri);
                hit = Some(RayHit {
                    distance: d.sqrt(),
                    at: q.to_array(),
                    normal: if normal.length_squared() > 1.0e-20 {
                        normal.normalize().to_array()
                    } else {
                        [0.0, 1.0, 0.0]
                    },
                    triangle: i,
                });
            }
        }
        hit
    }
}

fn build_node(
    triangles: &[Triangle],
    centers: &[[f32; 3]],
    order: &mut [u32],
    start: usize,
    end: usize,
    nodes: &mut Vec<Node>,
) -> u32 {
    let idx = nodes.len() as u32;
    nodes.push(Node {
        bounds: Aabb::empty(),
        start: start as u32,
        count: (end - start) as u32,
        left: LEAF,
        right: LEAF,
    });
    let mut bounds = Aabb::empty();
    for &id in &order[start..end] {
        bounds.union(&Aabb::from_triangle(&triangles[id as usize]));
    }
    nodes[idx as usize].bounds = bounds;

    let count = end - start;
    if count <= LEAF_SIZE {
        return idx;
    }
    let axis = bounds.longest_axis();
    let extent = bounds.max[axis] - bounds.min[axis];
    if extent <= 1.0e-9 {
        return idx;
    }
    let mid = start + count / 2;
    order[start..end].select_nth_unstable_by(mid - start, |&a, &b| {
        centers[a as usize][axis]
            .partial_cmp(&centers[b as usize][axis])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let left = build_node(triangles, centers, order, start, mid, nodes);
    let right = build_node(triangles, centers, order, mid, end, nodes);
    nodes[idx as usize].left = left;
    nodes[idx as usize].right = right;
    idx
}

fn moller_trumbore(origin: [f32; 3], dir: [f32; 3], tri: &Triangle) -> Option<f32> {
    let o = glam::Vec3::from_array(origin);
    let d = glam::Vec3::from_array(dir);
    let a = glam::Vec3::from_array(tri.a);
    let b = glam::Vec3::from_array(tri.b);
    let c = glam::Vec3::from_array(tri.c);
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1.0e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv_det;
    if !(-1.0e-6..=1.0 + 1.0e-6).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv_det;
    if v < -1.0e-6 || u + v > 1.0 + 1.0e-6 {
        return None;
    }
    let t = e2.dot(q) * inv_det;
    if t.is_finite() {
        Some(t)
    } else {
        None
    }
}

fn face_normal(tri: &Triangle) -> glam::Vec3 {
    let a = glam::Vec3::from_array(tri.a);
    let b = glam::Vec3::from_array(tri.b);
    let c = glam::Vec3::from_array(tri.c);
    (b - a).cross(c - a)
}

fn closest_on_triangle(p: glam::Vec3, tri: &Triangle) -> glam::Vec3 {
    let a = glam::Vec3::from_array(tri.a);
    let b = glam::Vec3::from_array(tri.b);
    let c = glam::Vec3::from_array(tri.c);
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let denom = d1 - d3;
        let v = if denom.abs() > 1.0e-20 {
            d1 / denom
        } else {
            0.0
        };
        return a + ab * v;
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let denom = d2 - d6;
        let w = if denom.abs() > 1.0e-20 {
            d2 / denom
        } else {
            0.0
        };
        return a + ac * w;
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let denom = (d4 - d3) + (d5 - d6);
        let w = if denom.abs() > 1.0e-20 {
            (d4 - d3) / denom
        } else {
            0.0
        };
        return b + (c - b) * w;
    }
    let denom = va + vb + vc;
    if denom.abs() <= 1.0e-20 {
        return a;
    }
    let v = vb / denom;
    let w = vc / denom;
    a + ab * v + ac * w
}

/// Absolute area of a simple 2D polygon via the shoelace formula.
pub fn projected_area(points: &[[f32; 2]]) -> f32 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        sum += a[0] as f64 * b[1] as f64 - b[0] as f64 * a[1] as f64;
    }
    (sum * 0.5).abs() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solid;

    #[test]
    fn projected_area_of_unit_square() {
        let square = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!((projected_area(&square) - 1.0).abs() < 1.0e-6);
        let reversed = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        assert!((projected_area(&reversed) - 1.0).abs() < 1.0e-6);
        assert_eq!(projected_area(&[[0.0, 0.0], [1.0, 0.0]]), 0.0);
    }

    #[test]
    fn ray_hits_box_at_expected_distance() {
        let m = solid::cuboid([2.0, 2.0, 2.0], true);
        let bvh = Bvh::build(&m);
        let hit = bvh
            .raycast([-5.0, 0.0, 0.0], [1.0, 0.0, 0.0], 100.0)
            .unwrap();
        assert!((hit.distance - 4.0).abs() < 1.0e-4, "{}", hit.distance);
        assert!((hit.at[0] + 1.0).abs() < 1.0e-4);
        assert!(hit.normal[0] < -0.9);
        assert!(hit.triangle < m.indices.len() / 3);
    }

    #[test]
    fn ray_respects_max_distance_and_misses() {
        let m = solid::cuboid([2.0, 2.0, 2.0], true);
        let bvh = Bvh::build(&m);
        assert!(bvh
            .raycast([-5.0, 0.0, 0.0], [1.0, 0.0, 0.0], 2.0)
            .is_none());
        assert!(bvh
            .raycast([-5.0, 3.0, 0.0], [1.0, 0.0, 0.0], 100.0)
            .is_none());
        assert!(bvh
            .raycast([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 10.0)
            .is_none());
    }

    #[test]
    fn nearest_point_on_box() {
        let m = solid::cuboid([2.0, 2.0, 2.0], true);
        let bvh = Bvh::build(&m);
        let hit = bvh.nearest_point([3.0, 0.0, 0.0]).unwrap();
        assert!((hit.distance - 2.0).abs() < 1.0e-4, "{}", hit.distance);
        assert!((hit.at[0] - 1.0).abs() < 1.0e-4);
        assert!(hit.normal[0] > 0.9);
    }

    #[test]
    fn empty_bvh_is_none() {
        let b = Bvh::build(&Mesh::new());
        assert!(b.raycast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 10.0).is_none());
        assert!(b.nearest_point([0.0, 0.0, 0.0]).is_none());
    }
}
