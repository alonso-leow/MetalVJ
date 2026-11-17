//! Gaussian splat objects in the room viewport, CPU layer.
//!
//! A `splat` object names a file under the data root. The file is read off
//! the UI thread, cropped and decimated once, and kept in a process-wide cache
//! keyed by `(file, budget, crop)`, so two objects that name the same file with
//! the same settings share one cloud. Each frame the viewport asks for the
//! ready clouds ([`splat_sets`]), projects them to screen ellipses (cached in
//! egui memory while the camera and the clouds stay put) and paints them as
//! textured quads between the faces. A wgpu layer (wave F) may take over by
//! setting [`GPU_LAYER`] and filling [`gpu_paint`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use web_time::{Duration, Instant, SystemTime};

use egui::epaint::Vertex;
use egui::{
    Color32, ColorImage, Id, Mesh, Pos2, Rect, Shape, TextureHandle, TextureId, TextureOptions,
    Vec2,
};
use glam::{Mat4, Vec3};
use metal_geom::splat::{self, Ellipse, Splats};
use metal_perform::venue::{splat_transform, ObjectForm, Venue};

use super::viewport::OrbitCamera;
use crate::shell::i18n::tr;

/// Set by the GPU layer once it draws splats itself. Off by default: the CPU
/// path paints.
pub(crate) static GPU_LAYER: AtomicBool = AtomicBool::new(false);

pub(crate) fn gpu_layer_active() -> bool {
    GPU_LAYER.load(Ordering::Relaxed)
}

/// One ready splat object: its decimated, cropped cloud in file space and the
/// model matrix that places it in the venue.
pub(crate) struct SplatSet {
    pub object: String,
    pub splats: Arc<Splats>,
    pub model: Mat4,
}

/// Axis-aligned `(min, max)` in file space.
type Bounds = ([f32; 3], [f32; 3]);

/// `(file, budget, crop as text)`.
type Key = (String, u32, String);

#[derive(Clone)]
enum Entry {
    Loading,
    Ready(Arc<Splats>, Bounds),
    Failed(Failure),
}

/// A failed load: the message, when it failed, and the file's mtime at that
/// attempt. A failed entry is retried when the file's mtime changes, and at
/// most every [`RETRY_EVERY`] otherwise.
#[derive(Clone)]
struct Failure {
    msg: String,
    at: Instant,
    mtime: Option<SystemTime>,
}

const RETRY_EVERY: Duration = Duration::from_secs(5);

