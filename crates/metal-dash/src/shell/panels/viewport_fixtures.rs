//! Live fixtures in the 3D venue viewport: the bodies (a yoke and a head for
//! a moving light, a can for the rest), the beam cones with their hit spots,
//! and the poll of `GET /venue/state` that feeds them.
//!
//! The viewport paints everything through [`OrbitCamera`] as polygons and
//! lines (see `viewport.rs`). This module owns the fixture part of that: the
//! pure geometry (cone rings, disc points, strobe phase, pose parsing) is
//! kept apart from the painter calls so it can be unit-tested.

use std::f32::consts::TAU;
use std::time::Duration;

use egui::{Color32, Id, Pos2, Rect, Shape, Stroke, Ui, Vec2};
use glam::Vec3;
use serde_json::Value;

use crate::call::{Call, Params};
use crate::shell::classes::Theme;
use metal_geom::splat::BeamLight;
use metal_perform::venue::{Fixture, FixtureKind, Hit, Pose, Venue};

use super::viewport::{fixture_dir, OrbitCamera};

/// The poll cadence of `GET /venue/state`, seconds.
const POLL_S: f64 = 0.1;
/// Beam length when nothing is hit, metres.
const BEAM_LEN: f32 = 12.0;
/// Half-angle when neither the live zoom nor the authored beam is set, degrees.
const DEFAULT_HALF_DEG: f32 = 15.0;
const CONE_SEGMENTS: usize = 16;
const SPOT_SEGMENTS: usize = 24;
const CONE_ALPHA: f32 = 0.18;
const SPOT_ALPHA: f32 = 0.35;
const EDGE_ALPHA: f32 = 0.6;
/// Below this dimmer a beam is not drawn at all.
const DIM_MIN: f32 = 0.02;
/// Above this strobe value the cone flickers.
const STROBE_MIN: f32 = 0.05;

/// One fixture's entry in the `/venue/state` reply. `actual` is `None` when
/// the reply carries no usable pose for it; the caller then falls back to
/// the authored pose.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LiveFixture {
    pub fixture: String,
    pub actual: Option<Pose>,
    pub strobe: f32,
    pub live: bool,
}

/// The whole `/venue/state` reply, as the viewport reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LiveState {
    pub fixtures: Vec<LiveFixture>,
}

impl LiveState {
    fn get(&self, id: &str) -> Option<&LiveFixture> {
        self.fixtures.iter().find(|f| f.fixture == id)
    }

    fn any_live(&self) -> bool {
        self.fixtures.iter().any(|f| f.live)
    }

    fn any_strobing(&self) -> bool {
        self.fixtures.iter().any(|f| f.live && f.strobe > STROBE_MIN)
    }
}

/// The poll's cached answer, kept in egui memory keyed by the venue id.
#[derive(Clone)]
struct Cached {
    at: f64,
    state: LiveState,
}

/// A pose from one `Pose` object. `pan` and `tilt` are required; the rest
/// default (`zoom` 0 means "use the authored beam", `dimmer` 1, white).
pub(crate) fn parse_pose(v: &Value) -> Option<Pose> {
    let pan = v.get("pan")?.as_f64()? as f32;
    let tilt = v.get("tilt")?.as_f64()? as f32;
    let num = |k: &str, d: f32| {
        v.get(k)
            .and_then(Value::as_f64)
            .map(|x| x as f32)
            .unwrap_or(d)
    };
    let rgb = v
        .get("rgb")
        .and_then(Value::as_array)
        .filter(|a| a.len() == 3)
        .map(|a| {
            let c = |i: usize| a[i].as_f64().unwrap_or(1.0) as f32;
            [c(0), c(1), c(2)]
        })
        .unwrap_or([1.0; 3]);
    Some(Pose {
        pan,
        tilt,
        rotation: num("rotation", 0.0),
        zoom: num("zoom", 0.0).max(0.0),
        dimmer: num("dimmer", 1.0).clamp(0.0, 1.0),
        rgb,
    })
}

