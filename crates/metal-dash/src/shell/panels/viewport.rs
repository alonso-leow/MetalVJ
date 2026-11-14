//! The venue's real 3D viewport: an orbit camera, a CPU projection and the
//! painter's-algorithm room, drawn with `egui::Painter` on top of the venue
//! reply. No wgpu pass: every surface, object, fixture and projector is
//! projected through [`OrbitCamera`] and painted as polygons, lines and dots.
//!
//! State lives in egui memory, keyed by the venue id, so the shell needs no
//! new threading: the camera, the selection, the palette, the mask draft and
//! the drag all survive a frame.

use std::collections::BTreeSet;

use egui::{
    Align2, Color32, Id, Key, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2,
};
use glam::Vec3;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{self, icon_button, Icon};
use crate::shell::i18n::{tr, trf};
use metal_perform::venue::{
    direction, Camera, Fixture, Object, ObjectForm, Projector, Surface, SurfaceForm, Venue,
    VenueMask,
};

/// The nearest click radius, in points, used to pick an item.
#[cfg_attr(not(test), allow(dead_code))]
const PICK_PX: f32 = 16.0;

/// The floor grid step, in metres, used when snap is on.
#[cfg_attr(not(test), allow(dead_code))]
const SNAP_STEP: f32 = 0.25;

/// The length of a transform-gizmo axis, in metres.
const AXIS_LEN: f32 = 1.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tool {
    #[default]
    Select,
    Ruler,
}

/// A transform-gizmo drag along one world axis.
#[derive(Clone, Debug)]
struct AxisDrag {
    item: Select,
    origin: [f32; 3],
    axis: Vec3,
    start_param: f32,
    at: [f32; 3],
}

/// What the viewport's own state remembers between frames.
#[derive(Clone)]
struct ViewState {
    camera: OrbitCamera,
    fitted: bool,
    selected: Option<Select>,
    drag: Option<Select>,
    drag_at: Option<[f32; 3]>,
    mask_surface: Option<String>,
    mask_points: Vec<[f32; 2]>,
    note: Option<String>,
    tool: Tool,
    snap: bool,
    ruler: Vec<[f32; 3]>,
    axis_drag: Option<AxisDrag>,
    saved_camera: Option<OrbitCamera>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            camera: OrbitCamera::default(),
            fitted: false,
            selected: None,
            drag: None,
            drag_at: None,
            mask_surface: None,
            mask_points: Vec::new(),
            note: None,
            tool: Tool::Select,
            snap: true,
            ruler: Vec::new(),
            axis_drag: None,
            saved_camera: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Select {
    kind: SelectKind,
    id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectKind {
    Object,
    Fixture,
    Projector,
    Surface,
    Display,
}

impl Select {
    fn new(kind: SelectKind, id: &str) -> Self {
        Self {
            kind,
            id: id.to_string(),
        }
    }

    fn movable(&self) -> bool {
        matches!(self.kind, SelectKind::Object | SelectKind::Fixture)
    }
}

/// An orbit camera looking at `target` from `distance`, at `yaw` and `pitch`
/// (radians) with a vertical field of view `fov` (radians).
#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub ortho: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: [0.0, 2.0, 0.0],
            distance: 18.0,
            yaw: 0.65,
            pitch: 0.45,
            fov: 55.0_f32.to_radians(),
            ortho: false,
        }
    }
}

impl OrbitCamera {
    /// The camera the room popup opens on: a three-quarter view of a room
    /// `span` metres across, tall enough to see the walls and the ceiling
    /// rig. The perspective viewport fits its own camera; the popup has no
    /// pointer to fit with, so this is its default.
    pub fn room(span: f32) -> Self {
        let d = span.max(4.0);
        OrbitCamera {
            target: [0.0, d * 0.18, -d * 0.2],
            distance: d * 1.35,
            yaw: 0.7,
            pitch: 0.42,
            fov: 55.0_f32.to_radians(),
            ortho: false,
        }
    }

    fn direction(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos())
    }

    fn eye(&self) -> Vec3 {
        Vec3::from(self.target) + self.direction() * self.distance
    }

    /// The eye's world position: the side a card's preview shows its faces to.
    pub fn eye_point(&self) -> [f32; 3] {
        self.eye().to_array()
    }

    /// Forward (eye → target), right and up, in world space.
    fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let eye = self.eye();
        let forward = (Vec3::from(self.target) - eye).normalize_or_zero();
        let mut right = forward.cross(Vec3::Y);
        if right.length_squared() < 1.0e-9 {
            right = Vec3::X;
        }
        let right = right.normalize();
        let up = right.cross(forward).normalize();
        (forward, right, up)
    }

    /// Project a world point into `rect`, or `None` when it is behind the
    /// camera (or the rect is degenerate).
    pub fn project(&self, p: [f32; 3], rect: Rect) -> Option<Pos2> {
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let (forward, right, up) = self.basis();
        let eye = self.eye();
        let rel = Vec3::from(p) - eye;
        let depth = rel.dot(forward);
        if depth <= 0.05 {
            return None;
        }
        let tan = (self.fov * 0.5).tan();
        let aspect = rect.width() / rect.height();
        let (nx, ny) = if self.ortho {
            // Keep the target plane's perspective size: its half-height is
            // `distance * tan(fov/2)` metres.
            let half_h = (self.distance * tan).max(1.0e-6);
            let half_w = half_h * aspect;
            (rel.dot(right) / half_w, rel.dot(up) / half_h)
        } else {
            (
                rel.dot(right) / (depth * tan * aspect),
                rel.dot(up) / (depth * tan),
            )
        };
        let centre = rect.center();
        Some(Pos2::new(
            centre.x + nx * rect.width() * 0.5,
            centre.y - ny * rect.height() * 0.5,
        ))
    }

    /// The perspective ray under a screen point: `(origin, unit direction)`.
    /// Even in orthographic projection this stays a perspective ray, because
    /// picking, masks and the floor drag all use it.
    pub fn ray(&self, px: Pos2, rect: Rect) -> ([f32; 3], [f32; 3]) {
        let (forward, right, up) = self.basis();
        let eye = self.eye();
        let centre = rect.center();
        let nx = if rect.width() > 0.0 {
            (px.x - centre.x) / (rect.width() * 0.5)
        } else {
            0.0
        };
        let ny = if rect.height() > 0.0 {
            -(px.y - centre.y) / (rect.height() * 0.5)
        } else {
            0.0
        };
        let tan = (self.fov * 0.5).tan();
        let aspect = if rect.height() > 0.0 {
            rect.width() / rect.height()
        } else {
            1.0
        };
        let dir = (forward + right * (nx * tan * aspect) + up * (ny * tan)).normalize_or_zero();
        (eye.to_array(), dir.to_array())
    }

    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw -= delta.x * 0.008;
        self.pitch = (self.pitch + delta.y * 0.008).clamp(-1.45, 1.45);
    }

    pub fn pan(&mut self, delta: Vec2, rect: Rect) {
        let (_, right, up) = self.basis();
        let scale = self.distance * 2.0 / rect.height().max(1.0);
        let shift = right * (-delta.x * scale) + up * (delta.y * scale);
        let t = Vec3::from(self.target) + shift;
        self.target = t.to_array();
    }

    pub fn dolly(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.distance = (self.distance / factor).clamp(0.4, 200.0);
        }
    }

    /// Frame a set of world points: their centre becomes the target and the
    /// distance clears their radius.
    pub fn fit(&mut self, points: &[[f32; 3]]) {
        let Some(first) = points.first() else {
            return;
        };
        let mut lo = Vec3::from(*first);
        let mut hi = lo;
        for p in points {
            let v = Vec3::from(*p);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let centre = (lo + hi) * 0.5;
        let radius = (hi - lo).length() * 0.5;
        self.target = centre.to_array();
        self.distance = (radius / (self.fov * 0.5).sin()).max(2.0) * 1.3;
    }
}

