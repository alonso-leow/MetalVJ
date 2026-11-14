//! The compositor — several rendered layers into one frame.
//!
//! Runs on the CPU, on the request thread, between the renderers and
//! [`crate::output::apply_masks`]. That order is the whole architecture in one
//! line: **layers make a picture, contours cut it.** An operator contours the
//! pillar once and every stack they ever put on that device lands on the
//! pillar.
//!
//! # Why the CPU
//!
//! It could be a wgpu pass. It is not, for three reasons that all point the
//! same way:
//!
//! 1. `apply_masks` is already here, in the same buffer, immediately after.
//!    Going to the GPU to blend and coming back to mask would be two readbacks
//!    a frame instead of one.
//! 2. Blending is memory-bound, not maths-bound. At 1920x1080 with four layers
//!    this is a few million multiply-adds over a buffer that is already in
//!    cache because the renderer just wrote it.
//! 3. It runs where there is no adapter. The whole point of `crate::stand` is
//!    that the app can be operated and reviewed on a machine with no GPU, and a
//!    compositor in a shader would mean the one genuinely new subsystem is the
//!    one part that cannot be tested there.
//!
//! # Sampling, not scaling
//!
//! A layer is rendered at its sector's own pixel size and written straight in,
//! one-to-one — no resampling in the normal case, because asking the renderer
//! for the size you want is free and resizing afterwards is not. The nearest
//! sampler below is only reached when the pixel budget has shrunk a layer
//! ([`crate::layers::Limits::scale_for`]), which is the degraded path and looks
//! like it.

use crate::escena::Homography;
use crate::layers::{Blend, ClipMode, Composition, Layer, Sector};
use crate::output::{Mask, Scanline};

/// How a warped tile's texels are read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sampling {
    /// The four texels around the point, weighted by where it falls between
    /// them, clamped at the tile's edges. The default: a warp resamples
    /// everywhere (unlike a budget-shrunk sector, it is never one-to-one), and
    /// nearest-neighbour there crawls as the wall's stripes cross pixel rows.
    #[default]
    Bilinear,
    /// The one texel under the point — the fallback if bilinear is ever too
    /// slow for a frame (the bench below measures both). Never a shader: the
    /// warp stays on the CPU with the cut it sits inside.
    Nearest,
}

/// A tile drawn **through a homography** instead of into its sector: the 3D
/// mask's content warp (plan: "3D mask with perspective correction").
///
/// `homography` takes the tile's own `(u, v)` — 0..1 over its pixels,
/// `(0, 0)` top left — to the device's 0..1, the same space [`Mask::points`]
/// lives in. For a source bound to a wall that is the wall's `(s, t)` box of
/// the contour, then the output's `H` for that wall (`escena::WallDraw`).
///
/// With a warp the tile's `sector` is only **where to look** — the rectangle
/// of device pixels walked, normally the projected contour's bounding box —
/// and its `clip` is the projected contour itself, cut exactly like a 2D one.
/// `giro` does not apply: a wall-bound source turns by turning the wall.
///
/// Carried beside the tiles (`compose_warped`) rather than as a field of
/// [`Tile`]: `overlay.rs` builds tiles with a struct literal, and a field
/// here would be a change to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    pub homography: Homography,
    pub sampling: Sampling,
}

impl Warp {
    /// A bilinear warp through `homography` — what the render path uses.
    pub fn new(homography: Homography) -> Warp {
        Warp {
            homography,
            sampling: Sampling::Bilinear,
        }
    }
}

/// One triangle of a [`WarpMesh`]. `d` is device `0..1` (`(0, 0)` top left),
/// `uv` is the surface's own `(s, t)` — the same coordinates the tile is
/// sampled in, so the pair is the whole of the mapping for that triangle.
#[derive(Clone, Debug, PartialEq)]
pub struct WarpTri {
    pub d: [[f32; 2]; 3],
    pub uv: [[f32; 2]; 3],
}

/// A mesh warp: the surface tessellated in device space, with each triangle
/// carrying the `(s, t)` its vertices came from. Used for non-planar surfaces,
/// whose projection is not a homography. A planar surface keeps the exact
/// [`Warp`] path, byte for byte.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WarpMesh {
    pub tris: Vec<WarpTri>,
}

impl WarpMesh {
    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// The bounding box of every triangle vertex in device `0..1`:
    /// `[x, y, w, h]`. `None` for a mesh with no triangles.
    pub fn bbox(&self) -> Option<[f32; 4]> {
        let mut lo = [f32::MAX; 2];
        let mut hi = [f32::MIN; 2];
        let mut any = false;
        for tri in &self.tris {
            for p in &tri.d {
                any = true;
                lo[0] = lo[0].min(p[0]);
                lo[1] = lo[1].min(p[1]);
                hi[0] = hi[0].max(p[0]);
                hi[1] = hi[1].max(p[1]);
            }
        }
        any.then(|| [lo[0], lo[1], hi[0] - lo[0], hi[1] - lo[1]])
    }

    /// Device `0..1` -> source `(s, t)`; `None` when outside every triangle.
    /// This is the simple linear scan, used by callers with one point in hand;
    /// the compositor builds a grid once per mesh instead (see `MeshLookup`).
    pub fn source_at(&self, d: [f32; 2]) -> Option<[f32; 2]> {
        self.tris.iter().find_map(|tri| {
            let b = barycentric(d, &tri.d)?;
            Some([
                b[0] * tri.uv[0][0] + b[1] * tri.uv[1][0] + b[2] * tri.uv[2][0],
                b[0] * tri.uv[0][1] + b[1] * tri.uv[1][1] + b[2] * tri.uv[2][1],
            ])
        })
    }
}

/// Barycentric weights of `p` in triangle `t`, or `None` outside it (a small
/// tolerance keeps edge pixels, which otherwise flicker along shared edges).
#[inline]
fn barycentric(p: [f32; 2], t: &[[f32; 2]; 3]) -> Option<[f32; 3]> {
    let (x, y) = (p[0], p[1]);
    let (x0, y0) = (t[0][0], t[0][1]);
    let (x1, y1) = (t[1][0], t[1][1]);
    let (x2, y2) = (t[2][0], t[2][1]);
    let den = (y1 - y2) * (x0 - x2) + (x2 - x1) * (y0 - y2);
    if den.abs() < 1e-12 || !den.is_finite() {
        return None;
    }
    let a = ((y1 - y2) * (x - x2) + (x2 - x1) * (y - y2)) / den;
    let b = ((y2 - y0) * (x - x2) + (x0 - x2) * (y - y2)) / den;
    let c = 1.0 - a - b;
    let eps = -1e-5;
    (a >= eps && b >= eps && c >= eps).then_some([a, b, c])
}

/// A [`WarpMesh`] as the per-pixel loop wants it: a fixed device grid of
/// triangle candidates, built once per tile per frame. A few dozen triangles
/// per mesh is normal; without the grid the 24 x 12 case is already a million
/// point-in-triangle tests a frame, and a finer display mesh would be worse.
/// The grid is deliberately small and bounds-checked rather than exact — a
/// candidate is still tested barycentrically, so a triangle listed in a cell
/// it barely touches can never produce a false hit.
struct MeshLookup {
    cells: Vec<Vec<u32>>,
}

const MESH_GRID: usize = 16;

impl MeshLookup {
    fn new(mesh: &WarpMesh) -> MeshLookup {
        let mut cells = vec![Vec::new(); MESH_GRID * MESH_GRID];
        for (i, tri) in mesh.tris.iter().enumerate() {
            let mut lo = [f32::MAX; 2];
            let mut hi = [f32::MIN; 2];
            for p in &tri.d {
                lo[0] = lo[0].min(p[0]);
                lo[1] = lo[1].min(p[1]);
                hi[0] = hi[0].max(p[0]);
                hi[1] = hi[1].max(p[1]);
            }
            let cell = |v: f32, n: usize| {
                ((v * n as f32).floor().max(0.0) as usize).min(n - 1)
            };
            let (x0, x1) = (cell(lo[0], MESH_GRID), cell(hi[0], MESH_GRID));
            let (y0, y1) = (cell(lo[1], MESH_GRID), cell(hi[1], MESH_GRID));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    cells[y * MESH_GRID + x].push(i as u32);
                }
            }
        }
        MeshLookup { cells }
    }

    fn source_at(&self, mesh: &WarpMesh, d: [f32; 2]) -> Option<[f32; 2]> {
        let cell = |v: f32| {
            ((v * MESH_GRID as f32).floor().max(0.0) as usize).min(MESH_GRID - 1)
        };
        let idx = cell(d[1]) * MESH_GRID + cell(d[0]);
        for &i in &self.cells[idx] {
            let tri = &mesh.tris[i as usize];
            if let Some(b) = barycentric(d, &tri.d) {
                return Some([
                    b[0] * tri.uv[0][0] + b[1] * tri.uv[1][0] + b[2] * tri.uv[2][0],
                    b[0] * tri.uv[0][1] + b[1] * tri.uv[1][1] + b[2] * tri.uv[2][1],
                ]);
            }
        }
        None
    }
}

/// The inverse map of `warp` in pixel units, row-major `3 x 3`: a device pixel
/// `(x, y, 1)` to a continuous texel coordinate of a `tw x th` tile (before the
/// projective divide). `None` for a warp that cannot be inverted. The CPU
/// [`Sampler`] and the GPU compose shader read the same numbers.
pub fn warp_matrix(warp: &Warp, tw: u32, th: u32, width: u32, height: u32) -> Option<[f32; 9]> {
    let inv = warp.homography.inverse()?;
    // Device pixels → device 0..1 (÷ size), `H⁻¹`, tile 0..1 → texels
    // (× size), composed entry by entry, multiplying before dividing: an
    // identity warp over a tile the device's own size then comes out as
    // the identity *exactly*, which is what keeps it byte-identical to no
    // warp at all.
    let dev = [width as f64, height as f64, 1.0];
    let tex = [tw as f64, th as f64, 1.0];
    let mut m = [0.0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            m[r * 3 + c] = (inv.0[r * 3 + c] * tex[r] / dev[c]) as f32;
        }
    }
    Some(m)
}

