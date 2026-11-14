//! The camera-solve seam: the venue tools that a camera has to see (M5).
//!
//! `scan_venue`, `discover_fixture` and `calibrate_projector` all measure the
//! room rather than carry it forward, and every one of them needs a camera;
//! the structured-light pair needs a projector throwing patterns too. Neither
//! exists on a developer machine, so the geometry lives here behind one small
//! seam, [`Camera`], and this tree ships exactly one implementation:
//! [`SyntheticCamera`], a camera pointed at the **known synthetic room** the
//! venue itself describes.
//!
//! It renders the frames the real solve would read — a flood plus the Gray-code
//! stripes on a surface, a fixture's flash, a plain view of the room — and the
//! solve decodes them the same way a `/dev/videoN` frame would be decoded:
//!
//! - *structured light* ([`pins`]): the projector paints Gray code into its own
//!   frame; the camera reads, at each pixel, the bit planes and recovers the
//!   projector coordinate. Those `(surface, device)` correspondences are far
//!   more numerous than the four corners and come mostly from the overlap, so a
//!   single DLT homography fits them and is then evaluated at the surface's own
//!   four corners: that is what `pins` are. The corner does **not** have to be
//!   lit for this to work, which is the whole point — a projector throws a
//!   picture smaller than the wall far more often than one that covers it.
//! - *flash* ([`discover`]): every fixture is lit alone, the camera frame
//!   thresholded, and its bright blob is the fixture.
//! - *plain capture* ([`Camera::capture`]): the room's surfaces, painted back to
//!   front, as a real PNG the vision model can look at.
//!
//! **What this proves, and what it does not.** The room it points at is the
//! venue under calibration, so the numbers it recovers are that venue's own
//! geometry: it proves the pipeline (encode → render → decode → solve) and that
//! the venue's homographies round-trip, not that a physical camera and
//! projector are aligned. The hardware run is plan §4 step 4. A real build's
//! `metal_ingest::camera` fills the same [`Frame`]s and the same decode runs;
//! everything below the seam is deterministic, so the tests are stable.

use glam::Vec3;

use crate::escena::{Homography, Wall};
use crate::venue::{self, Venue};

/// A surface's four corners in its own `(s, t)`, the order pins use:
/// `(0, 0) (1, 0) (1, 1) (0, 1)`.
pub const SURFACE_CORNERS: [[f32; 2]; 4] =
    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// Bits per axis of the Gray code: the projector frame resolves into `2^8`
/// columns and rows, so a decoded coordinate is quantised to ~0.4%, under a
/// tenth of a pin.
const GRAY_BITS: u32 = 8;
/// The structured-light read is small on purpose — it is sampled, not looked
/// at, and a 96×54 frame decodes to thousands of correspondences.
const GRAY_W: u32 = 96;
const GRAY_H: u32 = 54;
/// The synthetic room a plain capture is rendered at when the caller names no
/// size, the same default `capture_camera_frame` uses.
pub const CAPTURE_W: u32 = 1280;
pub const CAPTURE_H: u32 = 720;

/// One camera frame: 8-bit RGBA, row-major, top-left origin.
#[derive(Clone, PartialEq)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn new(w: u32, h: u32) -> Frame {
        Frame {
            w,
            h,
            rgba: vec![0; w as usize * h as usize * 4],
        }
    }

    /// One grey pixel.
    pub fn put(&mut self, x: u32, y: u32, v: u8) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = ((y * self.w + x) * 4) as usize;
        self.rgba[i] = v;
        self.rgba[i + 1] = v;
        self.rgba[i + 2] = v;
        self.rgba[i + 3] = 255;
    }

    /// The red channel — the only one a grey frame carries.
    pub fn luma(&self, x: u32, y: u32) -> u8 {
        self.rgba[((y * self.w + x) * 4) as usize]
    }

    /// PNG bytes, the shape a tool hands an image back in.
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(std::io::Cursor::new(&mut out), self.w, self.h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().map_err(|e| e.to_string())?;
            writer
                .write_image_data(&self.rgba)
                .map_err(|e| e.to_string())?;
            writer.finish().map_err(|e| e.to_string())?;
        }
        Ok(out)
    }
}

