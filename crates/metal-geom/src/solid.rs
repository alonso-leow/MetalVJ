//! Primitive solids.
//!
//! All primitives are indexed triangle meshes with outward-facing winding.
//! `+y` is up. `cuboid(size, center)` centres the box when `center` is true and
//! otherwise places its minimum corner at the origin, matching the Manifold
//! convention. `cylinder` and `cone` are centred vertically on the origin.

use glam::{Mat4, Quat, Vec3};

use crate::Mesh;

fn push_quad(mesh: &mut Mesh, p: [[f32; 3]; 4], normal: [f32; 3]) {
    let base = mesh.positions.len() as u32;
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    for (i, &pos) in p.iter().enumerate() {
        mesh.positions.push(pos);
        mesh.normals.push(normal);
        mesh.uvs.push(uvs[i]);
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Centred plane in the XZ plane at `y = 0`, normal `+y`, two triangles.
pub fn plane(width: f32, height: f32) -> Mesh {
    let (hw, hh) = (width * 0.5, height * 0.5);
    let mut mesh = Mesh::new();
    push_quad(
        &mut mesh,
        [
            [-hw, 0.0, -hh],
            [-hw, 0.0, hh],
            [hw, 0.0, hh],
            [hw, 0.0, -hh],
        ],
        [0.0, 1.0, 0.0],
    );
    mesh
}

/// Axis-aligned box. `center = false` places the minimum corner at the origin.
pub fn cuboid(size: [f32; 3], center: bool) -> Mesh {
    let half = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
    let (min, max) = if center {
        ([-half[0], -half[1], -half[2]], [half[0], half[1], half[2]])
    } else {
        ([0.0, 0.0, 0.0], [size[0], size[1], size[2]])
    };
    box_between(min, max)
}

fn box_between(min: [f32; 3], max: [f32; 3]) -> Mesh {
    let (x0, y0, z0) = (min[0], min[1], min[2]);
    let (x1, y1, z1) = (max[0], max[1], max[2]);
    let mut m = Mesh::new();
    // +x
    push_quad(
        &mut m,
        [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
        [1.0, 0.0, 0.0],
    );
    // -x
    push_quad(
        &mut m,
        [[x0, y0, z1], [x0, y1, z1], [x0, y1, z0], [x0, y0, z0]],
        [-1.0, 0.0, 0.0],
    );
    // +y
    push_quad(
        &mut m,
        [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
        [0.0, 1.0, 0.0],
    );
    // -y
    push_quad(
        &mut m,
        [[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]],
        [0.0, -1.0, 0.0],
    );
    // +z
    push_quad(
        &mut m,
        [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        [0.0, 0.0, 1.0],
    );
    // -z
    push_quad(
        &mut m,
        [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
        [0.0, 0.0, -1.0],
    );
    m
}

fn ring(radius: f32, y: f32, segments: usize) -> Vec<[f32; 3]> {
    let n = segments.max(3);
    (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            [radius * a.cos(), y, -radius * a.sin()]
        })
        .collect()
}

/// Cylinder along `+y`, centred at the origin.
pub fn cylinder(radius: f32, height: f32, segments: usize) -> Mesh {
    let n = segments.max(3);
    let (y0, y1) = (-height * 0.5, height * 0.5);
    let bottom = ring(radius, y0, n);
    let top = ring(radius, y1, n);
    let mut m = Mesh::new();
    for p in &bottom {
        m.positions.push(*p);
        m.uvs.push([0.0, 0.0]);
    }
    for p in &top {
        m.positions.push(*p);
        m.uvs.push([0.0, 1.0]);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let b0 = i as u32;
        let b1 = j as u32;
        let t0 = (n + i) as u32;
        let t1 = (n + j) as u32;
        m.indices.extend_from_slice(&[b0, b1, t0, b1, t1, t0]);
    }
    // Caps: duplicated rings with a centre vertex each.
    for (y, up) in [(y0, false), (y1, true)] {
        let center = m.positions.len() as u32;
        m.positions.push([0.0, y, 0.0]);
        m.uvs.push([0.5, 0.5]);
        let base = m.positions.len() as u32;
        let pts = ring(radius, y, n);
        for p in &pts {
            m.positions.push(*p);
            m.uvs.push([0.5 + p[0], 0.5 + p[2]]);
        }
        for i in 0..n {
            let j = (i + 1) % n;
            if up {
                m.indices
                    .extend_from_slice(&[center, base + i as u32, base + j as u32]);
            } else {
                m.indices
                    .extend_from_slice(&[center, base + j as u32, base + i as u32]);
            }
        }
    }
    m.recompute_normals();
    m
}

/// Cone along `+y`: base at `-height/2`, apex at `+height/2`.
pub fn cone(radius: f32, height: f32, segments: usize) -> Mesh {
    let n = segments.max(3);
    let y0 = -height * 0.5;
    let y1 = height * 0.5;
    let base = ring(radius, y0, n);
    let mut m = Mesh::new();
    for p in &base {
        m.positions.push(*p);
        m.uvs.push([0.0, 0.0]);
    }
    let apex = m.positions.len() as u32;
    m.positions.push([0.0, y1, 0.0]);
    m.uvs.push([0.5, 1.0]);
    for i in 0..n {
        let j = (i + 1) % n;
        m.indices.extend_from_slice(&[i as u32, j as u32, apex]);
    }
    let center = m.positions.len() as u32;
    m.positions.push([0.0, y0, 0.0]);
    m.uvs.push([0.5, 0.5]);
    let offset = m.positions.len() as u32;
    for p in &base {
        m.positions.push(*p);
        m.uvs.push([0.5 + p[0], 0.5 + p[2]]);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        m.indices
            .extend_from_slice(&[center, offset + j as u32, offset + i as u32]);
    }
    m.recompute_normals();
    m
}

/// UV sphere centred at the origin.
pub fn sphere(radius: f32, segments: usize) -> Mesh {
    let n = segments.max(4);
    let stacks = (n / 2).max(2);
    let mut m = Mesh::new();
    // Single top and bottom pole vertices keep the topology watertight.
    m.positions.push([0.0, radius, 0.0]);
    m.uvs.push([0.5, 0.0]);
    for s in 1..stacks {
        let phi = std::f32::consts::PI * s as f32 / stacks as f32;
        let y = radius * phi.cos();
        let r = radius * phi.sin();
        for i in 0..n {
            let theta = std::f32::consts::TAU * i as f32 / n as f32;
            m.positions.push([r * theta.cos(), y, -r * theta.sin()]);
            m.uvs.push([i as f32 / n as f32, s as f32 / stacks as f32]);
        }
    }
    let bottom = m.positions.len() as u32;
    m.positions.push([0.0, -radius, 0.0]);
    m.uvs.push([0.5, 1.0]);

    let row = |s: usize, i: usize| 1 + (s * n + i) as u32;
    // Top cap.
    for i in 0..n {
        let j = (i + 1) % n;
        m.indices.extend_from_slice(&[0, row(0, i), row(0, j)]);
    }
    // Middle bands (interior rows 0 .. stacks-2).
    for s in 0..stacks.saturating_sub(2) {
        for i in 0..n {
            let j = (i + 1) % n;
            let u0 = row(s, i);
            let u1 = row(s, j);
            let l0 = row(s + 1, i);
            let l1 = row(s + 1, j);
            m.indices.extend_from_slice(&[u0, l0, u1, u1, l0, l1]);
        }
    }
    // Bottom cap.
    let last = stacks - 2;
    for i in 0..n {
        let j = (i + 1) % n;
        m.indices
            .extend_from_slice(&[row(last, i), bottom, row(last, j)]);
    }
    m.recompute_normals();
    m
}

/// Flat grid in the XZ plane at `y = 0`, `x` by `z` cells.
pub fn grid(width: f32, depth: f32, x: usize, z: usize) -> Mesh {
    let nx = x.max(1);
    let nz = z.max(1);
    let mut m = Mesh::new();
    for iz in 0..=nz {
        for ix in 0..=nx {
            let u = ix as f32 / nx as f32;
            let v = iz as f32 / nz as f32;
            m.positions
                .push([-width * 0.5 + width * u, 0.0, -depth * 0.5 + depth * v]);
            m.uvs.push([u, v]);
            m.normals.push([0.0, 1.0, 0.0]);
        }
    }
    let stride = nx + 1;
    for iz in 0..nz {
        for ix in 0..nx {
            let a = (iz * stride + ix) as u32;
            let b = (iz * stride + ix + 1) as u32;
            let c = ((iz + 1) * stride + ix + 1) as u32;
            let d = ((iz + 1) * stride + ix) as u32;
            m.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    m
}

/// Thin cylinder from `a` to `b`.
pub fn line(a: [f32; 3], b: [f32; 3], radius: f32, segments: usize) -> Mesh {
    let va = Vec3::from_array(a);
    let vb = Vec3::from_array(b);
    let d = vb - va;
    let len = d.length();
    if len <= 1.0e-9 {
        return Mesh::new();
    }
    let m = cylinder(radius, len, segments);
    let rot = Quat::from_rotation_arc(Vec3::Y, d / len);
    let mat = Mat4::from_translation((va + vb) * 0.5) * Mat4::from_quat(rot);
    m.transformed(mat.to_cols_array_2d())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_watertight(input: &Mesh) {
        use std::collections::HashMap;
        let mut m = input.clone();
        m.weld(1.0e-5);
        let m = &m;
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for tri in m.indices.as_chunks::<3>().0 {
            for k in 0..3 {
                let a = tri[k];
                let b = tri[(k + 1) % 3];
                let key = if a < b { (a, b) } else { (b, a) };
                *edges.entry(key).or_insert(0) += if a < b { 1 } else { -1 };
            }
        }
        assert!(
            edges.values().all(|&v| v == 0),
            "mesh is not watertight: {edges:?}"
        );
    }

    #[test]
    fn plane_is_two_triangles_facing_up() {
        let m = plane(2.0, 3.0);
        assert_eq!(m.indices.len() / 3, 2);
        let (min, max) = m.bbox().unwrap();
        assert_eq!(min, [-1.0, 0.0, -1.5]);
        assert_eq!(max, [1.0, 0.0, 1.5]);
        assert!(m.normals.iter().all(|n| n[1] > 0.9));
    }

    #[test]
    fn cuboid_is_watertight_and_centred() {
        let m = cuboid([2.0, 4.0, 6.0], true);
        let (min, max) = m.bbox().unwrap();
        assert_eq!(min, [-1.0, -2.0, -3.0]);
        assert_eq!(max, [1.0, 2.0, 3.0]);
        assert_eq!(m.indices.len() / 3, 12);
        assert_watertight(&m);
    }

    #[test]
    fn cuboid_corner_mode() {
        let m = cuboid([2.0, 3.0, 4.0], false);
        assert_eq!(m.bbox().unwrap().0, [0.0, 0.0, 0.0]);
        assert_eq!(m.bbox().unwrap().1, [2.0, 3.0, 4.0]);
    }

    #[test]
    fn cylinder_and_cone_are_watertight() {
        let c = cylinder(1.0, 2.0, 16);
        let b = c.bbox().unwrap();
        assert!((b.0[1] + 1.0).abs() < 1.0e-5);
        assert!((b.1[1] - 1.0).abs() < 1.0e-5);
        assert_watertight(&c);

        let k = cone(1.0, 2.0, 16);
        let b = k.bbox().unwrap();
        assert!((b.0[1] + 1.0).abs() < 1.0e-5);
        assert!((b.1[1] - 1.0).abs() < 1.0e-5);
        assert_watertight(&k);
    }

    #[test]
    fn sphere_bbox_and_watertight() {
        let s = sphere(2.0, 16);
        let (min, max) = s.bbox().unwrap();
        for k in 0..3 {
            assert!((min[k] + 2.0).abs() < 1.0e-3);
            assert!((max[k] - 2.0).abs() < 1.0e-3);
        }
        assert_watertight(&s);
    }

    #[test]
    fn grid_is_flat() {
        let g = grid(4.0, 2.0, 4, 2);
        assert_eq!(g.indices.len() / 3, 4 * 2 * 2);
        assert!(g.positions.iter().all(|p| p[1] == 0.0));
        assert!(g.normals.iter().all(|n| n[1] == 1.0));
    }

    #[test]
    fn line_reaches_endpoints() {
        let m = line([0.0, 0.0, 0.0], [0.0, 2.0, 0.0], 0.1, 8);
        let (min, max) = m.bbox().unwrap();
        assert!((min[1] + 0.0).abs() < 1.0e-4);
        assert!((max[1] - 2.0).abs() < 1.0e-4);
        assert_watertight(&m);
    }
}
