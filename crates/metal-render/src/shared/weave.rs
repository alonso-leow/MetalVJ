//! The grid as a building material.
//!
//! Matrix's founding rule — *"an object is a region of grid"* — rests on one
//! thing holding: that a table leg and the wall behind it are the same cloth
//! cut smaller, not two grids that happen to look alike. That is not a
//! property of the model, it is a property of the **builder**, and there are
//! two models that want it now: a room, and a river valley. So the builder
//! lives here.
//!
//! **The rates are passed in, not baked.** A weave is a vocabulary plus two
//! numbers — how finely a strand is sampled, and how many cells a surface gets
//! per unit of its own size — and those two numbers cannot be shared across
//! worlds at different scales. Matrix's room is 2 units across and wants 9
//! points per unit; River's valley is seventeen units of water and would come
//! out at a hundred and fifty points a strand for nothing. What has to be one
//! thing is the *vocabulary*, so that a stone and a bank are the same cloth in
//! the same sense a leg and a wall are. Sharing the numbers too would make the
//! second model wrong in order to make the sharing look tidier.
//!
//! # What a vertex carries
//!
//! [`WeaveVertex`] has a `tag` slot where a mesh vertex would have a normal,
//! because the lines are not shaded: it is spent on a tag instead:
//!
//! * `tag.x` — the strand's own id, so two strands that cross do not flicker
//!   as one. A model may override it per point with [`Weave::strand_val`] when
//!   the role needs a coordinate of its own; River does, for the two roles that
//!   are displaced.
//! * `tag.y` — how far along its own strand this point is, `0..1`.
//! * `tag.z` — the role, which is how the strand is lit and nothing else.
//!
//! and `obj` is which *thing* the strand belongs to — a hash seed, so no two
//! objects answer the same instant identically.
//!
//! # The filled layer
//!
//! A line is not a surface, so a texture laid on one only ever reached the
//! pixels of the line. Every gridded surface ([`Weave::sheet`], so `flat` and
//! `boxy` with it, and [`Weave::quads`] for the ones a model builds itself) now
//! also emits its cells as triangles carrying the surface's **real normal**
//! (`nrm`, from the surface's own derivatives), in a second list. A model that
//! draws it gets faces that take a material and are lit; one that does not
//! pays nothing, and the lines are what they were. Translucent faces (a pane
//! of glass, water) go in a third list, because they must not write depth:
//! whatever is under the surface still has to be drawn.

use glam::Vec3;

/// A weave vertex. `tag` is `(strand id or role coordinate, 0..1 along the
/// strand, role)`; `nrm` is the surface normal where the point belongs to a
/// gridded surface and zero on a free strand.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WeaveVertex {
    pub pos: [f32; 3],
    pub tag: [f32; 3],
    pub obj: f32,
    pub nrm: [f32; 3],
}

impl WeaveVertex {
    fn new(pos: Vec3, tag: Vec3, obj: f32, nrm: Vec3) -> WeaveVertex {
        WeaveVertex { pos: pos.into(), tag: tag.into(), obj, nrm: nrm.into() }
    }
}

pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<WeaveVertex>() as wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32, 3 => Float32x3],
};

pub struct Weave {
    /// The lines.
    pub v: Vec<WeaveVertex>,
    /// The opaque faces, three vertices to a triangle.
    pub fill: Vec<WeaveVertex>,
    /// The translucent faces, picked by `translucent` on the role.
    pub glass: Vec<WeaveVertex>,
    /// Which roles are drawn translucent. Default: none.
    pub translucent: fn(f32) -> bool,
    /// A running id, so every strand ever laid down in this weave is distinct.
    strands: f32,
    points_per_unit: f32,
    max_points: usize,
    cells_per_unit: f32,
    max_cells: u32,
}

impl Weave {
    /// `points_per_unit` and `max_points` are how finely a strand is sampled —
    /// the luminance field is read at the samples, so a curved strand is lit
    /// along its curve rather than across its chord, and a displaced one bends
    /// at them. `cells_per_unit` and `max_cells` are how many cells a surface
    /// gets per unit of its own size, which is what makes the weave one weave.
    pub fn new(
        points_per_unit: f32,
        max_points: usize,
        cells_per_unit: f32,
        max_cells: u32,
    ) -> Weave {
        Weave {
            v: Vec::new(),
            fill: Vec::new(),
            glass: Vec::new(),
            translucent: |_| false,
            strands: 0.0,
            points_per_unit,
            max_points,
            cells_per_unit,
            max_cells,
        }
    }

    /// Empty the weave, keeping its rates, so a model that rebuilds its
    /// geometry in place reuses the builder instead of making a new one.
    pub fn clear(&mut self) {
        self.v.clear();
        self.fill.clear();
        self.glass.clear();
        self.strands = 0.0;
    }