/// The `/venue/state` reply. Entries without a `fixture` id are skipped.
pub(crate) fn parse_state(v: &Value) -> LiveState {
    let fixtures = v
        .get("fixtures")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(LiveFixture {
                        fixture: x.get("fixture")?.as_str()?.to_string(),
                        actual: x.get("actual").and_then(parse_pose),
                        strobe: x
                            .get("strobe")
                            .and_then(Value::as_f64)
                            .map(|s| (s as f32).clamp(0.0, 1.0))
                            .unwrap_or(0.0),
                        live: x.get("live").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    LiveState { fixtures }
}

/// The pose a fixture is drawn at: its live `actual`, else its authored pose.
pub(crate) fn pose_of(f: &Fixture, live: &LiveState) -> Pose {
    resolve(live, &f.id, authored_pose(f))
}

fn authored_pose(f: &Fixture) -> Pose {
    Pose {
        pan: f.yaw,
        tilt: f.pitch,
        rotation: f.roll,
        zoom: f.beam,
        dimmer: 1.0,
        rgb: [1.0; 3],
    }
}

fn resolve(live: &LiveState, id: &str, authored: Pose) -> Pose {
    live.get(id).and_then(|l| l.actual).unwrap_or(authored)
}

fn strobe_of(id: &str, live: &LiveState) -> f32 {
    live.get(id).map(|l| l.strobe).unwrap_or(0.0)
}

/// The cone's half-angle, degrees: the live zoom (a beam angle) halved, else
/// the authored beam halved, else 15°.
pub(crate) fn half_angle_deg(zoom: f32, beam: f32) -> f32 {
    if zoom > 0.0 {
        zoom / 2.0
    } else if beam > 0.0 {
        beam / 2.0
    } else {
        DEFAULT_HALF_DEG
    }
}

/// The beam's length: to the first hit, else 12 m.
pub(crate) fn beam_length(hit: Option<&Hit>) -> f32 {
    hit.map(|h| h.distance)
        .filter(|d| *d > 0.05)
        .unwrap_or(BEAM_LEN)
}

/// Whether a strobing beam is lit at `t` seconds: it flickers at
/// `1 + 24 × strobe` Hz, lit for the first half of each cycle. No strobe
/// (or one under the threshold) is always lit.
pub(crate) fn strobe_visible(t: f64, strobe: f32) -> bool {
    if strobe <= STROBE_MIN {
        return true;
    }
    let rate = 1.0 + 24.0 * f64::from(strobe.min(1.0));
    (t * rate).fract() < 0.5
}

/// Two unit vectors spanning the plane perpendicular to `d` (which is unit).
fn perp_basis(d: Vec3) -> (Vec3, Vec3) {
    let helper = if d.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u = d.cross(helper).normalize_or_zero();
    let v = d.cross(u);
    (u, v)
}

/// The far rim of a cone: `segments` points on the circle `length` along
/// `dir` from `apex`, with radius `length × tan(half)`.
pub(crate) fn cone_ring(
    apex: Vec3,
    dir: Vec3,
    half_deg: f32,
    length: f32,
    segments: usize,
) -> Vec<Vec3> {
    let d = dir.normalize_or_zero();
    let (u, v) = perp_basis(d);
    let r = length * half_deg.to_radians().tan();
    let c = apex + d * length;
    (0..segments)
        .map(|i| {
            let a = TAU * i as f32 / segments as f32;
            c + (u * a.cos() + v * a.sin()) * r
        })
        .collect()
}

/// A disc's rim in the plane through `centre` with normal `normal`. Seen
/// from an angle this projects to an ellipse, which is the hit spot.
pub(crate) fn disc_points(centre: Vec3, normal: Vec3, radius: f32, segments: usize) -> Vec<Vec3> {
    let n = if normal.length_squared() > 1e-9 {
        normal.normalize()
    } else {
        Vec3::Y
    };
    let (u, v) = perp_basis(n);
    (0..segments)
        .map(|i| {
            let a = TAU * i as f32 / segments as f32;
            centre + (u * a.cos() + v * a.sin()) * radius
        })
        .collect()
}

/// The horizontal axis a fixture's yoke turns about: perpendicular to its
/// aim at any tilt.
pub(crate) fn side_axis(pan_deg: f32) -> Vec3 {
    let y = pan_deg.to_radians();
    Vec3::new(y.cos(), 0.0, y.sin())
}

/// Half-extents of a body along (side, aim, up), metres.
fn body_half(kind: &FixtureKind) -> [f32; 3] {
    if matches!(kind, FixtureKind::Moving) {
        [0.125, 0.15, 0.125]
    } else {
        [0.1, 0.14, 0.1]
    }
}

/// The beam leaves the front face of the body.
fn beam_apex(at: [f32; 3], dir: Vec3, half_aim: f32) -> Vec3 {
    Vec3::from(at) + dir * half_aim
}

/// The silhouette of a cone seen from the camera: the two rim points that lie
/// furthest either side of the cone's screen axis.
fn silhouette(apex: Pos2, rim: &[Pos2], centre: Pos2) -> Option<(Pos2, Pos2)> {
    let axis = centre - apex;
    if axis.length() < 1.0 {
        return None;
    }
    let n = Vec2::new(-axis.y, axis.x).normalized();
    let side = |p: &Pos2| (*p - apex).dot(n);
    let lo = rim.iter().min_by(|a, b| side(a).total_cmp(&side(b)))?;
    let hi = rim.iter().max_by(|a, b| side(a).total_cmp(&side(b)))?;
    Some((*lo, *hi))
}

/// The cone's fan as one mesh: vertex 0 the apex, then the rim, one triangle
/// per rim edge.
pub(crate) fn cone_mesh(apex: Pos2, rim: &[Pos2], fill: Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(apex, fill);
    for p in rim {
        mesh.colored_vertex(*p, fill);
    }
    let n = rim.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    mesh
}

fn project_all(cam: &OrbitCamera, rect: Rect, pts: &[Vec3]) -> Option<Vec<Pos2>> {
    pts.iter()
        .map(|p| cam.project(p.to_array(), rect))
        .collect()
}

/// An RGBA colour: `rgb × dimmer` with `alpha × dimmer`.
fn beam_colour(rgb: [f32; 3], dimmer: f32, alpha: f32) -> Color32 {
    let d = dimmer.clamp(0.0, 1.0);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * d * 255.0 + 0.5) as u8;
    Color32::from_rgba_unmultiplied(
        byte(rgb[0]),
        byte(rgb[1]),
        byte(rgb[2]),
        (alpha * d * 255.0 + 0.5) as u8,
    )
}

/// One lit beam this frame: the pose it comes from, its cone and first hit.
struct Beam {
    pose: Pose,
    apex: Vec3,
    dir: Vec3,
    hit: Option<Hit>,
    length: f32,
    half: f32,
}

/// The beams that show this frame: placed fixtures above the dimmer floor
/// whose strobe is in its lit phase.
fn beams(venue: &Venue, live: &LiveState, now: f64) -> Vec<Beam> {
    let mut out = Vec::new();
    for f in &venue.fixtures {
        let Some(at) = f.at else {
            continue;
        };
        let pose = pose_of(f, live);
        if pose.dimmer < DIM_MIN || !strobe_visible(now, strobe_of(&f.id, live)) {
            continue;
        }
        let dir = fixture_dir(pose.pan, pose.tilt);
        let apex = beam_apex(at, dir, body_half(&f.kind)[1]);
        let hit = venue.hit(apex, dir);
        let length = beam_length(hit.as_ref());
        let half = half_angle_deg(pose.zoom, f.beam);
        out.push(Beam { pose, apex, dir, hit, length, half });
    }
    out
}

/// The beams as light on splats ([`metal_geom::splat::beam_light`]): the
/// same cones [`draw_beams`] draws, colour times dimmer.
pub(crate) fn beam_lights(venue: &Venue, live: &LiveState, now: f64) -> Vec<BeamLight> {
    beams(venue, live, now)
        .into_iter()
        .map(|b| BeamLight {
            apex: b.apex.to_array(),
            dir: b.dir.to_array(),
            cos_half: b.half.to_radians().cos(),
            length: b.length,
            rgb: b.pose.rgb.map(|c| c * b.pose.dimmer),
        })
        .collect()
}

/// The beams and hit spots of every fixture with a place in the room. Drawn
/// after the faces, before the bodies.
pub(crate) fn draw_beams(
    painter: &egui::Painter,
    theme: &Theme,
    rect: Rect,
    cam: &OrbitCamera,
    venue: &Venue,
    live: &LiveState,
    now: f64,
) {
    for Beam { pose, apex, dir, hit, length, half } in beams(venue, live, now) {
        let rim = cone_ring(apex, dir, half, length, CONE_SEGMENTS);
        let (Some(apex2), Some(rim2)) = (
            cam.project(apex.to_array(), rect),
            project_all(cam, rect, &rim),
        ) else {
            continue;
        };
        // One unfeathered mesh: as separate polygons the fan's needle-thin
        // slices get anti-aliasing spikes that run far past apex and rim.
        let fill = beam_colour(pose.rgb, pose.dimmer, CONE_ALPHA);
        painter.add(Shape::mesh(cone_mesh(apex2, &rim2, fill)));
        let edge = theme.line_stroke(beam_colour(pose.rgb, pose.dimmer, EDGE_ALPHA));
        let centre = cam.project((apex + dir * length).to_array(), rect);
        if let Some((lo, hi)) = centre.and_then(|c| silhouette(apex2, &rim2, c)) {
            painter.line_segment([apex2, lo], edge);
            painter.line_segment([apex2, hi], edge);
        }

        if let Some(h) = &hit {
            let radius = h.distance * half.to_radians().tan();
            let disc = disc_points(Vec3::from(h.at), Vec3::from(h.normal), radius, SPOT_SEGMENTS);
            if let Some(poly) = project_all(cam, rect, &disc) {
                painter.add(Shape::convex_polygon(
                    poly,
                    beam_colour(pose.rgb, pose.dimmer, SPOT_ALPHA),
                    Stroke::NONE,
                ));
            }
        }
    }
}

/// The bodies: a yoke and a head for a moving light, a can for the rest,
/// each aimed by its pose. The selected one is drawn in the highlight colour.
pub(crate) fn draw_bodies(
    painter: &egui::Painter,
    theme: &Theme,
    rect: Rect,
    cam: &OrbitCamera,
    venue: &Venue,
    live: &LiveState,
    selected: Option<&str>,
) {
    for f in &venue.fixtures {
        let Some(at) = f.at else {
            continue;
        };
        let pose = pose_of(f, live);
        let sel = selected == Some(f.id.as_str());
        let stroke = theme.line_stroke(if sel {
            theme.phosphor_lit
        } else {
            theme.accent().color
        });
        let dir = fixture_dir(pose.pan, pose.tilt);
        let side = side_axis(pose.pan);
        let up = side.cross(dir).normalize_or_zero();
        let pivot = Vec3::from(at);

        if matches!(f.kind, FixtureKind::Moving) {
            let fwd = fixture_dir(pose.pan, 0.0);
            let left = pivot - side * 0.16;
            let back = pivot - fwd * 0.12;
            let right = pivot + side * 0.16;
            let yoke = theme.line_stroke(theme.ink_weak);
            for (a, b) in [(left, back), (back, right)] {
                if let (Some(pa), Some(pb)) = (
                    cam.project(a.to_array(), rect),
                    cam.project(b.to_array(), rect),
                ) {
                    painter.line_segment([pa, pb], yoke);
                }
            }
        }

        let [hs, hf, hu] = body_half(&f.kind);
        let corner = |i: usize| {
            let sx = if i & 1 != 0 { 1.0 } else { -1.0 };
            let sy = if i & 2 != 0 { 1.0 } else { -1.0 };
            let sz = if i & 4 != 0 { 1.0 } else { -1.0 };
            pivot + side * (hs * sx) + dir * (hf * sy) + up * (hu * sz)
        };
        let corners: Vec<Option<Pos2>> = (0..8)
            .map(|i| cam.project(corner(i).to_array(), rect))
            .collect();
        for i in 0..8usize {
            for bit in [1usize, 2, 4] {
                if i & bit != 0 {
                    continue;
                }
                if let (Some(a), Some(b)) = (corners[i], corners[i | bit]) {
                    painter.line_segment([a, b], stroke);
                }
            }
        }
    }
}

/// Polls `GET /venue/state` at most every 100 ms and keeps the answer in egui
/// memory keyed by the venue id. A failed poll keeps the last answer. Asks
/// for a repaint while any fixture is live, and sooner while one strobes.
pub(crate) fn poll(ui: &Ui, door: &dyn Call, venue_id: &str) -> LiveState {
    let id = Id::new(("shell/region/venue/fixture-live", venue_id));
    let now = ui.input(|i| i.time);
    let cached = ui.data(|d| d.get_temp::<Cached>(id));
    let state = match cached {
        Some(c) if now - c.at < POLL_S => c.state,
        prev => {
            let state = fetch(door, venue_id)
                .or_else(|| prev.map(|c| c.state))
                .unwrap_or_default();
            ui.data_mut(|d| {
                d.insert_temp(
                    id,
                    Cached {
                        at: now,
                        state: state.clone(),
                    },
                )
            });
            state
        }
    };
    if state.any_strobing() {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    } else if state.any_live() {
        ui.ctx()
            .request_repaint_after(Duration::from_secs_f64(POLL_S));
    }
    state
}

fn fetch(door: &dyn Call, venue_id: &str) -> Option<LiveState> {
    let params = Params {
        query: [("id".to_string(), venue_id.to_string())].into_iter().collect(),
        body: Vec::new(),
        tail: None,
    };
    match door.call("GET", "/venue/state", params) {
        Some(reply) if reply.status == 200 => {
            crate::json_memo::parse(&reply.body).ok().map(|v| parse_state(&v))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEST_W: f32 = 800.0;
    const TEST_H: f32 = 600.0;

    fn rect() -> Rect {
        Rect::from_min_max(Pos2::ZERO, Pos2::new(TEST_W, TEST_H))
    }

    #[test]
    fn cone_rim_sits_on_the_far_circle_at_the_half_angle() {
        let rim = cone_ring(Vec3::ZERO, Vec3::NEG_Z, 30.0, 10.0, 16);
        assert_eq!(rim.len(), 16);
        let r = 10.0 * 30f32.to_radians().tan();
        for p in &rim {
            assert!((p.z + 10.0).abs() < 1.0e-4, "{p:?}");
            assert!((Vec3::new(p.x, p.y, 0.0).length() - r).abs() < 1.0e-3, "{p:?}");
        }
    }

    #[test]
    fn disc_rim_lies_in_the_plane_of_the_normal() {
        let c = Vec3::new(1.0, 2.0, 3.0);
        let disc = disc_points(c, Vec3::Y, 2.0, 24);
        assert_eq!(disc.len(), 24);
        for p in &disc {
            assert!((p.y - 2.0).abs() < 1.0e-4, "{p:?}");
            assert!(((*p - c).length() - 2.0).abs() < 1.0e-4, "{p:?}");
        }
    }

    #[test]
    fn a_degenerate_normal_falls_back_to_up() {
        let disc = disc_points(Vec3::ZERO, Vec3::ZERO, 1.0, 8);
        for p in &disc {
            assert!(p.y.abs() < 1.0e-5, "{p:?}");
        }
    }

    #[test]
    fn strobe_is_always_lit_without_a_strobe() {
        for t in [0.0, 0.013, 0.5, 3.7] {
            assert!(strobe_visible(t, 0.0));
            assert!(strobe_visible(t, 0.04));
        }
    }

    #[test]
    fn strobe_flickers_at_one_plus_24_times_strobe_hertz() {
        // strobe 1 is 25 Hz: lit in the first half of each 40 ms cycle.
        assert!(strobe_visible(0.0, 1.0));
        assert!(strobe_visible(0.015, 1.0));
        assert!(!strobe_visible(0.03, 1.0));
        assert!(strobe_visible(0.041, 1.0));
    }

    #[test]
    fn half_angle_is_zoom_then_beam_then_fifteen_degrees() {
        assert_eq!(half_angle_deg(30.0, 60.0), 15.0);
        assert_eq!(half_angle_deg(0.0, 40.0), 20.0);
        assert_eq!(half_angle_deg(0.0, 0.0), 15.0);
    }

    #[test]
    fn beam_runs_to_the_first_hit_or_twelve_metres() {
        let hit = Hit {
            what: "wall".into(),
            at: [0.0, 0.0, -3.0],
            distance: 3.0,
            normal: [0.0, 0.0, 1.0],
        };
        assert_eq!(beam_length(Some(&hit)), 3.0);
        assert_eq!(beam_length(None), 12.0);
    }

    #[test]
    fn a_pose_needs_pan_and_tilt_and_fills_the_rest() {
        assert!(parse_pose(&json!({ "tilt": 10.0 })).is_none());
        let p = parse_pose(&json!({ "pan": 45.0, "tilt": -20.0 })).expect("pose");
        assert_eq!((p.pan, p.tilt), (45.0, -20.0));
        assert_eq!(p.zoom, 0.0);
        assert_eq!(p.dimmer, 1.0);
        assert_eq!(p.rgb, [1.0; 3]);
        let q = parse_pose(&json!({ "pan": 0, "tilt": 0, "dimmer": 2.0, "rgb": [0.5, 0.25, 0.0] }))
            .expect("pose");
        assert_eq!(q.dimmer, 1.0);
        assert_eq!(q.rgb, [0.5, 0.25, 0.0]);
    }

    #[test]
    fn the_state_reply_skips_entries_without_an_id() {
        let v = json!({
            "venue": "v",
            "fixtures": [
                { "fixture": "a", "actual": { "pan": 5, "tilt": 6 }, "strobe": 0.5, "live": true },
                { "actual": { "pan": 1, "tilt": 1 } },
                { "fixture": "b" }
            ]
        });
        let s = parse_state(&v);
        assert_eq!(s.fixtures.len(), 2);
        assert_eq!(s.fixtures[0].fixture, "a");
        assert_eq!(s.fixtures[0].strobe, 0.5);
        assert!(s.fixtures[0].live);
        assert!(s.fixtures[0].actual.is_some());
        assert_eq!(s.fixtures[1].actual, None);
        assert!(!s.fixtures[1].live);
        assert!(parse_state(&json!({})).fixtures.is_empty());
    }

    #[test]
    fn a_fixture_without_live_state_keeps_its_authored_pose() {
        let authored = Pose {
            pan: 30.0,
            tilt: -10.0,
            zoom: 25.0,
            dimmer: 1.0,
            ..Pose::default()
        };
        let p = resolve(&LiveState::default(), "h1", authored);
        assert_eq!((p.pan, p.tilt, p.zoom, p.dimmer), (30.0, -10.0, 25.0, 1.0));
        let live = parse_state(&json!({ "fixtures": [
            { "fixture": "h1", "actual": { "pan": 5, "tilt": 6 } }
        ] }));
        assert_eq!(resolve(&live, "h1", authored).pan, 5.0);
        assert_eq!(resolve(&live, "other", authored).pan, 30.0);
    }

    #[test]
    fn the_yoke_turns_with_pan_and_stays_across_the_aim() {
        for pan in [0.0, 37.0, -120.0] {
            for tilt in [0.0, 25.0, -60.0] {
                let d = fixture_dir(pan, tilt);
                assert!(side_axis(pan).dot(d).abs() < 1.0e-5);
            }
        }
    }

    #[test]
    fn a_cone_seen_from_the_room_projects_into_the_viewport() {
        let cam = OrbitCamera::room(10.0);
        let apex = Vec3::new(0.0, 3.0, -2.0);
        let dir = fixture_dir(0.0, -90.0 + 1.0);
        let rim = cone_ring(apex, dir, 15.0, 2.0, CONE_SEGMENTS);
        assert!(project_all(&cam, rect(), &rim).is_some());
        assert!(cam.project(apex.to_array(), rect()).is_some());
    }
}
