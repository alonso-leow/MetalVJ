# `metal-geom` public API (contract)

Frozen for this batch. `metal-perform` and `metal-dash` code against these
signatures; `metal-geom` may add more, but must not change these.

Units: metres, right-handed, `+y` up, floor at `y = 0` (venue convention).

```rust
// lib.rs
pub const UP: [f32; 3];
pub struct Triangle { pub a: [f32;3], pub b: [f32;3], pub c: [f32;3] }

// mesh.rs
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}
impl Mesh {
    pub fn new() -> Self;
    pub fn bbox(&self) -> Option<([f32;3], [f32;3])>;
    pub fn triangles(&self) -> Vec<Triangle>;
    pub fn transformed(&self, m: [[f32;4];4]) -> Mesh; // column-major or document
    pub fn recompute_normals(&mut self);
    pub fn weld(&mut self, eps: f32);
}

// gltf.rs
#[derive(Debug)]
pub enum GltfError { Io(std::io::Error), Gltf(gltf::Error), Empty }
pub fn load_gltf(path: &std::path::Path) -> Result<Mesh, GltfError>;

// curve.rs
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SplineKind { #[default] Polyline, CatmullRom, Bezier }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve2 { pub points: Vec<[f32;2]>, pub closed: bool, pub kind: SplineKind }
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve3 { pub points: Vec<[f32;3]>, pub closed: bool, pub kind: SplineKind }

impl Curve2 {
    pub fn eval(&self, t: f32) -> [f32;2];          // t in 0..=1
    pub fn flatten(&self, tolerance: f32) -> Vec<[f32;2]>;
    pub fn length(&self) -> f32;
    pub fn resample(&self, n: usize) -> Curve2;
    pub fn signed_area(&self) -> f32;                 // for profile orientation
    pub fn reverse(&self) -> Curve2;
}
impl Curve3 {
    pub fn eval(&self, t: f32) -> [f32;3];
    pub fn flatten(&self, tolerance: f32) -> Vec<[f32;3]>;
    pub fn length(&self) -> f32;
    pub fn resample(&self, n: usize) -> Curve3;
    pub fn smooth(&self, iterations: usize, factor: f32) -> Curve3;
    pub fn trim(&self, t0: f32, t1: f32) -> Curve3;
    pub fn extend(&self, by: f32) -> Curve3;
    pub fn reverse(&self) -> Curve3;
}

// sweep.rs
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepOptions {
    pub caps: bool,      // close the two ends
    pub closed: bool,    // profile is a loop (always true for a solid)
    pub segments: usize, // samples along the path
}
impl Default for SweepOptions { /* caps: true, closed: true, segments: 32 */ }

/// Sweep `profile` (2D, in the path's local frame) along `path`.
pub fn sweep(path: &Curve3, profile: &Curve2, opts: SweepOptions) -> Mesh;

// solid.rs
pub fn plane(width: f32, height: f32) -> Mesh;          // centred, +y up, normal +y? document; 2 tris
pub fn cuboid(size: [f32;3], center: bool) -> Mesh;
pub fn cylinder(radius: f32, height: f32, segments: usize) -> Mesh;
pub fn cone(radius: f32, height: f32, segments: usize) -> Mesh;
pub fn sphere(radius: f32, segments: usize) -> Mesh;
pub fn grid(width: f32, depth: f32, x: usize, z: usize) -> Mesh; // y = 0
pub fn line(a: [f32;3], b: [f32;3], radius: f32, segments: usize) -> Mesh; // a thin cylinder

// csg.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BooleanOp { Intersect, Union, Difference }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CsgError { NonManifold, ResultTooBig, SolverNotAvailable, Unknown(String) }
pub fn boolean(op: BooleanOp, a: &Mesh, b: &Mesh) -> Result<Mesh, CsgError>;

// query.rs
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit { pub distance: f32, pub at: [f32;3], pub normal: [f32;3], pub triangle: usize }
pub struct Bvh { /* private */ }
impl Bvh {
    pub fn build(mesh: &Mesh) -> Self;
    pub fn raycast(&self, origin: [f32;3], dir: [f32;3], max_distance: f32) -> Option<RayHit>;
    pub fn nearest_point(&self, p: [f32;3]) -> Option<RayHit>;
}
pub fn projected_area(points: &[[f32;2]]) -> f32; // absolute polygon area
```

Serialization rule: `metal-geom` types do **not** need serde. Persisted venue
geometry stores plain `Vec<[f32;3]>` / `Vec<[f32;2]>` plus `SplineKind`, and
builds a `Curve2`/`Curve3` at use time.
