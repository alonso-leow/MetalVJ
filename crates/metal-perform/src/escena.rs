//! O.5 / J.1 (which expands H.5d): the projection area as 3D geometry, and
//! masks defined **on the geometry** instead of on the projector's plane.
//!
//! # The one decision everything else follows from
//!
//! A mask is still a polygon in a device's own 0..1 space
//! ([`crate::output::Mask`]) and is still cut by
//! [`crate::output::apply_masks`]. **This module does not cut anything.** It
//! is an *authoring source* for `Mask::points`: describe the room once as
//! planes, draw a contour on a wall, and [`Escena::project`] produces the 2D
//! contour each output would see.
//!
//! That seam is deliberate and it is the whole reason this is a small module
//! rather than a second renderer:
//!
//! - **One origin survives.** `output.rs` keeps being the only thing that
//!   cuts a frame, so nested masks, booleans and alpha (O.1) all keep working
//!   on a projected contour with no extra code — a scene mask is an ordinary
//!   `Mask` by the time anything looks at it.
//! - **The grid stays the short path.** H.5b/H.5c's numbered grid is
//!   untouched and is still how you map a floor of tiles in thirty seconds.
//!   *"The grid is the short path; the scene is the expressive path.
//!   Neither replaces the other."* If using the scene were ever the only way
//!   to get a deformed contour, this design would be wrong.
//!
//! # Why 3D and not a homography per wall
//!
//! J.1 gives two reasons and they are the same reason twice:
//!
//! 1. **Several projectors come out of one definition.** Each output is a
//!    camera inside the same geometry ([`Escena::cameras`]), so two
//!    projectors on one wall — or one per wall — are two views of one room
//!    rather than three calibrations somebody has to keep coherent by hand.
//! 2. **The space can be dissected to follow a body.** A person's position is
//!    a point in the same room the walls are in, so a mask that tracks them
//!    ([`Escena::spotlight`]) moves *because the person moved*, not because
//!    somebody animated a polygon. A per-wall homography can express the
//!    first case and cannot express this one at all: it has no room in it,
//!    only a wall.
//!
//! # What a plane is, and why a parallelogram
//!
//! A [`Wall`] is an origin plus two edge vectors, so a point on it is
//! `origin + s*u + t*v` with `s`/`t` in `0..1`. That is the same normalised
//! space `Mask::points` already uses, which means drawing on a wall and
//! drawing on a projector are the same gesture with the same numbers — and a
//! contour authored on the projector can be re-hung on a wall without being
//! re-drawn.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glam::{Mat3, Mat4, Vec2, Vec3, Vec4};
use serde::{Deserialize, Serialize};

use crate::output::Mask;

/// A flat surface in the room: `origin + s*u + t*v`, `s`/`t` in `0..1`.
///
/// Not an infinite plane with a normal: a wall has edges, and the edges are
/// what make `(s, t)` mean anything. A normal-plus-distance plane would need
/// a second, separate convention for "where on it", which is the thing this
/// module exists to avoid having two of.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wall {
    pub id: String,
    pub name: String,
    /// The `(0, 0)` corner, in metres.
    pub origin: [f32; 3],
    /// The edge that `s` runs along.
    pub u: [f32; 3],
    /// The edge that `t` runs along.
    pub v: [f32; 3],
    /// A hand-dragged calibration: output id -> the nine numbers of the
    /// homography taking this wall's `(s, t)` to that output's 0..1
    /// ([`Homography::to_fit`]). A [`View`] in the same scene wins over it;
    /// this is what an operator reaches for when there is no 3D scene to
    /// place a camera in (plan: "calibrate wall").
    #[serde(default)]
    pub fit: BTreeMap<String, [f32; 9]>,
}

impl Wall {
    /// A point on this wall, from its own `(s, t)`.
    pub fn at(&self, s: f32, t: f32) -> Vec3 {
        Vec3::from(self.origin) + Vec3::from(self.u) * s + Vec3::from(self.v) * t
    }

    /// The `(s, t)` of the point on this wall closest to `p`, and how far `p`
    /// is from the wall in metres.
    ///
    /// Unclamped on purpose: a body standing past the left edge of a wall
    /// reports `s < 0`, and a spotlight that wants to clamp can, while
    /// something asking "which wall is this person nearest" needs the honest
    /// distance rather than one folded back onto the surface.
    pub fn nearest(&self, p: Vec3) -> (f32, f32, f32) {
        let (u, v) = (Vec3::from(self.u), Vec3::from(self.v));
        let d = p - Vec3::from(self.origin);
        // Solved in the wall's own basis rather than by projecting onto a
        // normal: `u` and `v` need not be perpendicular (a trapezoid wall, a
        // sloped ceiling), and assuming they are is exactly the kind of
        // silently-wrong that only shows up on the one wall in the room that
        // is not square.
        let (uu, uv, vv) = (u.dot(u), u.dot(v), v.dot(v));
        let det = uu * vv - uv * uv;
        if det.abs() < 1e-9 {
            // A degenerate wall (zero-length or parallel edges) has no
            // interior to solve in. Report the origin and the raw distance
            // rather than dividing by zero.
            return (0.0, 0.0, d.length());
        }
        let (du, dv) = (d.dot(u), d.dot(v));
        let s = (du * vv - dv * uv) / det;
        let t = (dv * uu - du * uv) / det;
        (s, t, (p - self.at(s, t)).length())
    }
}

/// Where one output looks from, inside the room.
///
/// A projector and a camera are the same thing pointed the other way, which
/// is why this is one struct and not two: the frustum that throws light onto
/// a wall is the frustum that would photograph it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// The [`crate::output::Output::id`] this view belongs to.
    pub output: String,
    pub eye: [f32; 3],
    pub target: [f32; 3],
    #[serde(default = "up_y")]
    pub up: [f32; 3],
    /// Vertical field of view, in degrees.
    #[serde(default = "default_fov")]
    pub fov: f32,
    /// Width over height of the output this drives. Kept here rather than
    /// read off the `Output` so a scene can be reasoned about — and tested —
    /// without a device attached.
    #[serde(default = "default_aspect")]
    pub aspect: f32,
}

fn up_y() -> [f32; 3] {
    [0.0, 1.0, 0.0]
}
fn default_fov() -> f32 {
    60.0
}
fn default_aspect() -> f32 {
    16.0 / 9.0
}

impl View {
    /// World point to this output's own 0..1 space, `(0, 0)` top left — the
    /// same space [`Mask::points`] lives in.
    ///
    /// `None` for a point behind the eye. Not clamped to the frame: a contour
    /// corner that falls outside a projector's throw is real information (the
    /// wall is bigger than the picture) and folding it back onto the edge
    /// would silently reshape the polygon into something the room does not
    /// contain.
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let clip = self.view_projection() * p.extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        // NDC is `-1..1` with `+y` up; a frame is `0..1` with `+y` down.
        Some(Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5))
    }

    fn view_projection(&self) -> Mat4 {
        let proj = Mat4::perspective_rh(
            self.fov.to_radians(),
            self.aspect,
            // Near/far are fixed rather than authored: a room is metres
            // across, and exposing two more numbers whose only effect is to
            // clip geometry nobody can see is a knob that can only be set
            // wrong.
            0.01,
            1000.0,
        );
        proj * Mat4::look_at_rh(
            Vec3::from(self.eye),
            Vec3::from(self.target),
            Vec3::from(self.up),
        )
    }
}