/// The one seam between a tool and the hardware. A real build implements it
/// over `metal_ingest::camera`; [`SyntheticCamera`] is this tree's only
/// implementation and it is hardware-free.
pub trait Camera {
    /// One frame of the room, `w`×`h`.
    fn capture(&self, room: &Venue, w: u32, h: u32) -> Result<Frame, String>;
    /// The Gray code `output`'s projector throws on `surface`, as this camera
    /// reads it: a flood frame, then the bit planes.
    fn gray_code(&self, room: &Venue, output: &str, surface: &str) -> Result<GrayCode, String>;
    /// The room with `fixture` flashed alone.
    fn flash(&self, room: &Venue, fixture: &str) -> Result<Frame, String>;
}

/// A camera, as the venue's shots carry one.
#[derive(Clone)]
pub struct SyntheticCamera {
    pub camera: venue::Camera,
    pub bits: u32,
}

impl SyntheticCamera {
    /// The camera the venue's first shot stands for, else one placed to see the
    /// room. This is the plain-capture camera.
    pub fn for_venue(room: &Venue) -> SyntheticCamera {
        let camera = match room.shots.first() {
            Some(shot) => shot.camera.clone(),
            None => room_camera(room),
        };
        SyntheticCamera {
            camera,
            bits: GRAY_BITS,
        }
    }

    /// The camera a discovery scan stands for: one framing the whole room, so
    /// every patched fixture is a candidate and being in frame decides.
    pub fn for_discovery(room: &Venue) -> SyntheticCamera {
        SyntheticCamera {
            camera: room_camera(room),
            bits: GRAY_BITS,
        }
    }

    /// A camera on the lit side of `surface`, far enough back to see it whole
    /// — the synthetic stand-in for "a camera pointed at the wall". The side is
    /// the one `output`'s projector (else the venue's first shot) stands on.
    pub fn for_surface(
        room: &Venue,
        output: &str,
        surface: &str,
    ) -> Result<SyntheticCamera, String> {
        let surf = room
            .surfaces
            .iter()
            .find(|s| s.id == surface)
            .ok_or_else(|| format!("venue {} has no surface {surface:?}", room.venue.id))?;
        let wall = surf.wall();
        let centre = wall.at(0.5, 0.5);
        let normal = Vec3::from(wall.u)
            .cross(Vec3::from(wall.v))
            .normalize_or_zero();
        let anchor = room
            .projectors
            .iter()
            .find(|p| p.output == output)
            .and_then(|p| room.lens(p).ok())
            .or_else(|| room.shots.first().map(|s| Vec3::from(s.camera.eye)));
        let (fov, aspect) = (60.0f32, 16.0f32 / 9.0);
        let half_u = Vec3::from(wall.u).length() * 0.5;
        let half_v = Vec3::from(wall.v).length() * 0.5;
        let t = (fov.to_radians() * 0.5).tan();
        let dist = (half_v / t).max(half_u / (t * aspect)) * 1.25;
        let dist = dist.max(0.5);
        let dir = match anchor {
            Some(a) => {
                let d = (centre - a).normalize_or_zero();
                if d.length_squared() < 1e-6 || d.dot(normal).abs() < 0.2 {
                    if (a - centre).dot(normal) < 0.0 {
                        normal
                    } else {
                        -normal
                    }
                } else {
                    d
                }
            }
            None => -normal,
        };
        let eye = centre - dir * dist;
        Ok(SyntheticCamera {
            camera: camera_at(eye, centre, fov, aspect),
            bits: GRAY_BITS,
        })
    }
}

fn camera_at(eye: Vec3, target: Vec3, fov: f32, aspect: f32) -> venue::Camera {
    let d = (target - eye).normalize_or_zero();
    venue::Camera {
        eye: eye.into(),
        yaw: d.x.atan2(-d.z).to_degrees(),
        pitch: d.y.clamp(-1.0, 1.0).asin().to_degrees(),
        roll: 0.0,
        fov,
        aspect,
    }
}