/// Everything the painter needs from a venue, in one flat view of its items.
struct Scene<'a> {
    venue: &'a Venue,
}

impl<'a> Scene<'a> {
    fn new(venue: &'a Venue) -> Self {
        Self { venue }
    }

    fn surface(&self, id: &str) -> Option<&'a Surface> {
        self.venue.surfaces.iter().find(|s| s.id == id)
    }

    fn object(&self, id: &str) -> Option<&'a Object> {
        self.venue.objects.iter().find(|o| o.id == id)
    }

    fn points(&self) -> Vec<[f32; 3]> {
        let mut points = Vec::new();
        for s in &self.venue.surfaces {
            for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                points.push(s.at(u, v));
            }
        }
        for o in &self.venue.objects {
            let c = box_corners(o);
            points.extend(c);
        }
        for f in &self.venue.fixtures {
            if let Some(at) = f.at {
                points.push(at);
            }
        }
        for p in &self.venue.projectors {
            if let Some(at) = self.projector_at(p) {
                points.push(at);
            }
        }
        if points.is_empty() {
            points.push([0.0, 1.0, 0.0]);
        }
        points
    }

    fn projector_at(&self, p: &Projector) -> Option<[f32; 3]> {
        if let Some(o) = self.object(&p.on) {
            let mut v = Vec3::from(o.at) + Vec3::new(0.0, o.size[1], 0.0) + Vec3::from(p.lens);
            v = rotate_y(v - Vec3::from(o.at), o.yaw) + Vec3::from(o.at);
            Some(v.to_array())
        } else {
            self.surface(&p.on).map(|s| s.at(0.5, 0.5))
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn first_stand(&self) -> Option<String> {
        self.venue
            .objects
            .first()
            .map(|o| o.id.clone())
            .or_else(|| self.venue.surfaces.first().map(|s| s.id.clone()))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn first_surface(&self) -> Option<&'a Surface> {
        self.venue.surfaces.first()
    }
}

fn rotate_y(v: Vec3, yaw: f32) -> Vec3 {
    let q = glam::Quat::from_rotation_y(-yaw.to_radians());
    q * v
}

fn box_corners(o: &Object) -> Vec<[f32; 3]> {
    let h = [
        [-o.size[0] * 0.5, 0.0, -o.size[2] * 0.5],
        [o.size[0] * 0.5, 0.0, -o.size[2] * 0.5],
        [o.size[0] * 0.5, 0.0, o.size[2] * 0.5],
        [-o.size[0] * 0.5, 0.0, o.size[2] * 0.5],
        [-o.size[0] * 0.5, o.size[1], -o.size[2] * 0.5],
        [o.size[0] * 0.5, o.size[1], -o.size[2] * 0.5],
        [o.size[0] * 0.5, o.size[1], o.size[2] * 0.5],
        [-o.size[0] * 0.5, o.size[1], o.size[2] * 0.5],
    ];
    let base = Vec3::from(o.at);
    h.iter()
        .map(|p| (rotate_y(Vec3::from(*p), o.yaw) + base).to_array())
        .collect()
}

fn project_poly(cam: &OrbitCamera, rect: Rect, pts: &[[f32; 3]]) -> Option<Vec<Pos2>> {
    let mut out = Vec::with_capacity(pts.len());
    for p in pts {
        out.push(cam.project(*p, rect)?);
    }
    Some(out)
}

fn centroid(pts: &[[f32; 3]]) -> [f32; 3] {
    if pts.is_empty() {
        return [0.0; 3];
    }
    let mut c = Vec3::ZERO;
    for p in pts {
        c += Vec3::from(*p);
    }
    (c / pts.len() as f32).to_array()
}

fn depth(eye: Vec3, p: [f32; 3]) -> f32 {
    (Vec3::from(p) - eye).length_squared()
}

struct Face {
    pts: Vec<Pos2>,
    fill: Color32,
    stroke: Stroke,
    depth: f32,
}

/// Bilinear inverse of `origin + s·u + t·v`; `None` when degenerate.
fn plane_st(p: [f32; 3], origin: [f32; 3], u: [f32; 3], v: [f32; 3]) -> Option<[f32; 2]> {
    let d = Vec3::from(p) - Vec3::from(origin);
    let u = Vec3::from(u);
    let v = Vec3::from(v);
    let uu = u.dot(u);
    let uv = u.dot(v);
    let vv = v.dot(v);
    let det = uu * vv - uv * uv;
    if det.abs() < 1.0e-9 {
        return None;
    }
    let du = d.dot(u);
    let dv = d.dot(v);
    let s = (du * vv - dv * uv) / det;
    let t = (dv * uu - du * uv) / det;
    Some([s, t])
}

/// A plane hit by a ray: the point and its `(s, t)`.
fn ray_plane(
    origin: [f32; 3],
    dir: [f32; 3],
    plane_origin: [f32; 3],
    u: [f32; 3],
    v: [f32; 3],
) -> Option<([f32; 3], [f32; 2])> {
    let o = Vec3::from(origin);
    let d = Vec3::from(dir);
    let po = Vec3::from(plane_origin);
    let n = Vec3::from(u).cross(Vec3::from(v)).normalize_or_zero();
    if n.length_squared() < 1.0e-9 {
        return None;
    }
    let denom = n.dot(d);
    if denom.abs() < 1.0e-9 {
        return None;
    }
    let t = (po - o).dot(n) / denom;
    if t <= 0.0 {
        return None;
    }
    let p = o + d * t;
    let st = plane_st(p.to_array(), plane_origin, u, v)?;
    Some((p.to_array(), st))
}

/// Find the nearest `(s, t)` on a surface along a ray. A plane is exact; a
/// sweep samples a 12×6 grid and then refines locally, which is enough for
/// drawing a mask.
fn ray_surface(s: &Surface, eye: [f32; 3], dir: [f32; 3]) -> Option<([f32; 3], [f32; 2])> {
    match &s.form {
        SurfaceForm::Plane => ray_plane(eye, dir, s.origin, s.u, s.v),
        SurfaceForm::Swept { .. } => {
            let (mut bs, mut bt, mut best) = (0.0f32, 0.0f32, f32::INFINITY);
            let (nu, nv) = (12usize, 6usize);
            for j in 0..=nv {
                for i in 0..=nu {
                    let u = i as f32 / nu as f32;
                    let v = j as f32 / nv as f32;
                    if let Some((d, _)) = ray_point_distance(eye, dir, s.at(u, v)) {
                        if d < best {
                            best = d;
                            bs = u;
                            bt = v;
                        }
                    }
                }
            }
            // Refine around the best sample.
            let mut step = 1.0 / 12.0;
            for _ in 0..7 {
                let (cu, cv) = (bs, bt);
                for dj in -1..=1 {
                    for di in -1..=1 {
                        let u = (cu + di as f32 * step).clamp(0.0, 1.0);
                        let v = (cv + dj as f32 * step).clamp(0.0, 1.0);
                        if let Some((d, _)) = ray_point_distance(eye, dir, s.at(u, v)) {
                            if d < best {
                                best = d;
                                bs = u;
                                bt = v;
                            }
                        }
                    }
                }
                step *= 0.5;
            }
            if best.is_finite() {
                Some((s.at(bs, bt), [bs, bt]))
            } else {
                None
            }
        }
    }
}

/// Distance from a ray to a point, along the ray (rejects points behind).
fn ray_point_distance(eye: [f32; 3], dir: [f32; 3], p: [f32; 3]) -> Option<(f32, f32)> {
    let o = Vec3::from(eye);
    let d = Vec3::from(dir);
    let rel = Vec3::from(p) - o;
    let t = rel.dot(d);
    if t <= 0.0 {
        return None;
    }
    Some(((rel - d * t).length_squared(), t))
}

fn floor_point(eye: [f32; 3], dir: [f32; 3]) -> Option<[f32; 3]> {
    let o = Vec3::from(eye);
    let d = Vec3::from(dir);
    if d.y.abs() < 1.0e-6 {
        return None;
    }
    let t = -o.y / d.y;
    if t <= 0.0 {
        return None;
    }
    Some((o + d * t).to_array())
}

fn snap_value(v: f32) -> f32 {
    (v / SNAP_STEP).round() * SNAP_STEP
}

/// Snap a floor point to the grid; the floor is always `y = 0`.
fn snap_floor(p: [f32; 3]) -> [f32; 3] {
    [snap_value(p[0]), 0.0, snap_value(p[2])]
}

/// Snap a gizmo-dragged point along its dominant axis, leaving the other
/// components where the axis put them.
fn snap_axis_point(p: [f32; 3], axis: Vec3) -> [f32; 3] {
    if axis.y.abs() > 0.5 {
        [p[0], snap_value(p[1]), p[2]]
    } else if axis.x.abs() > 0.5 {
        [snap_value(p[0]), p[1], p[2]]
    } else {
        [p[0], p[1], snap_value(p[2])]
    }
}

/// The point on the axis line nearest the pointer ray, with its axis
/// parameter in metres (the axis is expected to be unit length). `None` when
/// the two lines are parallel.
fn closest_on_line(
    ray_origin: [f32; 3],
    ray_dir: [f32; 3],
    axis_origin: [f32; 3],
    axis_dir: [f32; 3],
) -> Option<([f32; 3], f32)> {
    let d1 = Vec3::from(ray_dir);
    let d2 = Vec3::from(axis_dir);
    let w0 = Vec3::from(ray_origin) - Vec3::from(axis_origin);
    let a = d1.dot(d1);
    let b = d1.dot(d2);
    let c = d2.dot(d2);
    let d = d1.dot(w0);
    let e = d2.dot(w0);
    let denom = a * c - b * b;
    if denom.abs() < 1.0e-9 {
        return None;
    }
    let t = (a * e - b * d) / denom;
    Some(((Vec3::from(axis_origin) + d2 * t).to_array(), t))
}

/// Distance between two world points, in metres.
fn measure_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (Vec3::from(a) - Vec3::from(b)).length()
}