/// A contour drawn **on a wall**, in that wall's own `(s, t)`.
///
/// The 3D counterpart of [`Mask`], and deliberately not a variant of it: a
/// `Mask` is what a device cuts with and every consumer of one is entitled to
/// assume its points are in that device's space. This is what a contour *is*
/// before any projector has seen it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WallMask {
    pub id: String,
    pub name: String,
    /// Which [`Wall`] it is drawn on.
    pub wall: String,
    /// The contour, in the wall's `(s, t)`.
    pub points: Vec<[f32; 2]>,
    /// Carried through to the projected [`Mask`] untouched — nesting and
    /// alpha are properties of the contour, not of the projection.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default = "one")]
    pub alpha: f32,
}

fn one() -> f32 {
    1.0
}

/// The room: its surfaces, the outputs looking at it, and the contours drawn
/// on it.
///
/// # `Escena` as the combinator
///
/// > *"Until now 'several outputs' was a list; with `escena` it is a scene with
/// > several views."*
///
/// That is what [`Escena::views`] is. Two projectors on one wall are two
/// entries here, not two calibrations — and adding a third is adding a
/// camera, not redoing the geometry.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Escena {
    #[serde(default)]
    pub walls: Vec<Wall>,
    #[serde(default)]
    pub views: Vec<View>,
    #[serde(default)]
    pub masks: Vec<WallMask>,
}

/// Where a scene lives, next to the rest of `data/`.
///
/// One file and not one per device, because that is the point: *"several
/// outputs"* used to be a list and a scene is one room several outputs look
/// into. A scene per device would be back to calibrations kept coherent by
/// hand.
pub fn path(root: &std::path::Path) -> std::path::PathBuf {
    // Tests read and write a per-process scratch scene, never the operator's
    // (rule 8) — the same fence `engine::graphs_dir` puts around the graphs,
    // and for the same reason: `data_root()` is a `OnceLock` a test cannot
    // point elsewhere, and `POST /scene {op: "fit"}` writes this file.
    if cfg!(any(test, feature = "test-isolated-graphs")) && root == crate::output::data_root() {
        static SCRATCH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        return SCRATCH
            .get_or_init(|| {
                let dir = std::env::temp_dir().join(format!("fw-test-scene-{}", std::process::id()));
                let _ = metal_vfs::remove_dir_all(&dir);
                let _ = metal_vfs::create_dir_all(&dir);
                dir
            })
            .join("escena.json");
    }
    root.join("escena.json")
}