/// A camera for a venue with no shot: from the room's own corners, looking in.
fn room_camera(room: &Venue) -> venue::Camera {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for s in &room.surfaces {
        let w = s.wall();
        for [u, v] in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
            let p = w.at(u, v);
            min = min.min(p);
            max = max.max(p);
        }
    }
    if min.x > max.x {
        return venue::Camera {
            eye: [0.0, 1.6, 3.0],
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            fov: 60.0,
            aspect: 16.0 / 9.0,
        };
    }
    let centre = (min + max) * 0.5;
    let extent = (max - min).length();
    let dist = (extent * 1.1).max(1.0);
    let eye = centre + Vec3::new(0.0, extent * 0.15, dist);
    camera_at(eye, centre, 60.0, 16.0 / 9.0)
}

/// The flood frame plus the Gray-code bit planes, as a camera read them.
pub struct GrayCode {
    pub frames: Vec<Frame>,
    pub bits: u32,
    pub span: u32,
}

impl GrayCode {
    /// The decoded projector coordinate at a camera pixel — `None` where the
    /// projector lit nothing (the flood frame is dark).
    pub fn decode_at(&self, x: u32, y: u32) -> Option<[f32; 2]> {
        if self.frames.first()?.luma(x, y) <= 127 {
            return None;
        }
        let mut v = [0f32; 2];
        for (axis, out) in v.iter_mut().enumerate() {
            let mut idx = 0u32;
            for k in 0..self.bits {
                let f = 1 + axis as u32 * self.bits + k;
                idx |= ((self.frames[f as usize].luma(x, y) > 127) as u32) << k;
            }
            // Gray back to binary: the prefix XOR of the Gray bits.
            let mut b = 0u32;
            let mut m = idx;
            while m != 0 {
                b ^= m;
                m >>= 1;
            }
            *out = (b as f32 + 0.5) / self.span as f32;
        }
        Some(v)
    }
}

impl Camera for SyntheticCamera {
    fn capture(&self, room: &Venue, w: u32, h: u32) -> Result<Frame, String> {
        let view = self.camera.view("camera-solve");
        let eye = Vec3::from(self.camera.eye);
        let mut walls: Vec<(f32, Wall)> = room
            .surfaces
            .iter()
            .filter_map(|s| {
                let wall = s.wall();
                view.project(wall.at(0.5, 0.5))?;
                Some((eye.distance(wall.at(0.5, 0.5)), wall))
            })
            .collect();
        // Painter's algorithm: far to near.
        walls.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut frame = Frame::new(w, h);
        for (d, wall) in walls {
            let Some(h_to_st) = Homography::from_view(&view, &wall).inverse() else {
                continue;
            };
            let shade = shade_at(d);
            paint_quad(&mut frame, &view, &wall, &h_to_st, |_, _| shade);
        }
        Ok(frame)
    }

    fn gray_code(&self, room: &Venue, output: &str, surface: &str) -> Result<GrayCode, String> {
        let wall = room
            .surfaces
            .iter()
            .find(|s| s.id == surface)
            .ok_or_else(|| format!("venue {} has no surface {surface:?}", room.venue.id))?
            .wall();
        let view = self.camera.view("camera-solve");
        let h_to_st = Homography::from_view(&view, &wall)
            .inverse()
            .ok_or_else(|| format!("the camera sees no {surface:?}"))?;
        let h_proj = room
            .homography(output, surface)
            .ok_or_else(|| format!("no projector for output {output:?} on {surface:?}"))?;
        let n = self.bits;
        let span = 1u32 << n;
        let (w, h) = (GRAY_W, GRAY_H);
        let mut frames = Vec::with_capacity(1 + 2 * n as usize);
        frames.push(render(&view, &wall, &h_to_st, w, h, |s, t| {
            lit(h_proj.map([s, t]))
        }));
        for axis in 0..2u32 {
            for k in 0..n {
                frames.push(render(&view, &wall, &h_to_st, w, h, |s, t| {
                    let Some(d) = h_proj.map([s, t]) else {
                        return 0;
                    };
                    if !(0.0..=1.0).contains(&d[axis as usize]) {
                        return 0;
                    }
                    let idx = (d[axis as usize] as f32 * span as f32).floor() as i64;
                    let idx = idx.clamp(0, span as i64 - 1) as u32;
                    (((idx ^ (idx >> 1)) >> k) & 1) as u8 * 255
                }));
            }
        }
        Ok(GrayCode { frames, bits: n, span })
    }