/// A camera that looks down a shot from the shot's eye. `distance` is how far
/// along the shot direction its target sits.
#[cfg_attr(not(test), allow(dead_code))]
fn camera_from_shot(shot: &Camera, distance: f32) -> OrbitCamera {
    let dir = direction(shot.yaw, shot.pitch);
    let distance = distance.max(0.4);
    let eye = Vec3::from(shot.eye);
    OrbitCamera {
        target: (eye + dir * distance).to_array(),
        distance,
        yaw: -shot.yaw.to_radians(),
        pitch: -shot.pitch.to_radians(),
        fov: shot.fov.to_radians(),
        ortho: false,
    }
}

/// The gizmo axis whose screen handle is nearest `p`, if any is within
/// [`PICK_PX`].
fn axis_handle_at(cam: &OrbitCamera, rect: Rect, p: Pos2, origin: [f32; 3]) -> Option<Vec3> {
    let from = cam.project(origin, rect)?;
    let mut best: Option<(f32, Vec3)> = None;
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        let end = Vec3::from(origin) + axis * AXIS_LEN;
        let Some(to) = cam.project(end.to_array(), rect) else {
            continue;
        };
        let ab = to - from;
        let len2 = ab.length_sq();
        let t = if len2 > 0.0 {
            ((p - from).dot(ab) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let nearest = from + ab * t;
        let d = nearest.distance(p);
        if d <= PICK_PX && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
            best = Some((d, axis));
        }
    }
    best.map(|(_, axis)| axis)
}

/// The nearest surface point under the pointer, for a ruler endpoint.
fn ruler_point(
    scene: &Scene,
    cam: &OrbitCamera,
    rect: Rect,
    p: Pos2,
    snap: bool,
) -> Option<[f32; 3]> {
    let (eye, dir) = cam.ray(p, rect);
    let mut best: Option<([f32; 3], f32)> = None;
    for s in &scene.venue.surfaces {
        if let Some((hit, _)) = ray_surface(s, eye, dir) {
            let d = (Vec3::from(hit) - Vec3::from(eye)).length_squared();
            if best.as_ref().is_none_or(|(_, bd)| d < *bd) {
                best = Some((hit, d));
            }
        }
    }
    if let Some((hit, _)) = best {
        Some(hit)
    } else {
        floor_point(eye, dir).map(|p| if snap { snap_floor(p) } else { p })
    }
}

fn surface_polygon(s: &Surface) -> Vec<[f32; 3]> {
    vec![
        s.at(0.0, 0.0),
        s.at(1.0, 0.0),
        s.at(1.0, 1.0),
        s.at(0.0, 1.0),
    ]
}

fn point_in_poly(p: Pos2, poly: &[Pos2]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y)
            && p.x < (b.x - a.x) * (p.y - a.y) / ((b.y - a.y).max(1.0e-9)) + a.x
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The item nearest the pointer, preferring small items over the surface
/// behind them.
fn pick(scene: &Scene, cam: &OrbitCamera, rect: Rect, p: Pos2) -> Option<Select> {
    let mut best: Option<(f32, Select)> = None;
    let mut consider = |pos: Pos2, sel: Select| {
        let d = pos.distance(p);
        if d <= PICK_PX && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
            best = Some((d, sel));
        }
    };
    for o in &scene.venue.objects {
        if let Some(pos) = cam.project(centroid(&box_corners(o)), rect) {
            consider(pos, Select::new(SelectKind::Object, &o.id));
        }
    }
    for f in &scene.venue.fixtures {
        if let Some(at) = f.at {
            if let Some(pos) = cam.project(at, rect) {
                consider(pos, Select::new(SelectKind::Fixture, &f.id));
            }
        }
    }
    for pr in &scene.venue.projectors {
        if let Some(at) = scene.projector_at(pr) {
            if let Some(pos) = cam.project(at, rect) {
                consider(pos, Select::new(SelectKind::Projector, &pr.id));
            }
        }
    }
    for d in &scene.venue.displays {
        if let Some(s) = scene.surface(&d.surface) {
            if let Some(pos) = cam.project(s.at(0.5, 0.5), rect) {
                consider(pos, Select::new(SelectKind::Display, &d.id));
            }
        }
    }
    if best.is_some() {
        return best.map(|(_, s)| s);
    }
    for s in &scene.venue.surfaces {
        if let Some(poly) = project_poly(cam, rect, &surface_polygon(s)) {
            if point_in_poly(p, &poly) {
                return Some(Select::new(SelectKind::Surface, &s.id));
            }
        }
    }
    None
}