fn failed(msg: String, mtime: Option<SystemTime>) -> Entry {
    Entry::Failed(Failure {
        msg,
        at: Instant::now(),
        mtime,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Whether a failed load should start again: the file changed since the
/// failure, or the last attempt is at least [`RETRY_EVERY`] old. Browser
/// builds do not retry (their bytes come from the in-memory tree).
#[cfg(not(target_arch = "wasm32"))]
fn retry_due(f: &Failure, path: &Path) -> bool {
    file_mtime(path) != f.mtime || f.at.elapsed() >= RETRY_EVERY
}

#[cfg(target_arch = "wasm32")]
fn retry_due(_f: &Failure, _path: &Path) -> bool {
    false
}

const BUDGET_NATIVE: usize = 150_000;
const BUDGET_WASM: usize = 60_000;
/// egui indexes vertices with `u32`, and a quad is four of them; a mesh holds
/// at most this many before it is split.
const MAX_MESH_VERTICES: usize = 60_000;
const GAUSS_SIZE: usize = 64;

fn default_budget() -> usize {
    if cfg!(target_arch = "wasm32") {
        BUDGET_WASM
    } else {
        BUDGET_NATIVE
    }
}

fn cache() -> MutexGuard<'static, HashMap<Key, Entry>> {
    static CACHE: OnceLock<Mutex<HashMap<Key, Entry>>> = OnceLock::new();
    CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn key_of(file: &str, budget: u32, crop: Option<[[f32; 3]; 2]>) -> Key {
    (file.to_string(), budget, format!("{crop:?}"))
}

/// Reads a splat file. Natively it is a `std::fs` read. In the browser the
/// bytes come from the in-memory tree (`metal_vfs`), which is synchronous, so
/// a browser load is ready on the frame that asks for it.
fn read_splats(path: &Path) -> Result<Splats, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        splat::load(path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = metal_vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        splat::from_bytes(&path.to_string_lossy(), &bytes)
    }
}

/// Reads a splat file, crops it, and decimates it to `budget` (`0` picks the
/// default). Returns the cloud and its bounds before decimation, in file space.
pub(crate) fn load_entry(
    path: &Path,
    budget: u32,
    crop: Option<[[f32; 3]; 2]>,
) -> Result<(Arc<Splats>, Bounds), String> {
    let mut s = read_splats(path)?;
    if let Some([min, max]) = crop {
        s.crop(min, max);
    }
    if s.is_empty() {
        return Err(format!("no splats in {}", path.display()));
    }
    let bounds = s.bounds();
    let cap = if budget == 0 {
        default_budget()
    } else {
        budget as usize
    };
    Ok((Arc::new(s.decimate(cap)), bounds))
}

/// Puts an already-built cloud in the cache under the key a splat object with
/// `file`/`budget`/`crop` reads, so a test or a shot paints it without a file.
#[cfg(test)]
pub(crate) fn preload(file: &str, budget: u32, crop: Option<[[f32; 3]; 2]>, splats: Splats) {
    let bounds = splats.bounds();
    cache().insert(key_of(file, budget, crop), Entry::Ready(Arc::new(splats), bounds));
}

/// The entry for `key`, starting its load when it has none yet, or when its
/// last failure is due a retry.
fn ensure(ctx: &egui::Context, key: &Key, path: PathBuf, crop: Option<[[f32; 3]; 2]>) -> Entry {
    match cache().get(key).cloned() {
        Some(Entry::Failed(f)) if retry_due(&f, &path) => {}
        Some(e) => return e,
        None => {}
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        cache().insert(key.clone(), Entry::Loading);
        // Read the mtime before the bytes, so a write during the read still
        // differs from the stored mtime and triggers a retry.
        let mtime = file_mtime(&path);
        let (ctx, key, budget) = (ctx.clone(), key.clone(), key.1);
        std::thread::spawn(move || {
            let entry = match load_entry(&path, budget, crop) {
                Ok((s, b)) => Entry::Ready(s, b),
                Err(e) => failed(e, mtime),
            };
            cache().insert(key, entry);
            ctx.request_repaint();
        });
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        let entry = match load_entry(&path, key.1, crop) {
            Ok((s, b)) => Entry::Ready(s, b),
            Err(e) => failed(e, None),
        };
        cache().insert(key.clone(), entry);
    }
    cache().get(key).cloned().unwrap_or(Entry::Loading)
}

/// The ready splat objects of `venue`, starting the loads of the rest.
pub(crate) fn splat_sets(ctx: &egui::Context, venue: &Venue) -> Vec<SplatSet> {
    let mut out = Vec::new();
    for o in &venue.objects {
        let ObjectForm::Splat {
            file, budget, crop, ..
        } = &o.form
        else {
            continue;
        };
        let key = key_of(file, *budget, *crop);
        let path = crate::output::data_root().join(file);
        if let Entry::Ready(splats, bounds) = ensure(ctx, &key, path, *crop) {
            out.push(SplatSet {
                object: o.id.clone(),
                splats,
                model: splat_transform(o, bounds),
            });
        }
    }
    out
}

/// `(object id, note)` for every splat object that is still loading or failed.
pub(crate) fn status(venue: &Venue) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for o in &venue.objects {
        let ObjectForm::Splat {
            file, budget, crop, ..
        } = &o.form
        else {
            continue;
        };
        let entry = cache().get(&key_of(file, *budget, *crop)).cloned();
        match entry {
            Some(Entry::Ready(..)) => {}
            Some(Entry::Failed(f)) => out.push((o.id.clone(), format!("{}: {}", o.id, f.msg))),
            _ => out.push((o.id.clone(), format!("{}: {}", o.id, tr("loading splat")))),
        }
    }
    out
}

/// The splats projected to screen space, farthest first. Ellipse `depth` is
/// the squared world distance from the eye, the same measure the viewport's
/// faces are sorted by, so the two interleave.
#[derive(Clone, Debug, Default)]
pub(crate) struct Projected {
    pub ellipses: Vec<Ellipse>,
    /// World position of each ellipse's centre, for the beam light.
    pub world: Vec<[f32; 3]>,
}

impl Projected {
    /// The ellipses under the fixtures' beams ([`splat::beam_light`]). No
    /// copy when no beam is on.
    pub(crate) fn lit(&self, lights: &[splat::BeamLight]) -> std::borrow::Cow<'_, [Ellipse]> {
        if lights.is_empty() {
            return std::borrow::Cow::Borrowed(&self.ellipses);
        }
        let lit = self
            .ellipses
            .iter()
            .zip(&self.world)
            .map(|(e, w)| {
                let light = splat::beam_light(lights, Vec3::from(*w));
                if light == [0.0; 3] {
                    *e
                } else {
                    Ellipse { rgb: splat::lit(e.rgb, light), ..*e }
                }
            })
            .collect();
        std::borrow::Cow::Owned(lit)
    }
}