/// The scene on disk, or an empty one.
///
/// A missing or unreadable file is an empty scene, not an error: no scene is
/// the normal state — the grid is the short path and most rooms never need
/// this — and a projector that refused to start because `escena.json` had a
/// stray comma would be the feature punishing everyone who is not using it.
pub fn load(root: &std::path::Path) -> Escena {
    metal_vfs::read_to_string(path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

impl Escena {
    pub fn wall(&self, id: &str) -> Option<&Wall> {
        self.walls.iter().find(|w| w.id == id)
    }

    pub fn view(&self, output: &str) -> Option<&View> {
        self.views.iter().find(|v| v.output == output)
    }

    /// The warp from `wall`'s `(s, t)` to `output`'s 0..1, or `None` when the
    /// scene knows of neither a [`View`] for the output nor a [`fit`] on the
    /// wall.
    ///
    /// **A `View` wins over a `fit`** (plan: "A View, when there is one, wins
    /// over the fit"). The scene is the exact geometry; a fit is four points
    /// somebody dragged, and when the two disagree it is the drag that is
    /// stale. The disagreement is measured with [`Homography::distance`] and
    /// reported on stderr rather than silently swallowed, so a scene that has
    /// drifted from a hand calibration says so.
    ///
    /// [`fit`]: Wall::fit
    pub fn homography(&self, output: &str, wall: &Wall) -> Option<Homography> {
        let from_fit = wall
            .fit
            .get(output)
            .and_then(|f| Homography::from_fit(f));
        let from_view = self
            .view(output)
            .map(|v| Homography::from_view(v, wall))
            .filter(|h| h.inverse().is_some());
        match (from_view, from_fit) {
            (Some(view), Some(fit)) => {
                if let Some(d) = view.distance(&fit) {
                    if d > WARN_FIT_DISTANCE {
                        eprintln!(
                            "  scene: view and fit for output {output:?} wall {:?} differ by {d:.4}; using the view",
                            wall.id
                        );
                    }
                }
                Some(view)
            }
            (Some(view), None) => Some(view),
            (None, Some(fit)) => Some(fit),
            (None, None) => None,
        }
    }

    /// Write the scene to `escena.json` under `root` through
    /// [`crate::atomic::write`], so a crash cannot leave a half-written scene.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = path(root);
        if let Some(dir) = path.parent() {
            metal_vfs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::atomic::write(&path, format!("{text}\n"))
    }

    /// Every contour this scene puts on `output`, as ordinary [`Mask`]s.
    ///
    /// **This is the whole point of the module.** The projection of the scene
    /// to each output is what deforms the outline — *"that is the friendly way
    /// to distort a mask's outline: instead of dragging points until they fit,
    /// you move the wall"*. Downstream, nothing knows
    /// a scene was involved: these go into `Output::masks` and are cut by
    /// `apply_masks` exactly like a hand-drawn contour.
    ///
    /// A contour with any corner behind the projector is dropped rather than
    /// half-projected. A polygon missing one vertex is not a smaller polygon,
    /// it is a different shape, and quietly cutting the frame to it would be
    /// the worst kind of wrong: plausible.
    pub fn project(&self, output: &str) -> Vec<Mask> {
        let Some(view) = self.view(output) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for m in &self.masks {
            let Some(wall) = self.wall(&m.wall) else {
                continue;
            };
            let mut points = Vec::with_capacity(m.points.len());
            let mut whole = true;
            for p in &m.points {
                match view.project(wall.at(p[0], p[1])) {
                    Some(v) => points.push([v.x, v.y]),
                    None => {
                        whole = false;
                        break;
                    }
                }
            }
            if !whole {
                continue;
            }
            out.push(Mask {
                id: m.id.clone(),
                name: m.name.clone(),
                points,
                parent: m.parent.clone(),
                alpha: m.alpha,
                compose: None,
                stroke: None,
            });
        }
        out
    }

    /// Which wall a point in the room is nearest, and where on it.
    ///
    /// The dissection J.1 asks for: *"with the person's position taken from the
    /// camera and its approximation to each wall, the body's position is
    /// directly tied to the 3D definition of the masks"*. Returns `(wall id, s, t, distance)`.
    ///
    /// Nearest by distance to the surface, and **only counting walls the
    /// point is actually in front of** (`0..1` in both `s` and `t`): a person
    /// standing in the middle of a room is metres from the far wall's plane
    /// but not in front of it, and a spotlight that jumped to whichever plane
    /// happened to be closest in the abstract would land on a wall nobody is
    /// facing.
    pub fn nearest_wall(&self, p: [f32; 3]) -> Option<(&Wall, f32, f32, f32)> {
        let p = Vec3::from(p);
        self.walls
            .iter()
            .map(|w| {
                let (s, t, d) = w.nearest(p);
                (w, s, t, d)
            })
            .filter(|(_, s, t, _)| (0.0..=1.0).contains(s) && (0.0..=1.0).contains(t))
            .min_by(|a, b| a.3.total_cmp(&b.3))
    }

    /// A contour that follows a body: a box of `size` (in wall units) around
    /// wherever `body` is standing, on whichever wall they are in front of.
    ///
    /// J.3's script nodes are what make this *interesting* — "if the body is
    /// to the right, widen the mask to the right" is logic, and logic belongs
    /// in a script node, not here. This is the geometric half, and it is
    /// deliberately the boring half: give it a point, get a contour on the
    /// right wall. A spotlight that decided how to behave would be a second
    /// place where behaviour lives.
    pub fn spotlight(&self, id: &str, body: [f32; 3], size: (f32, f32)) -> Option<WallMask> {
        let (wall, s, t, _) = self.nearest_wall(body)?;
        let (hw, hh) = (size.0 / 2.0, size.1 / 2.0);
        Some(WallMask {
            id: id.to_string(),
            name: id.to_string(),
            wall: wall.id.clone(),
            points: vec![
                [s - hw, t - hh],
                [s + hw, t - hh],
                [s + hw, t + hh],
                [s - hw, t + hh],
            ],
            parent: None,
            alpha: 1.0,
        })
    }
}

// ---------------------------------------------------------------------------
// the warp: one homography per (output, wall)
// ---------------------------------------------------------------------------

/// A plane-to-plane projective map, row-major 3×3: `(s, t)` lands at
/// `(x / w, y / w)` where `(x, y, w) = H · (s, t, 1)`.
///
/// **What it is for.** A wall is flat, so everything a projector does to it
/// — perspective, keystone, the far edge coming out narrower — is exactly one
/// of these, from the wall's `(s, t)` to that output's 0..1 space. That is
/// what lets a 3D mask carry *content* and not only a contour: the source is
/// drawn in the wall's own space and each output warps it with its own `H`
/// (`compose::Warp`), computed once per frame and never per pixel.
///
/// Two ways to get one, and a [`View`] wins when both exist
/// ([`Escena::homography`]):
///
/// - [`Homography::from_view`]: the scene's own geometry, exact on the plane.
/// - [`Homography::from_corners`]: four `(s, t) ↔ (x, y)` pairs — the corners
///   an operator dragged onto the real wall, saved as the wall's `fit`.
///
/// `f64` inside: the four-point solve and the products it goes through are
/// where precision is lost; the compositor's per-pixel loop takes its own
/// `f32` copy once per tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

/// The smallest `w` a point may have and still count as in front of the
/// projector — the same threshold [`View::project`] uses on `clip.w`, which
/// is exactly the `w` [`Homography::from_view`] produces.
const IN_FRONT: f64 = 1e-6;

/// How far a `View` and a `fit` may put the same wall apart before
/// [`Escena::homography`] reports them: about half a percent of the frame, the
/// scale at which a projector's picture is visibly off the thing it should
/// cover. Shared with `venue::Venue::homography`, which makes the same
/// measurement for a projector's `pins`.
pub(crate) const WARN_FIT_DISTANCE: f32 = 0.005;

impl Homography {
    pub const IDENTITY: Homography = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    /// `view_projection · [u v origin]` → 3×3: the output's camera applied to
    /// the wall's own basis, then folded from NDC into the frame's 0..1 the
    /// way [`View::project`] folds a point. Exact for every point on the
    /// wall's plane, not an approximation fitted to its corners.
    ///
    /// Why this works: a point on the wall is `origin + s·u + t·v`, so its
    /// clip-space position is `s·(VP·u) + t·(VP·v) + VP·origin` — linear in
    /// `(s, t, 1)`. Dropping clip `z` (depth, which a 2D frame never reads)
    /// leaves the three rows `x, y, w` of a 3×3; `x' = 0.5·x/w + 0.5` and
    /// `y' = 0.5 − 0.5·y/w` share that `w`, so the fold is one more linear
    /// step on top.
    pub fn from_view(view: &View, wall: &Wall) -> Homography {
        let vp = view.view_projection();
        let column = |c: Vec4| {
            let (x, y, w) = (c.x as f64, c.y as f64, c.w as f64);
            [0.5 * x + 0.5 * w, 0.5 * w - 0.5 * y, w]
        };
        let a = column(vp * Vec3::from(wall.u).extend(0.0));
        let b = column(vp * Vec3::from(wall.v).extend(0.0));
        let c = column(vp * Vec3::from(wall.origin).extend(1.0));
        Homography([a[0], b[0], c[0], a[1], b[1], c[1], a[2], b[2], c[2]])
    }

    /// The inverse of [`Homography::from_view`]: the [`View`] a projector with
    /// these intrinsics would need to throw this map onto `wall`.
    ///
    /// P2 item 3, "fit without `View`": a planar homography is enough to place
    /// a camera, so a wall that only has a dragged `fit` can still be reasoned
    /// about as geometry (and its named contours projected) instead of falling
    /// back to a whole-wall rectangle. Standard planar-pose decomposition, in
    /// the form that keeps the sign honest: the wall's own `u`/`v` give an
    /// orthonormal basis, so recovering the pose is one 3×3 solve rather than
    /// the two-fold normal ambiguity of the textbook four-point method.
    ///
    /// `None` for a map that is not a camera pose — a wall seen edge-on, a
    /// `fit` whose corners do not define one, or one that puts the wall behind
    /// the projector. The caller decides what "no view" means; this only
    /// answers whether one exists.
    pub fn to_view(&self, wall: &Wall, fov: f32, aspect: f32) -> Option<View> {
        let (u, v) = (Vec3::from(wall.u), Vec3::from(wall.v));
        let origin = Vec3::from(wall.origin);
        let (un, vn) = (u.length(), v.length());
        if un < 1e-9 || vn < 1e-9 || !aspect.is_finite() || aspect <= 0.0 {
            return None;
        }
        let f = 1.0 / (fov.to_radians() * 0.5).tan();
        if !f.is_finite() || f <= 0.0 {
            return None;
        }
        // `from_view` folds clip space into 0..1 with `x' = (x/w)/2 + 1/2` and
        // `y' = 1/2 - (y/w)/2`, so the camera matrix is
        // `K = [[a, 0, 1/2], [0, -b, 1/2], [0, 0, 1]]` with `a = f/(2·aspect)`
        // and `b = f/2`; the columns of `K⁻¹·H` are then `λ·R·u`, `λ·R·v`
        // and `λ·R·(origin - eye)`.
        let (a, b) = ((f / (2.0 * aspect)) as f64, (f / 2.0) as f64);
        let m = &self.0;
        let column = |j: usize| {
            Vec3::new(
                ((m[j] - 0.5 * m[6 + j]) / a) as f32,
                ((-m[3 + j] + 0.5 * m[6 + j]) / b) as f32,
                m[6 + j] as f32,
            )
        };
        let (n0, n1, n2) = (column(0), column(1), column(2));
        // The wall has to be in front: `n2.z` is `λ` times the depth of the
        // wall's origin along the camera's forward axis.
        if !(n2.z > IN_FRONT as f32) {
            return None;
        }
        let lambda = n0.length() / un;
        if lambda < 1e-9 {
            return None;
        }
        // A right-handed basis in the wall's plane, then the camera's image of
        // it. `n0 × n1` is `λ²·det(R)·R(u × v)` and `R` here is improper (the
        // camera's `+z` forward against glam's right-handed view space), so
        // the cross carries a sign the normalised rotation does not.
        let q1 = u / un;
        let q3 = u.cross(v).normalize();
        if !q3.is_finite() {
            return None;
        }
        let q2 = q3.cross(q1);
        let rq1 = n0.normalize();
        let rq3 = -(n0.cross(n1)).normalize();
        if !rq1.is_finite() || !rq3.is_finite() {
            return None;
        }
        let rq2 = rq1.cross(rq3);
        let r = Mat3::from_cols(rq1, rq2, rq3) * Mat3::from_cols(q1, q2, q3).transpose();
        if !r.is_finite() {
            return None;
        }
        let eye = origin - r.transpose() * (n2 / lambda);
        let forward = r.row(2);
        let up = r.row(1);
        if !eye.is_finite() || !forward.is_finite() || !up.is_finite() {
            return None;
        }
        Some(View {
            output: String::new(),
            eye: eye.into(),
            target: (eye + forward).into(),
            up: up.into(),
            fov,
            aspect,
        })
    }

    /// The map taking each `st[i]` to `xy[i]` — the direct linear transform
    /// over four correspondences, solved in f64 after normalising both point
    /// sets (centroid at the origin, mean distance √2) so the answer does not
    /// depend on where in the unit square the points happen to sit.
    ///
    /// `None` when the four do not define one: three of them collinear on
    /// either side, points that coincide, or an order that folds the wall
    /// through infinity (two corners crossed) — every correspondence has to
    /// land in front, `w > 0`, the same side as the centre of the four.
    /// Those are not a calibration, and a warp built from one would smear
    /// the source across the whole frame.
    pub fn from_corners(st: &[[f32; 2]; 4], xy: &[[f32; 2]; 4]) -> Option<Homography> {
        if st.iter().chain(xy.iter()).any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return None;
        }
        let ts = normalising(st)?;
        let tx = normalising(xy)?;
        let at = |t: &[f64; 9], p: [f32; 2]| [t[0] * p[0] as f64 + t[2], t[4] * p[1] as f64 + t[5]];
        let mut a = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let [s, t] = at(&ts, st[i]);
            let [x, y] = at(&tx, xy[i]);
            a[2 * i] = [s, t, 1.0, 0.0, 0.0, 0.0, -s * x, -t * x, x];
            a[2 * i + 1] = [0.0, 0.0, 0.0, s, t, 1.0, -s * y, -t * y, y];
        }
        let h = solve8(a)?;
        // `h22 = 1` in normalised coordinates is the centroid of the `st`
        // points landing at a finite point with `w = 1`: true for any four
        // points that define a map at all, and it fixes the sign — the
        // centre of the quad is in front by construction.
        let hn = [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0];
        let back = Homography(tx).inverse()?;
        let out = Homography(mul3(&mul3(&back.0, &hn), &ts));
        // A solve can succeed and still hand back a rank-deficient matrix
        // (targets on one line): that squashes the wall onto a line.
        out.inverse()?;
        for (p, want) in st.iter().zip(xy.iter()) {
            let got = out.map(*p)?;
            if (got[0] - want[0]).abs() > 1e-3 || (got[1] - want[1]).abs() > 1e-3 {
                return None;
            }
        }
        Some(out)
    }

    /// The map best fitting `st[i] -> xy[i]` over four **or more**
    /// correspondences — the least-squares generalisation of
    /// [`Homography::from_corners`], for P2 item 3's "several coplanar fits".
    ///
    /// Exactly four pairs take the same exact path as `from_corners`; more
    /// pairs build the `2n × 8` DLT system (after the same Hartley
    /// normalisation) and solve its normal equations, so a surface covered by
    /// several projectors' pins becomes one map instead of one per projector.
    /// `None` when there are too few points, or when the best fit is not a map
    /// at all.
    pub fn from_correspondences(st: &[[f32; 2]], xy: &[[f32; 2]]) -> Option<Homography> {
        if st.len() < 4 || st.len() != xy.len() {
            return None;
        }
        if st.iter().chain(xy.iter()).any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return None;
        }
        if st.len() == 4 {
            return Homography::from_corners(&[st[0], st[1], st[2], st[3]], &[xy[0], xy[1], xy[2], xy[3]]);
        }
        let ts = normalising(st)?;
        let tx = normalising(xy)?;
        let at = |t: &[f64; 9], p: [f32; 2]| [t[0] * p[0] as f64 + t[2], t[4] * p[1] as f64 + t[5]];
        let mut rows: Vec<[f64; 9]> = Vec::with_capacity(2 * st.len());
        for i in 0..st.len() {
            let [s, t] = at(&ts, st[i]);
            let [x, y] = at(&tx, xy[i]);
            rows.push([s, t, 1.0, 0.0, 0.0, 0.0, -s * x, -t * x, x]);
            rows.push([0.0, 0.0, 0.0, s, t, 1.0, -s * y, -t * y, y]);
        }
        // Normal equations `AᵀA·h = Aᵀb`, accumulated as an 8×9 augmented
        // system (the ninth column is the right-hand side) and solved by the
        // same Gauss-Jordan `solve8` the exact four-point case uses.
        let mut ata = [[0.0f64; 9]; 8];
        for r in &rows {
            for i in 0..8 {
                for j in 0..8 {
                    ata[i][j] += r[i] * r[j];
                }
                ata[i][8] += r[i] * r[8];
            }
        }
        let h = solve8(ata)?;
        let hn = [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0];
        let back = Homography(tx).inverse()?;
        let out = Homography(mul3(&mul3(&back.0, &hn), &ts));
        out.inverse()?;
        for (p, want) in st.iter().zip(xy.iter()) {
            let got = out.map(*p)?;
            if (got[0] - want[0]).abs() > 1e-2 || (got[1] - want[1]).abs() > 1e-2 {
                return None;
            }
        }
        Some(out)
    }

    /// The map going the other way, or `None` for a singular one (a wall seen
    /// exactly edge-on, a fit whose corners collapsed onto a line).
    pub fn inverse(&self) -> Option<Homography> {
        let m = &self.0;
        let c00 = m[4] * m[8] - m[5] * m[7];
        let c01 = m[5] * m[6] - m[3] * m[8];
        let c02 = m[3] * m[7] - m[4] * m[6];
        let det = m[0] * c00 + m[1] * c01 + m[2] * c02;
        let norm = m.iter().map(|v| v * v).sum::<f64>().sqrt();
        if !det.is_finite() || det.abs() <= 1e-12 * norm.powi(3) {
            return None;
        }
        Some(Homography([
            c00 / det,
            (m[2] * m[7] - m[1] * m[8]) / det,
            (m[1] * m[5] - m[2] * m[4]) / det,
            c01 / det,
            (m[0] * m[8] - m[2] * m[6]) / det,
            (m[2] * m[3] - m[0] * m[5]) / det,
            c02 / det,
            (m[1] * m[6] - m[0] * m[7]) / det,
            (m[0] * m[4] - m[1] * m[3]) / det,
        ]))
    }

    /// Where `p` lands, or `None` when it is behind the projector (`w` at or
    /// below the same threshold [`View::project`] uses) — a point that folds
    /// through infinity is not a place on the frame.
    pub fn map(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let [x, y, w] = self.apply(p[0] as f64, p[1] as f64);
        let out = [(x / w) as f32, (y / w) as f32];
        (w > IN_FRONT && out[0].is_finite() && out[1].is_finite()).then_some(out)
    }

    /// `H · (s, t, 1)`, before the division.
    pub fn apply(&self, s: f64, t: f64) -> [f64; 3] {
        let m = &self.0;
        [
            m[0] * s + m[1] * t + m[2],
            m[3] * s + m[4] * t + m[5],
            m[6] * s + m[7] * t + m[8],
        ]
    }

    /// `self`, then `next`: the map `next ∘ self`.
    pub fn then(&self, next: &Homography) -> Homography {
        Homography(mul3(&next.0, &self.0))
    }

    /// The nine numbers as a wall's `fit` stores them.
    pub fn to_fit(&self) -> [f32; 9] {
        self.0.map(|v| v as f32)
    }

    /// A stored `fit` back as a map: `None` for one that is not finite or not
    /// invertible. A hand-written fit scaled by a negative number is the same
    /// map with every `w` flipped, so it is turned the right way round — in
    /// front at the middle of the wall — rather than read as "all behind".
    pub fn from_fit(fit: &[f32; 9]) -> Option<Homography> {
        if fit.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let mut h = Homography(fit.map(f64::from));
        h.inverse()?;
        if h.apply(0.5, 0.5)[2] < 0.0 {
            h.0 = h.0.map(|v| -v);
        }
        Some(h)
    }

    /// How far apart two maps put the same wall, in the output's 0..1: the
    /// largest distance over the four corners and the centre. `None` when
    /// either leaves one of those points behind the projector. What the
    /// "a `View` wins over a `fit`" warning measures.
    pub fn distance(&self, other: &Homography) -> Option<f32> {
        let mut worst = 0.0f32;
        for p in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.5, 0.5]] {
            let (a, b) = (self.map(p)?, other.map(p)?);
            worst = worst.max(((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt());
        }
        Some(worst)
    }
}