    /// How many cells something `len` long gets. Public because a model that
    /// builds its own surface — a river that stops at its own shoreline is not
    /// a parametric rectangle — still has to ask the weave rather than guess.
    pub fn cells(&self, len: f32) -> u32 {
        ((len * self.cells_per_unit).round() as u32).clamp(1, self.max_cells)
    }

    /// How many points a strand `len` long is sampled at.
    pub fn points(&self, len: f32) -> usize {
        ((len * self.points_per_unit).round() as usize).clamp(2, self.max_points)
    }

    /// One strand of the weave, sampled along `p(0..1)`. Straight or not.
    pub fn strand(&mut self, obj: f32, role: f32, len: f32, p: impl Fn(f32) -> Vec3) {
        let id = self.strands;
        self.emit(obj, role, len, |u| (p(u), id));
    }

    /// The same, with the `tag.x` slot given per point by the caller instead of
    /// carrying the strand's id.
    ///
    /// For the roles whose shader needs a coordinate the geometry knows and the
    /// vertex shader cannot recover: how deep the water is under a point (so a
    /// wave dies at the shoreline rather than climbing the bank), how high a
    /// leaf is above its own root (so a gust bends the tip and leaves the root
    /// planted). Both are *why the strand is where it is*, which is exactly the
    /// kind of thing that belongs in the mesh and not in a second copy of the
    /// terrain function written in WGSL.
    pub fn strand_val(&mut self, obj: f32, role: f32, len: f32, p: impl Fn(f32) -> (Vec3, f32)) {
        self.emit(obj, role, len, p);
    }

    fn emit(&mut self, obj: f32, role: f32, len: f32, p: impl Fn(f32) -> (Vec3, f32)) {
        self.emit_n(obj, role, len, Vec3::ZERO, p);
    }

    fn emit_n(&mut self, obj: f32, role: f32, len: f32, nrm: Vec3, p: impl Fn(f32) -> (Vec3, f32)) {
        let n = self.points(len);
        self.strands += 1.0;
        let mut previous: Option<WeaveVertex> = None;
        for k in 0..n {
            let u = k as f32 / (n - 1) as f32;
            let (pos, key) = p(u);
            let point = WeaveVertex::new(pos, Vec3::new(key, u, role), obj, nrm);
            if let Some(q) = previous {
                self.v.push(q);
                self.v.push(point);
            }
            previous = Some(point);
        }
    }

    pub fn line(&mut self, a: Vec3, b: Vec3, obj: f32, role: f32) {
        self.strand(obj, role, (b - a).length(), |u| a.lerp(b, u));
    }

    /// A gridded surface: `nu + 1` strands one way, `nv + 1` the other, both
    /// following `p`. Curved surfaces use the curvature; a flat one is the same
    /// call and pays nothing for the generality.
    pub fn sheet(&mut self, nu: u32, nv: u32, obj: f32, role: f32, p: impl Fn(f32, f32) -> Vec3) {
        let (nu, nv) = (nu.max(1), nv.max(1));
        let across = (p(1.0, 0.5) - p(0.0, 0.5)).length();
        let down = (p(0.5, 1.0) - p(0.5, 0.0)).length();
        for i in 0..=nu {
            let u = i as f32 / nu as f32;
            self.strand(obj, role, down, |t| p(u, t));
        }
        for j in 0..=nv {
            let v = j as f32 / nv as f32;
            self.strand(obj, role, across, |t| p(t, v));
        }
        self.quads(nu, nv, obj, role, |u, v| (p(u, v), 0.0));
    }