/// The materials and camera a room is painted with, for a caller that is not
/// this panel: the room popup's own view (`super::venue_popup`) draws the same
/// room with the same code, so the two can never show different geometry.
pub(crate) struct RoomView<'a> {
    pub camera: &'a OrbitCamera,
    pub masks: &'a [[f32; 2]],
    pub mask_surface: Option<&'a str>,
}

/// Paints the room (surfaces, objects, fixtures, projectors, displays, masks
/// already on surfaces, and — while a mask is being drawn — its draft) through
/// `view` into `rect`.
pub(crate) fn paint_room(
    painter: &egui::Painter,
    theme: &Theme,
    rect: Rect,
    view: &RoomView<'_>,
    venue: &Venue,
) {
    let scene = Scene::new(venue);
    let mut state = ViewState::default();
    state.camera = view.camera.clone();
    state.mask_surface = view.mask_surface.map(str::to_string);
    state.mask_points = view.masks.to_vec();
    draw_scene(painter, theme, rect, &state, &scene, None);
}

/// Draw the viewport and run its interactions.
///
/// The two things that used to live here as rows of text — the palette and the
/// add-screen form — are the node's own popup now (`super::venue_popup`, in
/// the gallery's column shape); this viewport is the room, the tool strip and
/// the gestures. Its tool strip is a vertical glyph bar at the canvas's top
/// left, the same component and corner the node canvas's strip uses.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn paint(ui: &mut Ui, theme: &Theme, door: &dyn Call, venue_id: &str, venue: &Venue) {
    // The module is a canvas of its own now: the properties' `venue editor`
    // page draws the two orthographic views, and the 3D room lives in the
    // node's popup (`venue_popup::room_view`) beside the screen cards. Kept
    // compiled because the popup reuses `paint_room` and the camera, and
    // because the pointer gestures below are what a future in-room editor
    // (dragging a screen on its wall) will run.
    let _ = (ui, theme, door, venue_id, venue);
}

#[cfg_attr(not(test), allow(dead_code))]
fn paint_interactive(ui: &mut Ui, theme: &Theme, door: &dyn Call, venue_id: &str, venue: &Venue) {
    let state_id = Id::new(("shell/region/venue/viewport", venue_id));
    let mut state = ui
        .data(|d| d.get_temp::<ViewState>(state_id))
        .unwrap_or_default();
    let scene = Scene::new(venue);

    // The viewport rectangle.
    let available = ui.available_rect_before_wrap();
    let height = (available.height() * 0.62).max(theme.header_height * 7.0);
    let rect = Rect::from_min_size(available.min, Vec2::new(available.width(), height));
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme.corner_radius(), theme.mask_outside);

    if !state.fitted {
        state.camera.fit(&scene.points());
        state.fitted = true;
    }

    let selected_surface = match &state.selected {
        Some(s) if s.kind == SelectKind::Surface => Some(s.id.clone()),
        _ => None,
    };

    // A selected surface's "draw mask" control.
    if let Some(surface) = selected_surface.clone() {
        ui.horizontal(|ui| {
            let on = state.mask_surface.as_deref() == Some(surface.as_str());
            if ui
                .selectable_label(
                    on,
                    RichText::new(tr("draw mask")).font(theme.font_small.clone()),
                )
                .clicked()
            {
                if on {
                    state.mask_surface = None;
                    state.mask_points.clear();
                } else {
                    state.mask_surface = Some(surface.clone());
                    state.mask_points.clear();
                }
            }
            if state.mask_surface.is_some() {
                ui.label(
                    RichText::new(tr(
                        "click points on the surface · enter closes · esc cancels",
                    ))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
                );
            }
        });
    }
    if state.mask_surface.is_some() && selected_surface.is_none() {
        state.mask_surface = None;
        state.mask_points.clear();
    }

    // Input: wheel dollies, F fits, O toggles ortho, Esc cancels, Enter
    // closes a mask.
    let hovered = response.hovered();
    let (wheel, fit, esc, enter, ortho_key) = ui.input(|i| {
        (
            i.smooth_scroll_delta.y,
            i.key_pressed(Key::F),
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::O),
        )
    });
    if hovered && wheel.abs() > 0.0 {
        state.camera.dolly(1.0 + wheel * 0.0015);
    }
    if hovered && fit {
        state.camera.fit(&scene.points());
    }
    if ortho_key {
        state.camera.ortho = !state.camera.ortho;
    }
    if esc {
        state.mask_surface = None;
        state.mask_points.clear();
        state.ruler.clear();
        if state.tool == Tool::Ruler {
            state.tool = Tool::Select;
        }
    }

    let primary_drag = response.dragged_by(PointerButton::Primary);
    let pan_drag = response.dragged_by(PointerButton::Middle)
        || response.dragged_by(PointerButton::Secondary)
        || (ui.input(|i| i.key_down(Key::Space)) && primary_drag);
    let delta = response.drag_delta();
    if primary_drag && state.axis_drag.is_some() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let (eye, dir) = state.camera.ray(pointer, rect);
            if let Some(ad) = &mut state.axis_drag {
                if let Some((_, param)) = closest_on_line(eye, dir, ad.origin, ad.axis.to_array()) {
                    let moved = Vec3::from(ad.origin) + ad.axis * (param - ad.start_param);
                    let at = if state.snap {
                        snap_axis_point(moved.to_array(), ad.axis)
                    } else {
                        moved.to_array()
                    };
                    ad.at = at;
                }
            }
        }
    } else if pan_drag {
        state.camera.pan(delta, rect);
    } else if primary_drag && state.drag.is_some() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let (eye, dir) = state.camera.ray(pointer, rect);
            if let Some(hit) = floor_point(eye, dir) {
                state.drag_at = Some(if state.snap {
                    snap_floor(hit)
                } else {
                    [hit[0], 0.0, hit[2]]
                });
            }
        }
    } else if primary_drag {
        state.camera.orbit(delta);
    }

    if response.drag_started() {
        if let Some(pointer) = response.interact_pointer_pos() {
            if state.tool == Tool::Select && state.mask_surface.is_none()
            {
                state.drag = None;
                state.drag_at = None;
                state.axis_drag = None;
                if let Some(sel) = state.selected.clone().filter(Select::movable) {
                    if let Some(origin) = selected_point(&scene, &sel) {
                        if let Some(axis) = axis_handle_at(&state.camera, rect, pointer, origin) {
                            let (eye, dir) = state.camera.ray(pointer, rect);
                            if let Some((_, start_param)) =
                                closest_on_line(eye, dir, origin, axis.to_array())
                            {
                                state.axis_drag = Some(AxisDrag {
                                    item: sel,
                                    origin,
                                    axis,
                                    start_param,
                                    at: origin,
                                });
                            }
                        }
                    }
                }
                if state.axis_drag.is_none() {
                    state.drag = pick(&scene, &state.camera, rect, pointer).filter(Select::movable);
                }
            }
        }
    }

    let mut post: Option<Value> = None;
    if response.drag_stopped() {
        if let Some(ad) = state.axis_drag.take() {
            post = Some(json!({
                "op": "move", "id": venue_id, "item": ad.item.id, "at": ad.at
            }));
        } else if let (Some(sel), Some(at)) = (state.drag.take(), state.drag_at.take()) {
            post = Some(json!({
                "op": "move", "id": venue_id, "item": sel.id, "at": at
            }));
        }
    }

    if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            if state.mask_surface.is_some() {
                if response.double_clicked() {
                    if state.mask_points.len() >= 3 {
                        post = Some(mask_body(venue_id, venue, &state));
                        state.mask_points.clear();
                        state.mask_surface = None;
                    }
                } else if let Some(surface_id) = state.mask_surface.clone() {
                    if let Some(surface) = scene.surface(&surface_id) {
                        let (eye, dir) = state.camera.ray(pointer, rect);
                        if let Some((_, st)) = ray_surface(surface, eye, dir) {
                            state.mask_points.push(st);
                        }
                    }
                }
            } else if state.tool == Tool::Ruler {
                if let Some(point) = ruler_point(&scene, &state.camera, rect, pointer, state.snap) {
                    if state.ruler.len() >= 2 {
                        state.ruler.clear();
                    }
                    state.ruler.push(point);
                }
            } else {
                state.selected = pick(&scene, &state.camera, rect, pointer);
            }
        }
    }

    if enter && state.mask_surface.is_some() && state.mask_points.len() >= 3 {
        post = Some(mask_body(venue_id, venue, &state));
        state.mask_points.clear();
        state.mask_surface = None;
    }

    // What is selected, and (for a projector) where it throws. Adding a
    // screen or a projection is the node's popup, not a button here.
    if let Some(sel) = &state.selected {
        ui.label(
            RichText::new(trf(
                "{kind}: {id}",
                &[("kind", &kind_word(sel.kind)), ("id", &sel.id)],
            ))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
        );
    }

    // Area readout for a selected projector.
    if let Some(sel) = &state.selected {
        if sel.kind == SelectKind::Projector {
            if let Ok(reply) = door.json(
                "POST",
                "/venue",
                Some(json!({ "op": "area", "id": venue_id, "projector": sel.id })),
            ) {
                let area = reply["area_m2"].as_f64().unwrap_or(0.0);
                let throw = reply["throw_m"].as_f64().unwrap_or(0.0);
                ui.label(
                    RichText::new(trf(
                        "area {area} m² · throw {throw} m",
                        &[
                            ("area", &format!("{area:.1}")),
                            ("throw", &format!("{throw:.1}")),
                        ],
                    ))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
                );
            }
        }
    }

    draw_scene(
        &painter,
        theme,
        rect,
        &state,
        &scene,
        selected_surface.as_deref(),
    );

    if let Some(note) = &state.note {
        components::note(ui, theme, note);
    }

    if let Some(body) = post {
        match door.json("POST", "/venue", Some(body)) {
            Ok(_) => state.note = None,
            Err((status, _)) => {
                state.note = Some(trf("venue write failed: {status}", &[("status", &status)]));
            }
        }
    }

    tool_strip(ui, theme, &mut state, rect);
    ui.data_mut(|d| d.insert_temp(state_id, state));
}