/// Projects every splat of `sets` into `rect` (positions relative to its
/// top-left corner), sorted back to front.
pub(crate) fn project_sets(sets: &[SplatSet], cam: &OrbitCamera, rect: Rect) -> Projected {
    if sets.is_empty() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return Projected::default();
    }
    let (view, proj) = cam.view_proj(rect);
    let eye = Vec3::from(cam.eye_point());
    let vp = [rect.width(), rect.height()];
    let mut ells: Vec<Ellipse> = Vec::new();
    let mut worlds: Vec<[f32; 3]> = Vec::new();
    for s in sets {
        let mv = view * s.model;
        // SH is in file space, so the view direction is taken there too.
        let eye_file = s.model.inverse().transform_point3(eye);
        for i in 0..s.splats.len() {
            let Some(mut e) = s.splats.project(i, mv, proj, vp) else {
                continue;
            };
            let world = s.model.transform_point3(Vec3::from(s.splats.pos[i]));
            e.depth = (world - eye).length_squared();
            if s.splats.sh_degree > 0 {
                e.rgb = s.splats.colour_from(i, eye_file);
            }
            ells.push(e);
            worlds.push(world.to_array());
        }
    }
    let depths: Vec<f32> = ells.iter().map(|e| e.depth).collect();
    let order = splat::sort_back_to_front(&depths);
    Projected {
        ellipses: order.iter().map(|&i| ells[i as usize]).collect(),
        world: order.iter().map(|&i| worlds[i as usize]).collect(),
    }
}

type CamKey = [u32; 10];
type SetsKey = Vec<(String, usize, [u32; 16])>;

#[derive(Clone)]
struct ProjCache {
    cam: CamKey,
    sets: SetsKey,
    out: Arc<Projected>,
}

fn cam_key(cam: &OrbitCamera, rect: Rect) -> CamKey {
    let t = cam.target;
    [
        t[0].to_bits(),
        t[1].to_bits(),
        t[2].to_bits(),
        cam.distance.to_bits(),
        cam.yaw.to_bits(),
        cam.pitch.to_bits(),
        cam.fov.to_bits(),
        u32::from(cam.ortho),
        rect.width().to_bits(),
        rect.height().to_bits(),
    ]
}

fn sets_key(sets: &[SplatSet]) -> SetsKey {
    sets.iter()
        .map(|s| {
            let m = s.model.to_cols_array().map(f32::to_bits);
            (s.object.clone(), Arc::as_ptr(&s.splats) as usize, m)
        })
        .collect()
}