/// A [`Warp`] as the per-pixel loop wants it, built once per tile per frame:
/// the inverse map in **pixel units** — a device pixel centre to a continuous
/// texel coordinate, texel centres at `+0.5` — in `f32`.
struct Sampler {
    m: [f32; 9],
    tw: i32,
    th: i32,
    sampling: Sampling,
}

impl Sampler {
    /// `None` for a warp that cannot be inverted or a tile with no pixels: a
    /// degenerate warp has nowhere to read from, so the tile draws nothing.
    fn new(warp: &Warp, tile: &Tile, width: u32, height: u32) -> Option<Sampler> {
        if tile.w == 0 || tile.h == 0 || tile.pixels.len() < (tile.w as usize) * (tile.h as usize) * 4 {
            return None;
        }
        let m = warp_matrix(warp, tile.w, tile.h, width, height)?;
        Some(Sampler {
            m,
            tw: tile.w as i32,
            th: tile.h as i32,
            sampling: warp.sampling,
        })
    }

    /// Device pixel `(dx, dy)` to this tile's continuous texel coordinate via
    /// the inverse map, or `None` when the projective `w` vanishes (a device
    /// pixel that folds through the wall). Whole number at texel centres.
    #[inline]
    fn at(&self, dx: u32, dy: u32) -> Option<(f32, f32)> {
        let (x, y) = (dx as f32, dy as f32);
        let m = &self.m;
        let w = m[6] * x + m[7] * y + m[8];
        if w.abs() < 1e-9 || !w.is_finite() {
            return None;
        }
        Some(((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w))
    }

    /// The tile's colour at continuous texel coordinate `(tx, ty)` (texel
    /// centres on the integers), each channel `0.0..=255.0`.
    ///
    /// Bilinear is `p00 + (p10 − p00)·ax` along each row and the same between
    /// the two rows, so a point exactly on a texel centre returns that
    /// texel's bytes unchanged, not a rounding of them.
    #[inline]
    fn sample(&self, px: &[u8], tx: f32, ty: f32) -> [f32; 4] {
        sample_texel(px, self.tw, self.th, self.sampling, tx, ty)
    }
}

/// Read one tile at continuous texel coordinate `(tx, ty)` (texel centres on
/// the integers). Shared by the homography [`Sampler`] and the mesh one so
/// both paths blend alpha and interpolate identically.
#[inline]
fn sample_texel(px: &[u8], tw: i32, th: i32, sampling: Sampling, tx: f32, ty: f32) -> [f32; 4] {
    let at = |x: i32, y: i32| ((y * tw + x) * 4) as usize;
    match sampling {
        Sampling::Nearest => {
            let x = ((tx + 0.5).floor() as i32).clamp(0, tw - 1);
            let y = ((ty + 0.5).floor() as i32).clamp(0, th - 1);
            let i = at(x, y);
            [px[i] as f32, px[i + 1] as f32, px[i + 2] as f32, px[i + 3] as f32]
        }
        Sampling::Bilinear => {
            let (fx, fy) = (tx.floor(), ty.floor());
            let (ax, ay) = (tx - fx, ty - fy);
            let (xi, yi) = (fx as i32, fy as i32);
            let (x0, x1) = (xi.clamp(0, tw - 1), xi.saturating_add(1).clamp(0, tw - 1));
            let (y0, y1) = (yi.clamp(0, th - 1), yi.saturating_add(1).clamp(0, th - 1));
            let (i00, i10, i01, i11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
            let mut out = [0.0f32; 4];
            for (k, v) in out.iter_mut().enumerate() {
                let top = px[i00 + k] as f32 + (px[i10 + k] as f32 - px[i00 + k] as f32) * ax;
                let bottom = px[i01 + k] as f32 + (px[i11 + k] as f32 - px[i01 + k] as f32) * ax;
                *v = top + (bottom - top) * ay;
            }
            out
        }
    }
}

/// A [`WarpMesh`] as the per-pixel loop wants it. Mesh warps are CPU-only for
/// now, so this is always bilinear (the mesh's triangles are the resampling).
struct MeshSampler<'a> {
    mesh: &'a WarpMesh,
    lookup: MeshLookup,
    tw: i32,
    th: i32,
}

impl<'a> MeshSampler<'a> {
    fn new(mesh: &'a WarpMesh, tile: &Tile) -> Option<MeshSampler<'a>> {
        if mesh.is_empty()
            || tile.w == 0
            || tile.h == 0
            || tile.pixels.len() < (tile.w as usize) * (tile.h as usize) * 4
        {
            return None;
        }
        Some(MeshSampler {
            mesh,
            lookup: MeshLookup::new(mesh),
            tw: tile.w as i32,
            th: tile.h as i32,
        })
    }

    /// Device pixel `(dx, dy)` to this tile's colour; `None` outside the mesh.
    #[inline]
    fn sample(&self, px: &[u8], dx: u32, dy: u32, width: u32, height: u32) -> Option<[f32; 4]> {
        let d = [dx as f32 / width as f32, dy as f32 / height as f32];
        let uv = self.lookup.source_at(self.mesh, d)?;
        Some(sample_texel(
            px,
            self.tw,
            self.th,
            Sampling::Bilinear,
            uv[0] * self.tw as f32,
            uv[1] * self.th as f32,
        ))
    }
}

/// One rendered layer, waiting to be composited.
pub struct Tile<'a> {
    /// RGBA8, `w * h * 4` bytes.
    pub pixels: &'a [u8],
    pub w: u32,
    pub h: u32,
    /// Where on the device it goes, and how it combines.
    pub sector: Sector,
    pub opacity: f32,
    pub blend: Blend,
    /// Rotation applied by [`source_at`] around the sector's own centre,
    /// in radians — [`Layer::giro`], the design notes PARTE IV §17.2. Zero, the
    /// default, takes the fast unrotated path below and costs nothing.
    pub turn: f32,
    /// What happens at the sector's edges once `giro` is non-zero.
    /// Meaningless at `giro == 0.0`, same as the field it is copied from.
    pub giro_mode: ClipMode,
    /// The contour this source is cut to, if it is bound to one.
    ///
    /// **This is not the device-wide cut.** That still happens afterwards, in
    /// `crate::output::apply_masks`, over the finished frame. This one runs
    /// per source and is what makes *"different sources on one same
    /// device"* possible: two sources bound to two contours on the same
    /// projector light two real objects with two animations.
    ///
    /// `sector` is expected to be the contour's own
    /// [`bounds_in`](Mask::bounds_in) when this is set — the caller does that,
    /// so the renderer is asked for the pixels the polygon can actually use
    /// rather than for a full frame that is then thrown away.
    pub clip: Option<&'a Mask>,
    /// The rest of the device's masks, as the context `clip` resolves
    /// against.
    ///
    /// A contour is no longer self-contained: since H.5b/H.10 it may name a
    /// `parent` it is clipped to, or be the `union`/`intersect`/`subtract` of
    /// two others. Neither can be evaluated from the mask alone, so the list
    /// travels with it. Empty is the honest value for a caller that has no
    /// device (the tests below, mostly) — a `clip` with no `parent` and no
    /// `compose` resolves the same either way.
    pub masks: &'a [Mask],
}

impl<'a> Tile<'a> {
    pub fn of(
        layer: &Layer,
        pixels: &'a [u8],
        w: u32,
        h: u32,
        sector: Sector,
        clip: Option<&'a Mask>,
        masks: &'a [Mask],
    ) -> Tile<'a> {
        Tile {
            pixels,
            w,
            h,
            sector,
            opacity: layer.opacity.clamp(0.0, 1.0),
            blend: layer.blend,
            turn: layer.turn,
            giro_mode: layer.giro_mode,
            // An unfinished contour is not a shape yet, and clipping to it
            // would black out a source while its points are still being placed
            // — the same call `Mask::closed` makes everywhere else.
            clip: clip.filter(|m| m.closed()),
            masks,
        }
    }
}

/// Composites `tiles` into a black **opaque** frame of `width x height` and
/// returns it — the one path a device's finished frame takes, unchanged by
/// B2: this ends in a JPEG on a projector, and a projector's "nothing" is
/// black.
///
/// `tiles` must already be in draw order, back to front — that is
/// [`crate::layers::layers_on`]'s job, and doing it here as well would
/// be two places that decide what is in front.
///
/// This is now [`compose_rgba`] with an opaque black background — see that
/// function for why the split exists (`the design notes` §10 Conjunto
/// B2). It is what makes an empty stack correct rather than undefined — a
/// device with every layer bypassed shows black, which is what bypassing
/// every layer means — and it is pixel-for-pixel what this function produced
/// before B2, since a background composited under fully-opaque tiles is
/// indistinguishable from initializing the buffer to that colour.
pub fn compose(width: u32, height: u32, mode: Composition, tiles: &[Tile]) -> Vec<u8> {
    compose_rgba(width, height, mode, tiles, Some([0, 0, 0, 255]))
}