    fn flash(&self, room: &Venue, fixture: &str) -> Result<Frame, String> {
        let f = room
            .fixtures
            .iter()
            .find(|f| f.id == fixture)
            .ok_or_else(|| format!("venue {} has no fixture {fixture:?}", room.venue.id))?;
        let (w, h) = (GRAY_W, GRAY_H);
        let view = self.camera.view("camera-solve");
        let mut frame = Frame::new(w, h);
        for s in &room.surfaces {
            let wall = s.wall();
            let Some(h_to_st) = Homography::from_view(&view, &wall).inverse() else {
                continue;
            };
            paint_quad(&mut frame, &view, &wall, &h_to_st, |_, _| 18);
        }
        let at = room.footprint(f)?.at;
        if let Some(p) = view.project(Vec3::from(at)) {
            if (0.0..1.0).contains(&p.x) && (0.0..1.0).contains(&p.y) {
                paint_disc(&mut frame, p.x * w as f32, p.y * h as f32, 6.0, 255);
            }
        }
        Ok(frame)
    }
}

/// Whether a projector coordinate lands on the surface (`lit`).
fn lit(d: Option<[f32; 2]>) -> u8 {
    match d {
        Some(d) if (0.0..=1.0).contains(&d[0]) && (0.0..=1.0).contains(&d[1]) => 255,
        _ => 0,
    }
}

fn shade_at(depth: f32) -> u8 {
    (240.0 / (1.0 + 0.6 * depth)).clamp(24.0, 235.0) as u8
}

/// Paint the wall's image footprint with `f(s, t)`.
fn paint_quad<F: FnMut(f32, f32) -> u8>(
    frame: &mut Frame,
    view: &crate::escena::View,
    wall: &Wall,
    h_to_st: &Homography,
    mut f: F,
) {
    let (w, h) = (frame.w as f32, frame.h as f32);
    let mut min = [f32::MAX; 2];
    let mut max = [f32::MIN; 2];
    for [s, t] in SURFACE_CORNERS {
        let Some(p) = view.project(wall.at(s, t)) else {
            return;
        };
        min[0] = min[0].min(p.x);
        min[1] = min[1].min(p.y);
        max[0] = max[0].max(p.x);
        max[1] = max[1].max(p.y);
    }
    let x0 = (min[0] * w).floor().max(0.0) as u32;
    let x1 = (max[0] * w).ceil().min(w) as u32;
    let y0 = (min[1] * h).floor().max(0.0) as u32;
    let y1 = (max[1] * h).ceil().min(h) as u32;
    for y in y0..y1.max(y0) {
        for x in x0..x1.max(x0) {
            let p = [(x as f32 + 0.5) / w, (y as f32 + 0.5) / h];
            let Some(st) = h_to_st.map(p) else {
                continue;
            };
            if !(0.0..=1.0).contains(&st[0]) || !(0.0..=1.0).contains(&st[1]) {
                continue;
            }
            frame.put(x, y, f(st[0], st[1]));
        }
    }
}

fn render<F: FnMut(f32, f32) -> u8>(
    view: &crate::escena::View,
    wall: &Wall,
    h_to_st: &Homography,
    w: u32,
    h: u32,
    f: F,
) -> Frame {
    let mut frame = Frame::new(w, h);
    paint_quad(&mut frame, view, wall, h_to_st, f);
    frame
}