/// [`project_sets`], kept in egui memory under the venue's id: a frame whose
/// camera, rect and clouds match the last one reuses the projection.
pub(crate) fn project_cached(
    ctx: &egui::Context,
    venue_id: &str,
    sets: &[SplatSet],
    cam: &OrbitCamera,
    rect: Rect,
) -> Arc<Projected> {
    let id = Id::new(("shell/splat/projected", venue_id));
    let cam_k = cam_key(cam, rect);
    let sets_k = sets_key(sets);
    if let Some(hit) = ctx.data(|d| d.get_temp::<ProjCache>(id)) {
        if hit.cam == cam_k && hit.sets == sets_k {
            return hit.out;
        }
    }
    let out = Arc::new(project_sets(sets, cam, rect));
    ctx.data_mut(|d| {
        d.insert_temp(
            id,
            ProjCache {
                cam: cam_k,
                sets: sets_k,
                out: out.clone(),
            },
        )
    });
    out
}

/// A `GAUSS_SIZE`² premultiplied white image: `exp(-4.5 r²)`, with `r` 0 at the
/// centre and 1 at the edge midpoint (the edge is the 3σ ellipse).
fn gaussian_image() -> ColorImage {
    let n = GAUSS_SIZE;
    let mut px = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let u = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let v = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let a = (-4.5 * (u * u + v * v)).exp();
            let c = (a * 255.0).round() as u8;
            px.extend_from_slice(&[c, c, c, c]);
        }
    }
    ColorImage::from_rgba_premultiplied([n, n], &px)
}

/// The Gaussian texture, loaded once and kept in egui memory.
pub(crate) fn gaussian_texture(ctx: &egui::Context) -> TextureId {
    let id = Id::new("shell/splat/gaussian");
    if let Some(h) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return h.id();
    }
    let h = ctx.load_texture("shell/splat/gaussian", gaussian_image(), TextureOptions::LINEAR);
    let tid = h.id();
    ctx.data_mut(|d| d.insert_temp(id, h));
    tid
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// One textured quad per ellipse, at `offset` (the viewport's top-left). Split
/// into several meshes so no mesh passes `MAX_MESH_VERTICES`.
pub(crate) fn mesh_for(ellipses: &[Ellipse], tex: TextureId, offset: Pos2) -> Vec<Mesh> {
    ellipses
        .chunks(MAX_MESH_VERTICES / 4)
        .map(|chunk| {
            let mut mesh = Mesh::with_texture(tex);
            mesh.vertices.reserve(chunk.len() * 4);
            mesh.indices.reserve(chunk.len() * 6);
            for e in chunk {
                let c = offset + Vec2::from(e.centre);
                let a = Vec2::from(e.axis_a);
                let b = Vec2::from(e.axis_b);
                let color = Color32::from_rgba_unmultiplied(
                    to_u8(e.rgb[0]),
                    to_u8(e.rgb[1]),
                    to_u8(e.rgb[2]),
                    to_u8(e.alpha),
                );
                let base = mesh.vertices.len() as u32;
                let corners = [
                    (c - a - b, [0.0, 0.0]),
                    (c + a - b, [1.0, 0.0]),
                    (c + a + b, [1.0, 1.0]),
                    (c - a + b, [0.0, 1.0]),
                ];
                for (pos, uv) in corners {
                    mesh.vertices.push(Vertex {
                        pos,
                        uv: Pos2::new(uv[0], uv[1]),
                        color,
                    });
                }
                mesh.indices
                    .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            mesh
        })
        .collect()
}

/// Paints one run of splats as meshes.
pub(crate) fn paint_batch(
    painter: &egui::Painter,
    tex: TextureId,
    offset: Pos2,
    ellipses: &[Ellipse],
) {
    for m in mesh_for(ellipses, tex, offset) {
        painter.add(Shape::mesh(m));
    }
}

/// One step of the back-to-front sequence: a face or a splat, by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layer {
    Face(usize),
    Splat(usize),
}