fn mul3(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    let mut out = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
        }
    }
    out
}

/// The similarity that moves a point set's centroid to the origin and its
/// mean distance from it to √2 (Hartley's normalisation), or `None` when they
/// all coincide.
fn normalising(points: &[[f32; 2]]) -> Option<[f64; 9]> {
    let n = points.len() as f64;
    let cx = points.iter().map(|p| p[0] as f64).sum::<f64>() / n;
    let cy = points.iter().map(|p| p[1] as f64).sum::<f64>() / n;
    let mean = points
        .iter()
        .map(|p| ((p[0] as f64 - cx).powi(2) + (p[1] as f64 - cy).powi(2)).sqrt())
        .sum::<f64>()
        / n;
    if mean < 1e-9 {
        return None;
    }
    let k = std::f64::consts::SQRT_2 / mean;
    Some([k, 0.0, -k * cx, 0.0, k, -k * cy, 0.0, 0.0, 1.0])
}

/// Gauss-Jordan with partial pivoting over an 8×8 system (the ninth column is
/// the right-hand side). `None` when a pivot vanishes: the correspondences do
/// not pin a map down.
fn solve8(mut a: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-10 {
            return None;
        }
        a.swap(col, pivot);
        for row in 0..8 {
            if row == col {
                continue;
            }
            let f = a[row][col] / a[col][col];
            if f != 0.0 {
                for k in col..9 {
                    a[row][k] -= f * a[col][k];
                }
            }
        }
    }
    let mut x = [0.0; 8];
    for (i, v) in x.iter_mut().enumerate() {
        *v = a[i][8] / a[i][i];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 wall in the `x/y` plane at `z = 0`, with `(0,0)` at its top-left
    /// as seen by a camera looking down `-z` — so `v` points *down*, matching
    /// the `(0,0)`-top-left convention `Mask::points` uses.
    fn front_wall() -> Wall {
        Wall {
            id: "front".into(),
            name: "front".into(),
            origin: [-1.0, 1.0, 0.0],
            u: [2.0, 0.0, 0.0],
            v: [0.0, -2.0, 0.0],
            ..Default::default()
        }
    }

    /// A camera dead in front of `front_wall`, framing it exactly. At a 90°
    /// vertical fov the visible half-height at distance `d` is `d`, so a
    /// 2-unit-tall wall is filled exactly from 1 unit away.
    fn head_on() -> View {
        View {
            output: "proj".into(),
            eye: [0.0, 0.0, 1.0],
            target: [0.0, 0.0, 0.0],
            up: up_y(),
            fov: 90.0,
            aspect: 1.0,
        }
    }

    fn square(id: &str, a: f32, b: f32) -> WallMask {
        WallMask {
            id: id.into(),
            name: id.into(),
            wall: "front".into(),
            points: vec![[a, a], [b, a], [b, b], [a, b]],
            parent: None,
            alpha: 1.0,
        }
    }

    fn scene() -> Escena {
        Escena {
            walls: vec![front_wall()],
            views: vec![head_on()],
            masks: vec![],
        }
    }

    #[test]
    fn a_wall_filling_the_frame_head_on_projects_to_the_points_it_was_drawn_with() {
        // The identity O.5 turns on: with the scene degenerate to one plane
        // filling one frustum, drawing on the wall and drawing on the
        // projector are the same act and must produce the same numbers. If
        // this drifts, every contour authored the 2D way changes shape the
        // day somebody adds a scene.
        let mut e = scene();
        e.masks = vec![square("m", 0.25, 0.75)];
        let out = e.project("proj");
        assert_eq!(out.len(), 1);
        for (got, want) in out[0]
            .points
            .iter()
            .zip(square("m", 0.25, 0.75).points.iter())
        {
            assert!((got[0] - want[0]).abs() < 1e-4, "{got:?} vs {want:?}");
            assert!((got[1] - want[1]).abs() < 1e-4, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn two_outputs_looking_at_one_wall_from_two_angles_get_two_coherent_contours() {
        // The case that makes this 3D rather than a homography per wall: one
        // definition, two views, and neither had to be calibrated against the
        // other.
        let mut e = scene();
        e.views.push(View {
            output: "left".into(),
            // Off to the side, still aimed at the middle of the wall.
            eye: [-1.5, 0.0, 1.5],
            target: [0.0, 0.0, 0.0],
            ..head_on()
        });
        e.masks = vec![square("m", 0.25, 0.75)];

        let head_on_mask = &e.project("proj")[0];
        let angled = &e.project("left")[0];

        // Same contour, same id, same corner count — one shape, two views.
        assert_eq!(head_on_mask.id, angled.id);
        assert_eq!(head_on_mask.points.len(), angled.points.len());

        // Head-on it is a square: both top corners share a y, both left
        // corners share an x.
        let h = &head_on_mask.points;
        assert!(
            (h[0][1] - h[1][1]).abs() < 1e-4,
            "head-on should be square: {h:?}"
        );
        // From the left it is a trapezoid — the far edge is smaller. That is
        // the deformation, and it is what "the wall moves" buys.
        let a = &angled.points;
        let near_edge = (a[3][1] - a[0][1]).abs();
        let far_edge = (a[2][1] - a[1][1]).abs();
        assert!(
            (near_edge - far_edge).abs() > 1e-3,
            "an angled view has to foreshorten, got {near_edge} and {far_edge}"
        );
        // And the nearer edge is the taller one — if this inverts, the camera
        // basis is mirrored and every contour would be deformed backwards.
        assert!(
            near_edge > far_edge,
            "the near edge must be the tall one: {a:?}"
        );
    }

    #[test]
    fn a_contour_behind_the_projector_is_dropped_whole_rather_than_half_projected() {
        let mut e = scene();
        e.walls.push(Wall {
            id: "behind".into(),
            name: "behind".into(),
            origin: [-1.0, 1.0, 3.0],
            u: [2.0, 0.0, 0.0],
            v: [0.0, -2.0, 0.0],
            ..Default::default()
        });
        e.masks = vec![WallMask {
            wall: "behind".into(),
            ..square("m", 0.25, 0.75)
        }];
        assert!(
            e.project("proj").is_empty(),
            "a polygon missing a vertex is a different shape"
        );
    }

    #[test]
    fn a_mask_on_a_wall_no_scene_knows_about_is_skipped_not_guessed() {
        let mut e = scene();
        e.masks = vec![WallMask {
            wall: "deleted".into(),
            ..square("m", 0.25, 0.75)
        }];
        assert!(e.project("proj").is_empty());
    }

    #[test]
    fn an_output_with_no_view_in_the_scene_gets_no_masks_from_it() {
        // And that is how the grid stays the short path: a device that was
        // never placed in the room is untouched by the room existing.
        let mut e = scene();
        e.masks = vec![square("m", 0.25, 0.75)];
        assert!(e.project("other-projector").is_empty());
    }

    #[test]
    fn nesting_and_alpha_ride_through_the_projection_unchanged() {
        // A projected mask is an ordinary `Mask`, so O.1's whole tree works
        // on it with no code here knowing about it.
        let mut e = scene();
        e.masks = vec![
            square("outer", 0.1, 0.9),
            WallMask {
                parent: Some("outer".into()),
                alpha: 0.4,
                ..square("inner", 0.3, 0.7)
            },
        ];
        let out = e.project("proj");
        assert_eq!(out[1].parent.as_deref(), Some("outer"));
        assert_eq!(out[1].alpha, 0.4);
        // And it really is inside its parent once projected, so
        // `Mask::contains` resolves the nesting the way the author drew it.
        let (cx, cy) = (0.5, 0.5);
        assert!(out[1].contains(&out, cx, cy));
        assert!(
            !out[1].contains(&out, 0.15, 0.15),
            "inside the parent, outside the child"
        );
    }

    #[test]
    fn a_body_is_placed_on_the_wall_it_is_in_front_of() {
        let mut e = scene();
        e.walls.push(Wall {
            id: "side".into(),
            name: "side".into(),
            origin: [-1.0, 1.0, 0.0],
            u: [0.0, 0.0, 2.0],
            v: [0.0, -2.0, 0.0],
            ..Default::default()
        });

        // Standing near the middle of the room, a little in front of `front`.
        let (wall, s, t, d) = e
            .nearest_wall([0.0, 0.0, 0.3])
            .expect("in front of something");
        assert_eq!(wall.id, "front");
        assert!(
            (s - 0.5).abs() < 1e-4 && (t - 0.5).abs() < 1e-4,
            "s={s} t={t}"
        );
        assert!((d - 0.3).abs() < 1e-4, "d={d}");

        // Hard against the left wall, and it wins even though `front` is also
        // in range — nearest by distance to the surface.
        let (wall, _, _, _) = e
            .nearest_wall([-0.9, 0.0, 1.0])
            .expect("in front of something");
        assert_eq!(wall.id, "side");
    }

    #[test]
    fn a_point_not_in_front_of_any_wall_places_nowhere_rather_than_on_the_nearest_plane() {
        let e = scene();
        // Off past the wall's right edge: its plane is close, but the person
        // is not in front of the wall.
        assert!(e.nearest_wall([5.0, 0.0, 0.5]).is_none());
    }

    #[test]
    fn a_spotlight_lands_on_the_body_and_moves_because_the_body_moved() {
        let e = scene();
        let left = e
            .spotlight("spot", [-0.5, 0.0, 0.5], (0.2, 0.4))
            .expect("a wall to land on");
        let right = e
            .spotlight("spot", [0.5, 0.0, 0.5], (0.2, 0.4))
            .expect("a wall to land on");

        assert_eq!(left.wall, "front");
        // Centre of the box tracks the body along the wall.
        let cx = |m: &WallMask| (m.points[0][0] + m.points[1][0]) / 2.0;
        assert!((cx(&left) - 0.25).abs() < 1e-4, "{left:?}");
        assert!((cx(&right) - 0.75).abs() < 1e-4, "{right:?}");
        assert!(cx(&right) > cx(&left), "the mask has to follow, not mirror");
    }

    #[test]
    fn a_wall_with_edges_that_are_not_perpendicular_still_solves() {
        // A sloped ceiling or a trapezoid flat. Projecting onto a normal
        // would put the point in the wrong place here and nowhere else, which
        // is the worst place for a bug to live.
        let w = Wall {
            id: "sloped".into(),
            name: "sloped".into(),
            origin: [0.0, 0.0, 0.0],
            u: [2.0, 0.0, 0.0],
            v: [1.0, 2.0, 0.0],
            ..Default::default()
        };
        // `at(0.25, 0.5)` by construction, so `nearest` has to give it back.
        let p = w.at(0.25, 0.5);
        let (s, t, d) = w.nearest(p);
        assert!((s - 0.25).abs() < 1e-4, "s={s}");
        assert!((t - 0.5).abs() < 1e-4, "t={t}");
        assert!(d < 1e-4, "a point on the wall is zero from it, got {d}");
    }

    #[test]
    fn a_degenerate_wall_reports_rather_than_dividing_by_zero() {
        let w = Wall {
            id: "flat".into(),
            name: "none".into(),
            origin: [0.0, 0.0, 0.0],
            u: [1.0, 0.0, 0.0],
            // Parallel to `u`: no interior.
            v: [2.0, 0.0, 0.0],
            ..Default::default()
        };
        let (s, t, d) = w.nearest(Vec3::new(0.0, 1.0, 0.0));
        assert_eq!((s, t), (0.0, 0.0));
        assert!((d - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_scene_round_trips_through_json_with_its_defaults_filled_in() {
        // The fields a hand-written scene may leave out — `up`, `fov`,
        // `aspect`, `alpha` — have to land on something usable, or the first
        // scene anybody writes by hand is a black frame.
        let json = r#"{
            "walls": [{"id":"w","name":"W","origin":[0,0,0],"u":[1,0,0],"v":[0,1,0]}],
            "views": [{"output":"proj","eye":[0,0,1],"target":[0,0,0]}],
            "masks": [{"id":"m","name":"M","wall":"w","points":[[0,0],[1,0],[1,1]]}]
        }"#;
        let e: Escena = serde_json::from_str(json).expect("a hand-written scene parses");
        assert_eq!(e.views[0].fov, 60.0);
        assert_eq!(e.views[0].up, [0.0, 1.0, 0.0]);
        assert_eq!(e.masks[0].alpha, 1.0);
        assert_eq!(e.masks[0].parent, None);
    }

    // -- the warp: `Homography` ----------------------------------------------

    const UNIT: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

    fn close(a: [f32; 2], b: [f32; 2], tol: f32) -> bool {
        (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
    }

    /// Off to the left of `front_wall`, still aimed at its middle: the wall's
    /// left edge is the near one.
    fn angled() -> View {
        View {
            output: "left".into(),
            eye: [-1.5, 0.0, 1.5],
            target: [0.0, 0.0, 0.0],
            ..head_on()
        }
    }

    #[test]
    fn the_identity_maps_every_point_to_itself_and_is_its_own_inverse() {
        let id = Homography::IDENTITY;
        for p in [[0.0, 0.0], [0.25, 0.75], [1.0, 1.0], [-0.5, 2.0]] {
            assert_eq!(id.map(p), Some(p));
        }
        assert_eq!(id.inverse(), Some(Homography::IDENTITY));
        // Four corners onto themselves solve to the identity.
        let fit = Homography::from_corners(&UNIT, &UNIT).expect("the unit square defines a map");
        for p in [[0.1, 0.9], [0.5, 0.5], [0.3, 0.2], [1.4, -0.3]] {
            let got = fit.map(p).expect("in front");
            assert!(close(got, p, 1e-6), "{p:?} went to {got:?}");
        }
    }

    #[test]
    fn from_view_lands_every_point_of_the_wall_where_the_view_projects_it() {
        // Exact on the plane, not fitted to the corners: interior points agree
        // with `View::project` as closely as its own corners do.
        for view in [head_on(), angled()] {
            let wall = front_wall();
            let h = Homography::from_view(&view, &wall);
            for s in [0.0, 0.3, 0.5, 0.77, 1.0] {
                for t in [0.0, 0.25, 0.6, 1.0] {
                    let want = view.project(wall.at(s, t)).expect("in front");
                    let got = h.map([s, t]).expect("in front");
                    assert!(close(got, [want.x, want.y], 1e-5), "({s},{t}): {got:?} vs {want:?}");
                }
            }
        }
    }

    #[test]
    fn from_view_equals_from_corners_for_the_same_wall() {
        // The calibration identity batch 1 turns on: four corners dragged to
        // where the scene's view puts them give back the scene's own map, so
        // a `fit` and a `View` of one wall are two routes to one warp.
        let wall = front_wall();
        let view = angled();
        let from_view = Homography::from_view(&view, &wall);
        let corners = UNIT.map(|[s, t]| {
            let p = view.project(wall.at(s, t)).expect("in front");
            [p.x, p.y]
        });
        let from_corners = Homography::from_corners(&UNIT, &corners).expect("four corners in front");
        for s in [-0.2, 0.0, 0.25, 0.5, 0.9, 1.0, 1.2] {
            for t in [-0.1, 0.0, 0.4, 1.0] {
                let a = from_view.map([s, t]).expect("in front");
                let b = from_corners.map([s, t]).expect("in front");
                assert!(close(a, b, 1e-4), "({s},{t}): {a:?} vs {b:?}");
            }
        }
        // The same nine numbers too, once both are scaled to `h22 = 1`.
        let unit = |h: &Homography| h.0.map(|v| v / h.0[8]);
        for (a, b) in unit(&from_view).iter().zip(unit(&from_corners).iter()) {
            assert!((a - b).abs() < 1e-3 * a.abs().max(1.0), "{a} vs {b}");
        }
        assert!(from_view.distance(&from_corners).expect("both in front") < 1e-4);
    }

    #[test]
    fn the_inverse_takes_a_projected_point_back_to_where_it_was_on_the_wall() {
        let h = Homography::from_view(&angled(), &front_wall());
        let back = h.inverse().expect("a wall seen from the side is not edge-on");
        for p in [[0.1, 0.2], [0.5, 0.5], [0.95, 0.8]] {
            let there = h.map(p).expect("in front");
            let again = back.map(there).expect("the image of a point in front is in front");
            assert!(close(again, p, 1e-5), "{p:?} came back as {again:?}");
        }
        // And composing the two is the identity.
        let round = h.then(&back);
        assert!(round.distance(&Homography::IDENTITY).unwrap() < 1e-5);
    }

    #[test]
    fn a_single_fit_yields_a_view() {
        // P2.4: the inverse of `from_view`. A `fit` is four dragged corners;
        // from them and the projector's own intrinsics the pose that would
        // have put them there comes back — so a wall that only has a fit can
        // still project a named contour instead of falling back to the whole
        // wall.
        let close3 = |a: [f32; 3], b: [f32; 3]| {
            (a[0] - b[0]).abs() < 2e-3 && (a[1] - b[1]).abs() < 2e-3 && (a[2] - b[2]).abs() < 2e-3
        };
        for view in [head_on(), angled()] {
            let wall = front_wall();
            let h = Homography::from_view(&view, &wall);
            let back = h.to_view(&wall, view.fov, view.aspect).expect("a view is a pose");
            assert!(close3(back.eye, view.eye), "{:?} vs {:?}", back.eye, view.eye);
            assert!(close3(back.up, view.up), "{:?} vs {:?}", back.up, view.up);
            let dir = |v: &View| {
                let t = Vec3::from(v.target) - Vec3::from(v.eye);
                (t / t.length()).to_array()
            };
            assert!(close3(dir(&back), dir(&view)), "forward {:?} vs {:?}", dir(&back), dir(&view));
            // And the recovered pose reproduces the map it came from.
            let again = Homography::from_view(&back, &wall);
            assert!(again.distance(&h).expect("both in front") < 1e-4);
        }
        // The same through an actual four-corner fit, which is the case the
        // engine meets: a homography from `from_corners`, not `from_view`.
        let view = angled();
        let wall = front_wall();
        let corners = UNIT.map(|[s, t]| {
            let p = view.project(wall.at(s, t)).expect("in front");
            [p.x, p.y]
        });
        let fit = Homography::from_corners(&UNIT, &corners).expect("four corners in front");
        let back = fit.to_view(&wall, view.fov, view.aspect).expect("a fit is a pose");
        assert!(close3(back.eye, view.eye), "{:?} vs {:?}", back.eye, view.eye);
    }

    #[test]
    fn several_coplanar_correspondences_solve_one_homography() {
        // The least-squares half of `from_corners`: two quads of a coplanar
        // surface (eight pairs, not four) still give one map, and exactly the
        // one four of them would have.
        let a = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let b = [[2.0f32, 0.0], [3.0, 0.0], [3.0, 1.0], [2.0, 1.0]];
        let h = Homography::from_view(&angled(), &front_wall());
        let mut st = Vec::new();
        let mut xy = Vec::new();
        for q in [&a, &b] {
            for p in q {
                st.push(*p);
                xy.push(h.map(*p).expect("in front"));
            }
        }
        let ls = Homography::from_correspondences(&st, &xy).expect("eight pairs define a map");
        assert!(ls.distance(&h).expect("both in front") < 1e-3);
        // Too few points is refused rather than guessed.
        assert!(Homography::from_correspondences(&st[..3], &xy[..3]).is_none());
    }

    #[test]
    fn a_point_behind_the_projector_does_not_map() {
        let behind = Wall {
            id: "behind".into(),
            name: "behind".into(),
            origin: [-1.0, 1.0, 3.0],
            u: [2.0, 0.0, 0.0],
            v: [0.0, -2.0, 0.0],
            ..Default::default()
        };
        let h = Homography::from_view(&head_on(), &behind);
        assert_eq!(h.map([0.5, 0.5]), None, "a point behind the eye is not a place on the frame");
    }

    #[test]
    fn four_corners_that_are_not_a_calibration_are_refused() {
        // Three on one line: no map sends a square there.
        let line = [[0.1, 0.1], [0.5, 0.5], [0.9, 0.9], [0.1, 0.9]];
        assert!(Homography::from_corners(&UNIT, &line).is_none());
        // Two corners crossed: the bow tie folds the wall through infinity.
        let crossed = [[0.1, 0.1], [0.9, 0.1], [0.1, 0.9], [0.9, 0.9]];
        assert!(Homography::from_corners(&UNIT, &crossed).is_none());
        // All four in one place.
        assert!(Homography::from_corners(&UNIT, &[[0.5, 0.5]; 4]).is_none());
        // Not a number.
        let nan = [[f32::NAN, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(Homography::from_corners(&UNIT, &nan).is_none());
        // A keystoned quad, on the other hand, is exactly what a fit is.
        let keystone = [[0.2, 0.1], [0.8, 0.15], [0.9, 0.9], [0.1, 0.85]];
        let h = Homography::from_corners(&UNIT, &keystone).expect("a convex quad is a calibration");
        for (p, want) in UNIT.iter().zip(keystone.iter()) {
            assert!(close(h.map(*p).unwrap(), *want, 1e-5));
        }
    }

    #[test]
    fn a_stored_fit_round_trips_and_a_negated_one_reads_the_same_way_round() {
        let keystone = [[0.2, 0.1], [0.8, 0.15], [0.9, 0.9], [0.1, 0.85]];
        let h = Homography::from_corners(&UNIT, &keystone).unwrap();
        let again = Homography::from_fit(&h.to_fit()).expect("a saved fit reads back");
        assert!(again.distance(&h).unwrap() < 1e-5);
        let flipped = h.to_fit().map(|v| -v);
        let read = Homography::from_fit(&flipped).expect("the same map, scaled by -1");
        assert!(read.distance(&h).unwrap() < 1e-5, "a negated fit is the same map, not all-behind");
        assert!(Homography::from_fit(&[0.0; 9]).is_none(), "a singular fit is not a map");
        assert!(Homography::from_fit(&[f32::INFINITY, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]).is_none());
    }

    // -- `Escena::homography`: view wins, fit falls back -----------------------

    /// A fit that is deliberately not the view's own map: a keystone quad.
    fn keystone_fit(output: &str) -> Wall {
        let mut wall = front_wall();
        let keystone = [[0.2, 0.1], [0.8, 0.15], [0.9, 0.9], [0.1, 0.85]];
        let h = Homography::from_corners(&UNIT, &keystone).expect("a convex quad");
        wall.fit.insert(output.into(), h.to_fit());
        wall
    }

    #[test]
    fn with_no_view_the_walls_fit_is_the_warp() {
        let mut e = scene();
        e.views.clear();
        let wall = keystone_fit("proj");
        let h = e.homography("proj", &wall).expect("the fit is a map");
        let fit = Homography::from_fit(wall.fit.get("proj").unwrap()).unwrap();
        assert!(h.distance(&fit).unwrap() < 1e-5, "the fit is used when no view exists");
    }

    #[test]
    fn a_view_wins_over_a_fit_that_disagrees_with_it() {
        let e = scene();
        let wall = keystone_fit("proj");
        let h = e.homography("proj", &wall).expect("the view is a map");
        let view = Homography::from_view(&e.view("proj").unwrap(), &wall);
        let fit = Homography::from_fit(wall.fit.get("proj").unwrap()).unwrap();
        assert!(h.distance(&view).unwrap() < 1e-5, "the view is used");
        // And the disagreement the warning measures is real and wide.
        let gap = view.distance(&fit).expect("both in front");
        assert!(gap > WARN_FIT_DISTANCE, "the two should visibly differ, got {gap}");
    }

    #[test]
    fn a_scene_with_neither_a_view_nor_a_fit_has_no_warp_for_a_wall() {
        let mut e = scene();
        e.views.clear();
        assert!(e.homography("proj", &front_wall()).is_none());
        // A fit on a *different* output is not a warp for this one.
        let wall = keystone_fit("left");
        assert!(e.homography("proj", &wall).is_none());
    }

    #[test]
    fn a_saved_scene_loads_back_with_its_fits_and_carries_them_to_homography() {
        // Rule 8: a per-process scratch, never the real `data/`.
        let dir = std::env::temp_dir().join(format!("fw-scene-save-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).expect("scratch dir");
        let mut e = Escena {
            walls: vec![keystone_fit("proj")],
            ..Default::default()
        };
        e.views.clear();
        e.save(&dir).expect("save");
        let back = load(&dir);
        assert_eq!(back, e, "the scene round-trips whole");
        let h = back.homography("proj", &back.walls[0]).expect("the fit came back");
        let fit = Homography::from_fit(back.walls[0].fit.get("proj").unwrap()).unwrap();
        assert!(h.distance(&fit).unwrap() < 1e-5);
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}