/// The viewport's tool strip: the four gestures as glyphs, in the same
/// vertical `Area` at the canvas's top-left corner the node canvas uses
/// (`shell/region/nodes/tools`), so the two canvases read as one application.
///
/// - `select` / `ruler`: which click gesture the pointer runs.
/// - `snap`: the quarter-metre floor grid, independent of the gesture.
/// - `ortho`: the projection, independent of both (the `O` key is the same
///   toggle).
///
/// `back to orbit` rides at the bottom while a shot is being looked through,
/// because it is the only way back from that camera besides Esc.
fn tool_strip(ui: &mut Ui, theme: &Theme, state: &mut ViewState, canvas: Rect) {
    egui::Area::new(Id::new("shell/region/venue/viewport/tools"))
        .order(egui::Order::Foreground)
        .fixed_pos(canvas.min + theme.item_gap)
        .constrain_to(canvas)
        .show(ui.ctx(), |ui| {
            theme.card_frame().inner_margin(theme.bar_margin()).show(ui, |ui| {
                ui.spacing_mut().item_spacing = theme.item_gap;
                ui.vertical(|ui| {
                    if icon_button(ui, theme, Icon::Pointer, state.tool == Tool::Select, tr("select")).clicked() {
                        state.tool = Tool::Select;
                    }
                    if icon_button(ui, theme, Icon::Ruler, state.tool == Tool::Ruler, tr("ruler")).clicked() {
                        state.tool = Tool::Ruler;
                        state.ruler.clear();
                    }
                    if icon_button(ui, theme, Icon::Magnet, state.snap, tr("snap")).clicked() {
                        state.snap = !state.snap;
                    }
                    if icon_button(ui, theme, Icon::Ortho, state.camera.ortho, tr("ortho")).clicked() {
                        state.camera.ortho = !state.camera.ortho;
                    }
                    if state.saved_camera.is_some()
                        && icon_button(ui, theme, Icon::Revert, false, tr("back to orbit")).clicked()
                    {
                        if let Some(cam) = state.saved_camera.take() {
                            state.camera = cam;
                        }
                    }
                });
            });
        });
}

fn kind_word(kind: SelectKind) -> String {
    match kind {
        SelectKind::Object => tr("object").to_string(),
        SelectKind::Fixture => tr("fixture").to_string(),
        SelectKind::Projector => tr("projector").to_string(),
        SelectKind::Surface => tr("surface").to_string(),
        SelectKind::Display => tr("display").to_string(),
    }
}

fn unique_id(venue: &Venue, prefix: &str) -> String {
    let mut used: BTreeSet<&str> = BTreeSet::new();
    used.extend(venue.objects.iter().map(|o| o.id.as_str()));
    used.extend(venue.fixtures.iter().map(|f| f.id.as_str()));
    used.extend(venue.projectors.iter().map(|p| p.id.as_str()));
    used.extend(venue.surfaces.iter().map(|s| s.id.as_str()));
    used.extend(venue.masks.iter().map(|m| m.id.as_str()));
    let mut n = 1;
    loop {
        let id = format!("{prefix}-{n}");
        if !used.contains(id.as_str()) {
            return id;
        }
        n += 1;
    }
}

fn mask_body(venue_id: &str, venue: &Venue, state: &ViewState) -> Value {
    let id = unique_id(venue, "mask");
    let points: Vec<[f32; 2]> = state.mask_points.clone();
    json!({
        "op": "mask", "id": venue_id, "mask": id,
        "on": state.mask_surface.clone().unwrap_or_default(),
        "points": points,
    })
}