/// Composites `tiles` into a `width x height` buffer, real alpha and all.
///
/// **`background = None` is the intermediate case** — `the design notes`
/// §10 Conjunto B2: a node in the graph whose output feeds another node
/// (an `fx.*`, a future `mask`) rather than a device's last pass starts
/// transparent, so two masked, overlapping sources composited into one of
/// these can be blended again downstream without a black seam where nothing
/// was drawn yet. **`background = Some(rgba)`** is what [`compose`] uses: the
/// device's own final pass, which is the only place in this pipeline that is
/// allowed to decide what "nothing here" looks like on a screen.
///
/// A tile's own alpha (`tile.pixels[..][3]`) now participates: a
/// less-than-opaque source tile is blended by its own coverage as well as
/// its layer opacity, rather than assumed fully opaque. Every renderer this
/// repo has today (`Paint::solo`, both painters) still writes 255 into every
/// pixel it draws, so for any real frame this is arithmetically identical to
/// the alpha-255 assumption it replaces — the difference only shows up once
/// something upstream (a `mask` node, Conjunto H) starts leaving pixels
/// transparent on purpose.
pub fn compose_rgba(
    width: u32,
    height: u32,
    mode: Composition,
    tiles: &[Tile],
    background: Option<[u8; 4]>,
) -> Vec<u8> {
    compose_rgba_warped(width, height, mode, tiles, &[], background)
}

/// [`compose`], with some tiles drawn through a [`Warp`]: `warps[i]` belongs
/// to `tiles[i]`, and a `None` or missing entry is an ordinary tile — so an
/// empty `warps` is [`compose`] exactly, byte for byte.
pub fn compose_warped(
    width: u32,
    height: u32,
    mode: Composition,
    tiles: &[Tile],
    warps: &[Option<Warp>],
) -> Vec<u8> {
    compose_rgba_warped(width, height, mode, tiles, warps, Some([0, 0, 0, 255]))
}

/// [`compose_rgba`] with [`Warp`]s — see [`compose_warped`]. A warped tile
/// follows the mode like any other: blended in overlap, written over what is
/// under it in sectors, and in both it skips every pixel outside its contour
/// rather than blacking it, exactly as a 2D-clipped tile does.
pub fn compose_rgba_warped(
    width: u32,
    height: u32,
    mode: Composition,
    tiles: &[Tile],
    warps: &[Option<Warp>],
    background: Option<[u8; 4]>,
) -> Vec<u8> {
    compose_rgba_warped_impl(width, height, mode, tiles, warps, None, background)
}

/// [`compose_rgba_warped`] with mesh warps beside the homography ones:
/// `meshes[i]` belongs to `tiles[i]` and, when present, takes precedence over
/// `warps[i]`. With every mesh `None` (or `meshes` shorter/empty) this is
/// byte-for-byte [`compose_rgba_warped`] — it delegates straight to it.
///
/// A mesh triangle carries the device point and the source `(s, t)` of each
/// vertex, so a pixel is mapped by barycentric interpolation rather than a
/// homography. Pixels outside every triangle write nothing, exactly like a
/// clipped or un-invertible homography pixel. Alpha and mode handling are the
/// same loops as the homography path.
pub fn compose_rgba_warped_mesh(
    width: u32,
    height: u32,
    mode: Composition,
    tiles: &[Tile],
    warps: &[Option<Warp>],
    meshes: &[Option<WarpMesh>],
    background: Option<[u8; 4]>,
) -> Vec<u8> {
    if meshes.iter().all(|m| m.is_none()) {
        return compose_rgba_warped(width, height, mode, tiles, warps, background);
    }
    compose_rgba_warped_impl(width, height, mode, tiles, warps, Some(meshes), background)
}

fn compose_rgba_warped_impl(
    width: u32,
    height: u32,
    mode: Composition,
    tiles: &[Tile],
    warps: &[Option<Warp>],
    meshes: Option<&[Option<WarpMesh>]>,
    background: Option<[u8; 4]>,
) -> Vec<u8> {
    let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
    if let Some(bg) = background {
        for p in out.chunks_exact_mut(4) {
            p.copy_from_slice(&bg);
        }
    }
    // `background = None` leaves `out` at all-zero — RGBA (0,0,0,0),
    // genuinely transparent, not "black that happens to encode the same as
    // transparent" the way the old opaque-black initialization was.

    // Only the intermediate case (`background = None`) writes a composited
    // alpha channel back out. The device's own final pass (`background =
    // Some`) leaves it exactly as the background set it — untouched, the
    // same as every build of this function before B2 — so that path is
    // bit-for-bit what it always was, never merely "close".
    let write_alpha = background.is_none();

    for (i, tile) in tiles.iter().enumerate() {
        // One `Sampler` per tile per frame: the inverse homography is built
        // here and read a pixel at a time, never rebuilt inside the loop. A
        // `None` warp (or a missing entry) is the un-warped path, byte for
        // byte the same as it always was.
        let sampler = warps
            .get(i)
            .and_then(|w| w.as_ref())
            .and_then(|w| Sampler::new(w, tile, width, height));
        // A mesh is built and indexed once per tile per frame, and wins over
        // the homography for that tile.
        let mesh = meshes
            .and_then(|ms| ms.get(i))
            .and_then(|m| m.as_ref())
            .and_then(|m| MeshSampler::new(m, tile));
        match mode {
            Composition::Overlap => blend_tile(&mut out, width, height, tile, sampler.as_ref(), mesh.as_ref(), write_alpha),
            // In sectors, each layer *replaces* its rectangle. Walking the list
            // in draw order means the front-most layer over a given pixel is
            // the last to write it, so a pixel ends up belonging to exactly one
            // layer — *they do not cross* — even when two rectangles cross,
            // which they will while somebody is dragging one into place.
            Composition::Sectors => replace_tile(&mut out, width, height, tile, sampler.as_ref(), mesh.as_ref(), write_alpha),
        }
    }
    out
}

/// `S.O`: composites `tiles` **onto a frame that already exists**, source-over,
/// back to front — the overlay half of the compositor, as opposed to
/// [`compose`]/[`compose_rgba`], which build a frame from nothing.
///
/// This is what an overlay that is a *picture* rather than a *row* — a solid
/// colour for N seconds (`S.O.2`), a rasterised buffer — goes through:
/// [`crate::overlay::Overlays::paint`] builds one [`Tile`] per active overlay
/// and hands them here, after [`crate::engine::render_device`] has finished
/// the device's own layers and before `crate::output::apply_masks` cuts the
/// result to the projector's contours. Always [`Composition::Overlap`]
/// semantics whatever the device's own composition is: an overlay sits *on
/// top of* the finished frame, it does not own a rectangle of it. The frame's
/// alpha is left alone (`write_alpha = false`) for the same reason `compose`
/// leaves it: this is the device's final pass.
pub fn over(out: &mut [u8], width: u32, height: u32, tiles: &[Tile]) {
    for tile in tiles {
        blend_tile(out, width, height, tile, None, None, false);
    }
}

/// Where in `tile` to read for device pixel `(dx, dy)`, or `None` if that
/// pixel is outside the tile's sector — including, now, a pixel a [`giro`]
/// turns past the sector's own edge in [`ClipMode::Crop`].
///
/// [`giro`]: Layer::giro
///
/// Nearest-neighbour on purpose. This is only ever off one-to-one when the
/// pixel budget has shrunk the layer, and at that point the honest thing is a
/// visibly coarser picture rather than a soft one that looks like the projector
/// is out of focus.
///
/// # `giro` — the design notes PARTE IV §17.2
///
/// `tile.giro == 0.0` — every tile before this existed, and almost every one
/// after — takes the exact path above, unchanged down to the byte: no
/// `sin`/`cos`, no float rounding, nothing new to be wrong. A non-zero `giro`
/// composes one more mapping in *front* of it: instead of reading `(dx, dy)`
/// directly, walk backwards by `-giro` around the sector's own centre first,
/// and read whatever that lands on instead. Turning the read rather than the
/// write is what keeps [`Sector`] an axis-aligned rectangle — the trap
/// the design notes PARTE IV §17.2 names by name — because nothing about the sector's
/// own geometry, or `Sector::pixels`/`overlaps`/`is_full`, ever has to know a
/// rotation happened.
///
/// `ClipMode::Fit` scales that backward step by [`fit_scale`] first, so a
/// pixel at the sector's own corner — the one `Crop` leaves empty at 45° —
/// still lands inside the tile, at the cost of never reading the tile's own
/// corners.
#[inline]
fn source_at(tile: &Tile, x0: u32, y0: u32, sw: u32, sh: u32, dx: u32, dy: u32) -> Option<usize> {
    let (dx, dy) = if tile.turn == 0.0 {
        (dx, dy)
    } else {
        // The point `giro` turns around — a float centre, not rounded to a
        // pixel, so the mapping is exactly self-inverse rather than off by
        // half a pixel on odd-sized sectors.
        let cx = x0 as f32 + sw as f32 / 2.0;
        let cy = y0 as f32 + sh as f32 / 2.0;
        let px_rel = dx as f32 - cx;
        let py_rel = dy as f32 - cy;
        let (sin_g, cos_g) = tile.turn.sin_cos();
        // Inverse rotation: reading a tile that was turned by `giro` means
        // walking the destination backwards by `-giro` to find which source
        // pixel landed there.
        let mut rx = cos_g * px_rel + sin_g * py_rel;
        let mut ry = -sin_g * px_rel + cos_g * py_rel;
        if tile.giro_mode == ClipMode::Fit {
            let k = fit_scale(sw, sh, cos_g.abs(), sin_g.abs());
            rx *= k;
            ry *= k;
        }
        // Rounded rather than truncated: at a right-angle `giro` the maths
        // above lands exactly on an integer, and `sin`/`cos`'s own float
        // error is as likely to land a hair under it as over — truncating
        // would make that a coin-flip off-by-one on every 90° turn instead
        // of the exact permutation it should be.
        let fx = (rx + cx).round();
        let fy = (ry + cy).round();
        // `Crop`: outside the sector, in tile-relative float space, before
        // the cast — pixel coordinates are unsigned, so a negative result
        // has to be caught here, not after it wraps. `Fit`'s whole point is
        // that `fit_scale` keeps this from firing; it is still checked,
        // because a formula that is only "a reasonable approximation" (see
        // `fit_scale`) earns a real bounds check rather than a trust fall.
        if fx < x0 as f32 || fy < y0 as f32 || fx >= (x0 + sw) as f32 || fy >= (y0 + sh) as f32 {
            return None;
        }
        (fx as u32, fy as u32)
    };
    let (sx, sy) = if tile.w == sw && tile.h == sh {
        (dx - x0, dy - y0)
    } else {
        let u = (dx - x0) as f32 / sw as f32;
        let v = (dy - y0) as f32 / sh as f32;
        (
            ((u * tile.w as f32) as u32).min(tile.w.saturating_sub(1)),
            ((v * tile.h as f32) as u32).min(tile.h.saturating_sub(1)),
        )
    };
    Some(((sy as usize) * (tile.w as usize) + sx as usize) * 4)
}

