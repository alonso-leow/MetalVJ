//! Indexed triangle mesh with per-corner attributes.
//!
//! `indices` is a flat triangle list: every three entries form one triangle.
//! Positions, normals and uvs are per-vertex. Empty meshes are valid and every
//! query here tolerates them.

use crate::Triangle;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Axis-aligned bounds `(min, max)` over the positions, if any exist.
    pub fn bbox(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.positions.iter();
        let first = *it.next()?;
        let mut min = first;
        let mut max = first;
        for p in it {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        Some((min, max))
    }

    /// Expand the index list into a triangle view. Out-of-range indices are
    /// skipped rather than panicking.
    pub fn triangles(&self) -> Vec<Triangle> {
        let mut out = Vec::with_capacity(self.indices.len() / 3);
        for tri in self.indices.as_chunks::<3>().0 {
            let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if let (Some(&a), Some(&b), Some(&c)) = (
                self.positions.get(a),
                self.positions.get(b),
                self.positions.get(c),
            ) {
                out.push(Triangle { a, b, c });
            }
        }
        out
    }

    /// Apply a 4x4 transform given in **column-major** order, matching the
    /// memory layout of `glam::Mat4` / wgpu uniform matrices.
    pub fn transformed(&self, m: [[f32; 4]; 4]) -> Mesh {
        let mut out = self.clone();
        for p in &mut out.positions {
            let x = m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0];
            let y = m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1];
            let z = m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2];
            *p = [x, y, z];
        }
        if !out.normals.is_empty() {
            let a = glam::Mat3::from_cols(
                glam::Vec3::new(m[0][0], m[0][1], m[0][2]),
                glam::Vec3::new(m[1][0], m[1][1], m[1][2]),
                glam::Vec3::new(m[2][0], m[2][1], m[2][2]),
            );
            let normal_mat = if a.determinant().abs() > 1.0e-12 {
                a.inverse().transpose()
            } else {
                a
            };
            for n in &mut out.normals {
                let v = normal_mat * glam::Vec3::from_array(*n);
                *n = if v.length_squared() > 1.0e-20 {
                    v.normalize().to_array()
                } else {
                    [0.0, 1.0, 0.0]
                };
            }
        }
        out
    }

    /// Recompute smooth per-vertex normals by area-weighted accumulation of
    /// face normals. Meshes with no indices keep an empty normal list.
    pub fn recompute_normals(&mut self) {
        self.normals.clear();
        if self.positions.is_empty() || self.indices.len() < 3 {
            return;
        }
        let mut acc = vec![glam::Vec3::ZERO; self.positions.len()];
        for tri in self.indices.as_chunks::<3>().0 {
            let (ia, ib, ic) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let (Some(&a), Some(&b), Some(&c)) = (
                self.positions.get(ia),
                self.positions.get(ib),
                self.positions.get(ic),
            ) else {
                continue;
            };
            let a = glam::Vec3::from_array(a);
            let b = glam::Vec3::from_array(b);
            let c = glam::Vec3::from_array(c);
            let n = (b - a).cross(c - a);
            if n.length_squared() > 1.0e-20 {
                acc[ia] += n;
                acc[ib] += n;
                acc[ic] += n;
            }
        }
        self.normals = acc
            .into_iter()
            .map(|n| {
                if n.length_squared() > 1.0e-20 {
                    n.normalize().to_array()
                } else {
                    [0.0, 1.0, 0.0]
                }
            })
            .collect();
    }

    /// Merge positions closer than `eps`, remapping indices and keeping the
    /// first attribute set seen for each merged vertex. `eps <= 0` is a no-op.
    pub fn weld(&mut self, eps: f32) {
        if eps <= 0.0 || self.positions.is_empty() {
            return;
        }
        let cell = eps.max(1.0e-9);
        let inv = 1.0 / cell;
        let mut buckets: std::collections::HashMap<(i64, i64, i64), Vec<usize>> =
            std::collections::HashMap::new();
        let mut remap = vec![0usize; self.positions.len()];
        let mut reps: Vec<usize> = Vec::new();
        let key = |p: [f32; 3]| {
            (
                (p[0] * inv).round() as i64,
                (p[1] * inv).round() as i64,
                (p[2] * inv).round() as i64,
            )
        };
        let eps2 = eps * eps;
        for (i, &p) in self.positions.iter().enumerate() {
            let k = key(p);
            let mut found = None;
            'search: for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(ids) = buckets.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                            for &r in ids {
                                let q = self.positions[r];
                                let d = glam::Vec3::from_array(p) - glam::Vec3::from_array(q);
                                if d.length_squared() <= eps2 {
                                    found = Some(r);
                                    break 'search;
                                }
                            }
                        }
                    }
                }
            }
            match found {
                Some(r) => remap[i] = r,
                None => {
                    remap[i] = i;
                    reps.push(i);
                    buckets.entry(k).or_default().push(i);
                }
            }
        }
        if reps.len() == self.positions.len() {
            return;
        }

        let mut new_id = vec![usize::MAX; self.positions.len()];
        for (slot, &r) in reps.iter().enumerate() {
            new_id[r] = slot;
        }
        let mut local = vec![0usize; self.positions.len()];
        for (i, slot) in local.iter_mut().enumerate() {
            *slot = new_id[remap[i]];
        }

        let has_normals = self.normals.len() == self.positions.len();
        let has_uvs = self.uvs.len() == self.positions.len();
        let mut acc_pos = vec![glam::Vec3::ZERO; reps.len()];
        let mut acc_n = vec![glam::Vec3::ZERO; reps.len()];
        let mut acc_uv = vec![[0.0f32; 2]; reps.len()];
        let mut counts = vec![0u32; reps.len()];
        for (i, &p) in self.positions.iter().enumerate() {
            let slot = local[i];
            counts[slot] += 1;
            acc_pos[slot] += glam::Vec3::from_array(p);
            if has_normals {
                acc_n[slot] += glam::Vec3::from_array(self.normals[i]);
            }
            if has_uvs {
                acc_uv[slot][0] += self.uvs[i][0];
                acc_uv[slot][1] += self.uvs[i][1];
            }
        }
        let mut new_pos = Vec::with_capacity(reps.len());
        let mut new_normals = Vec::with_capacity(reps.len());
        let mut new_uvs = Vec::with_capacity(reps.len());
        for slot in 0..reps.len() {
            let c = counts[slot].max(1) as f32;
            new_pos.push((acc_pos[slot] / c).to_array());
            let n = acc_n[slot];
            new_normals.push(if n.length_squared() > 1.0e-20 {
                n.normalize().to_array()
            } else {
                [0.0, 1.0, 0.0]
            });
            new_uvs.push([acc_uv[slot][0] / c, acc_uv[slot][1] / c]);
        }

        self.positions = new_pos;
        self.normals = if has_normals { new_normals } else { Vec::new() };
        self.uvs = if has_uvs { new_uvs } else { Vec::new() };
        for idx in &mut self.indices {
            let old = *idx as usize;
            if old < local.len() {
                *idx = local[old] as u32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> Mesh {
        Mesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
            ],
            normals: vec![],
            uvs: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
        }
    }

    #[test]
    fn empty_mesh_is_safe() {
        let m = Mesh::new();
        assert_eq!(m.bbox(), None);
        assert!(m.triangles().is_empty());
        let mut n = m.clone();
        n.recompute_normals();
        n.weld(0.1);
        assert!(n.is_empty());
        assert!(n.normals.is_empty());
    }

    #[test]
    fn bbox_and_triangles() {
        let m = quad();
        let (min, max) = m.bbox().unwrap();
        assert_eq!(min, [0.0, 0.0, 0.0]);
        assert_eq!(max, [1.0, 0.0, 1.0]);
        assert_eq!(m.triangles().len(), 2);
    }

    #[test]
    fn transform_is_column_major() {
        let m = quad();
        let mut t = [[0.0f32; 4]; 4];
        t[0][0] = 1.0;
        t[1][1] = 1.0;
        t[2][2] = 1.0;
        t[3][3] = 1.0;
        t[3][0] = 2.0;
        t[3][1] = 3.0;
        let out = m.transformed(t);
        let (min, max) = out.bbox().unwrap();
        assert_eq!(min, [2.0, 3.0, 0.0]);
        assert_eq!(max, [3.0, 3.0, 1.0]);
    }

    #[test]
    fn normals_point_up() {
        let mut m = quad();
        m.recompute_normals();
        for n in &m.normals {
            assert!((n[1] - 1.0).abs() < 1.0e-5);
        }
    }

    #[test]
    fn weld_merges_duplicates() {
        let mut m = quad();
        m.positions.push([0.0, 0.0, 0.0]);
        m.positions.push([1.0, 0.0, 0.0]);
        m.indices.extend_from_slice(&[4, 5, 2]);
        m.weld(1.0e-5);
        assert_eq!(m.positions.len(), 4);
        assert!(m.indices.iter().all(|&i| i < 4));
    }
}