fn draw_scene(
    painter: &egui::Painter,
    theme: &Theme,
    rect: Rect,
    state: &ViewState,
    scene: &Scene,
    selected_surface: Option<&str>,
) {
    let cam = &state.camera;
    let eye = cam.eye();

    // Floor grid at y = 0, around the target.
    let centre = cam.target;
    let span = (cam.distance * 0.7).clamp(6.0, 40.0).ceil() as i32;
    let x0 = (centre[0] - span as f32).floor() as i32;
    let x1 = (centre[0] + span as f32).ceil() as i32;
    let z0 = (centre[2] - span as f32).floor() as i32;
    let z1 = (centre[2] + span as f32).ceil() as i32;
    for x in x0..=x1 {
        if let (Some(a), Some(b)) = (
            cam.project([x as f32, 0.0, z0 as f32], rect),
            cam.project([x as f32, 0.0, z1 as f32], rect),
        ) {
            let major = x.rem_euclid(5) == 0;
            painter.line_segment(
                [a, b],
                theme.line_stroke(if major { theme.rule_bright } else { theme.rule }),
            );
        }
    }
    for z in z0..=z1 {
        if let (Some(a), Some(b)) = (
            cam.project([x0 as f32, 0.0, z as f32], rect),
            cam.project([x1 as f32, 0.0, z as f32], rect),
        ) {
            let major = z.rem_euclid(5) == 0;
            painter.line_segment(
                [a, b],
                theme.line_stroke(if major { theme.rule_bright } else { theme.rule }),
            );
        }
    }

    let mut faces: Vec<Face> = Vec::new();

    // Surfaces: a plane is one quad; a swept form is a 12×6 grid.
    for s in &scene.venue.surfaces {
        let sel = selected_surface == Some(s.id.as_str());
        let fill = if sel {
            theme.phosphor_deep
        } else if s.material.to_lowercase().contains("led") {
            theme.soft_dim
        } else {
            theme.panel_head
        };
        let stroke = if sel {
            theme.accent()
        } else {
            theme.line_stroke(theme.rule_bright)
        };
        match &s.form {
            SurfaceForm::Plane => {
                let pts = surface_polygon(s);
                if let Some(poly) = project_poly(cam, rect, &pts) {
                    faces.push(Face {
                        pts: poly,
                        fill,
                        stroke,
                        depth: depth(eye, centroid(&pts)),
                    });
                }
            }
            SurfaceForm::Swept { .. } => {
                let nu = 12usize;
                let nv = 6usize;
                for j in 0..nv {
                    for i in 0..nu {
                        let (u0, u1) = (i as f32 / nu as f32, (i + 1) as f32 / nu as f32);
                        let (v0, v1) = (j as f32 / nv as f32, (j + 1) as f32 / nv as f32);
                        let pts = vec![s.at(u0, v0), s.at(u1, v0), s.at(u1, v1), s.at(u0, v1)];
                        if let Some(poly) = project_poly(cam, rect, &pts) {
                            faces.push(Face {
                                pts: poly,
                                fill,
                                stroke: theme.line_stroke(theme.rule),
                                depth: depth(eye, centroid(&pts)),
                            });
                        }
                    }
                }
            }
        }
    }

    // Objects: boxes keep the fast six-quad path; any other form is drawn
    // from its `metal_geom` mesh, one triangle at a time, into the same
    // depth-sorted face list.
    let light = Vec3::new(0.4, 0.8, 0.45).normalize();
    for o in &scene.venue.objects {
        let sel = state
            .selected
            .as_ref()
            .is_some_and(|s| s.kind == SelectKind::Object && s.id == o.id);
        let fill = if sel {
            theme.phosphor_deep
        } else {
            theme.tube_raised
        };
        if matches!(o.form, ObjectForm::Box) {
            let corner = box_corners(o);
            let quads = [
                [0usize, 1, 2, 3],
                [4, 5, 6, 7],
                [0, 1, 5, 4],
                [1, 2, 6, 5],
                [2, 3, 7, 6],
                [3, 0, 4, 7],
            ];
            for q in quads {
                let pts = [corner[q[0]], corner[q[1]], corner[q[2]], corner[q[3]]];
                if let Some(poly) = project_poly(cam, rect, &pts) {
                    faces.push(Face {
                        pts: poly,
                        fill,
                        stroke: theme.line_stroke(theme.rule_bright),
                        depth: depth(eye, centroid(&pts)),
                    });
                }
            }
        } else {
            for t in o.mesh().triangles() {
                let pts = [t.a, t.b, t.c];
                let Some(poly) = project_poly(cam, rect, &pts) else {
                    continue;
                };
                let n = (Vec3::from(t.b) - Vec3::from(t.a))
                    .cross(Vec3::from(t.c) - Vec3::from(t.a))
                    .normalize_or_zero();
                let lit = n.dot(light).abs();
                let fill = if sel {
                    theme.phosphor_deep
                } else if lit > 0.65 {
                    theme.tube_raised
                } else if lit > 0.3 {
                    theme.panel_head
                } else {
                    theme.inset
                };
                faces.push(Face {
                    pts: poly,
                    fill,
                    stroke: theme.line_stroke(theme.rule_bright),
                    depth: depth(eye, centroid(&pts)),
                });
            }
        }
        // The crown as a sphere silhouette (a box's crown is not in its mesh).
        if matches!(o.form, ObjectForm::Box) {
            if let Some(c) = &o.crown {
                if let Some(p) = cam.project(c.center, rect) {
                    let edge = Vec3::from(c.center) + Vec3::X * c.radius;
                    let radius = cam
                        .project(edge.to_array(), rect)
                        .map(|e| (e - p).length())
                        .unwrap_or(theme.hairline * 2.0);
                    painter.circle_filled(
                        p,
                        radius,
                        if sel {
                            theme.phosphor_lit
                        } else {
                            theme.ink_weak
                        },
                    );
                }
            }
        }
        // A dummy's capsule silhouette.
        if o.kind == "dummy" {
            let base = Vec3::from(o.at);
            let top = base + Vec3::Y * o.size[1];
            if let (Some(a), Some(b)) = (
                cam.project(base.to_array(), rect),
                cam.project(top.to_array(), rect),
            ) {
                painter.line_segment([a, b], theme.accent());
                painter.circle_filled(b, theme.hairline * 3.0, theme.ink);
            }
        }
    }

    // Displays: their surface quad, already drawn as a surface; add a label.
    for d in &scene.venue.displays {
        if let Some(s) = scene.surface(&d.surface) {
            if let Some(p) = cam.project(s.at(0.5, 0.5), rect) {
                painter.text(
                    p,
                    Align2::CENTER_CENTER,
                    &d.id,
                    theme.font_small.clone(),
                    theme.ink,
                );
            }
        }
    }

    // Masks already on surfaces, and the draft.
    for m in &scene.venue.masks {
        draw_mask(painter, theme, rect, cam, scene, m);
    }
    if let Some(sid) = &state.mask_surface {
        if let Some(s) = scene.surface(sid) {
            if state.mask_points.len() >= 2 {
                let pts: Vec<[f32; 3]> = state
                    .mask_points
                    .iter()
                    .map(|st| s.at(st[0], st[1]))
                    .collect();
                if let Some(poly) = project_poly(cam, rect, &pts) {
                    painter.add(Shape::closed_line(poly, theme.accent()));
                }
            }
            for st in &state.mask_points {
                if let Some(p) = cam.project(s.at(st[0], st[1]), rect) {
                    painter.circle_filled(p, theme.hairline * 2.5, theme.phosphor_lit);
                }
            }
        }
    }

    faces.sort_by(|a, b| b.depth.total_cmp(&a.depth));
    for face in faces {
        if face.pts.len() >= 3 {
            painter.add(Shape::convex_polygon(face.pts, face.fill, face.stroke));
        }
    }

    // Fixtures: a dot and a beam line.
    for f in &scene.venue.fixtures {
        if let Some(at) = f.at {
            let sel = state
                .selected
                .as_ref()
                .is_some_and(|s| s.kind == SelectKind::Fixture && s.id == f.id);
            if let Some(p) = cam.project(at, rect) {
                painter.circle_filled(
                    p,
                    if sel {
                        theme.hairline * 4.0
                    } else {
                        theme.hairline * 2.5
                    },
                    if sel {
                        theme.phosphor_lit
                    } else {
                        theme.accent().color
                    },
                );
            }
            let dir = fixture_direction(f);
            let end = (Vec3::from(at) + dir * 3.0).to_array();
            if let (Some(a), Some(b)) = (cam.project(at, rect), cam.project(end, rect)) {
                painter.line_segment([a, b], theme.line_stroke(theme.warn));
            }
            if sel {
                labels(painter, theme, rect, cam, &f.id, at);
            }
        }
    }

    // Projectors: a small box and a frustum.
    for pr in &scene.venue.projectors {
        let Some(at) = scene.projector_at(pr) else {
            continue;
        };
        let sel = state
            .selected
            .as_ref()
            .is_some_and(|s| s.kind == SelectKind::Projector && s.id == pr.id);
        if let Some(p) = cam.project(at, rect) {
            painter.circle_filled(
                p,
                if sel {
                    theme.hairline * 4.0
                } else {
                    theme.hairline * 3.0
                },
                if sel { theme.phosphor_lit } else { theme.ink },
            );
        }
        let dir = fixture_dir(pr.yaw, pr.pitch);
        let (_, right, up) = cam.basis();
        let far = Vec3::from(at) + dir * 6.0;
        let spread = 1.0;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let end = (far + right * (a * spread) + up * (b * spread)).to_array();
            if let (Some(from), Some(to)) = (cam.project(at, rect), cam.project(end, rect)) {
                painter.line_segment([from, to], theme.line_stroke(theme.phosphor_dim));
            }
        }
        if sel {
            labels(painter, theme, rect, cam, &pr.id, at);
        }
    }

    // Transform gizmo: three world axes from the selected movable item.
    if let Some(sel) = &state.selected {
        if sel.movable() {
            if let Some(origin) = selected_point(scene, sel) {
                if let Some(from) = cam.project(origin, rect) {
                    for (axis, color) in [
                        (Vec3::X, theme.warn),
                        (Vec3::Y, theme.ok),
                        (Vec3::Z, theme.logo_metal),
                    ] {
                        let end = Vec3::from(origin) + axis * AXIS_LEN;
                        if let Some(to) = cam.project(end.to_array(), rect) {
                            painter.line_segment([from, to], theme.line_stroke(color));
                            painter.circle_filled(to, theme.hairline * 3.0, color);
                        }
                    }
                }
            }
        }
    }
    if let Some(ad) = &state.axis_drag {
        if let (Some(from), Some(to)) = (cam.project(ad.origin, rect), cam.project(ad.at, rect)) {
            painter.line_segment([from, to], theme.accent());
            painter.circle_filled(to, theme.hairline * 4.0, theme.phosphor_lit);
        }
    }

    // Ruler: points, the measured line, its end ticks and the label.
    if state.tool == Tool::Ruler && !state.ruler.is_empty() {
        let tick = theme.hairline * 4.0;
        for p in &state.ruler {
            if let Some(at) = cam.project(*p, rect) {
                painter.circle_filled(at, theme.hairline * 2.5, theme.phosphor_lit);
                painter.line_segment(
                    [at + Vec2::X * tick, at - Vec2::X * tick],
                    theme.line_stroke(theme.accent().color),
                );
                painter.line_segment(
                    [at + Vec2::Y * tick, at - Vec2::Y * tick],
                    theme.line_stroke(theme.accent().color),
                );
            }
        }
        if state.ruler.len() == 2 {
            let (a, b) = (state.ruler[0], state.ruler[1]);
            let d = measure_distance(a, b);
            if let (Some(pa), Some(pb)) = (cam.project(a, rect), cam.project(b, rect)) {
                painter.line_segment([pa, pb], theme.accent());
                let along = (pb - pa).normalized();
                let perp = Vec2::new(-along.y, along.x) * tick;
                painter.line_segment([pa + perp, pa - perp], theme.accent());
                painter.line_segment([pb + perp, pb - perp], theme.accent());
                painter.text(
                    pa + (pb - pa) * 0.5 + perp,
                    Align2::CENTER_BOTTOM,
                    trf("{d} m", &[("d", &format!("{d:.2}"))]),
                    theme.font_small.clone(),
                    theme.ink,
                );
            }
        }
    }

    // The selected item's label, wherever it lives.
    if let Some(sel) = &state.selected {
        if let Some(at) = selected_point(scene, sel) {
            labels(painter, theme, rect, cam, &sel.id, at);
        }
    }
}