/// The factor [`ClipMode::Fit`] scales the backward read by: shrinks the
/// sampled offset so a destination pixel anywhere in the sector, however far
/// the turn pushes it, still lands inside the tile — the opposite trade from
/// `Crop`, which leaves the sector's own corners empty rather than lose any
/// of the tile's.
///
/// **Exact for a rectangle, up to a deliberate 1% inset.** Half-extents
/// `a = sw/2`, `b = sh/2`; the worst-case destination corner, walked
/// backwards through the rotation, lands at `(a·|cos| + b·|sin|, a·|sin| +
/// b·|cos|)` — the standard rotated-bounding-box formula, run backwards — and
/// `k` is the smaller of the two axis limits that brings that point back to
/// exactly `a` or `b`. "Exactly" is the part worth distrusting: at that scale
/// the worst-case corner lands *exactly on* the tile's own edge, and which
/// side of a `>=` a value on the line falls on is a coin flip against float
/// error. The 1% margin trades an imperceptible extra sliver of zoom for
/// making that coin flip disappear, on every aspect ratio, not just the
/// square one this module's test checks by hand.
///
/// Squares are exact by construction; the ask's own suggested formula was
/// checked against this one and agrees whenever `sw == sh`. Away from square
/// — a very wide or very tall sector — this is the same well-known
/// rotated-rectangle-inscribing formula either way, not an approximation
/// invented for this feature, but it has only been verified numerically for
/// the square case below, not for extreme aspect ratios.
pub(crate) fn fit_scale(sw: u32, sh: u32, cos_g: f32, sin_g: f32) -> f32 {
    const MARGIN: f32 = 0.99;
    let a = sw as f32 / 2.0;
    let b = sh as f32 / 2.0;
    if a <= 0.0 || b <= 0.0 {
        return 1.0;
    }
    let kx = a / (a * cos_g + b * sin_g);
    let ky = b / (a * sin_g + b * cos_g);
    kx.min(ky).min(1.0) * MARGIN
}

/// The clip state for one tile, or `None` when it fills its rectangle.
///
/// Allocated once per tile rather than once per row: a 1080p tile is a thousand
/// rows, and a `Vec<bool>` per row is a thousand allocations per source per
/// frame for a buffer that is overwritten each time anyway.
fn clipper(tile: &Tile, width: u32) -> Option<Scanline> {
    tile.clip.map(|_| Scanline::new(width))
}

/// Advances `scan` to device row `dy`, and says how much of column `dx` the
/// contour keeps: `0.0` clipped away, `1.0` untouched, in between for a
/// contour that asks for less than full opacity (`Mask::alpha`, H.5b).
///
/// Returning coverage rather than a bool is what lets a nested mask dim or
/// punch a hole in the source it sits on, which is H.5b's stroke/shadow case.
/// A mask at the default `alpha = 1.0` — every contour that existed before
/// H.5b — returns exactly `1.0` or `0.0`, so it still reads as the plain
/// keep/skip it always was.
#[inline]
fn kept(scan: &mut Option<Scanline>, tile: &Tile, dy: u32, height: u32, dx: u32) -> f32 {
    let Some(scan) = scan else { return 1.0 };
    let Some(mask) = tile.clip else { return 1.0 };
    // An operator mid-contour. `Tile::of` already filters these out; this is
    // the same call made where the pixels are, because a hand-built `Tile` that
    // skipped that filter would black out the source rather than fail loudly.
    if !mask.closed() {
        return 1.0;
    }
    // Refilled on the first column of each row. `dx == x0` would be wrong —
    // the loop starts at the sector's left edge, which is the contour's own
    // bounding box, so the first column of the row is whatever that is.
    if scan.row != Some(dy) {
        scan.fill(tile.masks, &[mask], dy as usize, height);
        scan.row = Some(dy);
    }
    scan.cover.get(dx as usize).copied().unwrap_or(0.0)
}

/// `write_alpha` is `false` on the device's own final pass — see [`compose`]
/// / [`compose_rgba`] — and `true` for an intermediate buffer, where the
/// alpha this writes is what lets a later composite tell "nothing drawn
/// here yet" from "something drawn here, fully covering it".
fn blend_tile(
    out: &mut [u8],
    width: u32,
    height: u32,
    tile: &Tile,
    sampler: Option<&Sampler>,
    mesh: Option<&MeshSampler>,
    write_alpha: bool,
) {
    if tile.opacity <= 0.0 {
        return;
    }
    let (x0, y0, sw, sh) = tile.sector.pixels(width, height);
    let mut scan = clipper(tile, width);
    for dy in y0..(y0 + sh).min(height) {
        for dx in x0..(x0 + sw).min(width) {
            let clip = kept(&mut scan, tile, dy, height, dx);
            if clip <= 0.0 {
                continue;
            }
            // The colour this destination pixel reads: through the warp's
            // inverse map when the tile has one (`Sampler::at`, bilinear with
            // clamp), otherwise the un-warped `source_at`. A `None` from
            // either is a pixel skipped exactly like a clipped one above,
            // leaving whatever was already under it.
            let col = if let Some(mesh) = mesh {
                // A mesh takes precedence over this tile's homography. A
                // pixel outside every triangle writes nothing, like a
                // clipped one.
                let Some(col) = mesh.sample(tile.pixels, dx, dy, width, height) else {
                    continue;
                };
                col
            } else {
                match sampler {
                    Some(s) => {
                        let Some((tx, ty)) = s.at(dx, dy) else {
                            continue;
                        };
                        s.sample(tile.pixels, tx, ty)
                    }
                    None => {
                        let Some(src) = source_at(tile, x0, y0, sw, sh, dx, dy) else {
                            continue;
                        };
                        [
                            tile.pixels[src] as f32,
                            tile.pixels[src + 1] as f32,
                            tile.pixels[src + 2] as f32,
                            tile.pixels[src + 3] as f32,
                        ]
                    }
                }
            };
            let dst = ((dy as usize) * (width as usize) + dx as usize) * 4;
            // The tile's own alpha, folded into how much of it lands — every
            // renderer this repo has today writes 255 here, so `cover` is
            // `tile.opacity` exactly, same as before B2. A `mask` node
            // (Conjunto H) leaving pixels transparent on purpose is the only
            // thing that will ever make `ta` less than 1.0.
            //
            // `clip` joins the same product rather than getting a step of its
            // own: a contour at half alpha over a layer at half opacity is a
            // quarter, which is the same arithmetic two nested contours would
            // give, and is `1.0` — a no-op — for every mask without an alpha.
            let ta = col[3] / 255.0;
            let cover = (tile.opacity * ta * clip).clamp(0.0, 1.0);
            for c in 0..3 {
                let under = out[dst + c] as f32 / 255.0;
                let over = col[c] / 255.0;
                let v = tile.blend.mix(under, over, cover);
                out[dst + c] = (v * 255.0 + 0.5) as u8;
            }
            if write_alpha {
                // Source-over on the alpha channel alone — blend modes are a
                // colour operation, not an alpha one.
                let under_a = out[dst + 3] as f32 / 255.0;
                let new_a = cover + under_a * (1.0 - cover);
                out[dst + 3] = (new_a * 255.0 + 0.5) as u8;
            }
        }
    }
}

/// Sectors mode: the layer owns its rectangle outright.
///
/// Opacity still applies, and still fades toward black rather than toward
/// whatever was underneath — because in this mode there is nothing underneath
/// by definition. A half-opacity door is a dimmer door, not a door mixed with
/// its neighbour. That is the distinction that makes this a second mode rather
/// than overlap with `Blend::Normal`.
fn replace_tile(
    out: &mut [u8],
    width: u32,
    height: u32,
    tile: &Tile,
    sampler: Option<&Sampler>,
    mesh: Option<&MeshSampler>,
    write_alpha: bool,
) {
    let (x0, y0, sw, sh) = tile.sector.pixels(width, height);
    let a = tile.opacity.clamp(0.0, 1.0);
    let mut scan = clipper(tile, width);
    for dy in y0..(y0 + sh).min(height) {
        for dx in x0..(x0 + sw).min(width) {
            // Skipped, not blacked out. In sectors mode a layer owns its
            // rectangle, but a *contour* is a hole in that ownership: the
            // pixels outside the polygon belong to whatever was under them, or
            // to black if nothing was. Writing black here would punch the
            // bounding box of every contour out of the frame.
            let clip = kept(&mut scan, tile, dy, height, dx);
            if clip <= 0.0 {
                continue;
            }
            // Same disposition as the contour clip just above, and the same
            // warped-or-not choice as `blend_tile`: a pixel either read
            // carries past the sector's edge (`giro` Crop, an un-invertible
            // warp) is skipped, not blacked out.
            let col = if let Some(mesh) = mesh {
                // A mesh takes precedence over this tile's homography. A
                // pixel outside every triangle writes nothing, like a
                // clipped one.
                let Some(col) = mesh.sample(tile.pixels, dx, dy, width, height) else {
                    continue;
                };
                col
            } else {
                match sampler {
                    Some(s) => {
                        let Some((tx, ty)) = s.at(dx, dy) else {
                            continue;
                        };
                        s.sample(tile.pixels, tx, ty)
                    }
                    None => {
                        let Some(src) = source_at(tile, x0, y0, sw, sh, dx, dy) else {
                            continue;
                        };
                        [
                            tile.pixels[src] as f32,
                            tile.pixels[src + 1] as f32,
                            tile.pixels[src + 2] as f32,
                            tile.pixels[src + 3] as f32,
                        ]
                    }
                }
            };
            let dst = ((dy as usize) * (width as usize) + dx as usize) * 4;
            // Same fold-in as `blend_tile`: `ta` is 1.0 for every real
            // renderer today and `clip` is 1.0 for every mask without an
            // alpha, so `cover` is `a` exactly — unchanged.
            let ta = col[3] / 255.0;
            let cover = (a * ta * clip).clamp(0.0, 1.0);
            for c in 0..3 {
                out[dst + c] = (col[c] * cover + 0.5) as u8;
            }
            if write_alpha {
                out[dst + 3] = (cover * 255.0 + 0.5) as u8;
            }
        }
    }
}