    /// The faces of a gridded surface, without its lines: `nu` by `nv` cells,
    /// two triangles each, `p(u, v)` giving the point and the value for `tag.x`
    /// (as [`Weave::strand_val`] does for a strand). For a surface a model cuts
    /// itself — a river that stops at its shoreline — where the strands are
    /// already its own.
    pub fn quads(
        &mut self,
        nu: u32,
        nv: u32,
        obj: f32,
        role: f32,
        p: impl Fn(f32, f32) -> (Vec3, f32),
    ) {
        let (nu, nv) = (nu.max(1), nv.max(1));
        let at = |i: u32, j: u32| {
            let (u, v) = (i as f32 / nu as f32, j as f32 / nv as f32);
            let (pos, key) = p(u, v);
            // The normal from the surface's own derivatives, taken a little
            // inside the edge so a sphere's pole (where one derivative
            // vanishes) still has one.
            let e = 0.5 / (nu.max(nv) as f32 * 8.0);
            let (uc, vc) = (u.clamp(e, 1.0 - e), v.clamp(e, 1.0 - e));
            let du = p(uc + e, vc).0 - p(uc - e, vc).0;
            let dv = p(uc, vc + e).0 - p(uc, vc - e).0;
            let n = du.cross(dv).try_normalize().unwrap_or(Vec3::Y);
            WeaveVertex::new(pos, Vec3::new(key, v, role), obj, n)
        };
        let list = if (self.translucent)(role) { &mut self.glass } else { &mut self.fill };
        for j in 0..nv {
            for i in 0..nu {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                list.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }

    /// A flat gridded quad, its cell count taken from its own size so the weave
    /// has one scale everywhere in the world.
    pub fn flat(&mut self, o: Vec3, du: Vec3, dv: Vec3, obj: f32, role: f32) {
        let (nu, nv) = (self.cells(du.length()), self.cells(dv.length()));
        self.sheet(nu, nv, obj, role, |u, v| o + du * u + dv * v);
    }

    /// A box, as six gridded faces. The cheapest thing that still reads as
    /// occupying a volume.
    pub fn boxy(&mut self, c: Vec3, h: Vec3, obj: f32, role: f32) {
        let (x, y, z) = (Vec3::X * h.x * 2.0, Vec3::Y * h.y * 2.0, Vec3::Z * h.z * 2.0);
        let o = c - h;
        self.flat(o, x, z, obj, role);
        self.flat(o + y, x, z, obj, role);
        self.flat(o, y, z, obj, role);
        self.flat(o + x, y, z, obj, role);
        self.flat(o, x, y, obj, role);
        self.flat(o + z, x, y, obj, role);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    const FLAT: f32 = 1.0;
    const GLASS: f32 = 4.0;

    fn weave() -> Weave {
        Weave::new(1.0, 64, 1.0, 16)
    }

    /// A cell's normal is the surface's own, taken from its derivatives, and
    /// unit length — the whole point of the filled layer.
    #[test]
    fn a_cell_normal_is_the_surfaces_own() {
        let mut w = weave();
        let o = Vec3::new(0.3, -0.2, 0.7);
        let du = Vec3::new(2.0, 0.0, 0.0);
        let dv = Vec3::new(0.5, 0.0, 1.5);
        let surface = du.cross(dv).normalize();
        w.flat(o, du, dv, 0.0, FLAT);

        assert!(!w.fill.is_empty());
        assert_eq!(w.fill.len() % 3, 0);
        for v in &w.fill {
            let n = Vec3::from(v.nrm);
            assert!((n.length() - 1.0).abs() < 1e-5, "cell normal is not unit: {}", n.length());
            assert!(n.dot(surface) > 0.999, "cell normal {n} is not the surface's {surface}");
        }
    }

    /// The filled layer is a second list: the lines are exactly the ones the
    /// builder emitted before it had faces, byte for byte, and adding more
    /// faces never touches them.
    #[test]
    fn a_filled_layer_leaves_the_lines_alone() {
        let mut w = weave();
        w.sheet(2, 2, 0.0, FLAT, |u, v| Vec3::new(u, v, 0.0));
        // Three strands each way, two points to a strand: twelve line vertices,
        // the count the layer-less builder produced.
        assert_eq!(w.v.len(), 12);
        assert_eq!(w.fill.len(), 4 * 6);
        assert!(w.glass.is_empty());

        let lines = w.v.clone();
        w.quads(3, 3, 0.0, FLAT, |u, v| (Vec3::new(u, v, 0.0), 0.0));
        assert_eq!(
            bytemuck::cast_slice::<WeaveVertex, u8>(&w.v),
            bytemuck::cast_slice::<WeaveVertex, u8>(&lines)
        );
    }

    /// A weave of free strands has no faces at all, and clearing returns the
    /// builder to that state.
    #[test]
    fn free_strands_hold_no_faces_and_clearing_empties_the_weave() {
        let mut w = weave();
        w.line(Vec3::ZERO, Vec3::X, 0.0, FLAT);
        w.strand(0.0, FLAT, 2.0, |u| Vec3::new(u, u * u, 0.0));
        assert!(w.fill.is_empty());
        assert!(w.glass.is_empty());

        w.flat(Vec3::ZERO, Vec3::X, Vec3::Y, 0.0, FLAT);
        assert!(!w.fill.is_empty());
        w.clear();
        assert!(w.v.is_empty());
        assert!(w.fill.is_empty());
        assert!(w.glass.is_empty());
    }

    /// A role the builder was told is translucent goes to `glass`, so the host
    /// can draw it without writing depth; every other role is opaque.
    #[test]
    fn translucent_roles_go_to_glass() {
        let mut w = weave();
        w.translucent = |role| role == GLASS;
        w.flat(Vec3::ZERO, Vec3::X, Vec3::Y, 0.0, GLASS);
        assert!(w.fill.is_empty());
        assert!(!w.glass.is_empty());

        w.clear();
        w.flat(Vec3::ZERO, Vec3::X, Vec3::Y, 0.0, FLAT);
        assert!(!w.fill.is_empty());
        assert!(w.glass.is_empty());
    }
}