/// Merges two back-to-front lists (both sorted farthest first) into one
/// sequence. On a tie the face goes first.
pub(crate) fn interleave(face_depths: &[f32], splat_depths: &[f32]) -> Vec<Layer> {
    let (mut f, mut s) = (0, 0);
    let mut out = Vec::with_capacity(face_depths.len() + splat_depths.len());
    while f < face_depths.len() || s < splat_depths.len() {
        let take_splat = s < splat_depths.len()
            && (f >= face_depths.len() || splat_depths[s] > face_depths[f]);
        if take_splat {
            out.push(Layer::Splat(s));
            s += 1;
        } else {
            out.push(Layer::Face(f));
            f += 1;
        }
    }
    out
}

/// The wgpu splat layer (wave F) draws here when it can; `false` means the CPU path draws.
#[allow(unused_variables)]
pub(crate) fn gpu_paint(
    painter: &egui::Painter,
    rect: Rect,
    cam: &super::viewport::OrbitCamera,
    sets: &[SplatSet],
    venue: &Venue,
    lights: &[splat::BeamLight],
) -> bool {
    #[cfg(feature = "gpu")]
    {
        if !gpu_layer_active() || sets.is_empty() {
            return false;
        }
        let (view, proj) = cam.view_proj(rect);
        let refs: Vec<crate::splat_gpu::SetRef<'_>> = sets
            .iter()
            .map(|s| crate::splat_gpu::SetRef {
                splats: s.splats.as_ref(),
                model: s.model,
                object: s.object.as_str(),
            })
            .collect();
        crate::splat_gpu::paint_layer(painter, rect, view, proj, &refs, venue, lights)
    }
    #[cfg(not(feature = "gpu"))]
    {
        let _ = (painter, rect, cam, sets, venue, lights);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn ellipse(x: f32, depth: f32) -> Ellipse {
        Ellipse {
            centre: [x, 0.0],
            axis_a: [2.0, 0.0],
            axis_b: [0.0, 1.0],
            depth,
            rgb: [1.0, 0.5, 0.0],
            alpha: 0.5,
        }
    }

    fn cloud(n: usize) -> Splats {
        Splats {
            pos: (0..n).map(|i| [i as f32 * 0.5, 0.0, 0.0]).collect(),
            scale: vec![[0.1; 3]; n],
            rot: vec![[0.0, 0.0, 0.0, 1.0]; n],
            opacity: vec![1.0; n],
            rgb: vec![[1.0, 0.5, 0.2]; n],
            ..Default::default()
        }
    }

    #[test]
    fn mesh_for_makes_four_vertices_and_six_indices_per_splat() {
        let tex = TextureId::default();
        let meshes = mesh_for(&[ellipse(1.0, 1.0), ellipse(5.0, 1.0)], tex, pos2(10.0, 20.0));
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].vertices.len(), 8);
        assert_eq!(meshes[0].indices.len(), 12);
        // The first corner is centre - a - b, shifted by the offset.
        assert_eq!(meshes[0].vertices[0].pos, pos2(10.0 + 1.0 - 2.0, 20.0 - 1.0));
        assert_eq!(meshes[0].vertices[2].uv, pos2(1.0, 1.0));
    }

    #[test]
    fn mesh_for_splits_when_vertices_pass_the_limit() {
        let tex = TextureId::default();
        let many = vec![ellipse(0.0, 1.0); 15_001];
        let meshes = mesh_for(&many, tex, Pos2::ZERO);
        assert_eq!(meshes.len(), 2);
        assert_eq!(meshes[0].vertices.len(), 60_000);
        assert_eq!(meshes[1].vertices.len(), 4);
        assert_eq!(meshes[1].indices.len(), 6);
        assert!(meshes.iter().all(|m| m.vertices.len() <= 60_000));
    }

    #[test]
    fn interleave_puts_a_face_between_the_splats_it_separates() {
        // Splats at 8 and 2 with a face at 5: far splat, face, near splat.
        let order = interleave(&[5.0], &[8.0, 2.0]);
        assert_eq!(
            order,
            vec![Layer::Splat(0), Layer::Face(0), Layer::Splat(1)]
        );
        assert_eq!(interleave(&[], &[1.0]), vec![Layer::Splat(0)]);
        assert_eq!(interleave(&[1.0], &[]), vec![Layer::Face(0)]);
    }

    #[test]
    fn gaussian_is_opaque_at_the_centre_and_faint_at_the_edge() {
        let img = gaussian_image();
        assert_eq!(img.size, [GAUSS_SIZE, GAUSS_SIZE]);
        let centre = img.pixels[(GAUSS_SIZE / 2 * GAUSS_SIZE + GAUSS_SIZE / 2)];
        let corner = img.pixels[0];
        assert!(centre.a() > 240, "{centre:?}");
        assert!(corner.a() < 10, "{corner:?}");
    }

    #[test]
    fn load_entry_reads_crops_and_decimates_a_temp_splat_file() {
        let dir = std::env::temp_dir().join(format!("mvj-splat-wave-e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("two.splat");
        let mut bytes = Vec::new();
        for x in [1.0f32, 20.0] {
            for v in [x, 2.0, 3.0] {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            for _ in 0..3 {
                bytes.extend_from_slice(&0.1f32.to_le_bytes());
            }
            bytes.extend_from_slice(&[255, 0, 0, 255]);
            bytes.extend_from_slice(&[255, 128, 128, 128]);
        }
        std::fs::write(&path, &bytes).unwrap();

        let (all, bounds) = load_entry(&path, 0, None).expect("loads");
        assert_eq!(all.len(), 2);
        assert_eq!(bounds.0[0], 1.0);
        assert_eq!(bounds.1[0], 20.0);

        let (cropped, _) =
            load_entry(&path, 0, Some([[0.0; 3], [10.0; 3]])).expect("crops");
        assert_eq!(cropped.len(), 1);

        let (one, _) = load_entry(&path, 1, None).expect("decimates");
        assert_eq!(one.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projection_is_reused_while_the_camera_and_clouds_are_unchanged() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let sets = vec![SplatSet {
            object: "s".into(),
            splats: Arc::new(cloud(3)),
            model: Mat4::IDENTITY,
        }];
        let mut cam = OrbitCamera::default();
        cam.target = [0.0, 0.0, 0.0];
        let first = project_cached(&ctx, "v", &sets, &cam, rect);
        assert!(!first.ellipses.is_empty());
        let again = project_cached(&ctx, "v", &sets, &cam, rect);
        assert!(Arc::ptr_eq(&first, &again));

        cam.yaw += 0.1;
        let moved = project_cached(&ctx, "v", &sets, &cam, rect);
        assert!(!Arc::ptr_eq(&first, &moved));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_failed_load_retries_when_the_file_changes_not_before() {
        let dir = std::env::temp_dir().join(format!("mvj-splat-retry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("late.splat");
        // The file is absent: the failure records no mtime.
        let _ = std::fs::remove_file(&path);
        let f = Failure { msg: "x".into(), at: Instant::now(), mtime: file_mtime(&path) };
        assert!(!retry_due(&f, &path), "a fresh failure waits");
        // The file appears: its mtime differs from the failure's, so retry now.
        std::fs::write(&path, b"x").unwrap();
        assert!(retry_due(&f, &path));
        let same = Failure { msg: "x".into(), at: Instant::now(), mtime: file_mtime(&path) };
        assert!(!retry_due(&same, &path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn projected_ellipses_are_farthest_first() {
        let sets = vec![SplatSet {
            object: "s".into(),
            splats: Arc::new(cloud(4)),
            model: Mat4::IDENTITY,
        }];
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let cam = OrbitCamera::default();
        let p = project_sets(&sets, &cam, rect);
        assert!(p.ellipses.windows(2).all(|w| w[0].depth >= w[1].depth));
    }
}
