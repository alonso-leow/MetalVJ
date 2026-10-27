//! Robust CSG via the Manifold library.
//!
//! Manifold expects a watertight, consistently-wound triangle mesh. Inputs are
//! welded by position before import; if construction still fails the mesh is
//! reported as [`CsgError::NonManifold`] rather than panicking. Note that
//! coplanar or exactly-touching faces can occasionally make a boolean
//! degenerate; those failures surface as [`CsgError::Unknown`].

use std::panic::{catch_unwind, AssertUnwindSafe};

use manifold3d::{Manifold, OpType};

use crate::mesh::Mesh;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BooleanOp {
    Intersect,
    Union,
    Difference,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CsgError {
    NonManifold,
    ResultTooBig,
    SolverNotAvailable,
    Unknown(String),
}

fn map_manifold_error(e: manifold3d::CsgError) -> CsgError {
    match e {
        manifold3d::CsgError::InvalidInput(msg) => CsgError::Unknown(msg),
        manifold3d::CsgError::ManifoldStatus(status) => {
            let _ = status;
            CsgError::NonManifold
        }
        other => CsgError::Unknown(format!("{other:?}")),
    }
}

fn to_manifold(mesh: &Mesh) -> Result<Manifold, CsgError> {
    if mesh.indices.len() < 3 || mesh.positions.is_empty() {
        return Err(CsgError::NonManifold);
    }
    let mut welded = mesh.clone();
    welded.weld(1.0e-5);
    if welded.indices.len() < 3 {
        return Err(CsgError::NonManifold);
    }
    let mut verts = Vec::with_capacity(welded.positions.len() * 3);
    for p in &welded.positions {
        verts.extend_from_slice(p);
    }
    Manifold::from_mesh_f32(&verts, 3, &welded.indices).map_err(map_manifold_error)
}

fn from_manifold(m: &Manifold) -> Result<Mesh, CsgError> {
    m.status().map_err(map_manifold_error)?;
    let (verts, n_props, indices) = m.to_mesh_f32();
    if n_props < 3 {
        return Err(CsgError::Unknown(format!(
            "manifold returned {n_props} vertex properties"
        )));
    }
    let mut mesh = Mesh::new();
    mesh.positions.reserve(verts.len() / n_props);
    for chunk in verts.chunks_exact(n_props) {
        mesh.positions.push([chunk[0], chunk[1], chunk[2]]);
    }
    mesh.indices = indices;
    mesh.recompute_normals();
    Ok(mesh)
}

/// Boolean operation between two meshes.
pub fn boolean(op: BooleanOp, a: &Mesh, b: &Mesh) -> Result<Mesh, CsgError> {
    let op_type = match op {
        BooleanOp::Union => OpType::Add,
        BooleanOp::Difference => OpType::Subtract,
        BooleanOp::Intersect => OpType::Intersect,
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let ma = to_manifold(a)?;
        let mb = to_manifold(b)?;
        let out = ma.boolean(&mb, op_type);
        from_manifold(&out)
    }));
    result.unwrap_or_else(|_| Err(CsgError::Unknown("manifold panicked".to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solid;
    use crate::Mesh;

    fn volume_bbox(m: &Mesh) -> ([f32; 3], [f32; 3]) {
        m.bbox().unwrap()
    }

    #[test]
    fn union_grows_bbox() {
        let a = solid::cuboid([2.0, 2.0, 2.0], true);
        let b = solid::cuboid([2.0, 2.0, 2.0], true).transformed({
            let mut t = [[0.0f32; 4]; 4];
            t[0][0] = 1.0;
            t[1][1] = 1.0;
            t[2][2] = 1.0;
            t[3][3] = 1.0;
            t[3][0] = 1.0;
            t
        });
        let u = boolean(BooleanOp::Union, &a, &b).unwrap();
        let (min, max) = volume_bbox(&u);
        assert!((max[0] - 2.0).abs() < 1.0e-3, "max x {}", max[0]);
        assert!((min[0] + 1.0).abs() < 1.0e-3);
        assert_eq!(u.indices.len() % 3, 0);
        assert!(!u.normals.is_empty());
    }

    #[test]
    fn difference_shrinks_bbox() {
        let a = solid::cuboid([4.0, 4.0, 4.0], true);
        let b = solid::cuboid([6.0, 1.0, 1.0], true);
        let d = boolean(BooleanOp::Difference, &a, &b).unwrap();
        let (min, max) = volume_bbox(&d);
        assert!((max[0] - 2.0).abs() < 1.0e-3);
        assert!((min[0] + 2.0).abs() < 1.0e-3);
        // Cross-section is reduced even though the outer bbox is unchanged.
        assert!(d.indices.len() / 3 > 12);
    }

    #[test]
    fn intersect_of_offset_boxes_is_smaller() {
        let a = solid::cuboid([2.0, 2.0, 2.0], true);
        let b = solid::cuboid([2.0, 2.0, 2.0], true).transformed({
            let mut t = [[0.0f32; 4]; 4];
            t[0][0] = 1.0;
            t[1][1] = 1.0;
            t[2][2] = 1.0;
            t[3][3] = 1.0;
            t[3][0] = 1.0;
            t
        });
        let i = boolean(BooleanOp::Intersect, &a, &b).unwrap();
        let (min, max) = volume_bbox(&i);
        assert!((min[0] - 0.0).abs() < 1.0e-3);
        assert!((max[0] - 1.0).abs() < 1.0e-3);
    }

    #[test]
    fn invalid_input_is_an_error_not_a_panic() {
        let a = Mesh::new();
        let b = solid::cuboid([1.0, 1.0, 1.0], true);
        let r = boolean(BooleanOp::Union, &a, &b);
        assert!(matches!(r, Err(CsgError::NonManifold)));
    }
}