fn labels(
    painter: &egui::Painter,
    theme: &Theme,
    rect: Rect,
    cam: &OrbitCamera,
    id: &str,
    at: [f32; 3],
) {
    if let Some(p) = cam.project(at, rect) {
        painter.text(
            p + theme.item_gap,
            Align2::LEFT_TOP,
            id,
            theme.font_small.clone(),
            theme.ink,
        );
    }
}

fn selected_point(scene: &Scene, sel: &Select) -> Option<[f32; 3]> {
    match sel.kind {
        SelectKind::Object => scene.object(&sel.id).map(|o| centroid(&box_corners(o))),
        SelectKind::Fixture => scene
            .venue
            .fixtures
            .iter()
            .find(|f| f.id == sel.id)
            .and_then(|f| f.at),
        SelectKind::Projector => scene
            .venue
            .projectors
            .iter()
            .find(|p| p.id == sel.id)
            .and_then(|p| scene.projector_at(p)),
        SelectKind::Surface => scene.surface(&sel.id).map(|s| s.at(0.5, 0.5)),
        SelectKind::Display => scene
            .venue
            .displays
            .iter()
            .find(|d| d.id == sel.id)
            .and_then(|d| scene.surface(&d.surface).map(|s| s.at(0.5, 0.5))),
    }
}

