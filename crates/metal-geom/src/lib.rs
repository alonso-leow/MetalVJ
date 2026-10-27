//! `metal-geom` — the venue geometry kernel.
//!
//! Pure geometry: indexed meshes, primitives, curves, sweeps, CSG and ray
//! queries. No egui, no wgpu, no I/O, no venue types. The venue
//! (`metal-perform::venue`) is the first client; a future 3D model preview is
//! the second, which is why nothing here knows what a projector or a mask is.
//!
//! The design borrows Blender's operator discipline (pure ops, named inputs,
//! per-op marking) without its half-edge B-rep or GPL source. See
//! `the venue design notes` §2.

pub mod csg;
pub mod curve;
pub mod gltf;
pub mod mesh;
pub mod query;
pub mod solid;
pub mod sweep;

pub use csg::{BooleanOp, CsgError};
pub use curve::{Curve2, Curve3, SplineKind};
pub use gltf::{load_gltf, GltfError};
pub use mesh::Mesh;
pub use query::{Bvh, RayHit};
pub use sweep::SweepOptions;

/// Metres are the unit everywhere, right-handed, `+y` up, floor at `y = 0` —
/// the venue's own convention (`metal-perform::venue`).
pub const UP: [f32; 3] = [0.0, 1.0, 0.0];

/// A triangle-view of a mesh, for ray queries and warps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub c: [f32; 3],
}