fn paint_disc(frame: &mut Frame, cx: f32, cy: f32, radius: f32, v: u8) {
    let x0 = (cx - radius).floor().max(0.0) as u32;
    let x1 = (cx + radius).ceil().min(frame.w as f32) as u32;
    let y0 = (cy - radius).floor().max(0.0) as u32;
    let y1 = (cy + radius).ceil().min(frame.h as f32) as u32;
    for y in y0..y1.max(y0) {
        for x in x0..x1.max(x0) {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= radius * radius {
                frame.put(x, y, v);
            }
        }
    }
}

/// The projector's `pins` for `surface`: the four surface corners in the
/// output's 0..1, solved from the structured-light correspondence. The corners
/// need not be lit — the homography is fit from the overlap and evaluated at
/// them — so a projector smaller than the wall still calibrates.
pub fn pins(room: &Venue, output: &str, surface: &str) -> Result<[[f32; 2]; 4], String> {
    let camera = SyntheticCamera::for_surface(room, output, surface)?;
    let code = camera.gray_code(room, output, surface)?;
    let wall = room
        .surfaces
        .iter()
        .find(|s| s.id == surface)
        .ok_or_else(|| format!("venue {} has no surface {surface:?}", room.venue.id))?
        .wall();
    let view = camera.camera.view("camera-solve");
    let h_to_st = Homography::from_view(&view, &wall)
        .inverse()
        .ok_or_else(|| format!("the camera sees no {surface:?}"))?;
    let (w, h) = (code.frames[0].w, code.frames[0].h);
    let mut st: Vec<[f32; 2]> = Vec::new();
    let mut xy: Vec<[f32; 2]> = Vec::new();
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            if let Some(dev) = code.decode_at(x, y) {
                let p = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
                if let Some(s) = h_to_st.map(p) {
                    if (0.0..=1.0).contains(&s[0]) && (0.0..=1.0).contains(&s[1]) {
                        st.push(s);
                        xy.push(dev);
                    }
                }
            }
            x += 2;
        }
        y += 2;
    }
    if st.len() < 4 {
        return Err(format!(
            "the camera read {} lit point(s) on {surface:?}; too few to solve",
            st.len()
        ));
    }
    let fit = Homography::from_correspondences(&st, &xy).ok_or_else(|| {
        format!("the structured-light correspondence on {surface:?} did not solve")
    })?;
    let mut pins = [[0.0f32; 2]; 4];
    for (i, c) in SURFACE_CORNERS.iter().enumerate() {
        pins[i] = fit
            .map(*c)
            .ok_or_else(|| format!("corner {i} of {surface:?} folds through infinity"))?;
    }
    Ok(pins)
}

/// Every fixture the camera sees flash, with where its blob lands. A fixture
/// behind the camera, or outside the frame, is not found.
pub fn discover(room: &Venue) -> Result<Vec<(String, [f32; 2])>, String> {
    let camera = SyntheticCamera::for_discovery(room);
    let mut found = Vec::new();
    for f in &room.fixtures {
        let frame = camera.flash(room, &f.id)?;
        if let Some(p) = detect_flash(&frame) {
            found.push((f.id.clone(), p));
        }
    }
    Ok(found)
}