/// The size each layer should be rendered at, and the scale that was applied.
///
/// One pass over the stack up front rather than a decision per layer inside the
/// render loop, so the whole device is either inside its budget or uniformly
/// degraded, and `/state` can say which before a frame is drawn.
///
/// Takes the **effective** sectors rather than the layers, because a source
/// bound to a contour is not rendered at the rectangle in its own record — it is
/// rendered at the contour's bounding box, and a budget computed off the wrong
/// one would either over- or under-charge every mapped source in the room.
///
/// **Charges every sector independently.** [`plan_dedup`] is the version that
/// shares a budget across sectors backed by the same graph node; this is
/// `plan_dedup` called with every key distinct, kept as its own name because
/// almost every caller has no fan-out to report and should not have to
/// invent one.
pub fn plan(
    width: u32,
    height: u32,
    limits: &crate::layers::Limits,
    sectors: &[Sector],
) -> (Vec<(u32, u32)>, f32) {
    let keys: Vec<Option<usize>> = (0..sectors.len()).map(Some).collect();
    plan_dedup(width, height, limits, sectors, &keys)
}

/// [`plan`], but a node feeding more than one sector is only charged for its
/// pixels once — `the design notes` §10 Conjunto B5, the gap `43d60e6`
/// left open because the one real fixture in this repo (one collection, one
/// full-frame layer) has no fan-out to exercise it.
///
/// **`dedup_key[i] == dedup_key[j]` (both `Some`) means sectors `i` and `j`
/// are rendered by the same graph node** — two layers sharing a generator in
/// the compiled plan, not two layers that merely look alike. `None` always
/// counts on its own, for a sector with no node identity to compare (a
/// caller that has not compiled a graph yet). Only the *first* sector seen
/// for a given key counts toward the pixel budget the scale is computed
/// from; every sector still gets its own entry in the returned sizes,
/// because compositing needs a size per **tile**, not per generator — only
/// the accounting that decides `scale` is deduplicated.
pub fn plan_dedup(
    width: u32,
    height: u32,
    limits: &crate::layers::Limits,
    sectors: &[Sector],
    dedup_key: &[Option<usize>],
) -> (Vec<(u32, u32)>, f32) {
    let mut counted: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let wanted: u64 = sectors
        .iter()
        .zip(dedup_key.iter())
        .filter_map(|(s, key)| {
            if let Some(k) = key {
                if !counted.insert(*k) {
                    // Already charged once for this node — a second sector on
                    // the same generator is free.
                    return None;
                }
            }
            let (_, _, w, h) = s.pixels(width, height);
            Some(w as u64 * h as u64)
        })
        .sum();
    let scale = limits.scale_for(wanted);
    let sizes = sectors
        .iter()
        .map(|s| {
            let (_, _, w, h) = s.pixels(width, height);
            if scale >= 1.0 {
                (w, h)
            } else {
                // Never below 16px: past that a model is not a degraded picture,
                // it is a coloured rectangle, and the renderers have their own
                // floor anyway.
                (
                    ((w as f32 * scale) as u32).max(16),
                    ((h as f32 * scale) as u32).max(16),
                )
            }
        })
        .collect();
    (sizes, scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::{Blend, ClipMode, Composition, Limits, Sector};
    use crate::output::Mask;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = vec![0u8; (w * h * 4) as usize];
        for p in v.chunks_exact_mut(4) {
            p[..3].copy_from_slice(&rgb);
            p[3] = 255;
        }
        v
    }

    fn at(px: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
        let p = ((y * w + x) * 4) as usize;
        [px[p], px[p + 1], px[p + 2]]
    }

    fn tile<'a>(px: &'a [u8], w: u32, h: u32, s: Sector, o: f32, b: Blend) -> Tile<'a> {
        Tile {
            pixels: px,
            w,
            h,
            sector: s,
            opacity: o,
            blend: b,
            turn: 0.0,
            giro_mode: ClipMode::Crop,
            clip: None,
            masks: &[],
        }
    }

    /// The same tile, cut to a contour — what a source bound to a real object
    /// is.
    fn cut<'a>(px: &'a [u8], w: u32, h: u32, o: f32, b: Blend, m: &'a Mask) -> Tile<'a> {
        Tile {
            pixels: px,
            w,
            h,
            sector: m.bounds(),
            opacity: o,
            blend: b,
            turn: 0.0,
            giro_mode: ClipMode::Crop,
            clip: Some(m),
            masks: std::slice::from_ref(m),
        }
    }

    /// A tile turned by `giro`, the one knob the two helpers above hold at
    /// zero.
    fn turned<'a>(px: &'a [u8], w: u32, h: u32, s: Sector, turn: f32, mode: ClipMode) -> Tile<'a> {
        Tile {
            pixels: px,
            w,
            h,
            sector: s,
            opacity: 1.0,
            blend: Blend::Normal,
            turn,
            giro_mode: mode,
            clip: None,
            masks: &[],
        }
    }

    #[test]
    fn an_empty_stack_is_a_black_opaque_frame() {
        // A device with everything bypassed. Black is the answer; a frame left
        // at alpha 0 would encode as black anyway and hide the difference
        // between "off" and "the alpha channel was never written".
        let out = compose(8, 4, Composition::Overlap, &[]);
        assert!(out
            .chunks_exact(4)
            .all(|p| p[..3] == [0, 0, 0] && p[3] == 255));
    }

    #[test]
    fn one_full_frame_layer_is_exactly_itself() {
        // The path every existing setup takes: automix at rest is one layer.
        // If this were not pixel-identical, adding layers would have changed
        // what every device already shows.
        let src = solid(16, 9, [200, 30, 40]);
        let out = compose(
            16,
            9,
            Composition::Overlap,
            &[tile(&src, 16, 9, Sector::default(), 1.0, Blend::Normal)],
        );
        assert_eq!(at(&out, 16, 8, 4), [200, 30, 40]);
        assert_eq!(at(&out, 16, 0, 0), [200, 30, 40]);
    }

    /// A left-to-right ramp, so any horizontal warp shows up as a changed
    /// byte at every column rather than only at an edge.
    fn ramp(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let p = ((y * w + x) * 4) as usize;
                let r = (x * 255 / (w - 1).max(1)) as u8;
                v[p] = r;
                v[p + 1] = r;
                v[p + 2] = r;
                v[p + 3] = 255;
            }
        }
        v
    }

    #[test]
    fn an_identity_warp_is_byte_identical_to_no_warp_at_all() {
        // The property the whole design hangs on: a tile rendered at the
        // device's own size and seen through the identity is exactly the
        // un-warped tile, so a graph that never asks for a 3D mask cannot be
        // perturbed by the warp path existing.
        let src = ramp(16, 9);
        let t = [tile(&src, 16, 9, Sector::default(), 1.0, Blend::Normal)];
        let plain = compose(16, 9, Composition::Overlap, &t);
        let identity = Warp {
            homography: Homography::IDENTITY,
            sampling: Sampling::Bilinear,
        };
        let warped = compose_warped(16, 9, Composition::Overlap, &t, &[Some(identity)]);
        assert_eq!(plain, warped, "an identity warp has to be a no-op, byte for byte");
    }

    fn identity_mesh() -> WarpMesh {
        let d = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        WarpMesh {
            tris: vec![
                WarpTri { d: [d[0], d[1], d[2]], uv: [d[0], d[1], d[2]] },
                WarpTri { d: [d[0], d[2], d[3]], uv: [d[0], d[2], d[3]] },
            ],
        }
    }

    #[test]
    fn no_meshes_is_byte_identical_to_the_homography_only_compose() {
        let src = ramp(16, 9);
        let t = [tile(&src, 16, 9, Sector::default(), 1.0, Blend::Normal)];
        let shifted = Warp {
            homography: Homography([1.0, 0.0, -0.5, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            sampling: Sampling::Bilinear,
        };
        let want = compose_rgba_warped(16, 9, Composition::Overlap, &t, &[Some(shifted)], Some([0, 0, 0, 255]));
        let got = compose_rgba_warped_mesh(
            16, 9, Composition::Overlap, &t, &[Some(shifted)], &[None], Some([0, 0, 0, 255]),
        );
        assert_eq!(want, got);
    }

    #[test]
    fn an_identity_mesh_wins_over_a_homography_and_round_trips() {
        let src = ramp(16, 9);
        let t = [tile(&src, 16, 9, Sector::default(), 1.0, Blend::Normal)];
        let shifted = Warp {
            homography: Homography([1.0, 0.0, -0.5, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            sampling: Sampling::Bilinear,
        };
        let plain = compose(16, 9, Composition::Overlap, &t);
        let got = compose_rgba_warped_mesh(
            16,
            9,
            Composition::Overlap,
            &t,
            &[Some(shifted)],
            &[Some(identity_mesh())],
            Some([0, 0, 0, 255]),
        );
        assert_eq!(plain, got, "the mesh takes precedence and is the identity");
    }

    #[test]
    fn a_pixel_outside_the_mesh_writes_nothing() {
        let src = solid(16, 9, [255, 0, 0]);
        let t = [tile(&src, 16, 9, Sector::default(), 1.0, Blend::Normal)];
        // Only the left half is covered.
        let mesh = WarpMesh {
            tris: vec![WarpTri {
                d: [[0.0, 0.0], [0.5, 0.0], [0.5, 1.0]],
                uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            }, WarpTri {
                d: [[0.0, 0.0], [0.5, 1.0], [0.0, 1.0]],
                uv: [[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            }],
        };
        let out = compose_rgba_warped_mesh(
            16, 9, Composition::Overlap, &t, &[None], &[Some(mesh)], Some([0, 0, 0, 255]),
        );
        assert_eq!(at(&out, 16, 1, 4), [255, 0, 0], "inside the mesh");
        assert_eq!(at(&out, 16, 14, 4), [0, 0, 0], "outside the mesh stays background");
    }

    #[test]
    fn a_non_identity_warp_moves_the_pixels_it_reads() {
        // A half-frame translation: the tile's content shifts right, so a
        // column that used to read one place reads another. The tile stays
        // device-sized so the only difference is the warp itself.
        let src = ramp(16, 9);
        let t = [tile(&src, 16, 9, Sector::default(), 1.0, Blend::Normal)];
        let plain = compose(16, 9, Composition::Overlap, &t);
        let shifted = Warp {
            homography: Homography([1.0, 0.0, -0.5, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            sampling: Sampling::Bilinear,
        };
        let warped = compose_warped(16, 9, Composition::Overlap, &t, &[Some(shifted)]);
        assert_ne!(plain, warped, "a non-identity warp has to change something");
        // The tile is pushed half a frame left, so the output's middle ends up
        // reading near the tile's right edge.
        let mid = at(&warped, 16, 8, 4)[0];
        let right = at(&plain, 16, 15, 4)[0];
        let plain_mid = at(&plain, 16, 8, 4)[0];
        assert_ne!(mid, plain_mid, "the middle column should have moved");
        assert!(
            mid.abs_diff(right) < 64,
            "the shifted middle should read near the tile's right edge: {mid} vs {right}"
        );
    }

    #[test]
    fn a_warped_tile_still_respects_its_contour_clip() {
        // Warping does not replace the cut: the projected contour is still the
        // clip, so a pixel the contour keeps out stays out whatever the warp.
        let src = solid(16, 9, [200, 30, 40]);
        let mask = Mask {
            id: "m".into(),
            name: "m".into(),
            points: vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
            parent: None,
            alpha: 1.0,
            compose: None,
            stroke: None,
        };
        let t = [cut(&src, 16, 9, 1.0, Blend::Normal, &mask)];
        let shift = Warp {
            homography: Homography([1.0, 0.0, 0.2, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            sampling: Sampling::Bilinear,
        };
        let out = compose_warped(16, 9, Composition::Overlap, &t, &[Some(shift)]);
        assert_eq!(at(&out, 16, 0, 0), [0, 0, 0], "outside the contour stays cut");
        assert_eq!(at(&out, 16, 8, 4), [200, 30, 40], "inside it still draws");
    }

    #[test]
    fn a_layer_only_touches_its_own_sector() {
        let src = solid(50, 100, [255, 255, 255]);
        let right = Sector {
            x: 0.5,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        };
        let out = compose(
            100,
            100,
            Composition::Overlap,
            &[tile(&src, 50, 100, right, 1.0, Blend::Normal)],
        );
        assert_eq!(
            at(&out, 100, 75, 50),
            [255, 255, 255],
            "the sector went dark"
        );
        assert_eq!(
            at(&out, 100, 25, 50),
            [0, 0, 0],
            "the layer leaked out of its sector"
        );
    }

    #[test]
    fn the_front_layer_wins_where_they_cross() {
        let back = solid(100, 100, [255, 0, 0]);
        let front = solid(50, 50, [0, 0, 255]);
        let quarter = Sector {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        };
        let out = compose(
            100,
            100,
            Composition::Overlap,
            &[
                tile(&back, 100, 100, Sector::default(), 1.0, Blend::Normal),
                tile(&front, 50, 50, quarter, 1.0, Blend::Normal),
            ],
        );
        assert_eq!(at(&out, 100, 25, 25), [0, 0, 255], "the front layer lost");
        assert_eq!(
            at(&out, 100, 75, 75),
            [255, 0, 0],
            "the back layer was overwritten"
        );
    }

    #[test]
    fn half_opacity_lands_halfway() {
        let back = solid(10, 10, [0, 0, 0]);
        let front = solid(10, 10, [255, 255, 255]);
        let out = compose(
            10,
            10,
            Composition::Overlap,
            &[
                tile(&back, 10, 10, Sector::default(), 1.0, Blend::Normal),
                tile(&front, 10, 10, Sector::default(), 0.5, Blend::Normal),
            ],
        );
        let v = at(&out, 10, 5, 5)[0];
        assert!((127..=128).contains(&v), "half opacity landed at {v}");
    }

    #[test]
    fn add_and_screen_brighten_and_multiply_darkens_through_the_compositor() {
        // The maths is tested in `layers.rs`; this is that the compositor
        // actually calls it, per channel, in the right direction.
        let base = solid(4, 4, [100, 100, 100]);
        let over = solid(4, 4, [100, 100, 100]);
        let run = |b: Blend| {
            let out = compose(
                4,
                4,
                Composition::Overlap,
                &[
                    tile(&base, 4, 4, Sector::default(), 1.0, Blend::Normal),
                    tile(&over, 4, 4, Sector::default(), 1.0, b),
                ],
            );
            at(&out, 4, 2, 2)[0]
        };
        assert!(run(Blend::Add) > 100, "add darkened");
        assert!(run(Blend::Screen) > 100, "screen darkened");
        assert!(run(Blend::Multiply) < 100, "multiply brightened");
        assert_eq!(run(Blend::Normal), 100);
    }

    #[test]
    fn sectors_mode_does_not_mix_where_two_rectangles_cross() {
        // *"they do not cross"*. Two overlapping rectangles are a mapping the
        // operator is still dragging into place, and the overlap must belong to
        // one of them rather than becoming a third colour.
        let a = solid(60, 100, [255, 0, 0]);
        let b = solid(60, 100, [0, 0, 255]);
        let left = Sector {
            x: 0.0,
            y: 0.0,
            w: 0.6,
            h: 1.0,
        };
        let right = Sector {
            x: 0.4,
            y: 0.0,
            w: 0.6,
            h: 1.0,
        };
        let out = compose(
            100,
            100,
            Composition::Sectors,
            &[
                tile(&a, 60, 100, left, 1.0, Blend::Normal),
                tile(&b, 60, 100, right, 1.0, Blend::Normal),
            ],
        );
        // The crossing strip is 0.4..0.6. It must be one of the two, whole.
        let mid = at(&out, 100, 50, 50);
        assert_eq!(
            mid,
            [0, 0, 255],
            "the crossing strip was blended into a third colour"
        );
        assert_eq!(at(&out, 100, 10, 50), [255, 0, 0]);
        assert_eq!(at(&out, 100, 90, 50), [0, 0, 255]);
    }

    #[test]
    fn sectors_mode_ignores_the_blend_mode_it_was_given() {
        // Otherwise `Composition::Sectors` would just be overlap with extra
        // steps, and an `Add` left over from a stack that used to be in overlap
        // would quietly reintroduce mixing.
        let a = solid(100, 100, [200, 200, 200]);
        let b = solid(100, 100, [200, 200, 200]);
        let out = compose(
            100,
            100,
            Composition::Sectors,
            &[
                tile(&a, 100, 100, Sector::default(), 1.0, Blend::Normal),
                tile(&b, 100, 100, Sector::default(), 1.0, Blend::Add),
            ],
        );
        assert_eq!(at(&out, 100, 50, 50), [200, 200, 200], "sectors mode added");
    }

    #[test]
    fn a_faded_sector_goes_toward_black_not_toward_its_neighbour() {
        let a = solid(100, 100, [255, 255, 255]);
        let out = compose(
            100,
            100,
            Composition::Sectors,
            &[tile(&a, 100, 100, Sector::default(), 0.5, Blend::Normal)],
        );
        let v = at(&out, 100, 50, 50)[0];
        assert!((127..=128).contains(&v), "a faded sector landed at {v}");
    }

    #[test]
    fn a_shrunk_layer_still_covers_its_whole_sector() {
        // The degraded path. A layer rendered small and written one-to-one
        // would fill a corner of its sector and leave the rest black, which
        // looks like the sector is wrong rather than like the machine is busy.
        let small = solid(20, 20, [10, 220, 90]);
        let out = compose(
            200,
            200,
            Composition::Overlap,
            &[tile(&small, 20, 20, Sector::default(), 1.0, Blend::Normal)],
        );
        for (x, y) in [(0, 0), (199, 199), (100, 3), (3, 100), (199, 0)] {
            assert_eq!(
                at(&out, 200, x, y),
                [10, 220, 90],
                "({x},{y}) was left unpainted"
            );
        }
    }

    #[test]
    fn nothing_is_written_outside_the_frame() {
        // Straight indexing into a byte buffer with numbers that came off a
        // socket. A sector at the very edge is the case that would run past it.
        let src = solid(64, 64, [1, 2, 3]);
        for s in [
            Sector {
                x: 0.99,
                y: 0.99,
                w: 1.0,
                h: 1.0,
            },
            Sector {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            Sector {
                x: 0.5,
                y: 0.5,
                w: 0.5,
                h: 0.5,
            },
        ] {
            for mode in [Composition::Overlap, Composition::Sectors] {
                let out = compose(64, 64, mode, &[tile(&src, 64, 64, s, 1.0, Blend::Normal)]);
                assert_eq!(out.len(), 64 * 64 * 4);
            }
        }
    }

    #[test]
    fn a_stack_inside_the_budget_is_rendered_at_full_size() {
        let limits = Limits::default();
        let (sizes, scale) = plan(1280, 720, &limits, &[Sector::default()]);
        assert_eq!(scale, 1.0);
        assert_eq!(sizes, vec![(1280, 720)]);
    }

    #[test]
    fn a_stack_over_the_budget_shrinks_uniformly_rather_than_dropping_layers() {
        // *"lower resolution/quality before dropping FPS"*. Every layer must
        // still be in the list — a stack that silently rendered four of its six
        // would be an app that stops showing what the table says it is showing.
        let limits = Limits::default();
        let refs = vec![Sector::default(); 8];
        let (sizes, scale) = plan(1920, 1080, &limits, &refs);
        assert_eq!(
            sizes.len(),
            8,
            "the budget dropped a layer instead of shrinking it"
        );
        assert!(
            scale < 1.0,
            "eight full-frame 1080p layers should not be free"
        );
        let total: u64 = sizes.iter().map(|(w, h)| *w as u64 * *h as u64).sum();
        assert!(
            total <= limits.max_pixels + limits.max_pixels / 10,
            "the plan came out at {total} against a budget of {}",
            limits.max_pixels
        );
    }

    #[test]
    fn small_sectors_are_cheap_and_stay_full_size() {
        // The reason the budget counts pixels rather than layers: sixteen
        // door-sized sectors are cheaper than two full frames, and an operator
        // mapping sixteen doors should not be told they are over budget.
        let limits = Limits::default();
        let refs = vec![
            Sector {
                x: 0.0,
                y: 0.0,
                w: 0.2,
                h: 0.2
            };
            16
        ];
        let (_, scale) = plan(1920, 1080, &limits, &refs);
        assert_eq!(scale, 1.0, "sixteen small sectors should fit the budget");
    }

    // -----------------------------------------------------------------------
    // B5 — a fan-out node is charged once, not once per sector it lands in
    // -----------------------------------------------------------------------

    #[test]
    fn plan_is_plan_dedup_with_every_key_distinct() {
        let limits = Limits::default();
        let refs = vec![Sector::default(); 3];
        let keys: Vec<Option<usize>> = (0..refs.len()).map(Some).collect();
        assert_eq!(
            plan(1920, 1080, &limits, &refs),
            plan_dedup(1920, 1080, &limits, &refs, &keys),
            "plan() must be exactly plan_dedup() with nothing shared"
        );
    }

    #[test]
    fn a_shared_generators_pixels_are_charged_once_in_the_budget() {
        let limits = Limits::default();
        // Five full 1080p sectors on five independent generators is well
        // over `Limits::default().max_pixels` (four full frames) and must
        // shrink.
        let five = vec![Sector::default(); 5];
        let independent: Vec<Option<usize>> = (0..5).map(Some).collect();
        let (_, independent_scale) = plan_dedup(1920, 1080, &limits, &five, &independent);
        assert!(
            independent_scale < 1.0,
            "five independent full-frame layers should exceed the budget"
        );

        // The same five sectors, all backed by one generator: the budget
        // only ever sees one full frame, so nothing needs to shrink.
        let shared: Vec<Option<usize>> = vec![Some(0); 5];
        let (_, shared_scale) = plan_dedup(1920, 1080, &limits, &five, &shared);
        assert_eq!(
            shared_scale, 1.0,
            "a generator feeding five sectors must be charged once, not five times"
        );
    }

    #[test]
    fn every_sector_still_gets_its_own_size_even_when_its_cost_is_shared() {
        // Deduplicating the *budget* must not deduplicate the *tiles* —
        // compositing still draws one rectangle per layer, whatever it cost.
        let limits = Limits::default();
        let sectors = vec![Sector::default(); 3];
        let shared: Vec<Option<usize>> = vec![Some(0); 3];
        let (sizes, _) = plan_dedup(1920, 1080, &limits, &sectors, &shared);
        assert_eq!(sizes.len(), 3, "every layer needs its own tile size");
    }

    // -----------------------------------------------------------------------
    // per-source contours
    // -----------------------------------------------------------------------

    fn triangle(id: &str) -> Mask {
        // Points down the left half, so the right half of its own bounding box
        // is outside it — which is what makes it a contour test rather than a
        // rectangle test.
        Mask {
            id: id.into(),
            name: id.into(),
            points: vec![[0.0, 0.0], [0.5, 0.0], [0.0, 1.0]],
            ..Default::default()
        }
    }

    #[test]
    fn a_source_bound_to_a_contour_only_paints_inside_it() {
        let src = solid(100, 100, [255, 255, 255]);
        let m = triangle("t");
        let out = compose(
            100,
            100,
            Composition::Overlap,
            &[cut(&src, 100, 100, 1.0, Blend::Normal, &m)],
        );
        // Well inside the triangle.
        assert_eq!(
            at(&out, 100, 5, 10),
            [255, 255, 255],
            "the contour went dark"
        );
        // Inside the bounding box, outside the polygon. This is the pixel the
        // whole feature is about: without a clip it would be lit, and the wall
        // beside the object would be lit with it.
        assert_eq!(
            at(&out, 100, 45, 90),
            [0, 0, 0],
            "the contour leaked past its edge"
        );
        // Outside the bounding box entirely.
        assert_eq!(at(&out, 100, 90, 50), [0, 0, 0]);
    }

    #[test]
    fn two_sources_on_one_device_light_two_objects_with_two_animations() {
        // *"different sources on one same output device, to frame
        // real-world objects"* — the entire ask, in one frame.
        let red = solid(100, 100, [255, 0, 0]);
        let blue = solid(100, 100, [0, 0, 255]);
        let left = Mask {
            id: "l".into(),
            name: "pilar".into(),
            points: vec![[0.05, 0.2], [0.25, 0.2], [0.25, 0.8], [0.05, 0.8]],
            ..Default::default()
        };
        let right = Mask {
            id: "r".into(),
            name: "puerta".into(),
            points: vec![[0.7, 0.2], [0.9, 0.2], [0.9, 0.8], [0.7, 0.8]],
            ..Default::default()
        };
        let out = compose(
            100,
            100,
            Composition::Overlap,
            &[
                cut(&red, 20, 60, 1.0, Blend::Normal, &left),
                cut(&blue, 20, 60, 1.0, Blend::Normal, &right),
            ],
        );
        assert_eq!(
            at(&out, 100, 15, 50),
            [255, 0, 0],
            "the first object lost its source"
        );
        assert_eq!(
            at(&out, 100, 80, 50),
            [0, 0, 255],
            "the second object lost its source"
        );
        assert_eq!(
            at(&out, 100, 50, 50),
            [0, 0, 0],
            "the wall between them was lit"
        );
    }

    #[test]
    fn a_contour_in_sectors_mode_leaves_what_is_under_it_rather_than_blacking_it() {
        // `replace_tile` writes rather than blends, so the naive clip there is
        // "write black outside the polygon" — which punches the contour's whole
        // bounding box out of anything already drawn.
        let back = solid(100, 100, [90, 90, 90]);
        let front = solid(100, 100, [255, 255, 255]);
        let m = triangle("t");
        let out = compose(
            100,
            100,
            Composition::Sectors,
            &[
                tile(&back, 100, 100, Sector::default(), 1.0, Blend::Normal),
                cut(&front, 100, 100, 1.0, Blend::Normal, &m),
            ],
        );
        assert_eq!(
            at(&out, 100, 5, 10),
            [255, 255, 255],
            "the contour did not draw"
        );
        assert_eq!(
            at(&out, 100, 45, 90),
            [90, 90, 90],
            "the contour blacked out the layer under it"
        );
    }

    #[test]
    fn an_unfinished_contour_does_not_hide_the_source_bound_to_it() {
        // Two points placed is an operator mid-contour, and a source that goes
        // black while its shape is being drawn reads as a broken binding.
        let src = solid(40, 40, [10, 200, 10]);
        let half = Mask {
            id: "w".into(),
            name: "wip".into(),
            points: vec![[0.1, 0.1], [0.9, 0.1]],
            ..Default::default()
        };
        let out = compose(
            40,
            40,
            Composition::Overlap,
            &[cut(&src, 40, 40, 1.0, Blend::Normal, &half)],
        );
        assert_eq!(
            at(&out, 40, 20, 20),
            [10, 200, 10],
            "an unfinished contour blacked the source"
        );
    }

    // -----------------------------------------------------------------------
    // B2 — real alpha in intermediates, opaque black only on the final pass
    // -----------------------------------------------------------------------

    #[test]
    fn compose_and_compose_rgba_agree_byte_for_byte_on_the_final_pass() {
        // The invariant B2 must not break: `compose` is `compose_rgba` with
        // an opaque black background, and every existing caller of
        // `compose` must see exactly what it always saw.
        let a = solid(20, 20, [10, 220, 90]);
        let b = solid(20, 20, [200, 30, 40]);
        let tiles = [
            tile(&a, 20, 20, Sector::default(), 1.0, Blend::Normal),
            tile(
                &b,
                20,
                20,
                Sector {
                    x: 0.25,
                    y: 0.25,
                    w: 0.5,
                    h: 0.5,
                },
                0.6,
                Blend::Add,
            ),
        ];
        let old = compose(40, 40, Composition::Overlap, &tiles);
        let new = compose_rgba(40, 40, Composition::Overlap, &tiles, Some([0, 0, 0, 255]));
        assert_eq!(
            old, new,
            "compose() must remain byte-identical to its own RGBA form"
        );
    }

    #[test]
    fn an_intermediate_buffer_starts_transparent_not_black() {
        let out = compose_rgba(8, 4, Composition::Overlap, &[], None);
        assert!(
            out.chunks_exact(4).all(|p| p == [0, 0, 0, 0]),
            "an empty intermediate must be transparent, not opaque black"
        );
    }

    #[test]
    fn two_overlapping_masked_sources_leave_no_black_seam_in_an_intermediate() {
        // The B2 gate, almost verbatim: two contour-clipped sources that
        // overlap, composited into an intermediate (background = None).
        // Where neither source painted, the result must read as *nothing
        // drawn* (alpha 0) rather than the opaque black `compose()` used to
        // hard-code — that black is exactly the seam the gate is about.
        let red = solid(100, 100, [255, 0, 0]);
        let blue = solid(100, 100, [0, 0, 255]);
        let left = Mask {
            id: "l".into(),
            name: "l".into(),
            points: vec![[0.05, 0.2], [0.45, 0.2], [0.45, 0.8], [0.05, 0.8]],
            ..Default::default()
        };
        let right = Mask {
            id: "r".into(),
            name: "r".into(),
            points: vec![[0.35, 0.2], [0.9, 0.2], [0.9, 0.8], [0.35, 0.8]],
            ..Default::default()
        };
        let out = compose_rgba(
            100,
            100,
            Composition::Overlap,
            &[
                cut(&red, 100, 100, 1.0, Blend::Normal, &left),
                cut(&blue, 100, 100, 1.0, Blend::Normal, &right),
            ],
            None,
        );
        // Well outside both contours: untouched, transparent.
        assert_eq!(
            at(&out, 100, 5, 5),
            [0, 0, 0],
            "the corner was not transparent"
        );
        assert_eq!(
            out[((5 * 100 + 5) * 4) + 3],
            0,
            "the corner had alpha where nothing drew"
        );
        // Inside the overlap region (x in 0.35..0.45): the later (front)
        // tile wins, fully opaque — not a blend toward the black seam the
        // old opaque-black intermediate would have produced.
        let p = ((50 * 100 + 40) * 4) as usize;
        assert_eq!(
            &out[p..p + 3],
            &[0, 0, 255],
            "the overlap did not read as the front source"
        );
        assert_eq!(
            out[p + 3],
            255,
            "the overlap should be fully covered, not seamed"
        );
    }

    #[test]
    fn the_frame_sent_to_the_device_stays_opaque_even_with_a_translucent_layer() {
        // B2's other half of the gate: whatever an intermediate does, the
        // frame the device actually gets (`compose`, opaque background) is
        // still fully opaque — a half-opacity layer fades toward the opaque
        // background, it does not punch a transparent hole in the output.
        let front = solid(10, 10, [255, 255, 255]);
        let out = compose(
            10,
            10,
            Composition::Overlap,
            &[tile(&front, 10, 10, Sector::default(), 0.5, Blend::Normal)],
        );
        assert!(
            out.chunks_exact(4).all(|p| p[3] == 255),
            "the device frame must stay opaque"
        );
    }

    #[test]
    fn a_clipped_source_writes_nothing_outside_the_frame() {
        // Same bound as the sector test above, with the clip's own indexing on
        // top of it. A contour hard against the edge is the case that runs past.
        let src = solid(32, 32, [1, 2, 3]);
        let edge = Mask {
            id: "e".into(),
            name: "e".into(),
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            ..Default::default()
        };
        for mode in [Composition::Overlap, Composition::Sectors] {
            let out = compose(
                32,
                32,
                mode,
                &[cut(&src, 32, 32, 1.0, Blend::Normal, &edge)],
            );
            assert_eq!(out.len(), 32 * 32 * 4);
        }
    }

    // -----------------------------------------------------------------------
    // giro — the design notes PARTE IV §17.2
    // -----------------------------------------------------------------------

    #[test]
    fn giro_rotates_source_pixels() {
        // A 40x40 tile, black, with a red marker inset two pixels from the
        // top-left corner — inset rather than flush against it, so the check
        // below lands well clear of the sector's own edge, where a rotated
        // nearest-neighbour read is at its most sensitive to float error.
        let n = 40u32;
        let mut px = solid(n, n, [0, 0, 0]);
        for y in 2..10u32 {
            for x in 2..10u32 {
                let p = ((y * n + x) * 4) as usize;
                px[p..p + 3].copy_from_slice(&[220, 20, 20]);
            }
        }
        let sector = Sector::default(); // the whole device, matching the tile 1:1

        // Unrotated: the marker is exactly where it was drawn, nowhere else.
        let base = compose(
            n,
            n,
            Composition::Overlap,
            &[tile(&px, n, n, sector, 1.0, Blend::Normal)],
        );
        assert_eq!(
            at(&base, n, 5, 5),
            [220, 20, 20],
            "the marker was not where it was drawn"
        );
        assert_eq!(
            at(&base, n, 34, 5),
            [0, 0, 0],
            "the target quadrant was already lit"
        );

        // A quarter turn, Crop. Worked out by hand from `source_at`'s own
        // inverse-rotation formula, not guessed: at giro = 90°, centred on a
        // 40x40 sector, `source_at` reads destination (dx, dy) from tile
        // position (dy, 40 - dx) — so (34, 5) reads from roughly (5, 6),
        // inside the marker, and (5, 5) reads from roughly (5, 35), well
        // outside it.
        let out = compose(
            n,
            n,
            Composition::Overlap,
            &[turned(
                &px,
                n,
                n,
                sector,
                std::f32::consts::FRAC_PI_2,
                ClipMode::Crop,
            )],
        );
        assert_eq!(
            at(&out, n, 34, 5),
            [220, 20, 20],
            "giro did not carry the marker to the quadrant the maths predicts"
        );
        assert_eq!(
            at(&out, n, 5, 5),
            [0, 0, 0],
            "the marker's old quadrant should have turned away empty"
        );
    }

    #[test]
    fn giro_fit_keeps_full_square_visible_at_45_degrees() {
        // A square sector, filled edge to edge with a uniform colour, turned
        // 45° — the design notes PARTE IV §17.2's own example: *"at 45° a full
        // rect loses its corners"*. Composited with `background: None` so the
        // corners read unambiguously as covered (opaque) or not (alpha 0),
        // the same idiom B2's own tests use.
        let n = 40u32;
        let content = solid(n, n, [255, 255, 255]);
        let sector = Sector::default();
        let corners = [(0u32, 0u32), (n - 1, 0), (0, n - 1), (n - 1, n - 1)];

        // Crop: the doc's own claim about the default mode. All four corners
        // of the sector must show *nothing drawn* — alpha 0 — not merely a
        // dark colour, which is what an intermediate buffer with no opaque
        // background makes possible to tell apart.
        let cropped = compose_rgba(
            n,
            n,
            Composition::Overlap,
            &[turned(
                &content,
                n,
                n,
                sector,
                std::f32::consts::FRAC_PI_4,
                ClipMode::Crop,
            )],
            None,
        );
        for (x, y) in corners {
            let p = ((y * n + x) * 4) as usize;
            assert_eq!(
                cropped[p + 3],
                0,
                "Crop should have lost the corner at ({x},{y})"
            );
        }

        // Fit: the mode that exists so the same four corners do not do that.
        let fit = compose_rgba(
            n,
            n,
            Composition::Overlap,
            &[turned(
                &content,
                n,
                n,
                sector,
                std::f32::consts::FRAC_PI_4,
                ClipMode::Fit,
            )],
            None,
        );
        for (x, y) in corners {
            let p = ((y * n + x) * 4) as usize;
            assert_eq!(
                &fit[p..p + 3],
                &[255, 255, 255],
                "Fit left a corner empty at ({x},{y})"
            );
            assert_eq!(
                fit[p + 3],
                255,
                "Fit's corner should be fully covered at ({x},{y})"
            );
        }
    }

    #[test]
    fn giro_zero_is_pixel_identical_to_no_giro_at_all() {
        // The load-bearing regression: every layer on every real machine
        // today has no opinion about `giro`, and defaults to 0.0. That must
        // stay indistinguishable from the pre-giro code path, not merely
        // "close" — the whole feature is additive.
        let src = solid(20, 12, [40, 180, 90]);
        let sector = Sector {
            x: 0.1,
            y: 0.2,
            w: 0.6,
            h: 0.5,
        };
        let plain = Tile {
            pixels: &src,
            w: 20,
            h: 12,
            sector,
            opacity: 0.8,
            blend: Blend::Screen,
            turn: 0.0,
            giro_mode: ClipMode::Crop,
            clip: None,
            masks: &[],
        };
        let also_plain = Tile {
            pixels: &src,
            w: 20,
            h: 12,
            sector,
            opacity: 0.8,
            blend: Blend::Screen,
            turn: 0.0,
            giro_mode: ClipMode::Fit,
            clip: None,
            masks: &[],
        };
        let a = compose(64, 64, Composition::Overlap, &[plain]);
        let b = compose(64, 64, Composition::Overlap, &[also_plain]);
        assert_eq!(
            a, b,
            "giro = 0.0 must be pixel-identical regardless of giro_mode — there is nothing to clip yet"
        );
    }
}