fn draw_mask(
    painter: &egui::Painter,
    theme: &Theme,
    rect: Rect,
    cam: &OrbitCamera,
    scene: &Scene,
    m: &VenueMask,
) {
    let Some(s) = scene.surface(&m.on) else {
        return;
    };
    if m.points.len() < 3 {
        return;
    }
    let pts: Vec<[f32; 3]> = m.points.iter().map(|st| s.at(st[0], st[1])).collect();
    if let Some(poly) = project_poly(cam, rect, &pts) {
        let fill = m
            .rgb
            .map(|rgb| {
                theme.color_of([
                    rgb[0] as f32 / 255.0,
                    rgb[1] as f32 / 255.0,
                    rgb[2] as f32 / 255.0,
                ])
            })
            .unwrap_or(theme.mask_inside);
        painter.add(Shape::convex_polygon(poly, fill, theme.accent()));
    }
}

fn fixture_direction(f: &Fixture) -> Vec3 {
    fixture_dir(f.yaw, f.pitch)
}

fn fixture_dir(yaw: f32, pitch: f32) -> Vec3 {
    let y = yaw.to_radians();
    let p = pitch.to_radians();
    Vec3::new(y.sin() * p.cos(), p.sin(), -y.cos() * p.cos()).normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_W: f32 = 800.0;
    const TEST_H: f32 = 600.0;

    fn rect() -> Rect {
        Rect::from_min_max(Pos2::ZERO, Pos2::new(TEST_W, TEST_H))
    }

    #[test]
    fn target_projects_to_the_rect_centre() {
        let cam = OrbitCamera {
            target: [1.0, 2.0, -3.0],
            distance: 12.0,
            yaw: 0.7,
            pitch: 0.35,
            fov: 55.0_f32.to_radians(),
            ortho: false,
        };
        let p = cam.project(cam.target, rect()).expect("in front");
        assert!((p.x - rect().center().x).abs() < 1.0e-3, "{p:?}");
        assert!((p.y - rect().center().y).abs() < 1.0e-3, "{p:?}");
    }

    #[test]
    fn a_point_behind_the_camera_is_none() {
        let cam = OrbitCamera::default();
        let behind = cam.eye() + cam.direction() * cam.distance;
        assert!(cam.project(behind.to_array(), rect()).is_none());
    }

    #[test]
    fn snap_floor_rounds_to_the_quarter_metre_grid() {
        assert_eq!(snap_floor([0.13, 7.0, -0.38]), [0.25, 0.0, -0.5]);
        assert_eq!(snap_floor([-0.11, 2.0, 0.13]), [0.0, 0.0, 0.25]);
    }

    #[test]
    fn closest_on_line_projects_onto_the_axis() {
        let (p, t) = closest_on_line(
            [2.0, 3.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
        )
        .expect("not parallel");
        assert!((t - 2.0).abs() < 1.0e-4, "{p:?} {t}");
        assert!((p[0] - 2.0).abs() < 1.0e-4, "{p:?}");
        assert!(p[1].abs() < 1.0e-4, "{p:?}");
        assert!(p[2].abs() < 1.0e-4, "{p:?}");
    }

    #[test]
    fn camera_from_shot_keeps_the_shot_eye_and_look_direction() {
        let shot = Camera {
            eye: [3.0, 1.5, -2.0],
            yaw: 35.0,
            pitch: -12.0,
            roll: 0.0,
            fov: 48.0,
            aspect: 16.0 / 9.0,
        };
        let cam = camera_from_shot(&shot, 9.0);
        assert!((cam.distance - 9.0).abs() < 1.0e-4);
        assert!((cam.fov - 48.0_f32.to_radians()).abs() < 1.0e-5);
        let eye = cam.eye();
        assert!((eye - Vec3::from(shot.eye)).length() < 1.0e-4, "{eye:?}");
        let target = Vec3::from(cam.target);
        let want = Vec3::from(shot.eye) + direction(shot.yaw, shot.pitch) * 9.0;
        assert!((target - want).length() < 1.0e-4, "{target:?} {want:?}");
    }

    #[test]
    fn orthographic_keeps_the_target_centred_and_the_plane_scale() {
        let cam = OrbitCamera {
            target: [1.0, 2.0, -3.0],
            distance: 12.0,
            yaw: 0.7,
            pitch: 0.35,
            fov: 55.0_f32.to_radians(),
            ortho: true,
        };
        let centre = cam.project(cam.target, rect()).expect("in front");
        assert!((centre.x - rect().center().x).abs() < 1.0e-3, "{centre:?}");
        assert!((centre.y - rect().center().y).abs() < 1.0e-3, "{centre:?}");
        let (_, right, up) = cam.basis();
        let tan = (cam.fov * 0.5).tan();
        let aspect = TEST_W / TEST_H;
        let half_h = cam.distance * tan;
        let half_w = half_h * aspect;
        let x = cam
            .project((Vec3::from(cam.target) + right * half_w).to_array(), rect())
            .expect("in front");
        let y = cam
            .project((Vec3::from(cam.target) + up * half_h).to_array(), rect())
            .expect("in front");
        assert!((x.x - rect().max.x).abs() < 1.0e-2, "{x:?}");
        assert!((x.y - rect().center().y).abs() < 1.0e-2, "{x:?}");
        assert!((y.y - rect().min.y).abs() < 1.0e-2, "{y:?}");
        assert!((y.x - rect().center().x).abs() < 1.0e-2, "{y:?}");
    }

    #[test]
    fn measure_distance_is_the_world_distance() {
        assert!((measure_distance([0.0, 0.0, 0.0], [3.0, 4.0, 0.0]) - 5.0).abs() < 1.0e-5);
        assert!(measure_distance([1.0; 3], [1.0; 3]).abs() < 1.0e-6);
    }

    #[test]
    fn a_swept_object_mesh_projects_into_the_viewport() {
        let mut o = Object {
            id: "s".into(),
            name: "s".into(),
            kind: "stage".into(),
            at: [0.0, 0.0, 0.0],
            size: [2.0, 1.0, 1.0],
            yaw: 0.0,
            crown: None,
            occludes: true,
            emits: 0.0,
            faces: vec![],
            description: String::new(),
            form: ObjectForm::Swept {
                path: vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                profile: vec![[-0.5, 0.0], [0.5, 0.0], [0.5, 0.5], [-0.5, 0.5]],
                caps: true,
                closed: true,
                kind: "polyline".into(),
            },
            parent: None,
            attribution: String::new(),
            source: String::new(),
        };
        let mesh = o.mesh();
        assert!(!mesh.triangles().is_empty());
        let (lo, hi) = o.bbox().unwrap();
        let centre = (Vec3::from(lo) + Vec3::from(hi)) * 0.5;
        let mut cam = OrbitCamera::default();
        cam.target = centre.to_array();
        assert!(cam.project(centre.to_array(), rect()).is_some());
        o.yaw = 90.0;
        assert!(o.bbox().is_some());
    }

    #[test]
    fn plane_st_inverts_the_bilinear_plane() {
        let origin = [-2.0, 1.0, 3.0];
        let u = [4.0, 0.0, 0.0];
        let v = [0.0, 0.0, -5.0];
        for (s, t) in [(0.0, 0.0), (0.3, 0.9), (1.0, 1.0)] {
            let p = (Vec3::from(origin) + Vec3::from(u) * s + Vec3::from(v) * t).to_array();
            let got = plane_st(p, origin, u, v).expect("invertible");
            assert!((got[0] - s).abs() < 1.0e-4, "{got:?}");
            assert!((got[1] - t).abs() < 1.0e-4, "{got:?}");
        }
    }
}