/// The lit centroid of a flash frame, `0..1`, if it has a blob to find.
pub fn detect_flash(frame: &Frame) -> Option<[f32; 2]> {
    let mut sum = [0f64; 2];
    let mut n = 0u64;
    for y in 0..frame.h {
        for x in 0..frame.w {
            if frame.luma(x, y) > 200 {
                sum[0] += x as f64;
                sum[1] += y as f64;
                n += 1;
            }
        }
    }
    (n >= 4).then(|| {
        [
            (sum[0] / n as f64 / frame.w as f64) as f32,
            (sum[1] / n as f64 / frame.h as f64) as f32,
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bar example: two shots, one projector (`proj-1`) throwing onto
    /// `back-wall`, and five fixtures.
    fn bar() -> Venue {
        venue::load(&crate::output::data_root(), "bar").expect("the bar example")
    }

    #[test]
    fn a_plain_capture_is_a_real_frame_of_the_room() {
        let room = bar();
        let camera = SyntheticCamera::for_venue(&room);
        let frame = camera.capture(&room, 160, 90).expect("capture");
        assert_eq!((frame.w, frame.h), (160, 90));
        // The camera sees the back wall, so the frame is not blank.
        let lit = (0..90)
            .flat_map(|y| (0..160).map(move |x| (x, y)))
            .filter(|(x, y)| frame.luma(*x, *y) > 0)
            .count();
        assert!(lit > 100, "the room should paint something: {lit} px");
        let png = frame.to_png().expect("png");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    #[test]
    fn a_camera_placed_for_a_surface_sees_it_whole() {
        let room = bar();
        let camera = SyntheticCamera::for_surface(&room, "proyector-bar", "back-wall")
            .expect("a camera for the back wall");
        let frame = camera.capture(&room, 96, 54).unwrap();
        let lit = (0..54)
            .flat_map(|y| (0..96).map(move |x| (x, y)))
            .filter(|(x, y)| frame.luma(*x, *y) > 0)
            .count();
        assert!(lit > 200, "the wall should fill the frame: {lit} px");
    }

    #[test]
    fn structured_light_recovers_the_projectors_own_pins() {
        let room = bar();
        let pins = pins(&room, "proyector-bar", "back-wall").expect("a solve");
        // The pins live in the projector's 0..1; they must be finite and not
        // collapse to a line (the four corners keep a positive area).
        assert!(pins.iter().all(|p| p[0].is_finite() && p[1].is_finite()));
        let area = (pins[1][0] - pins[0][0]) * (pins[3][1] - pins[0][1]);
        assert!(area.abs() > 0.05, "a collapsed quad: {pins:?}");
        // And the solve round-trips: a homography through the pins, decomposed
        // back to the projector's view, lands within a few degrees of the
        // venue's own projector.
        let projector = room
            .projectors
            .iter()
            .find(|p| p.output == "proyector-bar")
            .unwrap();
        let wall = room
            .surfaces
            .iter()
            .find(|s| s.id == "back-wall")
            .unwrap()
            .wall();
        let h = Homography::from_corners(&SURFACE_CORNERS, &pins).expect("the pins make a map");
        let view = h
            .to_view(&wall, projector.fov(), projector.aspect)
            .expect("a view");
        let true_view = room.view(projector).expect("the venue's view");
        let a = Vec3::from(view.eye);
        let b = Vec3::from(true_view.eye);
        assert!(
            a.distance(b) < 0.25,
            "the solved eye {a:?} drifted from the projector's {b:?}"
        );
    }

    #[test]
    fn a_projector_that_covers_nothing_solves_nothing() {
        // `right-wall` faces +x and the projector throws at the back wall
        // (-z): it lights none of it, so there is no correspondence to fit.
        let room = bar();
        let solved = pins(&room, "proyector-bar", "right-wall");
        assert!(solved.is_err(), "an unlit wall must not calibrate: {solved:?}");
    }

    #[test]
    fn a_fixture_against_the_back_wall_is_found_by_its_flash() {
        let room = bar();
        let found = discover(&room).expect("discover");
        let ids: Vec<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
        // The washes hang between the camera and the back wall, in frame.
        for wash in ["wash-1", "wash-2", "wash-3", "wash-4"] {
            assert!(ids.contains(&wash), "{wash} not found in {ids:?}");
        }
        // Each blob lands somewhere in the frame.
        for (_, at) in &found {
            assert!((0.0..=1.0).contains(&at[0]) && (0.0..=1.0).contains(&at[1]));
        }
    }

    #[test]
    fn a_fixture_behind_the_discovery_camera_is_not_discovered() {
        let mut room = bar();
        // The discovery camera frames the room from outside it (+z, looking
        // -z); a fixture put well past the camera is behind it and unseen.
        let f = room.fixtures.iter_mut().find(|f| f.id == "wash-1").unwrap();
        f.at = Some([4.0, 2.7, 50.0]);
        let found = discover(&room).expect("discover");
        assert!(
            !found.iter().any(|(id, _)| id == "wash-1"),
            "a fixture behind the camera was found"
        );
    }
}
