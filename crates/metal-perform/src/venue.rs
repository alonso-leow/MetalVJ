//! 3D venues: the AI-authored description of a room (or a facade) that
//! compiles into an [`Escena`], and through it into ordinary [`Mask`]s.
//!
//! # Two names, one pipeline
//!
//! - A **shot** is what one camera saw: one stable image, read into six
//!   values, in this order: the camera's angle, the angle of the projection
//!   point, the projector's distance, the floor's brightness, the
//!   background's brightness and the horizon path. Two or three shots of
//!   the same place are what gives the model space awareness; each one is
//!   read on its own.
//! - A **venue** is the output: the one room those shots agree on. Its
//!   surfaces, objects, projectors, fixtures and masks, in metres.
//!
//! The model writes both, as TOML (`data/agent/venues/*.toml` are the
//! few-shot examples; generated venues land in `data/venues/`). Nothing here
//! trusts it: [`Venue::check`] re-derives every number a shot claims from
//! the venue's own geometry — each surface reprojected through each shot's
//! camera, the projector's throw ray-cast from the table it stands on — and
//! reports the disagreements. That is the cheap fidelity guard: the examples
//! teach the shape, the check measures how far a given answer drifted.
//!
//! # Why it compiles to `Escena` and cuts nothing
//!
//! Same seam `escena.rs` draws: *"the scene is an authoring source for
//! `Mask::points`"*. A venue adds what a photo gives and a hand-written
//! scene never had (objects that occlude, a projector placed on a table
//! rather than floating at a coordinate, fixtures and LED strips that take
//! their colour from the frame) and then hands `Escena::project` walls,
//! views and wall masks. By the time the device cuts, a venue mask is a
//! plain `Mask`.
//!
//! # Conventions
//!
//! Metres, right-handed, `+y` up, floor at `y = 0`. `yaw` in degrees, `0`
//! looks down `−z`, `+90` looks toward `+x`. `pitch` in degrees, positive
//! looks up. A surface is `origin + s·u + t·v` with `(0, 0)` its top-left as
//! seen from the side that gets lit (the `Wall` convention). Image points
//! are `0..1` with `(0, 0)` top-left (the `Mask::points` convention).
//!
//! # Image points are read off a grid
//!
//! A vision model that estimates a coordinate on a photo errs ~16–24%; the
//! same model choosing cells of a numbered grid errs ~3–4% (`locate`,
//! `metal_agent::locate`). So a shot read from an image gives its image points
//! (surface corners, the horizon path) as **cells** of the grid
//! `metal_agent::venue_gen` draws over every image it sends — `locate`'s own
//! [`GRID`]×[`GRID`] grid — written `[column, row, part]`, and they become
//! coordinates here, while parsing ([`cell_point`]). Everything past the
//! parse (the check, `look_venue`, `spans`) sees coordinates only. `[x, y]`
//! still loads: a designed viewpoint has no picture to read and gives exact
//! coordinates, and so do venues written before the grid.
//!
//! # The scene, folded in (P0)
//!
//! `escena.rs` and this module used to describe the same room twice. The
//! venue wins, and every field of the old [`Escena`] has a home here:
//!
//! | `Escena` | `Venue` |
//! |---|---|
//! | `Wall` | a `Surface`, or an `Object` face (`<id>.front`) |
//! | `View` | a `Projector`, placed on the object it stands on |
//! | `WallMask` | a `VenueMask` (`wall` → `on`) |
//! | `Wall::fit` | `Projector::pins` (four device-space corners per surface) |
//! | `Escena::spotlight` | [`Venue::spotlight`] |
//! | `Escena::nearest_wall` | [`Venue::nearest_wall`] |
//! | `Escena::homography` | [`Venue::homography`] |
//! | `Escena::project` | [`Venue::project`] |
//!
//! The maths is the same — `Homography`/`Warp` are untouched. The venue still
//! compiles to an `Escena` through [`Venue::escena`], and `project` reads
//! that; the new methods answer the same questions straight off the venue's
//! own surfaces and projectors.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glam::{Quat, Vec3};
use metal_geom::{Curve2, Curve3, Mesh, SplineKind, SweepOptions};
use serde::{Deserialize, Serialize};

use crate::compose;
use crate::escena::{Escena, Homography, View, Wall, WallMask};
use crate::output::Mask;

/// How far, in image units, a reported surface corner may sit from where the
/// venue's geometry puts it before the check says so. About the error the
/// `locate` measurements gave for a grid-cell answer (~3–4%).
pub const REPROJECTION_TOLERANCE: f32 = 0.04;
/// Degrees a shot's reading of the projector's aim may differ from the venue's.
pub const ANGLE_TOLERANCE: f32 = 3.0;
/// Relative error allowed between a shot's projector distance and the
/// ray-cast throw.
pub const DISTANCE_TOLERANCE: f32 = 0.08;
/// Above this background brightness a projection does not read.
pub const BRIGHT_BACKGROUND: f32 = 0.5;

/// The grid a shot's cells are read on: `GRID` columns numbered from 1 along
/// the top, `GRID` rows numbered from 1 down the left, over the whole image.
/// It is `locate`'s first-pass grid (`metal_agent::turn::COARSE_GRID`), drawn
/// by the same code (`metal_agent::locate::overlay_grid`) over every image
/// `metal_agent::venue_gen` sends; the `venue_gen` example refuses to compile
/// if the two numbers part.
pub const GRID: u32 = 9;

/// Where a grid cell's part sits in the image, `0..1`.
///
/// `column` and `row` are 1-based, like the numbers drawn on the grid. The
/// grid goes on past the picture's edge with cells of the same size, so a
/// point outside the image is a cell too: column 0 is the strip just left of
/// column 1, column `GRID + 1` the one just right of column `GRID`, and the
/// same for rows. `part` is the ninth of the cell the point sits in,
/// numbered like a phone keypad — `1 2 3` across the top, `4 5 6` through
/// the middle, `7 8 9` along the bottom — and the point is that ninth's
/// centre, so a reading is never more than 1/54 of the image off along
/// either axis (≈0.026 diagonally, under [`REPROJECTION_TOLERANCE`]).
pub fn cell_point(column: i32, row: i32, part: u8) -> Result<[f32; 2], String> {
    if !(1..=9).contains(&part) {
        return Err(format!(
            "part {part} is not 1..=9 (the cell's ninth, as on a phone keypad: 1 top-left, 5 centre, 9 bottom-right)"
        ));
    }
    let (i, j) = (f32::from((part - 1) % 3), f32::from((part - 1) / 3));
    let n = GRID as f32;
    Ok([
        ((column - 1) as f32 + (2.0 * i + 1.0) / 6.0) / n,
        ((row - 1) as f32 + (2.0 * j + 1.0) / 6.0) / n,
    ])
}

/// The cell and part a point `0..1` falls in: the inverse of
/// [`cell_point`], `[column, row, part]`.
pub fn point_cell(p: [f32; 2]) -> [i32; 3] {
    let thirds = |v: f32| (v * GRID as f32 * 3.0).floor() as i32;
    let (x, y) = (thirds(p[0]), thirds(p[1]));
    [
        x.div_euclid(3) + 1,
        y.div_euclid(3) + 1,
        y.rem_euclid(3) * 3 + x.rem_euclid(3) + 1,
    ]
}

/// One point of a shot's image as written: `[x, y]` in `0..1`, or a grid
/// cell `[column, row, part]` ([`cell_point`]). Three numbers or two is what
/// tells them apart; either way what is kept is the coordinate.
struct ImagePoint([f32; 2]);

impl<'de> Deserialize<'de> for ImagePoint {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Seen;
        impl<'de> serde::de::Visitor<'de> for Seen {
            type Value = ImagePoint;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an image point: a grid cell [column, row, part] or [x, y] in 0..1")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<ImagePoint, A::Error> {
                use serde::de::Error;
                let mut n = Vec::with_capacity(3);
                while let Some(v) = seq.next_element::<f64>()? {
                    if n.len() == 3 {
                        return Err(A::Error::invalid_length(4, &self));
                    }
                    n.push(v);
                }
                match n[..] {
                    [x, y] => Ok(ImagePoint([x as f32, y as f32])),
                    [c, r, k] => {
                        let whole = |v: f64| v.fract() == 0.0 && v.abs() <= 1000.0;
                        if !(whole(c) && whole(r) && whole(k)) || !(0.0..=255.0).contains(&k) {
                            return Err(A::Error::custom(format!(
                                "[{c}, {r}, {k}]: a grid cell is three whole numbers [column, row, part]"
                            )));
                        }
                        cell_point(c as i32, r as i32, k as u8)
                            .map(ImagePoint)
                            .map_err(A::Error::custom)
                    }
                    _ => Err(A::Error::invalid_length(n.len(), &self)),
                }
            }
        }
        d.deserialize_seq(Seen)
    }
}

fn image_corners<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[[f32; 2]; 4], D::Error> {
    Ok(<[ImagePoint; 4]>::deserialize(d)?.map(|p| p.0))
}

fn image_path<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<[f32; 2]>, D::Error> {
    Ok(Vec::<ImagePoint>::deserialize(d)?
        .into_iter()
        .map(|p| p.0)
        .collect())
}

/// The whole file: one `[venue]` table and its arrays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Venue {
    pub venue: Meta,
    #[serde(rename = "shot", default)]
    pub shots: Vec<Shot>,
    #[serde(rename = "surface", default)]
    pub surfaces: Vec<Surface>,
    #[serde(rename = "object", default)]
    pub objects: Vec<Object>,
    #[serde(rename = "projector", default)]
    pub projectors: Vec<Projector>,
    #[serde(rename = "fixture", default)]
    pub fixtures: Vec<Fixture>,
    #[serde(rename = "mask", default)]
    pub masks: Vec<VenueMask>,
    #[serde(rename = "display", default)]
    pub displays: Vec<Display>,
    #[serde(rename = "composition", default)]
    pub compositions: Vec<Composition>,
    /// H2: the capture cameras watching the room, appended so a venue written
    /// before head tracking loads with none. A `sensor.yolo` node names one by
    /// id, and [`Venue::viewer_from`] turns its eyes reading into a viewer
    /// position in metres.
    #[serde(rename = "sensor", default)]
    pub sensors: Vec<Sensor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub name: String,
    pub setting: Setting,
    /// The images or references the venue was read from.
    #[serde(default)]
    pub based_on: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

/// A room has no horizon; outside, the horizon path is what tells the model
/// where light is thrown away into the sky.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Setting {
    Indoor,
    Outdoor,
}

/// One camera's reading of one stable image. The six fields after `image`
/// are the hierarchy every shot carries, in order.
///
/// A shot with an empty `image` is a **designed viewpoint**: nobody
/// photographed it, somebody (or the model) decided where the audience
/// stands. It is checked the same way — its sightings still have to agree
/// with the geometry — and it is what a [`Composition`] lines its pieces up
/// from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub id: String,
    #[serde(default)]
    pub image: String,
    /// The live camera that takes this shot, when there is one
    /// (`/dev/video0`): what `look_venue` captures to compare the room
    /// against what the venue expects it to look like from here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// 1. The angle of the camera.
    pub camera: Camera,
    /// 2. The angle of the projection point, as this camera reads it. Absent
    ///    only in a venue with no projector at all — every output a screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<Projection>,
    /// 3. Distance of the projector: lens to where its axis lands, metres.
    ///    `0` when there is no projection.
    #[serde(default)]
    pub projector_distance: f32,
    /// 4. Mean brightness of the floor in this image, `0..1`.
    pub floor_brightness: f32,
    /// 5. Mean brightness of the surfaces behind the projection, `0..1`.
    pub background_brightness: f32,
    /// 6. Where the sky meets everything else, left to right, in image
    ///    `0..1`. Empty indoors. Written as grid cells or as `[x, y]`
    ///    ([`cell_point`]).
    #[serde(default, deserialize_with = "image_path")]
    pub horizon_path: Vec<[f32; 2]>,
    /// Where each visible surface's four corners fall in this image —
    /// `(0,0) (1,0) (1,1) (0,1)` of the surface, in that order.
    #[serde(default)]
    pub sees: Vec<Sighting>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub eye: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    #[serde(default)]
    pub roll: f32,
    /// Vertical field of view, degrees.
    pub fov: f32,
    /// Width over height of the image.
    pub aspect: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub projector: String,
    pub yaw: f32,
    pub pitch: f32,
    /// The surface the projector's axis lands on.
    pub lands_on: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sighting {
    pub surface: String,
    /// Image `0..1`. A shot read from a photo writes them as the grid cells
    /// they fall in, `[column, row, part]`, turned into coordinates here
    /// ([`cell_point`]); `[x, y]` loads as it is.
    #[serde(deserialize_with = "image_corners")]
    pub corners: [[f32; 2]; 4],
}

/// What kind of screen a [`Display`] is. Only a label for the model and the
/// check (a phone is expected in portrait); the geometry is its surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayKind {
    Tv,
    Monitor,
    Laptop,
    Tablet,
    Phone,
    /// One tile of a video wall: an LED panel whose pixels are on the wall
    /// itself (no projector between the frame and the room). Several tiles
    /// are several displays that one [`Composition`] lines up.
    #[serde(rename = "led_wall")]
    LedWall,
}

/// How a display's pixels reach it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Link {
    /// A screen plugged into the machine that composes (HDMI, the laptop's
    /// own panel).
    #[default]
    Local,
    /// A browser pointed at `/out/<output>` — any phone, nothing installed.
    Page,
    /// Another device running MetalVJ, paired over `metal-sync`: its own
    /// `node` shows the output (`Output::node`).
    Peer,
}

/// A screen that **is** an output: a TV, a monitor, a laptop, a phone.
///
/// No projector stands between the frame and the room, so the mapping is
/// the identity: the output's `0..1` is the screen surface's own `(s, t)`.
/// A mask on a display's surface lands on that output exactly as drawn.
/// The surface is the lit glass only, bezel excluded — that is the rectangle
/// the output's pixels cover, and the one a composition lines up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Display {
    pub id: String,
    pub name: String,
    pub kind: DisplayKind,
    /// The output id this screen is (`POST /outputs {op:"add", id}`).
    pub output: String,
    /// The surface that is its glass.
    pub surface: String,
    /// Pixels, width and height, as the screen is mounted (a phone in
    /// portrait is taller than wide).
    pub resolution: [u32; 2],
    #[serde(default)]
    pub link: Link,
    /// The paired device that shows it, for [`Link::Peer`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default)]
    pub notes: String,
}

/// A camera that watches the room (H2): where it stands, where it points, and
/// how wide its lens is. The `capture` node it names feeds a `sensor.yolo`
/// node, whose `Mode::Eyes` reading [`Venue::viewer_from`] turns into a viewer
/// position relative to the surface the projector lights.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sensor {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Position, metres, right-handed `+y` up.
    #[serde(default)]
    pub at: [f32; 3],
    /// Degrees; `0` looks down `−z`, `+90` toward `+x` (the venue convention).
    #[serde(default)]
    pub yaw: f32,
    /// Degrees, positive looks up.
    #[serde(default)]
    pub pitch: f32,
    /// Degrees about the view axis.
    #[serde(default)]
    pub roll: f32,
    /// Horizontal field of view, degrees.
    #[serde(default = "default_sensor_fov")]
    pub fov: f32,
    #[serde(default = "default_sensor_aspect")]
    pub aspect: f32,
    /// The `capture` node's source string (`/dev/video0`, …), when one is
    /// wired. Free-form here; the node owns the pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<String>,
}

fn default_sensor_fov() -> f32 {
    60.0
}

fn default_sensor_aspect() -> f32 {
    16.0 / 9.0
}

/// The viewer distance, in metres, that an `eye_z_est` of 0.5 means. Matches
/// `metal_sense::CALIBRATION_DISTANCE_M` (this crate does not depend on it, so
/// the number is repeated here on purpose — see `Venue::viewer_from`).
pub const SENSOR_CALIBRATION_DISTANCE_M: f32 = 0.60;

/// How far a full `eye_z_est` swing moves the viewer along the sensor's axis,
/// in metres.
pub const SENSOR_DEPTH_RANGE_M: f32 = 1.50;

/// One picture across several displays, lined up from one viewpoint.
///
/// Each display shows the piece of the picture that sits where its glass
/// sits as seen from `seen_from`'s camera: the pieces are the displays'
/// bounding boxes in that image, normalised to the box around all of them
/// ([`Venue::spans`]). They become [`crate::layers::Layer::span`] windows
/// on one source routed to every display's output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub id: String,
    pub name: String,
    /// The model the source draws, by id (`escape`, or `forge` with
    /// `shader`).
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shader: Option<String>,
    /// Display ids, in any order.
    pub displays: Vec<String>,
    /// The shot whose camera stands where the audience does.
    pub seen_from: String,
    #[serde(default)]
    pub content: String,
}

/// How a [`Surface`] is shaped: the original plane, or a profile swept along
/// a path (a stage fascia, a curved ribbon). The plane keeps its
/// `origin`/`u`/`v`; a swept surface uses those only as a fallback plane for
/// callers that have not checked [`Surface::is_planar`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum SurfaceForm {
    #[default]
    Plane,
    Swept {
        /// 3D control points of the path (the stage fascia), metres.
        path: Vec<[f32; 3]>,
        /// 2D profile in the path's local frame (x = right, y = up), metres.
        profile: Vec<[f32; 2]>,
        #[serde(default)]
        caps: bool,
        #[serde(default)]
        closed: bool,
        /// "polyline" | "catmull_rom" | "bezier"
        #[serde(default = "default_spline")]
        kind: String,
    },
}

fn default_spline() -> String {
    "catmull_rom".into()
}

/// A surface of the venue: a wall, a floor, a facade face, or a swept form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub id: String,
    pub name: String,
    pub origin: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    #[serde(default)]
    pub material: String,
    #[serde(default)]
    pub form: SurfaceForm,
    /// How one `venue.surface` node's many sources share this surface's
    /// pixels: the output's grid, decided here on the surface rather than on
    /// any one generator. Default is a projected split of one cell (the whole
    /// surface), so a venue written before this existed loads and saves
    /// byte-identically.
    #[serde(default, skip_serializing_if = "SectorGrid::is_default")]
    pub sectors: SectorGrid,
}

/// Where a surface's sectors come from: the projector's own geometry, or the
/// streaks an operator drew on the output's 0..1 rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectorSource {
    /// `n` wedges cast from the projector's lens through the surface plane —
    /// exactly what the projector throws, so a fan looks like a fan on the
    /// wall.
    #[default]
    Projected,
    /// The consecutive point pairs an operator clicked, each pair a line
    /// across the surface's projected 0..1 box — the mask-stroke vocabulary.
    Points,
}

/// A surface's split into sectors, read by [`Venue::sector_splits`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectorGrid {
    pub source: SectorSource,
    /// Width over height of the region in metres, so a projected fan divides
    /// a wide wall into wide wedges. `1.0` is a square — one cell, no split.
    #[serde(default = "one")]
    pub ratio: f32,
    /// The drawn grid in the output's own 0..1 space: consecutive pairs
    /// `[start, end]` form one streak that splits the rectangle, exactly as a
    /// mask stroke's pairs do.
    #[serde(default)]
    pub points: Vec<[f32; 2]>,
    /// The points were snapped to a 1/8 grid when they were drawn.
    #[serde(default)]
    pub magnet: bool,
}

impl Default for SectorGrid {
    fn default() -> SectorGrid {
        SectorGrid {
            source: SectorSource::default(),
            ratio: 1.0,
            points: Vec::new(),
            magnet: false,
        }
    }
}

impl SectorGrid {
    /// Whether this grid splits nothing — the serde skip, so a venue with no
    /// sectors writes no `[surface.sectors]` table at all. A `ratio` of `1` is
    /// one cell even when the table spells it, so loading and re-saving such a
    /// file drops the table rather than keeping a split that splits nothing.
    pub fn is_default(&self) -> bool {
        self == &SectorGrid::default()
    }

    /// The cell count a projected split would use for this grid: `ratio` in
    /// tenths of a generator, one for a square.
    pub fn projected_cells(&self) -> usize {
        if (self.ratio - 1.0).abs() < 1e-6 {
            1
        } else {
            (self.ratio * 3.0).round().clamp(2.0, 8.0) as usize
        }
    }

    /// How many cells the grid asks for: one per streak plus one, or — for a
    /// projected split — three per unit of `ratio`, from the plan's "three
    /// generators, three radii from the centre" example. A square (`ratio`
    /// `1`) is one cell, so a legacy venue splits nothing; the count is
    /// clamped to the eight a graph presents sensibly.
    pub fn cells(&self) -> usize {
        match self.source {
            SectorSource::Points => (self.points.len() / 2 + 1).clamp(1, 8),
            SectorSource::Projected => self.projected_cells(),
        }
    }
}

fn one() -> f32 {
    1.0
}

/// The shape of an [`Object`]: the original box, a profile swept along a
/// path, an imported triangle mesh, or a capsule (a stage dummy).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum ObjectForm {
    #[default]
    Box,
    Swept {
        #[serde(default)]
        path: Vec<[f32; 3]>,
        #[serde(default)]
        profile: Vec<[f32; 2]>,
        #[serde(default)]
        caps: bool,
        #[serde(default)]
        closed: bool,
        #[serde(default = "default_spline")]
        kind: String,
    },
    Mesh {
        #[serde(default)]
        positions: Vec<[f32; 3]>,
        #[serde(default)]
        indices: Vec<u32>,
        #[serde(default)]
        uvs: Vec<[f32; 2]>,
        #[serde(default)]
        normals: Vec<[f32; 3]>,
    },
    Capsule {
        #[serde(default = "default_capsule_radius")]
        radius: f32,
        #[serde(default = "default_capsule_height")]
        height: f32,
    },
}

fn default_capsule_radius() -> f32 {
    0.22
}

fn default_capsule_height() -> f32 {
    1.8
}

/// Something standing in the venue: an oriented box on the floor, with an
/// optional round mass on top. It occludes light unless it says otherwise,
/// and the faces it lists become surfaces masks can be drawn on
/// (`<id>.front`, …).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: String,
    pub name: String,
    /// Free vocabulary the model reads: `table`, `counter`, `lamp`, `tree`,
    /// `chair`, `parasol`, `pool`…
    pub kind: String,
    /// Centre of the base, on the floor.
    pub at: [f32; 3],
    /// Width (`x`), height (`y`), depth (`z`) before `yaw`.
    pub size: [f32; 3],
    #[serde(default)]
    pub yaw: f32,
    /// A tree's canopy, a lamp's shade.
    #[serde(default)]
    pub crown: Option<Crown>,
    #[serde(default = "yes")]
    pub occludes: bool,
    /// Light it gives off, `0..1` (a lamp). Zero for everything else.
    #[serde(default)]
    pub emits: f32,
    /// Which box faces are surfaces: `front`, `back`, `left`, `right`, `top`.
    #[serde(default)]
    pub faces: Vec<String>,
    #[serde(default)]
    pub description: String,
    /// The object's first-class 3D form; a legacy venue with no `form` is a
    /// box, exactly as before.
    #[serde(default)]
    pub form: ObjectForm,
    /// Another object this one is attached to (a forward reference is fine).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Where an imported mesh came from (licence, author, URL).
    #[serde(default)]
    pub attribution: String,
    /// The importer or file the mesh came from.
    #[serde(default)]
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Crown {
    pub center: [f32; 3],
    pub radius: f32,
}

fn yes() -> bool {
    true
}

/// A projector, placed on an object rather than at a coordinate: the table
/// is what the model can see in a photo, and the lens height follows from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projector {
    pub id: String,
    pub name: String,
    /// The [`crate::output::Output`] this projector is.
    pub output: String,
    /// The object it stands on.
    pub on: String,
    /// Lens offset from the centre of that object's top, metres.
    #[serde(default = "default_lens")]
    pub lens: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Throw distance over image width.
    pub throw_ratio: f32,
    #[serde(default = "default_aspect")]
    pub aspect: f32,
    #[serde(default)]
    pub lumens: Option<u32>,
    /// A hand-dragged calibration, per surface id: the four corners of that
    /// surface as they land in **this device's** 0..1 (`(0,0) (1,0) (1,1)
    /// (0,1)` of the surface, in that order). The venue's [`crate::escena::View`]
    /// for the projector wins over these when both exist — a pin set is what
    /// an operator reaches for when there is no 3D pose to place a camera in
    /// (the `calibrate wall` gesture, moved onto the projector).
    #[serde(default)]
    pub pins: BTreeMap<String, [[f32; 2]; 4]>,
    /// P-F: edge blending with the other projectors pinned to the same
    /// surface ([`crate::edge_blend`]). Off by default and not written while
    /// it is the default, so a venue file reads and saves as before.
    #[serde(default, skip_serializing_if = "crate::edge_blend::Blend::is_default")]
    pub blend: crate::edge_blend::Blend,
}

fn default_lens() -> [f32; 3] {
    [0.0, 0.12, 0.0]
}
fn default_aspect() -> f32 {
    16.0 / 9.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureKind {
    Wash,
    Spot,
    LedStrip,
    /// A pan/tilt head (a moving light): aims with `yaw`/`pitch`, shapes its
    /// beam. Rides DMX/sACN like a wash or a spot.
    Moving,
    /// A light behind the performers, aimed at them rather than at a surface
    /// the audience sees. Same geometry as a spot.
    Backlight,
    /// A flat light source (a soft panel or a bladed fixture). Same geometry
    /// as a spot: a beam from `at` along `yaw`/`pitch`.
    Panel,
    /// A continuous light with a big diffuser (a 60 W lamp): intensity and
    /// CCT only, maybe RGB. Same geometry as a wash. Appended 2026-10-07
    /// (M1 of the pixel map), serde-default in spirit: an older venue that
    /// never spells it still parses.
    Continuous,
    /// A solid LED bar: a line sampled as one colour. Appended 2026-10-07.
    LedBar,
    /// A camera: a sensor, not an emitter. Its footprint is its pose and
    /// field of view; it takes no colour from the frame. Appended 2026-10-07.
    Camera,
    /// A 2-D LED pixel grid (a media panel). Its `matrix` carries the grid,
    /// the scan order and the channel map; it samples the frame once per cell
    /// and writes each cell's RGB to consecutive DMX channels, wrapping into
    /// the next universe at 512 (D4, ShowXpress' `DMX_Matrix`). Appended
    /// 2026-10-15.
    Matrix,
}

impl FixtureKind {
    /// The snake_case word this kind is written and posted as (M4): the same
    /// spelling serde uses, so the editor's palette, the route and the file
    /// agree.
    pub fn word(self) -> &'static str {
        match self {
            FixtureKind::Wash => "wash",
            FixtureKind::Spot => "spot",
            FixtureKind::LedStrip => "led_strip",
            FixtureKind::Moving => "moving",
            FixtureKind::Backlight => "backlight",
            FixtureKind::Panel => "panel",
            FixtureKind::Continuous => "continuous",
            FixtureKind::LedBar => "led_bar",
            FixtureKind::Camera => "camera",
            FixtureKind::Matrix => "matrix",
        }
    }

    /// The kind a posted word names (M4), or `None` for a word this build
    /// does not have.
    pub fn from_word(word: &str) -> Option<FixtureKind> {
        Some(match word {
            "wash" => FixtureKind::Wash,
            "spot" => FixtureKind::Spot,
            "led_strip" => FixtureKind::LedStrip,
            "moving" => FixtureKind::Moving,
            "backlight" => FixtureKind::Backlight,
            "panel" => FixtureKind::Panel,
            "continuous" => FixtureKind::Continuous,
            "led_bar" => FixtureKind::LedBar,
            "camera" => FixtureKind::Camera,
            "matrix" => FixtureKind::Matrix,
            _ => return None,
        })
    }

    /// The default wire a freshly placed kind uses: a light rides `dmx`, an
    /// LED string `usb`, a camera nothing.
    pub fn default_wire(self) -> &'static str {
        match self {
            FixtureKind::LedStrip => "usb",
            FixtureKind::Camera => "",
            _ => "dmx",
        }
    }
}

/// Which corner a matrix's pixel block starts counting from. The names are
/// ShowXpress' `StartCorner` vocabulary (`Local>:Bottom-left`), read against
/// the surface's own `(s, t)`: `Bottom` is `t = 0`, `Left` is `s = 0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartCorner {
    #[default]
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}

impl StartCorner {
    pub fn word(self) -> &'static str {
        match self {
            StartCorner::BottomLeft => "bottom_left",
            StartCorner::BottomRight => "bottom_right",
            StartCorner::TopLeft => "top_left",
            StartCorner::TopRight => "top_right",
        }
    }

    pub fn from_word(word: &str) -> Option<StartCorner> {
        Some(match word {
            "bottom_left" => StartCorner::BottomLeft,
            "bottom_right" => StartCorner::BottomRight,
            "top_left" => StartCorner::TopLeft,
            "top_right" => StartCorner::TopRight,
            _ => return None,
        })
    }
}

/// A matrix's fast scan axis (ShowXpress' `Orientation`): `Horizontal` walks a
/// row of pixels before the next line, `Vertical` a column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

impl Orientation {
    pub fn word(self) -> &'static str {
        match self {
            Orientation::Horizontal => "horizontal",
            Orientation::Vertical => "vertical",
        }
    }

    pub fn from_word(word: &str) -> Option<Orientation> {
        Some(match word {
            "horizontal" => Orientation::Horizontal,
            "vertical" => Orientation::Vertical,
            _ => return None,
        })
    }
}

/// A matrix's serpentine scan (ShowXpress' `SnakeMode`): `On` walks every
/// other line back the way the previous one came, so the last pixel of one
/// line borders the first of the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Snake {
    #[default]
    Off,
    On,
}

impl Snake {
    pub fn word(self) -> &'static str {
        match self {
            Snake::Off => "off",
            Snake::On => "on",
        }
    }

    pub fn from_word(word: &str) -> Option<Snake> {
        Some(match word {
            "off" => Snake::Off,
            "on" => Snake::On,
            _ => return None,
        })
    }
}

/// How many DMX channels one matrix cell takes here: one per RGB component.
/// ShowXpress writes the same three slots per LED that a string's
/// `strip_slots` does.
pub const MATRIX_CHANNELS_PER_PIXEL: usize = 3;

/// A 2-D pixel matrix: the grid, the scan order and the channel map of a
/// media panel (D4, ShowXpress' `FUN_1402d663a` field set). The pixel block
/// sits after `first_channels` and before `last_channels`; a cell's colour
/// goes to `MATRIX_CHANNELS_PER_PIXEL` consecutive channels from its offset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Matrix {
    /// Columns (`XPixels`).
    #[serde(default = "one_pixel")]
    pub x_pixels: usize,
    /// Rows (`YPixels`).
    #[serde(default = "one_pixel")]
    pub y_pixels: usize,
    #[serde(default)]
    pub start_corner: StartCorner,
    #[serde(default)]
    pub orientation: Orientation,
    #[serde(default)]
    pub snake: Snake,
    /// How many channels precede the pixel block (dimmer, macro, …);
    /// ShowXpress' `FirstChannels` list, whose count shifts the base address.
    #[serde(default)]
    pub first_channels: usize,
    /// How many channels follow the pixel block (strobe, …); ShowXpress'
    /// `LastChannels` list, whose count extends the top address.
    #[serde(default)]
    pub last_channels: usize,
    /// One channel offset per cell in row-major order (`x + y·XPixels`),
    /// `-1` for a cell no channel drives (ShowXpress' `DMX_Matrix`). Empty
    /// derives a consecutive map from the scan order.
    #[serde(default)]
    pub dmx_matrix: Vec<i16>,
}

fn one_pixel() -> usize {
    1
}

impl Default for Matrix {
    fn default() -> Matrix {
        Matrix {
            x_pixels: 1,
            y_pixels: 1,
            start_corner: StartCorner::default(),
            orientation: Orientation::default(),
            snake: Snake::default(),
            first_channels: 0,
            last_channels: 0,
            dmx_matrix: Vec::new(),
        }
    }
}

impl Matrix {
    /// A `x × y` matrix, scan order at its defaults, no channel map yet.
    pub fn new(x_pixels: usize, y_pixels: usize) -> Matrix {
        Matrix {
            x_pixels,
            y_pixels,
            ..Matrix::default()
        }
    }

    /// How many cells the grid holds.
    pub fn cells(&self) -> usize {
        self.x_pixels * self.y_pixels
    }

    /// Whether the grid has at least one cell to drive.
    pub fn check(&self) -> Result<(), String> {
        if self.x_pixels == 0 || self.y_pixels == 0 {
            return Err(format!(
                "a matrix needs at least one pixel: {}×{}",
                self.x_pixels, self.y_pixels
            ));
        }
        if !self.dmx_matrix.is_empty() && self.dmx_matrix.len() != self.cells() {
            return Err(format!(
                "a DMX_Matrix of {} cells does not match a {}×{} grid",
                self.dmx_matrix.len(),
                self.x_pixels,
                self.y_pixels
            ));
        }
        Ok(())
    }

    /// The cells in the order the channels are numbered: `(x, y)` per cell,
    /// `x` in `0..XPixels`, `y` in `0..YPixels`. [`StartCorner`] fixes where
    /// the walk begins and which way each axis runs, [`Orientation`] which
    /// axis is fast, and [`Snake`] reverses every other line.
    pub fn scan_order(&self) -> Vec<(usize, usize)> {
        let (xn, yn) = (self.x_pixels, self.y_pixels);
        if xn == 0 || yn == 0 {
            return Vec::new();
        }
        let xs: Vec<usize> = match self.start_corner {
            StartCorner::BottomLeft | StartCorner::TopLeft => (0..xn).collect(),
            StartCorner::BottomRight | StartCorner::TopRight => (0..xn).rev().collect(),
        };
        let ys: Vec<usize> = match self.start_corner {
            StartCorner::BottomLeft | StartCorner::BottomRight => (0..yn).collect(),
            StartCorner::TopLeft | StartCorner::TopRight => (0..yn).rev().collect(),
        };
        let snake = self.snake == Snake::On;
        let mut out = Vec::with_capacity(xn * yn);
        match self.orientation {
            Orientation::Horizontal => {
                for (line, y) in ys.into_iter().enumerate() {
                    let mut xs = xs.clone();
                    if snake && line % 2 == 1 {
                        xs.reverse();
                    }
                    out.extend(xs.into_iter().map(|x| (x, y)));
                }
            }
            Orientation::Vertical => {
                for (line, x) in xs.into_iter().enumerate() {
                    let mut ys = ys.clone();
                    if snake && line % 2 == 1 {
                        ys.reverse();
                    }
                    out.extend(ys.into_iter().map(|y| (x, y)));
                }
            }
        }
        out
    }

    /// The channel offset of every cell's first (red) channel, indexed
    /// row-major (`x + y·XPixels`), `-1` for a cell no channel drives. The
    /// explicit `DMX_Matrix` when one is set; otherwise consecutive from the
    /// scan order: cell `k` at `first_channels + 1 + k·3`.
    pub fn pixel_channels(&self) -> Vec<i16> {
        let n = self.cells();
        let mut out = vec![-1i16; n];
        if !self.dmx_matrix.is_empty() {
            for (cell, offset) in out.iter_mut().zip(self.dmx_matrix.iter()) {
                *cell = *offset;
            }
            return out;
        }
        let stride = MATRIX_CHANNELS_PER_PIXEL as i16;
        for (rank, (x, y)) in self.scan_order().into_iter().enumerate() {
            out[x + y * self.x_pixels] = self.first_channels as i16 + 1 + rank as i16 * stride;
        }
        out
    }

    /// The channel offsets the map actually uses, in cell order.
    fn used_offsets(&self) -> Vec<i16> {
        self.pixel_channels()
            .into_iter()
            .filter(|o| *o >= 0)
            .collect()
    }

    /// The channel number the fixture's first DMX slot maps to — ShowXpress'
    /// base address: the lowest used offset less `first_channels`. It is 1
    /// for a map whose pixel block starts right after the first channels.
    pub fn base(&self) -> i16 {
        let min = self.used_offsets().into_iter().min().unwrap_or(1);
        min - self.first_channels as i16
    }

    /// The highest channel the fixture uses — ShowXpress' top address: the
    /// highest pixel's last colour channel plus `last_channels`.
    pub fn top(&self) -> i16 {
        let max = self.used_offsets().into_iter().max().unwrap_or(0);
        max + MATRIX_CHANNELS_PER_PIXEL as i16 - 1 + self.last_channels as i16
    }

    /// Where a channel offset lands: the universe and 1-based address `offset`
    /// maps to when the fixture's first slot sits at `address` of `universe`,
    /// wrapping into the next universe every 512 slots.
    pub fn channel_at(&self, universe: u16, address: u16, offset: i16) -> (u16, u16) {
        let rel = offset as i64 - self.base() as i64;
        let abs = (clamp_address(address) as i64 - 1) + rel;
        let u = universe as i64 + abs.div_euclid(512);
        let a = 1 + abs.rem_euclid(512);
        (u.clamp(0, u16::MAX as i64) as u16, a as u16)
    }
}

/// Which end of an LED string's polyline pixel 0 sits at — the `order` of
/// M1's pixel-map vocabulary. `Forward` walks the path as written; `Reverse`
/// walks it back, so a string wired from the far end still lights in its own
/// order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PixelOrder {
    #[default]
    Forward,
    Reverse,
}

/// Where a fixture's floats come from. `frame`: sampled from the
/// projector's rendered frame where the fixture's light (or pixel) lands, so
/// lights and strips move with the projection. `fixed`: the operator's
/// values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Drive {
    #[default]
    Frame,
    Fixed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fixture {
    pub id: String,
    pub name: String,
    pub kind: FixtureKind,
    /// A profile under `data/fixtures/` (`wash`/`spot`/`moving-head-beam`;
    /// a Bluetooth light's profile carries `wire = "bluetooth"` of its own).
    #[serde(default)]
    pub profile: Option<String>,
    /// Where it hangs (a light with a beam: `wash`, `spot`, `moving`,
    /// `backlight`, `panel`).
    #[serde(default)]
    pub at: Option<[f32; 3]>,
    #[serde(default)]
    pub yaw: f32,
    #[serde(default)]
    pub pitch: f32,
    /// Beam angle, degrees (a light with a beam).
    #[serde(default)]
    pub beam: f32,
    /// The surface an LED strip is glued to.
    #[serde(default)]
    pub on: Option<String>,
    /// The strip's path on that surface, in its `(s, t)`.
    #[serde(default)]
    pub path: Vec<[f32; 2]>,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub pixels_per_m: f32,
    /// An LED string's own pixel count, when it is authored rather than
    /// derived from `pixels_per_m` (M1).
    #[serde(default)]
    pub pixels: Option<usize>,
    /// Which end of the polyline pixel 0 sits at (M1).
    #[serde(default)]
    pub order: PixelOrder,
    /// The first pixel index along the polyline — a string cut at one end
    /// (M1).
    #[serde(default)]
    pub start: usize,
    /// The group a `venue.fixtures` node can drive on its own
    /// (`group = "front-wash"`), so one node drives all washes and another
    /// the heads (M3).
    #[serde(default)]
    pub group: Option<String>,
    /// A per-fixture universe override (M4.2); the profile's own when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub universe: Option<u16>,
    /// A per-fixture start address override (M4.2); the profile's own when
    /// `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u16>,
    /// Roll about the beam axis, degrees (a rotating gobo or barn doors).
    /// Appended 2026-10-07.
    #[serde(default)]
    pub roll: f32,
    /// How the floats reach the hardware: `dmx`, `sacn`, `usb`, `bluetooth`.
    /// A `bluetooth` fixture is driven by its profile (`wire = "bluetooth"`
    /// there too), not by a DMX universe.
    #[serde(default)]
    pub wire: String,
    #[serde(default)]
    pub drive: Drive,
    /// Whose frame `drive = "frame"` samples. Defaults to the first projector.
    #[serde(default)]
    pub projector: Option<String>,
    #[serde(default)]
    pub notes: String,
    /// A `FixtureKind::Matrix`'s grid, scan order and channel map (D4).
    /// `None` on every other kind. Appended 2026-10-15.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<Matrix>,
}

/// The 1-based DMX address a posted value lands at: a desk numbers a
/// universe's slots from 1 and a universe holds 512 of them, so an
/// out-of-range `address` clamps into `[1, 512]` rather than wrapping or
/// silently vanishing. `POST /venue { op: "fixture" }` is the one caller.
pub fn clamp_address(address: u16) -> u16 {
    address.clamp(1, 512)
}

impl Fixture {
    /// A fixture of `kind` with `id` and `name`, every optional field at its
    /// default: no pose yet, no profile, the kind's own wire, `drive = frame`.
    /// [`Venue::place`] and the settings page's "add" both start here, so a
    /// fixture's starting shape is written once.
    pub fn new(id: String, name: String, kind: FixtureKind) -> Fixture {
        Fixture {
            id,
            name,
            kind,
            profile: None,
            at: None,
            yaw: 0.0,
            pitch: -20.0,
            beam: 30.0,
            on: None,
            path: Vec::new(),
            closed: false,
            pixels_per_m: 0.0,
            pixels: None,
            order: PixelOrder::default(),
            start: 0,
            group: None,
            universe: None,
            address: None,
            roll: 0.0,
            wire: kind.default_wire().to_string(),
            drive: Drive::default(),
            projector: None,
            notes: String::new(),
            matrix: None,
        }
    }
}

impl Venue {
    /// The id a new fixture takes: `id` when it is free, otherwise the kind's
    /// word plus the next number free (`wash-1`, `wash-2`, …) — the naming
    /// [`Venue::place`] has always used, in one place now that both the
    /// editor's drop and the settings page's "add" need it.
    pub fn unique_fixture_id(&self, kind: FixtureKind, id: Option<String>) -> String {
        let id = id.unwrap_or_else(|| {
            let word = kind.word();
            let n = self.fixtures.iter().filter(|f| f.kind == kind).count() + 1;
            format!("{word}-{n}")
        });
        if self.fixtures.iter().any(|f| f.id == id) {
            let mut n = self.fixtures.len() + 1;
            loop {
                let candidate = format!("{}-{n}", kind.word());
                if !self.fixtures.iter().any(|f| f.id == candidate) {
                    break candidate;
                }
                n += 1;
            }
        } else {
            id
        }
    }

    /// Places a new fixture of `kind` at `at` (metres) and returns its id
    /// (M4). The editor's palette posts this; a profile-less fixture is still
    /// a valid entry, it just has nothing driving it yet. The id is unique
    /// among the venue's fixtures (`wash-1`, `wash-2`, …) unless one is given.
    pub fn place(&mut self, kind: FixtureKind, at: [f32; 3], id: Option<String>) -> String {
        let id = self.unique_fixture_id(kind, id);
        let mut fixture = Fixture::new(id.clone(), id.clone(), kind);
        fixture.at = Some(at);
        self.fixtures.push(fixture);
        id
    }

    /// Places any palette entry (M4.1) — a fixture, an object, a projector or
    /// a display — at `at`, and returns its id. The editor's palette posts
    /// the word this reads ([`FixtureKind::word`] for the fixtures).
    pub fn place_entry(&mut self, word: &str, at: [f32; 3]) -> Result<String, String> {
        if let Some(kind) = FixtureKind::from_word(word) {
            return Ok(self.place(kind, at, None));
        }
        let n = self.objects.len() + self.projectors.len() + self.displays.len() + 1;
        match word {
            "object" => {
                let id = format!("object-{n}");
                self.objects.push(Object {
                    id: id.clone(),
                    name: id.clone(),
                    kind: "box".to_string(),
                    at,
                    size: [0.6, 0.9, 0.6],
                    yaw: 0.0,
                    crown: None,
                    occludes: true,
                    emits: 0.0,
                    faces: Vec::new(),
                    description: String::new(),
                    form: ObjectForm::Box,
                    parent: None,
                    attribution: String::new(),
                    source: String::new(),
                });
                Ok(id)
            }
            "projector" => {
                let id = format!("projector-{n}");
                let on = self
                    .objects
                    .first()
                    .map(|o| o.id.clone())
                    .or_else(|| self.surfaces.first().map(|s| s.id.clone()))
                    .unwrap_or_default();
                self.projectors.push(Projector {
                    id: id.clone(),
                    name: id.clone(),
                    output: id.clone(),
                    on,
                    lens: default_lens(),
                    yaw: 0.0,
                    pitch: -20.0,
                    throw_ratio: 1.2,
                    aspect: default_aspect(),
                    lumens: None,
                    pins: Default::default(),
                    blend: Default::default(),
                });
                Ok(id)
            }
            "display" => {
                let id = format!("display-{n}");
                let surface = self
                    .surfaces
                    .first()
                    .map(|s| s.id.clone())
                    .unwrap_or_default();
                self.displays.push(Display {
                    id: id.clone(),
                    name: id.clone(),
                    kind: DisplayKind::Monitor,
                    output: id.clone(),
                    surface,
                    resolution: [1920, 1080],
                    link: Link::default(),
                    node: None,
                    notes: String::new(),
                });
                Ok(id)
            }
            other => Err(format!("no such kind {other:?}")),
        }
    }
}

/// What a fixture samples or shows of the frame — M1's one vocabulary for
/// every emitter and sensor. A pose (`at`, `yaw`, `pitch`, `roll`) plus the
/// shape it takes: a point, a line, a polyline of pixels, or a camera's
/// sensor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Footprint {
    /// Where it hangs (or stands), metres.
    pub at: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub shape: Shape,
}

/// The shape of a [`Footprint`]: how much of the frame the entry takes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "shape")]
pub enum Shape {
    /// One sample point (a wash, a spot, a continuous light, a panel, a
    /// moving head's beam).
    Point,
    /// A line sampled as one colour (an LED bar).
    Line,
    /// A polyline sampled into N pixels (an LED string).
    Pixels { pixels: usize },
    /// A camera's sensor: a field of view and the capture it feeds.
    Sensor { fov: f32, capture: String },
}

/// Where a beam ray first lands (M2): a surface, an object face, or the
/// floor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    /// The surface/object-face id, or `"floor"`.
    pub what: String,
    /// Where it lands, metres.
    pub at: [f32; 3],
    pub distance: f32,
    /// The surface's own normal at the hit, oriented back toward the ray's
    /// origin (the side the light arrives from). Appended 2026-10-15 (D5).
    pub normal: [f32; 3],
}

/// A fixture's live state: what the show **commanded** and what the head has
/// actually reached (M2). The commanded values are the authored source (a
/// pan/tilt float, or `aim`); the actual ones lag them by a slew model built
/// from the profile's speed limit, because a head takes time to get there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixtureState {
    pub fixture: String,
    pub commanded: Pose,
    pub actual: Pose,
}

/// One pose of a fixture: the values a head holds (M2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub pan: f32,
    pub tilt: f32,
    pub rotation: f32,
    pub zoom: f32,
    pub dimmer: f32,
    pub rgb: [f32; 3],
}

impl FixtureState {
    /// A state at rest: both commanded and actual at the authored pose.
    pub fn new(f: &Fixture) -> FixtureState {
        let pose = Pose {
            pan: f.yaw,
            tilt: f.pitch,
            rotation: f.roll,
            zoom: f.beam,
            dimmer: 1.0,
            rgb: [1.0; 3],
        };
        FixtureState {
            fixture: f.id.clone(),
            commanded: pose,
            actual: pose,
        }
    }

    /// One frame of the slew model: the actual values move toward the
    /// commanded ones at the profile's speed (degrees per second), never
    /// overshooting.
    pub fn step(&mut self, limits: &crate::fixture::Limits, dt: f32) {
        let step = limits.speed * dt.max(0.0);
        self.actual.pan = slew(self.actual.pan, self.commanded.pan, step, true);
        self.actual.tilt = slew(self.actual.tilt, self.commanded.tilt, step, false);
        self.actual.rotation = slew(self.actual.rotation, self.commanded.rotation, step, true);
        self.actual.zoom = slew(self.actual.zoom, self.commanded.zoom, step, false);
        self.actual.dimmer = slew(
            self.actual.dimmer,
            self.commanded.dimmer,
            (dt.max(0.0) * 4.0).min(1.0),
            false,
        );
    }
}

/// Move `actual` toward `commanded` by at most `step`; `wrap` folds the
/// difference into ±180° so a pan takes the near way round.
fn slew(actual: f32, commanded: f32, step: f32, wrap: bool) -> f32 {
    let mut delta = commanded - actual;
    if wrap {
        delta = delta.rem_euclid(360.0);
        if delta > 180.0 {
            delta -= 360.0;
        }
    }
    if delta.abs() <= step {
        commanded
    } else {
        actual + step * delta.signum()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueMask {
    pub id: String,
    pub name: String,
    /// A surface id, or an object face (`counter.front`).
    pub on: String,
    pub points: Vec<[f32; 2]>,
    /// The solid colour the test projection paints it with.
    #[serde(default)]
    pub rgb: Option<[u8; 3]>,
    /// What should play inside it, in words.
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default = "one")]
    pub alpha: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
}

/// One disagreement the check found. `at` names the thing (`shot bar-a`,
/// `mask panel-left`) so a model can be told exactly what to fix.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    pub level: Level,
    pub at: String,
    pub what: String,
}

/// What [`Venue::check`] found, plus the three numbers the generation
/// experiment ranks answers by. Each is the worst case, `None` when nothing
/// was there to measure.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    pub problems: Vec<Problem>,
    /// Worst surface-corner reprojection error, image units.
    pub reprojection: Option<f32>,
    /// Worst relative error of a shot's projector distance.
    pub distance: Option<f32>,
    /// Worst disagreement on the projector's aim, degrees.
    pub angle: Option<f32>,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.problems
            .iter()
            .filter(|p| p.level == Level::Error)
            .count()
    }
    pub fn warnings(&self) -> usize {
        self.problems
            .iter()
            .filter(|p| p.level == Level::Warning)
            .count()
    }
}

/// Where a pointing yaw/pitch looks.
pub fn direction(yaw: f32, pitch: f32) -> Vec3 {
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    Vec3::new(y.sin() * p.cos(), p.sin(), -y.cos() * p.cos())
}

fn up_for(dir: Vec3, roll: f32) -> Vec3 {
    Quat::from_axis_angle(dir.normalize(), roll.to_radians()) * Vec3::Y
}

impl Camera {
    /// This camera as a view, so the same projection math `Escena` uses for
    /// projectors reads the camera's image space.
    pub fn view(&self, name: &str) -> View {
        let eye = Vec3::from(self.eye);
        let dir = direction(self.yaw, self.pitch);
        View {
            output: name.to_string(),
            eye: self.eye,
            target: (eye + dir).into(),
            up: up_for(dir, self.roll).into(),
            fov: self.fov,
            aspect: self.aspect,
        }
    }
}

fn spline_kind(kind: &str) -> SplineKind {
    match kind {
        "polyline" => SplineKind::Polyline,
        "bezier" => SplineKind::Bezier,
        _ => SplineKind::CatmullRom,
    }
}

fn path_tangent(path: &Curve3, t: f32) -> Vec3 {
    let h = 1.0e-3;
    let a = Vec3::from(path.eval((t - h).max(0.0)));
    let b = Vec3::from(path.eval((t + h).min(1.0)));
    (b - a).normalize_or_zero()
}

/// Parallel-transport frame at `s`, starting from `UP` (or `X` when the
/// tangent is parallel to it) and carried forward from the path's start.
fn parallel_frame(path: &Curve3, s: f32) -> (Vec3, Vec3, Vec3) {
    let s = s.clamp(0.0, 1.0);
    let origin = Vec3::from(path.eval(s));
    let t0 = path_tangent(path, 0.0);
    let reference = if t0.dot(Vec3::Y).abs() < 0.99 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let mut u = reference.cross(t0).normalize_or_zero();
    if u.length_squared() < 1.0e-12 {
        u = Vec3::X;
    }
    let mut v = t0.cross(u).normalize_or_zero();
    if v.length_squared() < 1.0e-12 {
        v = Vec3::Y;
    }
    let mut last = t0;
    let steps = 64;
    for i in 1..=steps {
        let t = path_tangent(path, s * i as f32 / steps as f32);
        if t.length_squared() > 1.0e-12 && last.length_squared() > 1.0e-12 {
            let q = Quat::from_rotation_arc(last, t);
            u = (q * u).normalize_or_zero();
            v = (q * v).normalize_or_zero();
            last = t;
        }
    }
    (origin, u, v)
}

/// The flattened profile point at `t` in `0..=1` by arc length. A closed
/// profile wraps back to its first point at `t = 1`.
fn profile_at(profile: &Curve2, closed: bool, t: f32) -> [f32; 2] {
    let mut pts = profile.flatten(1.0e-4);
    if pts.len() < 2 {
        return pts.first().copied().unwrap_or([0.0, 0.0]);
    }
    if closed {
        let first = pts[0];
        if pts.last() != Some(&first) {
            pts.push(first);
        }
    }
    let mut lengths = Vec::with_capacity(pts.len());
    lengths.push(0.0);
    let mut total = 0.0;
    for w in pts.windows(2) {
        let dx = w[1][0] - w[0][0];
        let dy = w[1][1] - w[0][1];
        total += (dx * dx + dy * dy).sqrt();
        lengths.push(total);
    }
    if total <= 1.0e-9 {
        return pts[0];
    }
    let target = t.clamp(0.0, 1.0) * total;
    for i in 1..pts.len() {
        if lengths[i] >= target {
            let span = lengths[i] - lengths[i - 1];
            let f = if span > 1.0e-12 {
                (target - lengths[i - 1]) / span
            } else {
                0.0
            };
            return [
                pts[i - 1][0] + (pts[i][0] - pts[i - 1][0]) * f,
                pts[i - 1][1] + (pts[i][1] - pts[i - 1][1]) * f,
            ];
        }
    }
    *pts.last().unwrap()
}

impl Surface {
    pub fn wall(&self) -> Wall {
        Wall {
            id: self.id.clone(),
            name: self.name.clone(),
            origin: self.origin,
            u: self.u,
            v: self.v,
            ..Default::default()
        }
    }

    /// Whether this surface is the original `origin + s·u + t·v` plane.
    /// Callers of [`Surface::wall`], ray queries and homographies must check
    /// this first: those operations only make sense on a plane.
    pub fn is_planar(&self) -> bool {
        matches!(self.form, SurfaceForm::Plane)
    }

    /// This surface's point at `(s, t)`. On a plane this is exactly
    /// `origin + s·u + t·v`; on a sweep it evaluates the path at `s`, places
    /// a parallel-transport frame there, and follows the flattened profile
    /// by arc length at `t`.
    pub fn at(&self, s: f32, t: f32) -> [f32; 3] {
        match &self.form {
            SurfaceForm::Plane => {
                let p = Vec3::from(self.origin) + Vec3::from(self.u) * s + Vec3::from(self.v) * t;
                p.to_array()
            }
            SurfaceForm::Swept {
                path,
                profile,
                closed,
                kind,
                ..
            } => {
                let path = Curve3 {
                    points: path.clone(),
                    closed: false,
                    kind: spline_kind(kind),
                };
                let profile = Curve2 {
                    points: profile.clone(),
                    closed: *closed,
                    kind: spline_kind(kind),
                };
                let (origin, u, v) = parallel_frame(&path, s);
                let p = profile_at(&profile, *closed, t);
                (origin + u * p[0] + v * p[1]).to_array()
            }
        }
    }

    /// The unit normal at `(s, t)`. A plane's is `normalize(cross(u, v))`;
    /// a sweep's is the cross of finite differences of [`Surface::at`].
    pub fn normal(&self, s: f32, t: f32) -> [f32; 3] {
        match &self.form {
            SurfaceForm::Plane => {
                let n = Vec3::from(self.u)
                    .cross(Vec3::from(self.v))
                    .normalize_or_zero();
                if n.length_squared() < 1.0e-12 {
                    [0.0, 1.0, 0.0]
                } else {
                    n.to_array()
                }
            }
            SurfaceForm::Swept { .. } => {
                let h = 1.0e-3;
                let ds = Vec3::from(self.at((s + h).min(1.0), t))
                    - Vec3::from(self.at((s - h).max(0.0), t));
                let dt = Vec3::from(self.at(s, (t + h).min(1.0)))
                    - Vec3::from(self.at(s, (t - h).max(0.0)));
                let n = ds.cross(dt).normalize_or_zero();
                if n.length_squared() < 1.0e-12 {
                    [0.0, 1.0, 0.0]
                } else {
                    n.to_array()
                }
            }
        }
    }

    /// A triangle mesh for this surface. A plane is two triangles; a sweep
    /// goes through `metal_geom::sweep`, falling back to a sampled grid if
    /// the path or profile is too degenerate to sweep.
    pub fn tessellate(&self, u_segments: usize, v_segments: usize) -> Mesh {
        match &self.form {
            SurfaceForm::Plane => {
                let o = Vec3::from(self.origin);
                let u = Vec3::from(self.u);
                let v = Vec3::from(self.v);
                let n = Vec3::from(self.normal(0.0, 0.0));
                let mut mesh = Mesh::new();
                mesh.positions = vec![
                    o.to_array(),
                    (o + u).to_array(),
                    (o + u + v).to_array(),
                    (o + v).to_array(),
                ];
                mesh.normals = vec![n.to_array(); 4];
                mesh.uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
                mesh.indices = vec![0, 1, 2, 0, 2, 3];
                mesh
            }
            SurfaceForm::Swept {
                path,
                profile,
                caps,
                closed,
                kind,
            } => {
                let path = Curve3 {
                    points: path.clone(),
                    closed: false,
                    kind: spline_kind(kind),
                };
                let profile = Curve2 {
                    points: profile.clone(),
                    closed: *closed,
                    kind: spline_kind(kind),
                };
                let mesh = metal_geom::sweep::sweep(
                    &path,
                    &profile,
                    SweepOptions {
                        caps: *caps,
                        closed: *closed,
                        segments: u_segments.max(2),
                    },
                );
                if !mesh.is_empty() {
                    return mesh;
                }
                let us = u_segments.max(1);
                let vs = v_segments.max(1);
                let mut mesh = Mesh::new();
                for j in 0..=vs {
                    let t = j as f32 / vs as f32;
                    for i in 0..=us {
                        let s = i as f32 / us as f32;
                        mesh.positions.push(self.at(s, t));
                        mesh.uvs.push([s, t]);
                    }
                }
                let row = us + 1;
                for j in 0..vs {
                    for i in 0..us {
                        let a = (j * row + i) as u32;
                        let b = a + 1;
                        let c = a + row as u32;
                        let d = c + 1;
                        mesh.indices.extend_from_slice(&[a, b, c, b, d, c]);
                    }
                }
                mesh.recompute_normals();
                mesh
            }
        }
    }

    /// Axis-aligned bounds of this surface in metres.
    pub fn bbox(&self) -> ([f32; 3], [f32; 3]) {
        if self.is_planar() {
            let o = Vec3::from(self.origin);
            let u = Vec3::from(self.u);
            let v = Vec3::from(self.v);
            let corners = [o, o + u, o + u + v, o + v];
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for c in corners {
                for i in 0..3 {
                    min[i] = min[i].min(c[i]);
                    max[i] = max[i].max(c[i]);
                }
            }
            return (min, max);
        }
        if let Some(b) = self.tessellate(32, 16).bbox() {
            return b;
        }
        let p = self.at(0.0, 0.0);
        (p, p)
    }
}

/// Rotate a local mesh by `yaw` about `+y`, then translate it to `at`.
fn place_mesh(m: &Mesh, at: [f32; 3], yaw: f32) -> Mesh {
    let q = Quat::from_rotation_y(-yaw.to_radians());
    let base = Vec3::from(at);
    let mut out = m.clone();
    for p in &mut out.positions {
        *p = (q * Vec3::from(*p) + base).to_array();
    }
    for n in &mut out.normals {
        *n = (q * Vec3::from(*n)).to_array();
    }
    out
}

/// Concatenate two meshes, offsetting the second one's indices.
fn concat_meshes(a: &Mesh, b: &Mesh) -> Mesh {
    let offset = a.positions.len() as u32;
    let mut out = a.clone();
    out.positions.extend_from_slice(&b.positions);
    out.normals.extend_from_slice(&b.normals);
    out.uvs.extend_from_slice(&b.uvs);
    out.indices.extend(b.indices.iter().map(|i| i + offset));
    out
}

impl Object {
    fn rotation(&self) -> Quat {
        Quat::from_rotation_y(-self.yaw.to_radians())
    }

    fn world(&self, local: Vec3) -> Vec3 {
        Vec3::from(self.at) + self.rotation() * local
    }

    /// The object's world-space triangle mesh in its declared `form`. Boxes
    /// keep the old extents: a cuboid centred on the base's vertical middle,
    /// then yawed about `at`. A crown rides along as a sphere.
    pub fn mesh(&self) -> Mesh {
        let mut mesh = match &self.form {
            ObjectForm::Box => {
                let mut m = metal_geom::solid::cuboid(self.size, true);
                for p in &mut m.positions {
                    p[1] += self.size[1] / 2.0;
                }
                place_mesh(&m, self.at, self.yaw)
            }
            ObjectForm::Swept {
                path,
                profile,
                caps,
                closed,
                kind,
            } => {
                let path = Curve3 {
                    points: path.clone(),
                    closed: false,
                    kind: spline_kind(kind),
                };
                let profile = if profile.is_empty() {
                    let [w, h, _] = self.size;
                    Curve2 {
                        points: vec![[-w / 2.0, 0.0], [w / 2.0, 0.0], [w / 2.0, h], [-w / 2.0, h]],
                        closed: true,
                        kind: SplineKind::Polyline,
                    }
                } else {
                    Curve2 {
                        points: profile.clone(),
                        closed: *closed,
                        kind: spline_kind(kind),
                    }
                };
                let local = metal_geom::sweep::sweep(
                    &path,
                    &profile,
                    SweepOptions {
                        caps: *caps,
                        closed: *closed,
                        ..SweepOptions::default()
                    },
                );
                place_mesh(&local, self.at, self.yaw)
            }
            ObjectForm::Mesh {
                positions,
                indices,
                uvs,
                normals,
            } => {
                let local = Mesh {
                    positions: positions.clone(),
                    normals: normals.clone(),
                    uvs: uvs.clone(),
                    indices: indices.clone(),
                };
                place_mesh(&local, self.at, self.yaw)
            }
            ObjectForm::Capsule { radius, height } => {
                let mut lower = metal_geom::solid::cylinder(*radius, *height, 16);
                for p in &mut lower.positions {
                    p[1] += height / 2.0;
                }
                let mut top = metal_geom::solid::sphere(*radius, 16);
                for p in &mut top.positions {
                    p[1] += height;
                }
                let local = concat_meshes(&lower, &top);
                place_mesh(&local, self.at, self.yaw)
            }
        };
        if let Some(c) = &self.crown {
            let mut crown = metal_geom::solid::sphere(c.radius, 16);
            for p in &mut crown.positions {
                p[0] += c.center[0];
                p[1] += c.center[1];
                p[2] += c.center[2];
            }
            mesh = concat_meshes(&mesh, &crown);
        }
        mesh
    }

    /// The world-space bounds of [`Object::mesh`]; `None` for an empty form.
    pub fn bbox(&self) -> Option<([f32; 3], [f32; 3])> {
        self.mesh().bbox()
    }

    /// Centre of the top face — where a projector stands.
    pub fn top(&self) -> Vec3 {
        self.world(Vec3::new(0.0, self.size[1], 0.0))
    }

    /// One box face as a surface, seen from outside the box, top-left origin.
    pub fn face(&self, name: &str) -> Option<Wall> {
        let [w, h, d] = self.size;
        let (hw, hd) = (w / 2.0, d / 2.0);
        let (origin, u, v) = match name {
            "front" => ([-hw, h, hd], [w, 0.0, 0.0], [0.0, -h, 0.0]),
            "back" => ([hw, h, -hd], [-w, 0.0, 0.0], [0.0, -h, 0.0]),
            "left" => ([-hw, h, -hd], [0.0, 0.0, d], [0.0, -h, 0.0]),
            "right" => ([hw, h, hd], [0.0, 0.0, -d], [0.0, -h, 0.0]),
            "top" => ([-hw, h, -hd], [w, 0.0, 0.0], [0.0, 0.0, d]),
            _ => return None,
        };
        let r = self.rotation();
        Some(Wall {
            id: format!("{}.{name}", self.id),
            name: format!("{} ({name})", self.name),
            origin: self.world(Vec3::from(origin)).into(),
            u: (r * Vec3::from(u)).into(),
            v: (r * Vec3::from(v)).into(),
            ..Default::default()
        })
    }

    /// Distance along `dir` (unit) from `from` to where the ray first enters
    /// this object, if it does. A box with a zero dimension (a pool drawn as
    /// a footprint) blocks nothing.
    pub fn hit(&self, from: Vec3, dir: Vec3) -> Option<f32> {
        let mut best: Option<f32> = None;
        if self.size.iter().all(|s| *s > 0.0) {
            let inv = self.rotation().inverse();
            let o = inv * (from - Vec3::from(self.at));
            let d = inv * dir;
            let lo = Vec3::new(-self.size[0] / 2.0, 0.0, -self.size[2] / 2.0);
            let hi = Vec3::new(self.size[0] / 2.0, self.size[1], self.size[2] / 2.0);
            let (mut t0, mut t1) = (f32::NEG_INFINITY, f32::INFINITY);
            let mut inside = true;
            for i in 0..3 {
                if d[i].abs() < 1e-9 {
                    if o[i] < lo[i] || o[i] > hi[i] {
                        inside = false;
                        break;
                    }
                } else {
                    let (a, b) = ((lo[i] - o[i]) / d[i], (hi[i] - o[i]) / d[i]);
                    t0 = t0.max(a.min(b));
                    t1 = t1.min(a.max(b));
                }
            }
            if inside && t0 <= t1 && t1 > 0.0 {
                best = Some(t0.max(0.0));
            }
        }
        if let Some(c) = &self.crown {
            let oc = from - Vec3::from(c.center);
            let b = oc.dot(dir);
            let disc = b * b - (oc.length_squared() - c.radius * c.radius);
            if disc >= 0.0 {
                let t = -b - disc.sqrt();
                let t = if t > 0.0 { t } else { -b + disc.sqrt() };
                if t > 0.0 {
                    best = Some(best.map_or(t, |x| x.min(t)));
                }
            }
        }
        best
    }
}

/// One cell of a projected split, in the surface's own `(s, t)` — the wall's
/// `0..1` rectangle.
///
/// `n == 1` is the whole rectangle; anything else is the `n`-way fan the plan
/// asks for ("three generators, three radii from the centre"). The fan is `n`
/// rays from the rectangle's centre, `TAU / n` apart starting on the
/// top-right diagonal, so its cells are *angular* wedges: each traced from
/// the centre out along the rectangle, between two neighbouring rays.
///
/// Every cell is the rectangle clipped by the `n` half-planes, "left of ray
/// `i`" and "right of every other ray" — the intersection of half-planes,
/// which is what [`clip`] does. Because ray `k` in one cell and ray `k` in its
/// neighbour are the same numbers, neighbours share their seam exactly and
/// the cells cover the wall.
fn wedge(n: usize, i: usize) -> Vec<[f32; 2]> {
    const C: [f32; 2] = [0.5, 0.5];
    let n = n.max(1);
    if n == 1 {
        return SURFACE_CORNERS.to_vec();
    }
    if n == 2 {
        // Two halves: the vertical through the centre. A fan of two rays is
        // a line, and the two cells it makes are its two sides.
        let mut half: Vec<[f32; 2]> = SURFACE_CORNERS.to_vec();
        half = clip(&half, |p| if i == 0 { p[0] - C[0] } else { C[0] - p[0] });
        return half;
    }
    // A ray's direction, indexed by ring step and wrapped, so the ray that
    // closes the fan is the first one again, bit for bit.
    let ray = |k: usize| {
        let a = std::f32::consts::FRAC_PI_4 + (k % n) as f32 * (std::f32::consts::TAU / n as f32);
        [a.cos(), a.sin()]
    };
    let (a, b) = (ray(i), ray(i + 1));
    // Turning from `a` to `b` is the clockwise wedge (f32 screen angles grow
    // clockwise). `cross(a, p)` test for the leading edge and `cross(b, p)` for
    // the trailing one.
    let mut cell: Vec<[f32; 2]> = SURFACE_CORNERS.to_vec();
    cell = clip(&cell, |p| (p[0] - C[0]) * b[1] - (p[1] - C[1]) * b[0]);
    if cell.len() < 3 {
        return Vec::new();
    }
    cell = clip(&cell, |p| -((p[0] - C[0]) * a[1] - (p[1] - C[1]) * a[0]));
    cell
}

/// One convex polygon against the half-plane `side(p) <= 0`,
/// `Sutherland–Hodgman`. An endpoint exactly on the line is kept once and
/// yields no crossing point: re-emitting it would repeat a vertex, and a
/// polygon with a repeated point has a zero-area winding and reads as empty.
fn clip(poly: &[[f32; 2]], side: impl Fn([f32; 2]) -> f32) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(poly.len() + 2);
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (sa, sb) = (side(a), side(b));
        if sa <= 0.0 {
            out.push(a);
        }
        if (sa < 0.0 && sb > 0.0) || (sa > 0.0 && sb < 0.0) {
            let k = sa / (sa - sb);
            out.push([a[0] + k * (b[0] - a[0]), a[1] + k * (b[1] - a[1])]);
        }
    }
    out
}

/// A drawn streak through two points: the point the line passes through and
/// its unit normal. `None` for a zero-length pair, which draws nothing — the
/// caller reads it as a degenerate drawing.
fn streak(a: [f32; 2], b: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    (len > 1e-6).then(|| (a, [-d[1] / len, d[0] / len]))
}

/// Whether a streak crosses the unit square's interior: some corner strictly
/// on each side of its line. One running along an edge, or wholly outside,
/// splits nothing.
fn cuts_square((head, n): &([f32; 2], [f32; 2])) -> bool {
    let side = |p: [f32; 2]| (p[0] - head[0]) * n[0] + (p[1] - head[1]) * n[1];
    let sides: Vec<f32> = SURFACE_CORNERS.iter().map(|p| side(*p)).collect();
    sides.iter().any(|s| *s > 1e-6) && sides.iter().any(|s| *s < -1e-6)
}

/// The unit square cut into `lines.len() + 1` convex cells by the given
/// streaks, in drawn order: cell `j` is the part of the square on the near
/// side of every streak before `j` and on the far side of every streak from
/// `j` on. Every cell is the intersection of half-planes, so it stays convex
/// and neighbouring cells share their cut edges exactly; the cells cover the
/// square.
fn slabs(lines: &[([f32; 2], [f32; 2])]) -> Vec<Vec<[f32; 2]>> {
    let sign = |p: [f32; 2], (head, n): &([f32; 2], [f32; 2])| {
        (p[0] - head[0]) * n[0] + (p[1] - head[1]) * n[1]
    };
    let mut out = Vec::with_capacity(lines.len() + 1);
    for j in 0..=lines.len() {
        let mut cell: Vec<[f32; 2]> = SURFACE_CORNERS.to_vec();
        for (k, line) in lines.iter().enumerate() {
            // Near of the streaks before `j`, far of `j` and after: flipping
            // the sign for the far ones lets one clip keep the same half.
            let s = if k < j { 1.0 } else { -1.0 };
            cell = clip(&cell, |p| s * sign(p, line));
            if cell.len() < 3 {
                break;
            }
        }
        if cell.len() >= 3 {
            out.push(cell);
        }
    }
    out
}

/// Where a ray crosses a wall inside its edges: `(distance, s, t)`.
fn ray_wall(from: Vec3, dir: Vec3, wall: &Wall) -> Option<(f32, f32, f32)> {
    let (o, u, v) = (
        Vec3::from(wall.origin),
        Vec3::from(wall.u),
        Vec3::from(wall.v),
    );
    let n = u.cross(v);
    let denom = dir.dot(n);
    if denom.abs() < 1e-9 {
        return None;
    }
    let distance = (o - from).dot(n) / denom;
    if distance <= 1e-4 {
        return None;
    }
    let (s, t, _) = wall.nearest(from + dir * distance);
    ((-1e-4..=1.0001).contains(&s) && (-1e-4..=1.0001).contains(&t)).then_some((distance, s, t))
}

/// Keep the nearer of `best` and `h` (D5).
fn keep(best: &mut Option<Hit>, h: Hit) {
    if best.as_ref().is_none_or(|b| h.distance < b.distance) {
        *best = Some(h);
    }
}

/// A wall's own normal, oriented back toward the ray's origin so it is the
/// side the light arrives from.
fn facing(wall: &Wall, dir: Vec3) -> [f32; 3] {
    let n = Vec3::from(wall.u).cross(Vec3::from(wall.v));
    if n.length() < 1e-9 {
        return [0.0, 0.0, 1.0];
    }
    let n = n.normalize();
    let n = if n.dot(dir) > 0.0 { -n } else { n };
    n.into()
}

/// Ray/triangle (Möller–Trumbore): the distance to the first front hit of
/// the triangle `a, b, c`, or `None` when it misses or is behind the origin.
fn ray_triangle(from: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = from - a;
    let u = s.dot(p) * inv;
    if !(-1e-5..=1.0 + 1e-5).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < -1e-5 || u + v > 1.0 + 1e-5 {
        return None;
    }
    let distance = e2.dot(q) * inv;
    (distance > 1e-4).then_some(distance)
}

/// The first front hit of a wall's rectangle, as two ray/triangle tests.
fn face_hit(from: Vec3, dir: Vec3, wall: &Wall) -> Option<f32> {
    let (p00, p10, p11, p01) = (
        wall.at(0.0, 0.0),
        wall.at(1.0, 0.0),
        wall.at(1.0, 1.0),
        wall.at(0.0, 1.0),
    );
    ray_triangle(from, dir, p00, p10, p11).or_else(|| ray_triangle(from, dir, p00, p11, p01))
}

fn in_frame(p: [f32; 2]) -> bool {
    (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1])
}

/// Where cell `(x, y)` of a matrix sits on its surface, in `(s, t)`: the
/// middle of the cell. A `path` of four corners makes the panel that quad,
/// bilinearly interpolated; with no path the panel is the whole surface.
fn matrix_st(m: &Matrix, f: &Fixture, x: usize, y: usize) -> (f32, f32) {
    let u = (x as f32 + 0.5) / m.x_pixels as f32;
    let v = (y as f32 + 0.5) / m.y_pixels as f32;
    if f.path.len() >= 4 {
        let (p0, p1, p2, p3) = (f.path[0], f.path[1], f.path[2], f.path[3]);
        let (w00, w10, w11, w01) = ((1.0 - u) * (1.0 - v), u * (1.0 - v), u * v, (1.0 - u) * v);
        (
            p0[0] * w00 + p1[0] * w10 + p2[0] * w11 + p3[0] * w01,
            p0[1] * w00 + p1[1] * w10 + p2[1] * w11 + p3[1] * w01,
        )
    } else {
        (u, v)
    }
}

/// Angle difference folded into `0..180`.
fn angle_gap(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// A surface's four corners in its own `(s, t)`, in the order `(0,0) (1,0)
/// (1,1) (0,1)` — what a projector's `pins` are a calibration of, and the `st`
/// side of [`Homography::from_corners`].
const SURFACE_CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// Whether two walls lie in the same plane: parallel normals and no offset
/// along them. What lets a projector's pins on one surface stand in for the
/// correspondences of a coplanar neighbour when solving one `View`.
fn coplanar(a: &Wall, b: &Wall) -> bool {
    let (na, nb) = (
        Vec3::from(a.u).cross(Vec3::from(a.v)),
        Vec3::from(b.u).cross(Vec3::from(b.v)),
    );
    if na.length() < 1e-9 || nb.length() < 1e-9 {
        return false;
    }
    let (na, nb) = (na.normalize(), nb.normalize());
    if na.cross(nb).length() > 1e-3 {
        return false;
    }
    (Vec3::from(b.origin) - Vec3::from(a.origin)).dot(na).abs() < 1e-3
}

impl Projector {
    /// Vertical field of view from the throw ratio and aspect.
    pub fn fov(&self) -> f32 {
        let half_w = 0.5 / self.throw_ratio;
        (2.0 * (half_w / self.aspect).atan()).to_degrees()
    }
}

impl Venue {
    pub fn parse(text: &str) -> Result<Venue, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// P3: write this venue back to `data/venues/<id>.toml` (the scratch dir
    /// under test, rule 8) through [`crate::atomic::write`], so a crash cannot
    /// leave a half-written venue. The id has to be a venue id
    /// ([`load`]'s rule) so a `POST /venue` body cannot name a path.
    pub fn save(&self, root: &Path) -> Result<(), String> {
        let id = self.venue.id.as_str();
        if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
            return Err(format!("{id:?} is not a venue id"));
        }
        let dir = dir(root);
        metal_vfs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        crate::atomic::write(&dir.join(format!("{id}.toml")), text).map_err(|e| e.to_string())
    }

    /// Every surface masks can land on: the listed surfaces, then each
    /// object's listed faces.
    pub fn walls(&self) -> Vec<Wall> {
        let mut out: Vec<Wall> = self
            .surfaces
            .iter()
            .filter(|s| s.is_planar())
            .map(Surface::wall)
            .collect();
        for o in &self.objects {
            out.extend(o.faces.iter().filter_map(|f| o.face(f)));
        }
        out
    }

    pub fn object(&self, id: &str) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn projector(&self, id: &str) -> Option<&Projector> {
        self.projectors.iter().find(|p| p.id == id)
    }

    /// The lens position: the top of the object it stands on, plus `lens`.
    pub fn lens(&self, p: &Projector) -> Result<Vec3, String> {
        let table = self.object(&p.on).ok_or_else(|| {
            format!(
                "projector {} stands on {:?}, which is not an object",
                p.id, p.on
            )
        })?;
        Ok(table.top() + table.rotation() * Vec3::from(p.lens))
    }

    pub fn view(&self, p: &Projector) -> Result<View, String> {
        let eye = self.lens(p)?;
        let dir = direction(p.yaw, p.pitch);
        Ok(View {
            output: p.output.clone(),
            eye: eye.into(),
            target: (eye + dir).into(),
            up: up_for(dir, 0.0).into(),
            fov: p.fov(),
            aspect: p.aspect,
        })
    }

    /// P2.4: the [`View`] a projector's pins for one surface imply, by
    /// decomposing the homography the four pins define. The inverse of
    /// [`Homography::from_view`], so a wall that only has a dragged `fit`
    /// becomes a camera the rest of the scene can reason about.
    pub fn view_from_fit(&self, output: &str, surface: &str) -> Option<View> {
        let projector = self.projectors.iter().find(|p| p.output == output)?;
        let wall = self.walls().into_iter().find(|w| w.id == surface)?;
        let pins = projector.pins.get(surface)?;
        let h = Homography::from_corners(&SURFACE_CORNERS, pins)?;
        let mut view = h.to_view(&wall, projector.fov(), projector.aspect)?;
        view.output = projector.output.clone();
        Some(view)
    }

    /// P2.4: one [`View`] from every fit that bears on `surface` — this
    /// projector's pins on the surface and on any coplanar neighbour — solved
    /// as one least-squares homography and decomposed once. A surface split in
    /// two, or a projector that only pinned the neighbouring panel, still
    /// yields a single camera.
    ///
    /// TODO(P2.4): this is the coplanar least-squares homography plus one
    /// decomposition the plan allows in place of full PnP. A genuine multi-view
    /// bundle refinement (re-solving the pose against every correspondence at
    /// once, rather than collapsing them into one planar map first) is not
    /// done; the single-fit and several-coplanar-fit cases are.
    pub fn view_from_fits(&self, output: &str, surface: &str) -> Option<View> {
        let projector = self.projectors.iter().find(|p| p.output == output)?;
        let walls = self.walls();
        let target = walls.iter().find(|w| w.id == surface)?.clone();
        let (mut st, mut xy): (Vec<[f32; 2]>, Vec<[f32; 2]>) = (Vec::new(), Vec::new());
        for (sid, pins) in &projector.pins {
            let Some(w) = walls.iter().find(|w| w.id == *sid) else {
                continue;
            };
            if w.id != target.id && !coplanar(&target, w) {
                continue;
            }
            for (corner, pin) in SURFACE_CORNERS.iter().zip(pins.iter()) {
                let (s, t, d) = target.nearest(w.at(corner[0], corner[1]));
                if d > 1e-3 {
                    continue;
                }
                st.push([s, t]);
                xy.push(*pin);
            }
        }
        let h = Homography::from_correspondences(&st, &xy)?;
        let mut view = h.to_view(&target, projector.fov(), projector.aspect)?;
        view.output = projector.output.clone();
        Some(view)
    }

    /// Where the projector's axis lands: `(surface id, distance, s, t)`.
    pub fn throw(&self, p: &Projector) -> Result<Option<(String, f32, f32, f32)>, String> {
        let eye = self.lens(p)?;
        Ok(self.first_wall(eye, direction(p.yaw, p.pitch)))
    }

    fn first_wall(&self, from: Vec3, dir: Vec3) -> Option<(String, f32, f32, f32)> {
        self.walls()
            .iter()
            .filter_map(|w| ray_wall(from, dir, w).map(|(d, s, t)| (w.id.clone(), d, s, t)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The first occluding object between `from` and `to`, skipping
    /// `ignore` (the table a projector stands on).
    fn blocked(&self, from: Vec3, to: Vec3, ignore: &str) -> Option<&Object> {
        let span = to - from;
        let (len, dir) = (span.length(), span.normalize());
        self.objects
            .iter()
            .filter(|o| o.occludes && o.id != ignore)
            .filter_map(|o| o.hit(from, dir).map(|t| (o, t)))
            .filter(|(_, t)| *t < len - 0.02)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(o, _)| o)
    }

    /// The venue as a scene: every surface and listed face as a wall, one
    /// view per projector, every mask as a wall mask.
    pub fn scene(&self) -> Result<Escena, String> {
        let walls = self.walls();
        let mut views = Vec::with_capacity(self.projectors.len());
        for p in &self.projectors {
            views.push(self.view(p)?);
        }
        // A mask on a display's glass belongs to that display's own output
        // (identity, see [`Venue::project`]), not to whichever projector's
        // frame happens to cover the TV.
        let masks = self
            .masks
            .iter()
            .filter(|m| !self.displays.iter().any(|d| d.surface == m.on))
            // A mask on a curved surface is projected by `project_surface`;
            // the planar `Escena` cannot represent it.
            .filter(|m| {
                self.surfaces
                    .iter()
                    .find(|s| s.id == m.on)
                    .is_none_or(|s| s.is_planar())
            })
            .map(|m| {
                if !walls.iter().any(|w| w.id == m.on) {
                    return Err(format!(
                        "mask {} is on {:?}, which is not a surface",
                        m.id, m.on
                    ));
                }
                Ok(WallMask {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    wall: m.on.clone(),
                    points: m.points.clone(),
                    parent: m.parent.clone(),
                    alpha: m.alpha,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Escena {
            walls,
            views,
            masks,
        })
    }

    /// The masks one output gets: the scene projected through that output's
    /// projector, or — for an output that is a [`Display`] — the masks on
    /// its glass, point for point, because a screen's pixels are its surface.
    pub fn project(&self, output: &str) -> Result<Vec<Mask>, String> {
        let mut out = self.scene()?.project(output);
        for d in self.displays.iter().filter(|d| d.output == output) {
            out.extend(
                self.masks
                    .iter()
                    .filter(|m| m.on == d.surface)
                    .map(|m| Mask {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        points: m.points.clone(),
                        parent: m.parent.clone(),
                        alpha: m.alpha,
                        ..Mask::default()
                    }),
            );
        }
        // Curved surfaces have no wall plane for `Escena` to project, so
        // their silhouette becomes an ordinary `Mask` here.
        for s in self.surfaces.iter().filter(|s| !s.is_planar()) {
            out.extend(self.project_surface(output, &s.id)?);
        }
        Ok(out)
    }

    /// The outline of a non-planar surface as an ordinary [`Mask`] for
    /// `output`. Planar surfaces return nothing — [`Venue::project`]'s
    /// `Escena` path already handles them. A display's own curved glass is
    /// identity-mapped; that display's output is the only one it contributes
    /// to.
    pub fn project_surface(&self, output: &str, surface: &str) -> Result<Vec<Mask>, String> {
        let Some(surf) = self.surfaces.iter().find(|s| s.id == surface) else {
            return Ok(Vec::new());
        };
        if surf.is_planar() {
            return Ok(Vec::new());
        }
        // A screen's pixels are its surface: identity, point for point.
        if let Some(d) = self.displays.iter().find(|d| d.surface == surface) {
            if d.output != output {
                return Ok(Vec::new());
            }
            let mut out = vec![Mask {
                id: format!("{surface}.projected"),
                name: surface.to_string(),
                points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                ..Mask::default()
            }];
            out.extend(self.masks.iter().filter(|m| m.on == surface).map(|m| Mask {
                id: m.id.clone(),
                name: m.name.clone(),
                points: m.points.clone(),
                parent: m.parent.clone(),
                alpha: m.alpha,
                ..Mask::default()
            }));
            return Ok(out);
        }
        let Some(projector) = self.projectors.iter().find(|p| p.output == output) else {
            return Ok(Vec::new());
        };
        let view = self.view(projector)?;
        let (us, vs) = (32usize, 16usize);
        let mut ring: Vec<(f32, f32)> = Vec::new();
        for i in 0..us {
            ring.push((i as f32 / us as f32, 0.0));
        }
        for j in 0..vs {
            ring.push((1.0, j as f32 / vs as f32));
        }
        for i in (1..=us).rev() {
            ring.push((i as f32 / us as f32, 1.0));
        }
        for j in (1..=vs).rev() {
            ring.push((0.0, j as f32 / vs as f32));
        }
        let mut points: Vec<[f32; 2]> = Vec::new();
        for (s, t) in ring {
            if let Some(v) = view.project(Vec3::from(surf.at(s, t))) {
                let q = [v.x, v.y];
                let same = points
                    .last()
                    .is_some_and(|l| (l[0] - q[0]).abs() < 1.0e-6 && (l[1] - q[1]).abs() < 1.0e-6);
                if !same {
                    points.push(q);
                }
            }
        }
        if points.len() > 1 {
            let (a, b) = (points[0], *points.last().unwrap());
            if (a[0] - b[0]).abs() < 1.0e-6 && (a[1] - b[1]).abs() < 1.0e-6 {
                points.pop();
            }
        }
        if points.len() < 3 {
            return Ok(Vec::new());
        }
        let mut out = vec![Mask {
            id: format!("{surface}.projected"),
            name: surface.to_string(),
            points,
            ..Mask::default()
        }];
        // Each contour drawn on the curved surface, its `(s, t)` points run
        // through the same 3D→device projection as the surface's outline.
        for m in self.masks.iter().filter(|m| m.on == surface) {
            let mut mp: Vec<[f32; 2]> = Vec::new();
            for p in &m.points {
                if let Some(v) = view.project(Vec3::from(surf.at(p[0], p[1]))) {
                    mp.push([v.x, v.y]);
                }
            }
            if mp.len() >= 3 {
                out.push(Mask {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    points: mp,
                    parent: m.parent.clone(),
                    alpha: m.alpha,
                    ..Mask::default()
                });
            }
        }
        Ok(out)
    }

    /// The tessellated mesh of a named surface, for the viewport and warp.
    /// `None` for an id the venue does not know.
    pub fn surface_mesh(&self, surface: &str, u: usize, v: usize) -> Option<Mesh> {
        self.surfaces
            .iter()
            .find(|s| s.id == surface)
            .map(|s| s.tessellate(u, v))
    }

    /// P3: the mesh warp a non-planar surface needs, in device `0..1`.
    ///
    /// Planar surfaces return `None` — they go through
    /// [`surface_homography`](Self::surface_homography), whose exact
    /// homography path stays byte-identical. A display's output is the
    /// identity mesh: its pixels are its surface's own `(s, t)`.
    ///
    /// The surface is tessellated over `u × v` quads, two triangles each.
    /// Every vertex is projected with the projector's [`View`]; a triangle
    /// with any vertex behind the eye is dropped rather than dragged across
    /// the frame. `uv` is `(s, t)`, so the compositor can sample the source
    /// exactly as it does through a homography.
    pub fn warp_mesh(
        &self,
        output: &str,
        surface: &str,
        u: usize,
        v: usize,
    ) -> Option<compose::WarpMesh> {
        let (us, vs) = (u.max(2), v.max(2));

        // A display's glass is lit directly: device 0..1 *is* its (s, t).
        if self.displays.iter().any(|d| d.output == output) {
            let mut mesh = compose::WarpMesh::default();
            for j in 0..vs {
                for i in 0..us {
                    let (s0, s1) = (i as f32 / us as f32, (i + 1) as f32 / us as f32);
                    let (t0, t1) = (j as f32 / vs as f32, (j + 1) as f32 / vs as f32);
                    let d = [[s0, t0], [s1, t0], [s1, t1], [s0, t1]];
                    let uv = d;
                    mesh.tris.push(compose::WarpTri {
                        d: [d[0], d[1], d[2]],
                        uv: [uv[0], uv[1], uv[2]],
                    });
                    mesh.tris.push(compose::WarpTri {
                        d: [d[0], d[2], d[3]],
                        uv: [uv[0], uv[2], uv[3]],
                    });
                }
            }
            return (!mesh.is_empty()).then_some(mesh);
        }

        let surf = self.surfaces.iter().find(|s| s.id == surface)?;
        if surf.is_planar() {
            return None;
        }
        let projector = self.projectors.iter().find(|p| p.output == output)?;
        let view = self.view(projector).ok()?;

        // Tessellate to respect the surface's own sweep, then project the
        // grid vertices. `at` is the authoritative point for each (s, t),
        // the same one the fallback grid in `tessellate` uses.
        let _ = surf.tessellate(us, vs);
        let mut grid: Vec<Option<[f32; 2]>> = vec![None; (us + 1) * (vs + 1)];
        for j in 0..=vs {
            let t = j as f32 / vs as f32;
            for i in 0..=us {
                let s = i as f32 / us as f32;
                grid[j * (us + 1) + i] =
                    view.project(Vec3::from(surf.at(s, t))).map(|p| [p.x, p.y]);
            }
        }
        let at = |i: usize, j: usize| grid[j * (us + 1) + i];
        let uv = |i: usize, j: usize| [i as f32 / us as f32, j as f32 / vs as f32];
        let mut mesh = compose::WarpMesh::default();
        for j in 0..vs {
            for i in 0..us {
                for (a, b, c) in [
                    ((i, j), (i + 1, j), (i + 1, j + 1)),
                    ((i, j), (i + 1, j + 1), (i, j + 1)),
                ] {
                    let (Some(da), Some(db), Some(dc)) = (at(a.0, a.1), at(b.0, b.1), at(c.0, c.1))
                    else {
                        continue;
                    };
                    mesh.tris.push(compose::WarpTri {
                        d: [da, db, dc],
                        uv: [uv(a.0, a.1), uv(b.0, b.1), uv(c.0, c.1)],
                    });
                }
            }
        }
        (!mesh.is_empty()).then_some(mesh)
    }

    /// Which surface a point in the room is nearest, and where on it —
    /// [`Escena::nearest_wall`] read straight off the venue's surfaces and
    /// object faces. Returns `(surface id, s, t, distance)`, and only counts
    /// surfaces the point is actually in front of (`0..1` in both `s` and
    /// `t`): a body in the middle of the room is metres from the far wall's
    /// plane but not in front of it, and a spotlight that jumped to whichever
    /// plane happened to be closest would land on a wall nobody is facing.
    pub fn nearest_wall(&self, p: [f32; 3]) -> Option<(String, f32, f32, f32)> {
        let p = Vec3::from(p);
        self.walls()
            .into_iter()
            .map(|w| {
                let (s, t, d) = w.nearest(p);
                (w.id, s, t, d)
            })
            .filter(|(_, s, t, _)| (0.0..=1.0).contains(s) && (0.0..=1.0).contains(t))
            .min_by(|a, b| a.3.total_cmp(&b.3))
    }

    /// A contour that follows a body: a box of `size` (in surface units)
    /// around wherever `body` is standing, on whichever surface they are in
    /// front of — [`Escena::spotlight`] as a [`VenueMask`] rather than a
    /// `WallMask`, since the venue's contours live in `VenueMask`.
    pub fn spotlight(&self, id: &str, body: [f32; 3], size: (f32, f32)) -> Option<VenueMask> {
        let (on, s, t, _) = self.nearest_wall(body)?;
        let (hw, hh) = (size.0 / 2.0, size.1 / 2.0);
        Some(VenueMask {
            id: id.to_string(),
            name: id.to_string(),
            on,
            points: vec![
                [s - hw, t - hh],
                [s + hw, t - hh],
                [s + hw, t + hh],
                [s - hw, t + hh],
            ],
            rgb: None,
            content: String::new(),
            parent: None,
            alpha: 1.0,
        })
    }

    /// The warp from `surface`'s `(s, t)` to `output`'s 0..1 —
    /// [`Escena::homography`] with a projector's [`Projector::pins`] standing
    /// in for a wall's `fit`.
    ///
    /// **A view wins over a pin set**, the same rule and for the same reason:
    /// the projector's pose is exact geometry, a pin set is four points
    /// somebody dragged, and when the two disagree it is the drag that is
    /// stale. The disagreement is measured with [`Homography::distance`] and
    /// reported on stderr rather than silently swallowed, so a calibration
    /// that has drifted from the venue's geometry says so.
    ///
    /// `None` when the venue knows of neither a projector for `output` nor
    /// pins on `surface`.
    pub fn homography(&self, output: &str, surface: &str) -> Option<Homography> {
        if self
            .surfaces
            .iter()
            .find(|s| s.id == surface)
            .is_some_and(|s| !s.is_planar())
        {
            return None;
        }
        let wall = self.walls().into_iter().find(|w| w.id == surface)?;
        let projector = self.projectors.iter().find(|p| p.output == output);
        let from_pins = projector
            .and_then(|p| p.pins.get(surface))
            .and_then(|pins| Homography::from_corners(&SURFACE_CORNERS, pins));
        // P2.4: with no 3D pose to read, a pin set can still be decomposed into
        // one, so "a `View` wins" keeps holding without a projector at all.
        let from_fit_view = projector
            .and_then(|p| self.view_from_fits(&p.output, surface))
            .map(|v| Homography::from_view(&v, &wall))
            .filter(|h| h.inverse().is_some());
        let from_view = projector
            .and_then(|p| self.view(p).ok())
            .map(|v| Homography::from_view(&v, &wall))
            .filter(|h| h.inverse().is_some())
            .or(from_fit_view);
        match (from_view, from_pins) {
            (Some(view), Some(fit)) => {
                if let Some(d) = view.distance(&fit) {
                    if d > crate::escena::WARN_FIT_DISTANCE {
                        eprintln!(
                            "  venue: view and pins for output {output:?} surface {surface:?} differ by {d:.4}; using the view"
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

    /// P2.4: the warp from `surface`'s `(s, t)` to `output`'s 0..1 for a
    /// `venue.surface` node. A projector's hand-dragged [`Projector::pins`]
    /// win: a pin set is what an operator tuned against the real wall, and the
    /// surface node is the one place that calibration is asked for.
    ///
    /// When there are no pins **on this surface**, the fit is still enough:
    /// [`Venue::view_from_fits`] recovers one camera from the pins that do
    /// exist on it or on a coplanar neighbour, and its exact geometry is used
    /// ([`Homography::from_view`]). There is deliberately **no whole-wall
    /// fallback** — a surface node with nothing to calibrate it maps nothing,
    /// and the engine draws no warp rather than the projector's whole frame
    /// (P2 item 3).
    pub fn surface_homography(&self, output: &str, surface: &str) -> Option<Homography> {
        let wall = self.walls().into_iter().find(|w| w.id == surface)?;
        let projector = self.projectors.iter().find(|p| p.output == output);
        if let Some(pins) = projector.and_then(|p| p.pins.get(surface)) {
            if let Some(h) = Homography::from_corners(&SURFACE_CORNERS, pins) {
                return Some(h);
            }
        }
        let view = self.view_from_fits(output, surface)?;
        let h = Homography::from_view(&view, &wall);
        h.inverse().is_some().then_some(h)
    }

    pub fn display(&self, id: &str) -> Option<&Display> {
        self.displays.iter().find(|d| d.id == id)
    }

    pub fn shot(&self, id: &str) -> Option<&Shot> {
        self.shots.iter().find(|s| s.id == id)
    }

    pub fn sensor(&self, id: &str) -> Option<&Sensor> {
        self.sensors.iter().find(|s| s.id == id)
    }

    /// Where the viewer stands, in venue metres, from a normalized eyes
    /// reading (H2.2).
    ///
    /// `eye` is `[eye_x, eye_y, eye_z_est]` in `0..1`, `0.5` neutral — the
    /// shape `metal_sense`'s `Mode::Eyes` produces. The sensor's pose anchors a
    /// viewer at [`SENSOR_CALIBRATION_DISTANCE_M`] from it; `eye_x`/`eye_y`
    /// slide them across the sensor's own right/up plane, scaled by the lens,
    /// and `eye_z_est` moves them along the sensor's forward axis (toward the
    /// surface as the estimate rises). `None` for an id the venue does not
    /// have, so a stale `capture` binding is a missing viewer, not a guess.
    pub fn viewer_from(&self, sensor: &str, eye: [f32; 3]) -> Option<[f32; 3]> {
        let s = self.sensor(sensor)?;
        let forward = direction(s.yaw, s.pitch);
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let up = up_for(forward, s.roll);
        // The width a full frame covers at the calibration distance, from the
        // lens: a wide lens puts the same swing further out in the room.
        let half = (s.fov.to_radians() * 0.5).clamp(0.05, std::f32::consts::PI - 0.05);
        let span = 2.0 * half.tan() * SENSOR_CALIBRATION_DISTANCE_M;
        let dx = (eye[0] - 0.5) * span;
        let dy = (0.5 - eye[1]) * span;
        let dz = (eye[2] - 0.5) * SENSOR_DEPTH_RANGE_M;
        Some((Vec3::from(s.at) + right * dx + up * dy + forward * dz).into())
    }

    /// The sectors one `venue.surface` node's many sources land on, one
    /// polygon per input slot, in the order the split is presented to the
    /// graph: slot `i` receives the `i`-th upstream generator the walk
    /// gathers.
    ///
    /// The polygon is in the projector's/device's own `0..1` space, so the
    /// engine can clip a layer to it and derive its place and render size from
    /// its bounding box — the same space [`Venue::surface_homography`] maps
    /// into. A legacy surface (no `sectors`) gives exactly one cell, the whole
    /// surface plane, which is today's behaviour.
    ///
    /// `SectorSource::Projected` divides the surface plane into `n` wedges
    /// around its centre and maps each to the frame through the projector's
    /// warp: a fan the projector can actually throw, because every polygon
    /// comes from the projection of a piece of the wall. `Points` clips the
    /// surface's projected bounding box with the drawn streaks and falls back
    /// to the projected split when the strokes degenerate.
    pub fn sector_splits(&self, output: &str, surface: &str) -> Vec<Vec<[f32; 2]>> {
        let Some(face) = self.surfaces.iter().find(|s| s.id == surface && s.is_planar()) else {
            return vec![Vec::new()];
        };
        let projected = |n: usize| -> Option<Vec<Vec<[f32; 2]>>> {
            let h = self.surface_homography(output, surface)?;
            (0..n)
                .map(|i| {
                    wedge(n, i)
                        .iter()
                        .map(|p| h.map(*p))
                        .collect::<Option<Vec<[f32; 2]>>>()
                })
                .collect()
        };
        if face.sectors.source == SectorSource::Points {
            if let Some(cells) = self.drawn_splits(output, face) {
                return cells;
            }
        }
        let n = face.sectors.projected_cells();
        match projected(n) {
            Some(cells) if cells.len() == n => cells,
            _ => vec![SURFACE_CORNERS.to_vec()],
        }
    }

    /// The drawn streaks cut the surface plane's `0..1` box into convex
    /// cells ([`slabs`]); the whole surface when no warp can map them. `None`
    /// when the strokes degenerate (a zero-length streak, or a streak that
    /// leaves the square in fewer than `pairs + 1` cells), so the caller falls
    /// back to the projected split rather than drawing a hole.
    fn drawn_splits(&self, output: &str, s: &Surface) -> Option<Vec<Vec<[f32; 2]>>> {
        // An odd point count is half a streak, which draws nothing.
        if s.sectors.points.len() < 2 || s.sectors.points.len() % 2 != 0 {
            return None;
        }
        let pairs = s.sectors.points.len() / 2;
        let lines: Option<Vec<([f32; 2], [f32; 2])>> = s
            .sectors
            .points
            .chunks_exact(2)
            .map(|p| streak(p[0], p[1]))
            .collect();
        let lines = lines?;
        // A streak that does not cut the unit square in two (one running along
        // an edge, or lying outside it) is not a split; the drawing is
        // degenerate and the projected split answers instead.
        if lines.iter().any(|l| !cuts_square(l)) {
            return None;
        }
        let cells = slabs(&lines);
        if cells.len() != pairs + 1 {
            return None;
        }
        let h = self.surface_homography(output, &s.id);
        let mut out = Vec::with_capacity(cells.len());
        for cell in cells {
            match &h {
                Some(h) => out.push(cell.iter().map(|p| h.map(*p)).collect::<Option<Vec<_>>>()?),
                None => out.push(cell),
            }
        }
        Some(out)
    }

    /// Every output this venue drives: its projectors', then its displays'.
    pub fn outputs(&self) -> Vec<String> {
        self.projectors
            .iter()
            .map(|p| p.output.clone())
            .chain(self.displays.iter().map(|d| d.output.clone()))
            .collect()
    }

    /// Where a surface's four corners fall in a shot's image, or `None` if
    /// any of them is behind the camera.
    pub fn corners_in(&self, shot: &Shot, surface: &str) -> Option<[[f32; 2]; 4]> {
        let wall = self.walls().into_iter().find(|w| w.id == surface)?;
        let view = shot.camera.view(&shot.id);
        let mut out = [[0.0; 2]; 4];
        for (o, (s, t)) in out
            .iter_mut()
            .zip([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])
        {
            let v = view.project(wall.at(s, t))?;
            *o = [v.x, v.y];
        }
        Some(out)
    }

    /// What a shot's camera should see of each mask: its outline in that
    /// image, `(mask id, points)`. The venue's expectation of the room, in
    /// the camera's own coordinates — what a live frame from the same camera
    /// is compared against. A mask with a corner behind the camera is left
    /// out rather than drawn wrong.
    pub fn masks_in(&self, shot: &Shot) -> Vec<(String, Vec<[f32; 2]>)> {
        let walls = self.walls();
        let view = shot.camera.view(&shot.id);
        self.masks
            .iter()
            .filter_map(|m| {
                let w = walls.iter().find(|w| w.id == m.on)?;
                let pts = m
                    .points
                    .iter()
                    .map(|p| view.project(w.at(p[0], p[1])).map(|v| [v.x, v.y]))
                    .collect::<Option<Vec<_>>>()?;
                Some((m.id.clone(), pts))
            })
            .collect()
    }

    /// A composition's pieces: for each of its displays, the output and the
    /// window of the one picture that output shows — the display's bounding
    /// box in the viewer's image, normalised to the box around every
    /// display in the composition.
    ///
    /// Axis-aligned on purpose: a window is what [`crate::layers::Layer::span`]
    /// holds. A phone leaning back a few degrees gets the box around its
    /// glass, which is right to within that lean; a display seen steeply
    /// would need a per-display warp, and the check says so when the box is
    /// much bigger than the glass.
    pub fn spans(&self, c: &Composition) -> Result<Vec<(String, crate::layers::Sector)>, String> {
        let shot = self.shot(&c.seen_from).ok_or_else(|| {
            format!(
                "composition {} is seen from {:?}, which is not a shot",
                c.id, c.seen_from
            )
        })?;
        let mut boxes = Vec::with_capacity(c.displays.len());
        for id in &c.displays {
            let d = self.display(id).ok_or_else(|| {
                format!(
                    "composition {} names display {id:?}, which does not exist",
                    c.id
                )
            })?;
            let pts = self
                .corners_in(shot, &d.surface)
                .ok_or_else(|| format!("display {id} is behind shot {}'s camera", shot.id))?;
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for p in pts {
                for k in 0..2 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            boxes.push((d.output.clone(), lo, hi));
        }
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for (_, l, h) in &boxes {
            for k in 0..2 {
                lo[k] = lo[k].min(l[k]);
                hi[k] = hi[k].max(h[k]);
            }
        }
        let size = [(hi[0] - lo[0]).max(1e-6), (hi[1] - lo[1]).max(1e-6)];
        Ok(boxes
            .into_iter()
            .map(|(output, l, h)| {
                (
                    output,
                    crate::layers::Sector {
                        x: (l[0] - lo[0]) / size[0],
                        y: (l[1] - lo[1]) / size[1],
                        w: (h[0] - l[0]) / size[0],
                        h: (h[1] - l[1]) / size[1],
                    },
                )
            })
            .collect())
    }

    /// Where each of a fixture's samples lands in its projector's frame,
    /// `0..1` — one point per LED pixel for a string, one point at the
    /// middle of an LED bar's line, the beam's landing point for a wash or a
    /// spot. `None` for a sample outside the frame: no projected light
    /// reaches it, so it has no colour to take. A camera samples nothing.
    pub fn fixture_points(&self, f: &Fixture) -> Result<Vec<Option<[f32; 2]>>, String> {
        let p = match &f.projector {
            Some(id) => self.projector(id).ok_or_else(|| {
                format!(
                    "fixture {} samples projector {id:?}, which does not exist",
                    f.id
                )
            })?,
            None => self
                .projectors
                .first()
                .ok_or_else(|| format!("fixture {} has no projector to sample", f.id))?,
        };
        let view = self.view(p)?;
        let see = |q: Vec3| view.project(q).map(|v| [v.x, v.y]).filter(|v| in_frame(*v));
        match f.kind {
            FixtureKind::LedStrip => Ok(self.strip(f)?.into_iter().map(see).collect()),
            FixtureKind::Matrix => self.matrix_points(f),
            FixtureKind::LedBar => Ok(vec![see(self.line_point(f)?)]),
            FixtureKind::Wash
            | FixtureKind::Spot
            | FixtureKind::Moving
            | FixtureKind::Backlight
            | FixtureKind::Panel
            | FixtureKind::Continuous => {
                let at =
                    f.at.ok_or_else(|| format!("fixture {} is a light with no `at`", f.id))?;
                Ok(vec![self
                    .hit(Vec3::from(at), direction(f.yaw, f.pitch))
                    .and_then(|h| see(Vec3::from(h.at)))])
            }
            FixtureKind::Camera => Ok(Vec::new()),
        }
    }

    /// Where each of a matrix's cells lands in its projector's frame, `0..1`,
    /// in row-major cell order (`x + y·XPixels`) — one point per cell, the
    /// same order [`Matrix::pixel_channels`] indexes. `None` for a cell
    /// outside the frame. The panel's quad is its `path` (four `(s, t)`
    /// corners, `(0,0) (1,0) (1,1) (0,1)`) when one is written, else the whole
    /// surface it is `on`.
    pub fn matrix_points(&self, f: &Fixture) -> Result<Vec<Option<[f32; 2]>>, String> {
        let m = f
            .matrix
            .as_ref()
            .ok_or_else(|| format!("matrix {} has no `matrix`", f.id))?;
        m.check()?;
        let on =
            f.on.as_deref()
                .ok_or_else(|| format!("matrix {} is not on a surface", f.id))?;
        let wall = self
            .walls()
            .into_iter()
            .find(|w| w.id == on)
            .ok_or_else(|| format!("matrix {} is on {on:?}, which is not a surface", f.id))?;
        let p = match &f.projector {
            Some(id) => self.projector(id).ok_or_else(|| {
                format!(
                    "fixture {} samples projector {id:?}, which does not exist",
                    f.id
                )
            })?,
            None => self
                .projectors
                .first()
                .ok_or_else(|| format!("fixture {} has no projector to sample", f.id))?,
        };
        let view = self.view(p)?;
        let (xn, yn) = (m.x_pixels, m.y_pixels);
        let mut out = Vec::with_capacity(xn * yn);
        for y in 0..yn {
            for x in 0..xn {
                let (s, t) = matrix_st(m, f, x, y);
                out.push(
                    view.project(wall.at(s, t))
                        .map(|v| [v.x, v.y])
                        .filter(|v| in_frame(*v)),
                );
            }
        }
        Ok(out)
    }

    /// The polyline of an LED string (or bar) in the room: its path on its
    /// surface, in metres.
    fn path_points(&self, f: &Fixture) -> Result<Vec<Vec3>, String> {
        let on =
            f.on.as_deref()
                .ok_or_else(|| format!("strip {} is not on a surface", f.id))?;
        let wall = self
            .walls()
            .into_iter()
            .find(|w| w.id == on)
            .ok_or_else(|| format!("strip {} is on {on:?}, which is not a surface", f.id))?;
        let mut pts: Vec<Vec3> = f.path.iter().map(|p| wall.at(p[0], p[1])).collect();
        if f.closed && pts.len() > 2 {
            pts.push(pts[0]);
        }
        Ok(pts)
    }

    /// The middle of an LED bar's line, by length.
    fn line_point(&self, f: &Fixture) -> Result<Vec3, String> {
        let pts = self.path_points(f)?;
        if pts.is_empty() {
            return Err(format!("bar {} has no path", f.id));
        }
        let lengths: Vec<f32> = pts.windows(2).map(|w| (w[1] - w[0]).length()).collect();
        let total: f32 = lengths.iter().sum();
        if total <= 0.0 {
            return Ok(pts[0]);
        }
        let mut want = total / 2.0;
        for (k, len) in lengths.iter().enumerate() {
            if want <= *len || k + 1 == lengths.len() {
                return Ok(pts[k].lerp(pts[k + 1], (want / len).min(1.0)));
            }
            want -= len;
        }
        Ok(pts[pts.len() - 1])
    }

    /// How many pixels an LED string samples: its authored `pixels`, or its
    /// length cut at `pixels_per_m`.
    pub fn pixel_count(&self, f: &Fixture) -> Result<usize, String> {
        if let Some(n) = f.pixels {
            return Ok(n);
        }
        let pts = self.path_points(f)?;
        let total: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        Ok((total * f.pixels_per_m).round() as usize)
    }

    /// An LED strip's pixels in the room: its path on its surface, walked
    /// by length and cut at `pixels_per_m` (or the authored `pixels`), each
    /// pixel at the middle of its own span. `start` rotates where pixel 0
    /// sits along the line and `order` walks it forwards or back (M1).
    pub fn strip(&self, f: &Fixture) -> Result<Vec<Vec3>, String> {
        let pts = self.path_points(f)?;
        let lengths: Vec<f32> = pts.windows(2).map(|w| (w[1] - w[0]).length()).collect();
        let total: f32 = lengths.iter().sum();
        let n = self.pixel_count(f)?;
        let mut out = Vec::with_capacity(n);
        if n == 0 {
            return Ok(out);
        }
        for i in 0..n {
            let k = match f.order {
                PixelOrder::Forward => i,
                PixelOrder::Reverse => n - 1 - i,
            };
            let idx = (f.start + k) % n;
            let mut want = (idx as f32 + 0.5) / n as f32 * total;
            for (j, len) in lengths.iter().enumerate() {
                if want <= *len || j + 1 == lengths.len() {
                    out.push(pts[j].lerp(pts[j + 1], (want / len).min(1.0)));
                    break;
                }
                want -= len;
            }
        }
        Ok(out)
    }

    /// The footprint of one fixture: its pose and what of the frame it takes
    /// (M1's one vocabulary for every emitter and sensor).
    pub fn footprint(&self, f: &Fixture) -> Result<Footprint, String> {
        let shape = match f.kind {
            FixtureKind::Wash
            | FixtureKind::Spot
            | FixtureKind::Moving
            | FixtureKind::Backlight
            | FixtureKind::Panel
            | FixtureKind::Continuous => Shape::Point,
            FixtureKind::LedBar => Shape::Line,
            FixtureKind::LedStrip => Shape::Pixels {
                pixels: self.pixel_count(f)?,
            },
            FixtureKind::Matrix => Shape::Pixels {
                pixels: f.matrix.as_ref().map_or(0, |m| m.cells()),
            },
            FixtureKind::Camera => Shape::Sensor {
                fov: f.beam,
                capture: f.on.clone().unwrap_or_default(),
            },
        };
        Ok(Footprint {
            at: f.at.unwrap_or([0.0; 3]),
            yaw: f.yaw,
            pitch: f.pitch,
            roll: f.roll,
            shape,
        })
    }

    /// The fixtures a `venue.fixtures` node drives: the whole venue, or one
    /// named group (`group = "front-wash"`, M3).
    pub fn group(&self, group: Option<&str>) -> Vec<&Fixture> {
        self.fixtures
            .iter()
            .filter(|f| match group {
                None => true,
                Some(g) => f.group.as_deref() == Some(g),
            })
            .collect()
    }

    /// Where a ray first lands: a `Surface` plane or an `Object` face, or the
    /// floor (M2, D5). `from` is the emitter, `dir` a unit direction. The
    /// nearest hit wins; a hit at or behind the origin is ignored, and the
    /// returned normal points back toward the emitter.
    pub fn hit(&self, from: Vec3, dir: Vec3) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        // Surfaces: ray/plane against each parallelogram.
        for w in self.surfaces.iter().map(Surface::wall) {
            if let Some((d, _, _)) = ray_wall(from, dir, &w) {
                keep(
                    &mut best,
                    Hit {
                        what: w.id.clone(),
                        at: (from + dir * d).into(),
                        distance: d,
                        normal: facing(&w, dir),
                    },
                );
            }
        }
        // Object faces: ray/triangle against the two triangles of the rectangle.
        for o in &self.objects {
            for name in &o.faces {
                let Some(w) = o.face(name) else { continue };
                if let Some(d) = face_hit(from, dir, &w) {
                    keep(
                        &mut best,
                        Hit {
                            what: w.id.clone(),
                            at: (from + dir * d).into(),
                            distance: d,
                            normal: facing(&w, dir),
                        },
                    );
                }
            }
        }
        // The floor (y = 0), when the ray crosses it in front of the emitter.
        if dir.y < -1e-9 {
            let d = -from.y / dir.y;
            if d > 1e-4 {
                keep(
                    &mut best,
                    Hit {
                        what: "floor".into(),
                        at: (from + dir * d).into(),
                        distance: d,
                        normal: [0.0, 1.0, 0.0],
                    },
                );
            }
        }
        best
    }

    /// Where a head at `f` must pan/tilt to point at `target`: inverse
    /// kinematics from the pose, clamped to `limits` and taking the shortest
    /// path from where the head points now. Returns `(pan, tilt)` in world
    /// degrees, the same convention as `yaw`/`pitch` (M2).
    pub fn aim_with_limits(
        &self,
        f: &Fixture,
        target: [f32; 3],
        limits: &crate::fixture::Limits,
    ) -> Result<(f32, f32), String> {
        let at =
            f.at.ok_or_else(|| format!("fixture {} is a light with no `at`", f.id))?;
        let d = Vec3::from(target) - Vec3::from(at);
        let horizontal = (d.x * d.x + d.z * d.z).sqrt();
        if horizontal < 1e-6 && d.y.abs() < 1e-6 {
            return Err(format!("fixture {} is already at {target:?}", f.id));
        }
        let want_yaw = d.x.atan2(-d.z).to_degrees();
        let want_pitch = d.y.atan2(horizontal).to_degrees();
        // Shortest path: choose the equivalent yaw (mod 360°) nearest the
        // head's current aim among those inside the pan limits, so a 540°
        // head goes the near way round instead of folding past its stops.
        let [pan_lo, pan_hi] = limits.pan;
        let (lo, hi) = (pan_lo.min(pan_hi), pan_hi.max(pan_lo));
        let delta = |a: f32| {
            let mut d = (a - f.yaw).rem_euclid(360.0);
            if d > 180.0 {
                d -= 360.0;
            }
            d
        };
        let mut pan = want_yaw.clamp(lo, hi);
        let mut best = delta(pan).abs();
        for k in -2..=2 {
            let cand = want_yaw + 360.0 * k as f32;
            if cand < lo || cand > hi {
                continue;
            }
            let d = delta(cand).abs();
            if d < best {
                pan = cand;
                best = d;
            }
        }
        let [tilt_lo, tilt_hi] = limits.tilt;
        Ok((
            pan,
            want_pitch.clamp(tilt_lo.min(tilt_hi), tilt_hi.max(tilt_lo)),
        ))
    }

    /// The same, with the schema's own default limits.
    pub fn aim(&self, f: &Fixture, target: [f32; 3]) -> Result<(f32, f32), String> {
        self.aim_with_limits(f, target, &crate::fixture::Limits::default())
    }

    /// Solve a moving head so it points at a real 3D point, wherever it is
    /// hung: the inverse of [`direction`], returned as a live
    /// [`FixtureState`] (D5). The pan/tilt come from
    /// [`Venue::aim_with_limits`] — clamped to the profile's `limits` and
    /// taking the shortest path from the head's current aim — and both the
    /// commanded and the reached pose carry them, so a caller advancing the
    /// state with [`FixtureState::step`] models the slew. The beam's landing
    /// point is [`Venue::hit`] on the solved [`Pose`].
    pub fn solve_head(
        &self,
        fixture_id: &str,
        target: [f32; 3],
        limits: &crate::fixture::Limits,
    ) -> Result<FixtureState, String> {
        let f = self
            .fixtures
            .iter()
            .find(|f| f.id == fixture_id)
            .ok_or_else(|| format!("no fixture {fixture_id:?} in this venue"))?;
        let (pan, tilt) = self.aim_with_limits(f, target, limits)?;
        let mut state = FixtureState::new(f);
        state.commanded.pan = pan;
        state.commanded.tilt = tilt;
        state.actual = state.commanded;
        Ok(state)
    }

    /// The live state of every fixture, initialised from the authored pose
    /// (M2).
    pub fn states(&self) -> Vec<FixtureState> {
        self.fixtures.iter().map(FixtureState::new).collect()
    }

    /// Everything [`Venue::check`] can say about this venue, worst first.
    pub fn check(&self) -> Report {
        let mut r = Report::default();
        let mut say =
            |level: Level, at: String, what: String| r.problems.push(Problem { level, at, what });

        let walls = self.walls();
        let wall = |id: &str| walls.iter().find(|w| w.id == id);

        // References and ranges: a venue that names nothing real is an error
        // before any geometry is worth measuring.
        let mut seen = std::collections::HashSet::new();
        for id in walls
            .iter()
            .map(|w| &w.id)
            .chain(self.objects.iter().map(|o| &o.id))
        {
            if !seen.insert(id.clone()) {
                say(Level::Error, format!("id {id}"), "used twice".into());
            }
        }
        for m in &self.masks {
            if wall(&m.on).is_none() {
                say(
                    Level::Error,
                    format!("mask {}", m.id),
                    format!("is on {:?}, which is not a surface", m.on),
                );
            }
            if m.points.len() < 3 {
                say(
                    Level::Error,
                    format!("mask {}", m.id),
                    "needs at least three points".into(),
                );
            }
            if m.points.iter().any(|p| !in_frame(*p)) {
                say(
                    Level::Error,
                    format!("mask {}", m.id),
                    "has a point outside its surface's 0..1".into(),
                );
            }
        }
        for p in &self.projectors {
            if let Err(e) = self.lens(p) {
                say(Level::Error, format!("projector {}", p.id), e);
            }
            if p.throw_ratio <= 0.0 {
                say(
                    Level::Error,
                    format!("projector {}", p.id),
                    "throw_ratio must be positive".into(),
                );
            }
        }
        for s in &self.shots {
            let at = format!("shot {}", s.id);
            for (name, v) in [
                ("floor_brightness", s.floor_brightness),
                ("background_brightness", s.background_brightness),
            ] {
                if !(0.0..=1.0).contains(&v) {
                    say(
                        Level::Error,
                        at.clone(),
                        format!("{name} = {v} is outside 0..1"),
                    );
                }
            }
            if s.background_brightness > BRIGHT_BACKGROUND && !self.projectors.is_empty() {
                say(
                    Level::Warning,
                    at.clone(),
                    format!(
                        "background brightness {:.2} is above {BRIGHT_BACKGROUND}: the projection will not read until the light drops",
                        s.background_brightness
                    ),
                );
            }
            match self.venue.setting {
                Setting::Indoor if !s.horizon_path.is_empty() => say(
                    Level::Error,
                    at.clone(),
                    "an indoor venue has no horizon; horizon_path must be empty".into(),
                ),
                Setting::Outdoor if s.horizon_path.len() == 1 => say(
                    Level::Error,
                    at.clone(),
                    "a horizon path needs two points or none".into(),
                ),
                _ => {}
            }
            match &s.projection {
                Some(pr) if self.projector(&pr.projector).is_none() => say(
                    Level::Error,
                    at.clone(),
                    format!("reads projector {:?}, which does not exist", pr.projector),
                ),
                None if s.projector_distance != 0.0 => say(
                    Level::Error,
                    at.clone(),
                    "has a projector distance but no projection".into(),
                ),
                None if !self.projectors.is_empty() => say(
                    Level::Warning,
                    at.clone(),
                    "reads no projection, but the venue has projectors".into(),
                ),
                _ => {}
            }
        }
        let mut driven = std::collections::HashSet::new();
        for o in self.outputs() {
            if !driven.insert(o.clone()) {
                say(
                    Level::Error,
                    format!("output {o}"),
                    "is driven twice (two projectors or displays name it)".into(),
                );
            }
        }
        for d in &self.displays {
            let at = format!("display {}", d.id);
            let Some(w) = wall(&d.surface) else {
                say(
                    Level::Error,
                    at,
                    format!("is on {:?}, which is not a surface", d.surface),
                );
                continue;
            };
            if d.resolution[0] == 0 || d.resolution[1] == 0 {
                say(
                    Level::Error,
                    at.clone(),
                    "resolution must be two positive numbers".into(),
                );
                continue;
            }
            let glass = Vec3::from(w.u).length() / Vec3::from(w.v).length().max(1e-6);
            let pixels = d.resolution[0] as f32 / d.resolution[1] as f32;
            if (glass - pixels).abs() / pixels > 0.05 {
                say(
                    Level::Warning,
                    at.clone(),
                    format!("its glass is {glass:.3} wide per high, its pixels {pixels:.3}: masks on it stretch"),
                );
            }
            if d.link == Link::Peer && d.node.is_none() {
                say(
                    Level::Warning,
                    at,
                    "is a peer with no `node` naming the device that shows it".into(),
                );
            }
        }
        for c in &self.compositions {
            let at = format!("composition {}", c.id);
            if self.shot(&c.seen_from).is_none() {
                say(
                    Level::Error,
                    at.clone(),
                    format!("is seen from {:?}, which is not a shot", c.seen_from),
                );
            }
            for id in c.displays.iter().filter(|id| self.display(id).is_none()) {
                say(
                    Level::Error,
                    at.clone(),
                    format!("names display {id:?}, which does not exist"),
                );
            }
            if c.displays.len() < 2 {
                say(
                    Level::Warning,
                    at.clone(),
                    "spans fewer than two displays".into(),
                );
            }
            if c.model == "forge" && c.shader.is_none() {
                say(Level::Error, at, "draws `forge` with no `shader`".into());
            }
        }
        if self.venue.setting == Setting::Outdoor
            && self.shots.iter().all(|s| s.horizon_path.is_empty())
        {
            say(
                Level::Warning,
                "venue".into(),
                "outdoors, but no shot traced a horizon path".into(),
            );
        }
        if r.errors() > 0 {
            return r;
        }

        // 1. The camera: every surface corner a shot reported, reprojected.
        for s in &self.shots {
            let view = s.camera.view(&s.id);
            for seen in &s.sees {
                let Some(w) = wall(&seen.surface) else {
                    r.problems.push(Problem {
                        level: Level::Error,
                        at: format!("shot {}", s.id),
                        what: format!("sees {:?}, which is not a surface", seen.surface),
                    });
                    continue;
                };
                let corners = [
                    w.at(0.0, 0.0),
                    w.at(1.0, 0.0),
                    w.at(1.0, 1.0),
                    w.at(0.0, 1.0),
                ];
                let mut worst = 0.0f32;
                for (c, reported) in corners.iter().zip(seen.corners.iter()) {
                    let err = match view.project(*c) {
                        Some(p) => {
                            ((p.x - reported[0]).powi(2) + (p.y - reported[1]).powi(2)).sqrt()
                        }
                        None => f32::INFINITY,
                    };
                    worst = worst.max(err);
                }
                r.reprojection = Some(r.reprojection.map_or(worst, |x: f32| x.max(worst)));
                if worst > REPROJECTION_TOLERANCE {
                    r.problems.push(Problem {
                        level: Level::Warning,
                        at: format!("shot {}", s.id),
                        what: format!(
                            "{} is {worst:.3} away from where the camera and the geometry put it",
                            seen.surface
                        ),
                    });
                }
            }
        }

        // 2. and 3. The projection point and the projector's distance, per
        // shot, against the throw ray-cast from the table.
        for s in &self.shots {
            let Some(pr) = &s.projection else { continue };
            let Some(p) = self.projector(&pr.projector) else {
                continue;
            };
            let gap = angle_gap(pr.yaw, p.yaw).max(angle_gap(pr.pitch, p.pitch));
            r.angle = Some(r.angle.map_or(gap, |x: f32| x.max(gap)));
            let at = format!("shot {}", s.id);
            if gap > ANGLE_TOLERANCE {
                r.problems.push(Problem {
                    level: Level::Warning,
                    at: at.clone(),
                    what: format!("reads the projector's aim {gap:.1}° away from the venue's"),
                });
            }
            match self.throw(p) {
                Ok(Some((hit, distance, _, _))) => {
                    let rel = (s.projector_distance - distance).abs() / distance;
                    r.distance = Some(r.distance.map_or(rel, |x: f32| x.max(rel)));
                    if rel > DISTANCE_TOLERANCE {
                        r.problems.push(Problem {
                            level: Level::Warning,
                            at: at.clone(),
                            what: format!(
                                "projector distance {:.2} m, the throw measures {distance:.2} m",
                                s.projector_distance
                            ),
                        });
                    }
                    if hit != pr.lands_on {
                        r.problems.push(Problem {
                            level: Level::Warning,
                            at,
                            what: format!(
                                "says the axis lands on {}, it lands on {hit}",
                                pr.lands_on
                            ),
                        });
                    }
                }
                Ok(None) => r.problems.push(Problem {
                    level: Level::Warning,
                    at: format!("projector {}", p.id),
                    what: "its axis lands on no surface".into(),
                }),
                Err(_) => {}
            }
        }

        // Every mask corner: inside some projector's frame, not behind an
        // object, and (outdoors) not above any shot's horizon path.
        for m in &self.masks {
            let Some(w) = wall(&m.on) else { continue };
            let at = format!("mask {}", m.id);
            let corners: Vec<Vec3> = m.points.iter().map(|p| w.at(p[0], p[1])).collect();
            let on_glass = self.displays.iter().any(|d| d.surface == m.on);
            let mut lit = on_glass;
            for p in self.projectors.iter().filter(|_| !on_glass) {
                let (Ok(view), Ok(eye)) = (self.view(p), self.lens(p)) else {
                    continue;
                };
                let framed = corners
                    .iter()
                    .all(|c| view.project(*c).is_some_and(|v| in_frame([v.x, v.y])));
                if !framed {
                    continue;
                }
                lit = true;
                for (i, c) in corners.iter().enumerate() {
                    if let Some(o) = self.blocked(eye, *c, &p.on) {
                        r.problems.push(Problem {
                            level: Level::Warning,
                            at: at.clone(),
                            what: format!("corner {i} is behind {} from projector {}", o.id, p.id),
                        });
                    }
                }
            }
            if !lit {
                r.problems.push(Problem {
                    level: Level::Warning,
                    at: at.clone(),
                    what: "no projector's frame holds it whole".into(),
                });
            }
            for s in self.shots.iter().filter(|s| s.horizon_path.len() >= 2) {
                let view = s.camera.view(&s.id);
                let above = corners
                    .iter()
                    .filter_map(|c| view.project(*c))
                    .any(|v| skyline_at(&s.horizon_path, v.x).is_some_and(|sky| v.y < sky));
                if above {
                    r.problems.push(Problem {
                        level: Level::Warning,
                        at: at.clone(),
                        what: format!("reaches above the horizon path in shot {}: that light goes into the sky", s.id),
                    });
                }
            }
        }

        // A composition's displays, as its viewer sees them: in front of the
        // camera, and not so oblique that the axis-aligned window is mostly
        // not glass.
        for c in &self.compositions {
            let Some(shot) = self.shot(&c.seen_from) else {
                continue;
            };
            for d in c.displays.iter().filter_map(|id| self.display(id)) {
                let at = format!("composition {}", c.id);
                match self.corners_in(shot, &d.surface) {
                    None => r.problems.push(Problem {
                        level: Level::Warning,
                        at,
                        what: format!("display {} is behind the viewer in shot {}", d.id, shot.id),
                    }),
                    Some(q) => {
                        let area = 0.5
                            * ((q[0][0] * q[1][1] - q[1][0] * q[0][1])
                                + (q[1][0] * q[2][1] - q[2][0] * q[1][1])
                                + (q[2][0] * q[3][1] - q[3][0] * q[2][1])
                                + (q[3][0] * q[0][1] - q[0][0] * q[3][1]))
                                .abs();
                        let xs = q.iter().map(|p| p[0]);
                        let ys = q.iter().map(|p| p[1]);
                        let w = xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min);
                        let h = ys.clone().fold(f32::MIN, f32::max) - ys.fold(f32::MAX, f32::min);
                        if area < 0.7 * w * h {
                            r.problems.push(Problem {
                                level: Level::Warning,
                                at,
                                what: format!(
                                    "display {} is seen at a steep angle: its window is {:.0}% glass",
                                    d.id,
                                    100.0 * area / (w * h).max(1e-6)
                                ),
                            });
                        }
                    }
                }
            }
        }

        for f in &self.fixtures {
            // Only a fixture on `drive = "frame"` reads the projected frame:
            // a `fixed` light takes the operator's own values, so there is no
            // frame point to measure (and no projector needed for one).
            if f.drive != Drive::Frame {
                continue;
            }
            match self.fixture_points(f) {
                Ok(pts) => {
                    let dark = pts.iter().filter(|p| p.is_none()).count();
                    if dark > 0 {
                        r.problems.push(Problem {
                            level: Level::Warning,
                            at: format!("fixture {}", f.id),
                            what: format!(
                                "{dark} of {} samples fall outside the projector's frame",
                                pts.len()
                            ),
                        });
                    }
                }
                Err(e) => r.problems.push(Problem {
                    level: Level::Error,
                    at: format!("fixture {}", f.id),
                    what: e,
                }),
            }
        }

        r.problems.sort_by_key(|p| p.level != Level::Error);
        r
    }
}

/// The horizon path's height at image `x`, if the path spans it.
fn skyline_at(path: &[[f32; 2]], x: f32) -> Option<f32> {
    path.windows(2).find_map(|w| {
        let (a, b) = (w[0], w[1]);
        let (lo, hi) = (a[0].min(b[0]), a[0].max(b[0]));
        if x < lo || x > hi {
            return None;
        }
        if (b[0] - a[0]).abs() < 1e-6 {
            return Some(a[1].max(b[1]));
        }
        Some(a[1] + (b[1] - a[1]) * (x - a[0]) / (b[0] - a[0]))
    })
}

/// The RGB of an RGBA8 frame at `at` (`0..1`), as floats `0..1` — how a
/// fixture on `drive = "frame"` reads its colour. Nearest pixel; `None`
/// (outside the frame) is black.
pub fn sample_rgb(pixels: &[u8], w: u32, h: u32, at: Option<[f32; 2]>) -> [f32; 3] {
    let Some([x, y]) = at else { return [0.0; 3] };
    if w == 0 || h == 0 {
        return [0.0; 3];
    }
    let px = ((x * w as f32) as u32).min(w - 1);
    let py = ((y * h as f32) as u32).min(h - 1);
    let i = ((py * w + px) * 4) as usize;
    match pixels.get(i..i + 3) {
        Some(c) => [
            c[0] as f32 / 255.0,
            c[1] as f32 / 255.0,
            c[2] as f32 / 255.0,
        ],
        None => [0.0; 3],
    }
}

/// A light's `hue`/`sat`/`level` floats (each `0..1`, the fixture profile's
/// default ranges) from a sampled colour.
pub fn light_floats(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= 1e-6 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    let sat = if max <= 1e-6 { 0.0 } else { delta / max };
    [hue, sat, max]
}

/// The inverse of [`light_floats`]: the sRGB `0..1` a `hue`/`sat`/`level`
/// triple stands for. Used to read a fixture node's live swatch back off the
/// packet it sent (F10/F11/F14) — the colour that is on the wire, never a
/// re-run of the model or the frame that produced it.
pub fn rgb_from_light(hue: f32, sat: f32, level: f32) -> [f32; 3] {
    let max = level.clamp(0.0, 1.0);
    let s = sat.clamp(0.0, 1.0);
    if s <= 1e-6 {
        return [max, max, max];
    }
    let h = (hue.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = max * (1.0 - s);
    let q = max * (1.0 - s * f);
    let t = max * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => [max, t, p],
        1 => [q, max, p],
        2 => [p, max, t],
        3 => [p, q, max],
        4 => [t, p, max],
        _ => [max, p, q],
    }
}

/// Where generated venues are saved.
/// Remembers which venue was projected last, for `look_venue` to read back
/// without being told: `data/venues/last` holds its id.
pub fn remember(root: &Path, id: &str) -> std::io::Result<()> {
    metal_vfs::create_dir_all(dir(root))?;
    crate::atomic::write(&dir(root).join("last"), format!("{id}\n"))
}

/// The venue projected last, if any.
pub fn last(root: &Path) -> Option<String> {
    metal_vfs::read_to_string(dir(root).join("last"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn dir(root: &Path) -> PathBuf {
    // Tests read and write a per-process scratch venues directory, never the
    // operator's (rule 8) — the same fence `escena::path` and
    // `engine::graphs_dir` put around their files. `data_root()` is a
    // `OnceLock` a test cannot point elsewhere, and a test that writes a venue
    // to try a surface on must not touch `data/venues/`.
    if cfg!(any(test, feature = "test-isolated-graphs")) && root == crate::output::data_root() {
        static SCRATCH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        return SCRATCH
            .get_or_init(|| {
                let dir =
                    std::env::temp_dir().join(format!("fw-test-venues-{}", std::process::id()));
                let _ = metal_vfs::remove_dir_all(&dir);
                let _ = metal_vfs::create_dir_all(&dir);
                dir
            })
            .clone();
    }
    root.join("venues")
}

/// Where the few-shot examples live.
pub fn examples_dir(root: &Path) -> PathBuf {
    root.join("agent").join("venues")
}

/// Writes `v` to `data/venues/<id>.toml`, creating the directory — the one
/// write side of a venue file, used when a capture camera is placed in it
/// (H5.3, `POST /venue { op: "sensor" }`). The same id guard as [`load`], so
/// a venue written here is always one [`load`] can read back.
pub fn save(root: &Path, v: &Venue) -> Result<(), String> {
    let id = &v.venue.id;
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return Err(format!("{id:?} is not a venue id"));
    }
    metal_vfs::create_dir_all(dir(root)).map_err(|e| e.to_string())?;
    let text = toml::to_string_pretty(v).map_err(|e| e.to_string())?;
    crate::atomic::write(&dir(root).join(format!("{id}.toml")), text).map_err(|e| e.to_string())
}

/// A venue by id: `data/venues/<id>.toml` first, then the examples in
/// `data/agent/venues/`, so an example can be test-projected as it is.
pub fn load(root: &Path, id: &str) -> Result<Venue, String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return Err(format!("{id:?} is not a venue id"));
    }
    let name = format!("{id}.toml");
    let path = [dir(root).join(&name), examples_dir(root).join(&name)]
        .into_iter()
        .find(|p| metal_vfs::exists(&p))
        .ok_or_else(|| format!("no venue called {id:?}"))?;
    let text = metal_vfs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Venue::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every venue id on disk, generated ones first, each once.
pub fn list(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in [dir(root), examples_dir(root)] {
        let Ok(entries) = metal_vfs::read_dir(d) else {
            continue;
        };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension()? == "toml").then(|| p.file_stem()?.to_str().map(str::to_string))?
            })
            .filter(|id| !out.contains(id))
            .collect();
        ids.sort();
        out.extend(ids);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn examples() -> PathBuf {
        examples_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"))
    }

    fn example(id: &str) -> Venue {
        let text = metal_vfs::read_to_string(examples().join(format!("{id}.toml")))
            .expect("example on disk");
        Venue::parse(&text).unwrap_or_else(|e| panic!("{id}.toml: {e}"))
    }

    fn every_example() -> Vec<(String, Venue)> {
        let mut out: Vec<(String, Venue)> = metal_vfs::read_dir(examples())
            .expect("data/agent/venues")
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .map(|e| {
                let id = e.path().file_stem().unwrap().to_string_lossy().to_string();
                let v = example(&id);
                (id, v)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// H5.3: a placed capture camera (`Venue.sensors`) written with [`save`]
    /// reads back whole with [`load`], so the panel's placement and the
    /// runtime's read are one file.
    #[test]
    fn a_placed_sensor_round_trips_through_save_and_load() {
        let root = std::env::temp_dir().join(format!("fw-venue-save-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&root);
        let mut v = example("bar");
        v.sensors.push(Sensor {
            id: "cam-0".into(),
            name: "webcam".into(),
            at: [1.0, 1.5, -2.0],
            yaw: 180.0,
            pitch: -5.0,
            roll: 0.0,
            fov: 60.0,
            aspect: 16.0 / 9.0,
            capture: Some("v4l2:/dev/video0".into()),
        });
        save(&root, &v).expect("save the venue");
        let back = load(&root, "bar").expect("load it back");
        let s = back.sensor("cam-0").expect("the placed sensor survived");
        assert_eq!(s.capture.as_deref(), Some("v4l2:/dev/video0"));
        assert_eq!(s.at, [1.0, 1.5, -2.0]);
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D1: a fixture the settings page adds — [`Fixture::new`] plus the mapping
    /// fields `POST /venue { op: "fixture" }` writes — survives
    /// [`save`]/[`load`] whole, so listing it and reloading it are one file.
    #[test]
    fn a_settings_fixture_round_trips_through_save_and_load() {
        let root = std::env::temp_dir().join(format!("fw-venue-fixture-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&root);
        let mut v = example("bar");
        let id = v.unique_fixture_id(FixtureKind::Wash, None);
        let mut f = Fixture::new(id.clone(), "frente".into(), FixtureKind::Wash);
        f.group = Some("front-wash".into());
        f.wire = "dmx".into();
        f.universe = Some(3);
        f.address = Some(clamp_address(40));
        f.pixels = Some(24);
        f.order = PixelOrder::Reverse;
        f.start = 2;
        f.drive = Drive::Fixed;
        f.projector = Some("proyector-bar".into());
        let f_id = f.id.clone();
        v.fixtures.push(f);
        save(&root, &v).expect("save the venue");

        let back = load(&root, "bar").expect("load it back");
        let got = back
            .fixtures
            .iter()
            .find(|f| f.id == f_id)
            .expect("the added fixture survived");
        let want = v.fixtures.iter().find(|f| f.id == f_id).unwrap();
        assert_eq!(got, want, "the whole fixture round-trips");
        assert_eq!(got.universe, Some(3));
        assert_eq!(got.address, Some(40));
        assert_eq!(got.order, PixelOrder::Reverse);
        assert_eq!(got.drive, Drive::Fixed);
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D1: a posted `address` lands inside one universe — a desk numbers the
    /// 512 slots from 1, so 0 and 9999 clamp to 1 and 512.
    #[test]
    fn a_posted_fixture_address_clamps_to_the_universe() {
        assert_eq!(clamp_address(0), 1);
        assert_eq!(clamp_address(1), 1);
        assert_eq!(clamp_address(200), 200);
        assert_eq!(clamp_address(512), 512);
        assert_eq!(clamp_address(9999), 512);
    }

    // -- D4: the 2-D pixel matrix ------------------------------------------

    /// D4: a matrix fixture — kind, whole `matrix` field set and its mapping —
    /// round-trips through `save`/`load` whole, so the Settings page listing
    /// it and the file are the same facts.
    #[test]
    fn a_matrix_fixture_round_trips_through_save_and_load() {
        let root = std::env::temp_dir().join(format!("fw-venue-matrix-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&root);
        let mut v = example("bar");
        let mut f = Fixture::new("media".into(), "media".into(), FixtureKind::Matrix);
        f.on = Some("back-wall".into());
        f.wire = "sacn".into();
        f.universe = Some(2);
        f.address = Some(clamp_address(300));
        f.matrix = Some(Matrix {
            x_pixels: 4,
            y_pixels: 3,
            start_corner: StartCorner::TopRight,
            orientation: Orientation::Vertical,
            snake: Snake::On,
            first_channels: 1,
            last_channels: 2,
            dmx_matrix: Vec::new(),
        });
        v.fixtures.push(f);
        save(&root, &v).expect("save the venue");

        let back = load(&root, "bar").expect("load it back");
        let got = back
            .fixtures
            .iter()
            .find(|f| f.id == "media")
            .expect("the matrix survived");
        let want = v.fixtures.iter().find(|f| f.id == "media").unwrap();
        assert_eq!(got, want, "the whole fixture, matrix included, round-trips");
        assert_eq!(got.kind, FixtureKind::Matrix);
        let m = got.matrix.as_ref().expect("the matrix is on the fixture");
        assert_eq!((m.x_pixels, m.y_pixels), (4, 3));
        assert_eq!(m.start_corner, StartCorner::TopRight);
        assert_eq!(m.orientation, Orientation::Vertical);
        assert_eq!(m.snake, Snake::On);
        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// D4: the scan order a matrix writes its channels in — where the walk
    /// starts ([`StartCorner`]), which axis is fast ([`Orientation`]) and
    /// whether every other line comes back ([`Snake`]), on a square grid.
    #[test]
    fn a_matrix_scan_order_follows_corner_orientation_and_snake() {
        let mut m = Matrix::new(2, 2);
        assert_eq!(
            m.scan_order(),
            vec![(0, 0), (1, 0), (0, 1), (1, 1)],
            "bottom-left, horizontal, no snake walks rows left to right"
        );
        m.snake = Snake::On;
        assert_eq!(
            m.scan_order(),
            vec![(0, 0), (1, 0), (1, 1), (0, 1)],
            "snake turns the second row around"
        );
        m.snake = Snake::Off;
        m.orientation = Orientation::Vertical;
        assert_eq!(
            m.scan_order(),
            vec![(0, 0), (0, 1), (1, 0), (1, 1)],
            "vertical makes columns the fast axis"
        );
        m.start_corner = StartCorner::TopRight;
        assert_eq!(
            m.scan_order(),
            vec![(1, 1), (1, 0), (0, 1), (0, 0)],
            "top-right starts at the last cell of both axes"
        );
    }

    /// D4: every corner × orientation × snake combination walks the grid once
    /// (a permutation of its cells), and the derived channel map numbers those
    /// cells in exactly that walk, `MATRIX_CHANNELS_PER_PIXEL` apart, from
    /// `first_channels + 1`. Base is the lowest offset less `first_channels`.
    #[test]
    fn every_corner_orientation_and_snake_numbers_the_grid_once() {
        let corners = [StartCorner::BottomLeft, StartCorner::BottomRight, StartCorner::TopLeft, StartCorner::TopRight];
        let orientations = [Orientation::Horizontal, Orientation::Vertical];
        let snakes = [Snake::Off, Snake::On];
        let mut combos = 0;
        for corner in corners {
            for orientation in orientations {
                for snake in snakes {
                    combos += 1;
                    let m = Matrix {
                        x_pixels: 3,
                        y_pixels: 2,
                        start_corner: corner,
                        orientation,
                        snake,
                        first_channels: 2,
                        ..Matrix::default()
                    };
                    let order = m.scan_order();
                    let mut seen = order.clone();
                    seen.sort();
                    seen.dedup();
                    assert_eq!(order.len(), m.cells(), "{corner:?} {orientation:?} {snake:?}: one visit per cell");
                    assert_eq!(seen.len(), m.cells(), "{corner:?} {orientation:?} {snake:?}: no cell twice");
                    let channels = m.pixel_channels();
                    let stride = MATRIX_CHANNELS_PER_PIXEL as i16;
                    for (rank, (x, y)) in order.iter().enumerate() {
                        assert_eq!(
                            channels[x + y * m.x_pixels],
                            m.first_channels as i16 + 1 + rank as i16 * stride,
                            "{corner:?} {orientation:?} {snake:?}: cell ({x},{y}) at walk rank {rank}"
                        );
                    }
                    let min = *channels.iter().min().unwrap();
                    assert_eq!(m.base(), min - m.first_channels as i16);
                }
            }
        }
        assert_eq!(combos, 16);
    }

    /// D4: the derived channel map numbers cells consecutively in scan order,
    /// `MATRIX_CHANNELS_PER_PIXEL` channels each, and a slot past 512 wraps
    /// into the next universe.
    #[test]
    fn a_matrix_channel_map_numbers_cells_and_wraps_universes() {
        let mut m = Matrix::new(2, 2);
        m.first_channels = 2;
        let channels = m.pixel_channels();
        let stride = MATRIX_CHANNELS_PER_PIXEL as i16;
        for (rank, (x, y)) in m.scan_order().iter().enumerate() {
            let want = m.first_channels as i16 + 1 + rank as i16 * stride;
            assert_eq!(
                channels[x + y * m.x_pixels],
                want,
                "cell ({x},{y}) takes the {rank}th channel block"
            );
        }
        // The last cell's blue is the highest channel; at an address near the
        // end of a universe it wraps into the next one.
        let last = *channels.iter().max().unwrap();
        let offset = last + stride - 1;
        let abs = (511i64 - 1) + (offset - m.base()) as i64;
        let (u, a) = m.channel_at(1, 511, offset);
        assert_eq!(
            u,
            1 + (abs / 512) as u16,
            "a slot past 512 wraps into the next universe"
        );
        assert_eq!(
            a,
            (1 + abs % 512) as u16,
            "and lands back on the 1-based grid"
        );
    }

    /// D4: `base` and `top` are the ShowXpress derivation — the lowest used
    /// offset less the first channels, the highest plus the last channels —
    /// computed from the model's own offsets, not a hand count.
    #[test]
    fn a_matrix_base_and_top_derive_from_its_offsets() {
        let m = Matrix {
            x_pixels: 2,
            y_pixels: 2,
            first_channels: 2,
            last_channels: 1,
            dmx_matrix: vec![5, 8, -1, 11],
            ..Matrix::default()
        };
        let used: Vec<i16> = m.dmx_matrix.iter().copied().filter(|o| *o >= 0).collect();
        let min = used.iter().copied().min().unwrap();
        let max = used.iter().copied().max().unwrap();
        assert_eq!(m.base(), min - m.first_channels as i16);
        assert_eq!(
            m.top(),
            max + MATRIX_CHANNELS_PER_PIXEL as i16 - 1 + m.last_channels as i16
        );
        assert_eq!(
            m.channel_at(0, 100, m.base()),
            (0, 100),
            "the base offset maps to the fixture's own start address"
        );
    }

    #[test]
    fn every_example_parses_and_checks_without_errors() {
        let all = every_example();
        assert!(all.len() >= 2, "the main example and the auxiliary one");
        for (id, v) in &all {
            assert_eq!(&v.venue.id, id, "file name and [venue].id agree");
            let r = v.check();
            assert_eq!(r.errors(), 0, "{id}: {:#?}", r.problems);
        }
    }

    #[test]
    fn the_examples_hold_their_own_numbers_to_tolerance() {
        // An example teaches by being right: its reported corners, aim and
        // distance have to be what its own geometry says, or the model
        // learns to be wrong by a fixed amount.
        for (id, v) in every_example() {
            let r = v.check();
            assert!(
                r.reprojection.is_some_and(|e| e <= REPROJECTION_TOLERANCE),
                "{id}: {r:?}"
            );
            // A venue with no projector (every output a screen) has no
            // throw to measure; one with a projector must measure it.
            if !v.projectors.is_empty() {
                assert!(
                    r.distance.is_some_and(|e| e <= DISTANCE_TOLERANCE),
                    "{id}: {r:?}"
                );
                assert!(r.angle.is_some_and(|e| e <= ANGLE_TOLERANCE), "{id}: {r:?}");
            }
            let geometric: Vec<_> = r
                .problems
                .iter()
                .filter(|p| !p.what.contains("brightness"))
                .collect();
            assert!(geometric.is_empty(), "{id}: {geometric:#?}");
        }
    }

    #[test]
    fn the_bar_is_indoors_with_two_shots_and_no_horizon() {
        let v = example("bar");
        assert_eq!(v.venue.setting, Setting::Indoor);
        assert!(v.shots.len() >= 2, "two cameras on one room");
        assert!(v.shots.iter().all(|s| s.horizon_path.is_empty()));
        let washes = v
            .fixtures
            .iter()
            .filter(|f| f.kind == FixtureKind::Wash)
            .count();
        assert_eq!(washes, 4, "four washes in a line");
        assert!(v.fixtures.iter().any(|f| f.kind == FixtureKind::LedStrip));
        assert!(v.objects.iter().any(|o| o.kind == "lamp" && o.emits > 0.0));
    }

    #[test]
    fn the_facade_is_outdoors_with_a_horizon_a_tree_and_two_spots() {
        let v = example("facade");
        assert_eq!(v.venue.setting, Setting::Outdoor);
        assert!(v.shots.iter().all(|s| s.horizon_path.len() >= 2));
        assert!(v
            .objects
            .iter()
            .any(|o| o.kind == "tree" && o.crown.is_some()));
        let spots: Vec<_> = v
            .fixtures
            .iter()
            .filter(|f| f.kind == FixtureKind::Spot)
            .collect();
        assert_eq!(spots.len(), 2);
        assert_ne!(
            spots[0].at, spots[1].at,
            "two spotlights in different places"
        );
    }

    #[test]
    fn a_tv_is_an_output_and_its_glass_is_its_pixels() {
        let v = example("living-room");
        let tv = v.display("tv").expect("the TV is a display");
        assert_eq!(tv.kind, DisplayKind::Tv);
        let masks = v.project(&tv.output).unwrap();
        assert_eq!(masks.len(), 1, "only the mask on the glass reaches the TV");
        let on_glass = v.masks.iter().find(|m| m.on == tv.surface).unwrap();
        assert_eq!(
            masks[0].points, on_glass.points,
            "identity: the glass's (s,t) is the output's 0..1"
        );
        let halo = v.project("proyector-sala").unwrap();
        assert!(
            halo.iter().all(|m| m.id != on_glass.id),
            "the projector never gets the TV's mask"
        );
        // The projector's frame covers the TV, but its axis lands on the wall.
        let p = v.projector("proj-halo").unwrap();
        assert_eq!(v.throw(p).unwrap().unwrap().0, "tv-wall");
    }

    #[test]
    fn five_screens_split_one_picture_by_where_their_glass_is() {
        let v = example("desk");
        assert!(v.projectors.is_empty());
        assert_eq!(v.displays.len(), 5);
        assert_eq!(
            v.displays.iter().filter(|d| d.link == Link::Peer).count(),
            3,
            "three phones as peers"
        );
        assert!(v
            .shots
            .iter()
            .all(|s| s.projection.is_none() && s.projector_distance == 0.0));
        let spans = v.spans(&v.compositions[0]).unwrap();
        assert_eq!(spans.len(), 5);
        let at = |o: &str| spans.iter().find(|(out, _)| out == o).unwrap().1;
        let (laptop, monitor) = (at("laptop"), at("monitor"));
        assert!(
            laptop.x.abs() < 1e-4 && monitor.y.abs() < 1e-4,
            "the laptop is leftmost, the monitor topmost"
        );
        assert!(
            laptop.y > 0.5 && monitor.x > 0.45,
            "laptop lower left, monitor upper right"
        );
        // The phones stand in front of the monitor's bottom edge, left to right.
        let phones = [at("phone-1"), at("phone-2"), at("phone-3")];
        assert!(phones.windows(2).all(|w| w[0].x < w[1].x));
        assert!(phones
            .iter()
            .all(|p| p.x > monitor.x && p.x + p.w < monitor.x + monitor.w + 1e-4));
        // Together the windows reach every edge of the picture.
        let right = spans.iter().map(|(_, s)| s.x + s.w).fold(0.0f32, f32::max);
        let bottom = spans.iter().map(|(_, s)| s.y + s.h).fold(0.0f32, f32::max);
        assert!((right - 1.0).abs() < 1e-4 && (bottom - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_screen_and_a_projector_on_one_output_is_an_error() {
        let mut v = example("living-room");
        v.displays[0].output = "proyector-sala".into();
        assert!(v
            .check()
            .problems
            .iter()
            .any(|p| p.level == Level::Error && p.what.contains("driven twice")));
    }

    #[test]
    fn a_camera_expects_each_mask_where_the_geometry_puts_it() {
        let v = example("living-room");
        let shot = v.shot("couch-view").unwrap();
        let seen = v.masks_in(shot);
        let glass = seen.iter().find(|(id, _)| id == "tv-picture").unwrap();
        let sighting = shot.sees.iter().find(|s| s.surface == "tv-screen").unwrap();
        for (a, b) in glass.1.iter().zip(sighting.corners.iter()) {
            assert!(
                (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01,
                "{a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn every_example_projects_each_mask_whole_into_its_projector() {
        for (id, v) in every_example() {
            // A mask on a display's glass is that display's, not the
            // projector's.
            let thrown = v
                .masks
                .iter()
                .filter(|m| !v.displays.iter().any(|d| d.surface == m.on))
                .count();
            for p in &v.projectors {
                let masks = v.project(&p.output).expect("compiles");
                assert_eq!(
                    masks.len(),
                    thrown,
                    "{id}: every mask lands on {}",
                    p.output
                );
                for m in &masks {
                    assert!(
                        m.points.iter().all(|q| in_frame(*q)),
                        "{id}/{}: {:?}",
                        m.id,
                        m.points
                    );
                }
            }
        }
    }

    #[test]
    fn a_projector_stands_on_its_table() {
        let v = example("bar");
        let p = &v.projectors[0];
        let table = v.object(&p.on).unwrap();
        let lens = v.lens(p).unwrap();
        assert!(
            (lens.y - (table.size[1] + p.lens[1])).abs() < 1e-5,
            "{lens:?}"
        );
    }

    #[test]
    fn a_wall_head_on_is_hit_at_its_distance() {
        let v = Venue {
            venue: Meta {
                id: "t".into(),
                name: "t".into(),
                setting: Setting::Indoor,
                based_on: vec![],
                notes: String::new(),
            },
            shots: vec![],
            surfaces: vec![Surface {
                id: "wall".into(),
                name: "wall".into(),
                origin: [-2.0, 3.0, -5.0],
                u: [4.0, 0.0, 0.0],
                v: [0.0, -3.0, 0.0],
                material: String::new(),
                form: SurfaceForm::Plane,
                sectors: Default::default(),
            }],
            objects: vec![Object {
                id: "table".into(),
                name: "table".into(),
                kind: "table".into(),
                at: [0.0, 0.0, 0.0],
                size: [0.6, 0.7, 0.6],
                yaw: 0.0,
                crown: None,
                occludes: true,
                emits: 0.0,
                faces: vec![],
                description: String::new(),
                form: ObjectForm::Box,
                parent: None,
                attribution: String::new(),
                source: String::new(),
            }],
            projectors: vec![Projector {
                id: "p".into(),
                name: "p".into(),
                output: "out".into(),
                on: "table".into(),
                lens: [0.0, 0.1, 0.0],
                yaw: 0.0,
                pitch: 0.0,
                throw_ratio: 1.0,
                aspect: 16.0 / 9.0,
                lumens: None,
                pins: BTreeMap::new(),
                blend: Default::default(),
            }],
            fixtures: vec![],
            masks: vec![],
            displays: vec![],
            compositions: vec![],
            sensors: vec![],
        };
        let (hit, d, s, t) = v.throw(&v.projectors[0]).unwrap().unwrap();
        assert_eq!(hit, "wall");
        assert!((d - 5.0).abs() < 1e-4, "{d}");
        assert!(
            (s - 0.5).abs() < 1e-4 && (t - (3.0 - 0.8) / 3.0).abs() < 1e-4,
            "{s} {t}"
        );
    }

    #[test]
    fn box_faces_face_outward_with_a_top_left_origin() {
        let o = Object {
            id: "c".into(),
            name: "c".into(),
            kind: "counter".into(),
            at: [0.0, 0.0, 0.0],
            size: [2.0, 1.0, 0.5],
            yaw: 0.0,
            crown: None,
            occludes: true,
            emits: 0.0,
            faces: vec!["front".into()],
            description: String::new(),
            form: ObjectForm::Box,
            parent: None,
            attribution: String::new(),
            source: String::new(),
        };
        let f = o.face("front").unwrap();
        assert_eq!(f.origin, [-1.0, 1.0, 0.25]);
        // `u` runs right and `v` down as the viewer sees it, so the side
        // facing the viewer is `v × u`.
        let n = Vec3::from(f.v).cross(Vec3::from(f.u));
        assert!(n.z > 0.0, "front normal points +z, toward the room: {n:?}");
        // `yaw` turns clockwise seen from above, the same sense a camera's
        // yaw does: a front that faced +z faces −x after a quarter turn.
        let turned = Object { yaw: 90.0, ..o };
        let f = turned.face("front").unwrap();
        let n = Vec3::from(f.v).cross(Vec3::from(f.u));
        assert!(n.x < -0.9, "{n:?}");
    }

    fn form_object(form: ObjectForm) -> Object {
        Object {
            id: "o".into(),
            name: "o".into(),
            kind: "object".into(),
            at: [0.0, 0.0, 0.0],
            size: [2.0, 1.0, 0.6],
            yaw: 0.0,
            crown: None,
            occludes: true,
            emits: 0.0,
            faces: vec![],
            description: String::new(),
            form,
            parent: None,
            attribution: String::new(),
            source: String::new(),
        }
    }

    #[test]
    fn a_box_mesh_bbox_equals_its_size() {
        let o = form_object(ObjectForm::Box);
        let (lo, hi) = o.bbox().unwrap();
        assert_eq!(lo, [-1.0, 0.0, -0.3]);
        assert_eq!(hi, [1.0, 1.0, 0.3]);
    }

    #[test]
    fn a_swept_object_has_a_mesh() {
        let o = form_object(ObjectForm::Swept {
            path: vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            profile: vec![[-0.2, 0.0], [0.2, 0.0], [0.2, 0.2], [-0.2, 0.2]],
            caps: true,
            closed: true,
            kind: "polyline".into(),
        });
        assert!(!o.mesh().positions.is_empty());
        assert!(o.bbox().is_some());
    }

    #[test]
    fn a_capsule_object_has_a_mesh() {
        let o = form_object(ObjectForm::Capsule {
            radius: 0.22,
            height: 1.8,
        });
        let m = o.mesh();
        assert!(!m.positions.is_empty());
        assert!(!m.indices.is_empty());
        let (_, hi) = o.bbox().unwrap();
        assert!(hi[1] >= 1.8);
    }

    #[test]
    fn an_old_box_object_without_new_keys_loads_as_a_box() {
        let old = serde_json::json!({
            "id": "table", "name": "table", "kind": "table",
            "at": [0.0, 0.0, 0.0], "size": [1.0, 0.8, 0.6],
            "yaw": 0.0, "occludes": true, "emits": 0.0,
            "faces": [], "description": ""
        });
        let o: Object = serde_json::from_value(old).unwrap();
        assert_eq!(o.form, ObjectForm::Box);
        assert!(o.parent.is_none());
        assert!(o.attribution.is_empty() && o.source.is_empty());
    }

    #[test]
    fn an_object_between_projector_and_mask_is_reported() {
        let mut v = example("bar");
        let p = v.projectors[0].clone();
        let lens = v.lens(&p).unwrap();
        let axis = direction(p.yaw, p.pitch);
        // A pillar straight down the projector's axis, a metre out.
        let mid = lens + axis * 1.0;
        v.objects.push(Object {
            id: "pillar".into(),
            name: "pillar".into(),
            kind: "pillar".into(),
            at: [mid.x, 0.0, mid.z],
            size: [1.2, 3.0, 0.3],
            yaw: 0.0,
            crown: None,
            occludes: true,
            emits: 0.0,
            faces: vec![],
            description: String::new(),
            form: ObjectForm::Box,
            parent: None,
            attribution: String::new(),
            source: String::new(),
        });
        let r = v.check();
        assert!(
            r.problems.iter().any(|p| p.what.contains("behind pillar")),
            "{:#?}",
            r.problems
        );
    }

    #[test]
    fn an_indoor_horizon_is_an_error() {
        let mut v = example("bar");
        v.shots[0].horizon_path = vec![[0.0, 0.4], [1.0, 0.4]];
        let r = v.check();
        assert!(
            r.errors() > 0 && r.problems[0].what.contains("horizon"),
            "{:#?}",
            r.problems
        );
    }

    #[test]
    fn a_misread_distance_is_measured_not_trusted() {
        let mut v = example("bar");
        v.shots[0].projector_distance *= 1.5;
        let r = v.check();
        assert!(r.distance.unwrap() > DISTANCE_TOLERANCE);
        assert!(r.problems.iter().any(|p| p.what.contains("throw measures")));
    }

    #[test]
    fn an_led_strip_has_one_pixel_per_step_of_its_length() {
        let v = example("bar");
        let strip = v
            .fixtures
            .iter()
            .find(|f| f.kind == FixtureKind::LedStrip)
            .unwrap();
        let pixels = v.strip(strip).unwrap();
        let wall = v
            .walls()
            .into_iter()
            .find(|w| Some(&w.id) == strip.on.as_ref())
            .unwrap();
        let mut pts: Vec<Vec3> = strip.path.iter().map(|p| wall.at(p[0], p[1])).collect();
        pts.push(pts[0]);
        let length: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        assert_eq!(pixels.len(), (length * strip.pixels_per_m).round() as usize);
        // And every one of them takes its colour from the frame.
        let samples = v.fixture_points(strip).unwrap();
        assert!(
            samples.iter().all(Option::is_some),
            "the strip frames the projection"
        );
    }

    #[test]
    fn a_fixture_reads_its_floats_from_the_frame_where_its_light_lands() {
        // A 2x1 frame, red on the left and blue on the right.
        let frame = [255, 0, 0, 255, 0, 0, 255, 255];
        assert_eq!(sample_rgb(&frame, 2, 1, Some([0.2, 0.5])), [1.0, 0.0, 0.0]);
        assert_eq!(sample_rgb(&frame, 2, 1, Some([0.9, 0.5])), [0.0, 0.0, 1.0]);
        assert_eq!(sample_rgb(&frame, 2, 1, None), [0.0; 3]);
        let [hue, sat, level] = light_floats([0.0, 0.0, 1.0]);
        assert!((hue - 2.0 / 3.0).abs() < 1e-5 && sat == 1.0 && level == 1.0);
        // The inverse round-trips a colour (F10/F11/F14 read a swatch back).
        for rgb in [
            [1.0, 0.0, 0.0],
            [0.2, 0.8, 0.4],
            [0.5, 0.5, 0.5],
            [0.0, 0.0, 1.0],
        ] {
            let [h, s, l] = light_floats(rgb);
            let back = rgb_from_light(h, s, l);
            for (a, b) in rgb.iter().zip(back) {
                assert!((a - b).abs() < 1e-4, "{rgb:?} -> {back:?}");
            }
        }
        // The bar's washes land inside the projection.
        let v = example("bar");
        for f in v.fixtures.iter().filter(|f| f.kind == FixtureKind::Wash) {
            assert!(
                v.fixture_points(f).unwrap()[0].is_some(),
                "{} lands in the frame",
                f.id
            );
        }
    }

    // -- M1: one pixel-map vocabulary ---------------------------------------

    /// Every `FixtureKind` has a footprint, and every emitter takes at least
    /// one sample of the frame (a camera is a sensor and takes none).
    #[test]
    fn every_fixture_kind_has_a_footprint() {
        let v = example("bar");
        let base = v
            .fixtures
            .iter()
            .find(|f| f.kind == FixtureKind::Wash)
            .unwrap()
            .clone();
        let kinds = [
            FixtureKind::Wash,
            FixtureKind::Spot,
            FixtureKind::LedStrip,
            FixtureKind::Moving,
            FixtureKind::Backlight,
            FixtureKind::Panel,
            FixtureKind::Continuous,
            FixtureKind::LedBar,
            FixtureKind::Camera,
            FixtureKind::Matrix,
        ];
        for kind in kinds {
            let mut f = base.clone();
            f.id = format!("probe-{kind:?}").to_lowercase();
            f.kind = kind;
            f.on = Some("back-wall".into());
            f.path = vec![[0.2, 0.2], [0.8, 0.2]];
            f.pixels = Some(8);
            f.beam = 40.0;
            f.matrix = Some(Matrix::new(2, 2));
            let fp = v.footprint(&f).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert!(
                matches!(
                    fp.shape,
                    Shape::Point | Shape::Line | Shape::Pixels { .. } | Shape::Sensor { .. }
                ),
                "{kind:?} has no footprint"
            );
            if kind != FixtureKind::Camera {
                let pts = v.fixture_points(&f).unwrap();
                assert!(!pts.is_empty(), "{kind:?} must take a sample of the frame");
            }
        }
    }

    /// An LED string samples the authored `pixels` count along its polyline,
    /// and `order` walks the same pixels back.
    #[test]
    fn a_led_string_samples_n_pixels_along_its_polyline() {
        let v = example("bar");
        let mut strip = v
            .fixtures
            .iter()
            .find(|f| f.kind == FixtureKind::LedStrip)
            .unwrap()
            .clone();
        strip.pixels = Some(12);
        let pixels = v.strip(&strip).unwrap();
        assert_eq!(
            pixels.len(),
            12,
            "the authored pixel count, not pixels_per_m"
        );
        let wall = v
            .walls()
            .into_iter()
            .find(|w| Some(&w.id) == strip.on.as_ref())
            .unwrap();
        let n = Vec3::from(wall.u).cross(Vec3::from(wall.v)).normalize();
        for p in &pixels {
            assert!(
                ((*p - Vec3::from(wall.origin)).dot(n)).abs() < 1e-4,
                "{p:?} is off the wall"
            );
        }
        strip.order = PixelOrder::Reverse;
        let mut reversed = v.strip(&strip).unwrap();
        reversed.reverse();
        for (a, b) in pixels.iter().zip(reversed.iter()) {
            assert!((*a - *b).length() < 1e-4, "{a:?} vs {b:?}");
        }
    }

    // -- M2: live state of rotating and moving fixtures ---------------------

    #[test]
    fn aiming_at_a_point_gives_pan_tilt_within_limits() {
        let v = example("bar");
        let mut head = v
            .fixtures
            .iter()
            .find(|f| f.kind == FixtureKind::Wash)
            .unwrap()
            .clone();
        head.kind = FixtureKind::Moving;
        head.yaw = 0.0;
        head.pitch = 0.0;
        let target = [0.0, 1.5, -5.0];
        let (pan, tilt) = v.aim(&head, target).unwrap();
        let limits = crate::fixture::Limits::default();
        assert!(
            pan >= limits.pan[0] && pan <= limits.pan[1],
            "pan {pan} outside {:?}",
            limits.pan
        );
        assert!(
            tilt >= limits.tilt[0] && tilt <= limits.tilt[1],
            "tilt {tilt} outside {:?}",
            limits.tilt
        );
        // It points at the target: the ray's direction matches.
        let at = Vec3::from(head.at.unwrap());
        let want = (Vec3::from(target) - at).normalize();
        assert!(
            direction(pan, tilt).dot(want) > 0.999,
            "{:?} vs {want:?}",
            direction(pan, tilt)
        );
    }

    #[test]
    fn the_estimated_pan_lags_the_command_by_the_profile_speed() {
        let v = example("bar");
        let f = v
            .fixtures
            .iter()
            .find(|f| f.kind == FixtureKind::Wash)
            .unwrap();
        let mut st = FixtureState::new(f);
        st.commanded.pan = st.actual.pan + 90.0;
        let limits = crate::fixture::Limits {
            speed: 180.0,
            ..Default::default()
        };
        st.step(&limits, 0.25);
        assert!(
            (st.actual.pan - (st.commanded.pan - 45.0)).abs() < 1e-3,
            "{}",
            st.actual.pan
        );
        assert!(
            st.actual.pan < st.commanded.pan,
            "the head lags the command"
        );
        st.step(&limits, 1.0);
        assert_eq!(st.actual.pan, st.commanded.pan, "and catches up");
    }

    #[test]
    fn a_head_aimed_at_a_surface_samples_that_surface() {
        let v = example("bar");
        let mut head = v
            .fixtures
            .iter()
            .find(|f| f.kind == FixtureKind::Wash)
            .unwrap()
            .clone();
        head.kind = FixtureKind::Moving;
        // A point in the middle of the left projected panel on the back wall.
        let wall = v.walls().into_iter().find(|w| w.id == "back-wall").unwrap();
        let target = wall.at(0.38, 0.30).to_array();
        let (pan, tilt) = v.aim(&head, target).unwrap();
        head.yaw = pan;
        head.pitch = tilt;
        let at = Vec3::from(head.at.unwrap());
        let hit = v
            .hit(at, direction(pan, tilt))
            .expect("the beam lands somewhere");
        assert_eq!(hit.what, "back-wall", "aimed at the wall: {hit:?}");
        let pts = v.fixture_points(&head).unwrap();
        assert_eq!(pts.len(), 1);
        assert!(pts[0].is_some(), "the landing point is inside the frame");
    }

    #[test]
    fn a_venue_id_cannot_climb_out_of_its_folder() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        assert!(load(&root, "../outputs").is_err());
        assert!(load(&root, "bar").is_ok());
        assert!(list(&root).contains(&"facade".to_string()));
    }

    // -- D5: solve a head at a real point, and the ray query it lands on ----

    /// A head aimed at a point on a wall and read back through the ray query
    /// lands on that point within a tolerance.
    #[test]
    fn solving_a_head_aims_it_at_the_point_and_the_ray_reads_it_back() {
        let v = example("bar");
        let mut head = v
            .fixtures
            .iter()
            .find(|f| f.kind == FixtureKind::Wash)
            .unwrap()
            .clone();
        head.kind = FixtureKind::Moving;
        let wall = v.walls().into_iter().find(|w| w.id == "back-wall").unwrap();
        let target = wall.at(0.38, 0.30).to_array();
        let state = v
            .solve_head(&head.id, target, &crate::fixture::Limits::default())
            .expect("the head solves");
        let at = Vec3::from(head.at.unwrap());
        let hit = v
            .hit(at, direction(state.actual.pan, state.actual.tilt))
            .expect("the beam lands somewhere");
        assert_eq!(hit.what, "back-wall");
        for k in 0..3 {
            assert!(
                (hit.at[k] - target[k]).abs() < 1e-3,
                "{:?} vs {target:?}",
                hit.at
            );
        }
        // The commanded and the reached pose carry the same solved pan/tilt.
        assert_eq!(state.commanded.pan, state.actual.pan);
        assert_eq!(state.commanded.tilt, state.actual.tilt);
    }

    /// A head hung high and a head hung low both solve onto the same target.
    #[test]
    fn a_high_head_and_a_low_head_both_solve_onto_the_target() {
        let mut v = example("bar");
        let base = v
            .fixtures
            .iter()
            .find(|f| f.kind == FixtureKind::Wash)
            .unwrap()
            .clone();
        let target = [3.0, 2.5, -5.0];
        let limits = crate::fixture::Limits::default();
        for y in [2.8, 0.4] {
            let mut head = base.clone();
            head.kind = FixtureKind::Moving;
            head.at = Some([3.0, y, -2.0]);
            head.yaw = 0.0;
            head.pitch = 0.0;
            v.fixtures = vec![head.clone()];
            let state = v
                .solve_head(&head.id, target, &limits)
                .expect("the head solves");
            let hit = v
                .hit(
                    Vec3::from(head.at.unwrap()),
                    direction(state.actual.pan, state.actual.tilt),
                )
                .expect("the beam lands");
            for k in 0..3 {
                assert!(
                    (hit.at[k] - target[k]).abs() < 1e-3,
                    "y={y}: {:?} vs {target:?}",
                    hit.at
                );
            }
        }
    }

    /// A target the head cannot reach is clamped to its pan/tilt limits.
    #[test]
    fn solving_a_head_clamps_the_pose_into_the_profiles_limits() {
        let mut v = example("bar");
        let mut head = v
            .fixtures
            .iter()
            .find(|f| f.kind == FixtureKind::Wash)
            .unwrap()
            .clone();
        head.kind = FixtureKind::Moving;
        head.at = Some([0.0, 1.0, 0.0]);
        head.yaw = 0.0;
        head.pitch = 0.0;
        v.fixtures = vec![head.clone()];
        let limits = crate::fixture::Limits {
            pan: [-45.0, 45.0],
            tilt: [-20.0, 20.0],
            ..Default::default()
        };
        let state = v
            .solve_head(&head.id, [100.0, -100.0, 5.0], &limits)
            .unwrap();
        assert_eq!(state.actual.pan, 45.0, "pan clamped to its stop");
        assert_eq!(state.actual.tilt, -20.0, "tilt clamped to its stop");
    }

    /// The ray query hits the expected surface with a normal facing the
    /// emitter, and a ray pointing at nothing (a wall behind the origin)
    /// returns `None`.
    #[test]
    fn the_ray_query_hits_the_near_surface_with_a_normal_and_ignores_behind() {
        let v = example("bar");
        // Inside the room, aimed at the back wall some metres away.
        let from = Vec3::new(4.0, 1.5, -1.0);
        let hit = v
            .hit(from, Vec3::new(0.0, 0.0, -1.0))
            .expect("the back wall");
        assert_eq!(hit.what, "back-wall");
        assert!((hit.distance - 4.0).abs() < 1e-3, "{}", hit.distance);
        // The wall's lit side faces the room, so the normal comes back +z.
        assert!(hit.normal[2] > 0.9, "{:?}", hit.normal);
        // A ray away from every wall (toward +z, out of the room) hits nothing.
        assert!(v.hit(from, Vec3::new(0.0, 0.0, 1.0)).is_none());
        // An object face is hit by the ray/triangle test, not just surfaces.
        let front = v
            .hit(Vec3::new(4.0, 0.5, -3.0), Vec3::new(0.0, 0.0, -1.0))
            .expect("the counter front");
        assert_eq!(front.what, "counter.front");
        assert!(front.normal[2] > 0.9, "{:?}", front.normal);
    }

    // -- image points as grid cells -----------------------------------------

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
    }

    #[test]
    fn a_cell_is_the_centre_of_the_ninth_its_part_names() {
        let n = GRID as f32;
        // The middle of the top-left cell, then its top-left and bottom-right ninths.
        assert!(near(cell_point(1, 1, 5).unwrap(), [0.5 / n, 0.5 / n]));
        assert!(near(
            cell_point(1, 1, 1).unwrap(),
            [1.0 / 6.0 / n, 1.0 / 6.0 / n]
        ));
        assert!(near(
            cell_point(1, 1, 9).unwrap(),
            [5.0 / 6.0 / n, 5.0 / 6.0 / n]
        ));
        // Keypad order: 3 is top-right, 7 bottom-left.
        assert!(near(
            cell_point(4, 6, 3).unwrap(),
            [(3.0 + 5.0 / 6.0) / n, (5.0 + 1.0 / 6.0) / n]
        ));
        assert!(near(
            cell_point(4, 6, 7).unwrap(),
            [(3.0 + 1.0 / 6.0) / n, (5.0 + 5.0 / 6.0) / n]
        ));
        // The bottom-right cell's centre, and the grid going on past the edges.
        assert!(near(
            cell_point(GRID as i32, GRID as i32, 5).unwrap(),
            [1.0 - 0.5 / n, 1.0 - 0.5 / n]
        ));
        let [x, y] = cell_point(0, GRID as i32 + 1, 5).unwrap();
        assert!(x < 0.0 && y > 1.0 && near([x, y], [-0.5 / n, 1.0 + 0.5 / n]));
        assert!(cell_point(-3, 1, 5).unwrap()[0] < -3.0 / n);
        // A part that is not a key.
        assert!(cell_point(1, 1, 0).is_err());
        assert!(cell_point(1, 1, 10).is_err());
    }

    #[test]
    fn every_point_reads_back_as_the_cell_it_falls_in() {
        for c in -2..=GRID as i32 + 3 {
            for r in -2..=GRID as i32 + 3 {
                for part in 1..=9u8 {
                    let p = cell_point(c, r, part).unwrap();
                    assert_eq!(point_cell(p), [c, r, part as i32], "{p:?}");
                }
            }
        }
        // A reading never sits more than 1/54 from the point along an axis.
        for p in [
            [0.0, 0.0],
            [0.013, 0.999],
            [0.5, 0.25],
            [-0.31, 1.27],
            [0.9999, 0.0001],
        ] {
            let [c, r, k] = point_cell(p);
            let q = cell_point(c, r, k as u8).unwrap();
            let half = 0.5 / (3.0 * GRID as f32) + 1e-6;
            assert!(
                (p[0] - q[0]).abs() <= half && (p[1] - q[1]).abs() <= half,
                "{p:?} -> {q:?}"
            );
        }
    }

    /// The parse is where cells become coordinates: a sighting and a
    /// horizon path may mix both forms, told apart by their length.
    #[test]
    fn a_shot_reads_cells_and_coordinates_alike() {
        let text = r#"
            [venue]
            id = "t"
            name = "t"
            setting = "outdoor"

            [[shot]]
            id = "a"
            image = "a.jpg"
            camera = { eye = [0.0, 1.5, 0.0], yaw = 0.0, pitch = 0.0, fov = 50.0, aspect = 1.5 }
            floor_brightness = 0.2
            background_brightness = 0.1
            horizon_path = [[1, 3, 4], [0.5, 0.3], [9, 3, 6]]

            [[shot.sees]]
            surface = "w"
            corners = [[2, 2, 1], [8, 2, 3], [0.9, 0.8], [2, 8, 7]]
        "#;
        let v = Venue::parse(text).unwrap();
        let s = &v.shots[0];
        assert!(near(s.horizon_path[0], cell_point(1, 3, 4).unwrap()));
        assert_eq!(s.horizon_path[1], [0.5, 0.3]);
        assert!(near(s.horizon_path[2], cell_point(9, 3, 6).unwrap()));
        let c = s.sees[0].corners;
        assert!(
            near(c[0], cell_point(2, 2, 1).unwrap()) && near(c[1], cell_point(8, 2, 3).unwrap())
        );
        assert_eq!(c[2], [0.9, 0.8]);
        assert!(near(c[3], cell_point(2, 8, 7).unwrap()));
        // What is not a point or a cell is refused at the parse, not guessed.
        for bad in ["[2, 2, 0]", "[2.5, 2, 5]", "[1, 2, 3, 4]", "[0.5]"] {
            let t = text.replace("[2, 2, 1]", bad);
            assert!(Venue::parse(&t).is_err(), "{bad} should not parse");
        }
    }

    /// The bar is written in cells now. Its raw twin (the same venue with
    /// every point as the coordinate its cell stands for, what `GET /venue`
    /// serves and `look_venue` reads back) parses to the same venue and
    /// checks the same, to the last number.
    #[test]
    fn a_venue_written_in_cells_checks_like_its_raw_twin() {
        let text = metal_vfs::read_to_string(examples().join("bar.toml")).unwrap();
        assert!(
            text.contains("corners = [[0, 2, 7]"),
            "bar.toml reads its corners as cells"
        );
        let cells = Venue::parse(&text).unwrap();
        let mut raw = text.clone();
        for shot in &cells.shots {
            for seen in &shot.sees {
                let [c0, c1, c2, c3] = seen.corners.map(point_cell);
                let written = format!("corners = [{c0:?}, {c1:?}, {c2:?}, {c3:?}]");
                assert!(raw.contains(&written), "{written}");
                raw = raw.replacen(&written, &format!("corners = {:?}", seen.corners), 1);
            }
        }
        assert!(!raw.contains("corners = [[0, 2, 7]"));
        let twin = Venue::parse(&raw).unwrap();
        assert_eq!(twin, cells);
        let (a, b) = (cells.check(), twin.check());
        assert_eq!(a, b);
        assert_eq!(a.errors(), 0, "{:#?}", a.problems);
        assert!(
            a.reprojection.is_some_and(|e| e <= REPROJECTION_TOLERANCE),
            "{a:?}"
        );
        // And through JSON, the way `GET /venue` hands it to `look_venue`.
        let json: Venue = serde_json::from_value(serde_json::to_value(&cells).unwrap()).unwrap();
        assert_eq!(json, cells);
    }

    // -- the stage venue ----------------------------------------------------

    #[test]
    fn the_stage_parses_and_checks_without_errors() {
        let v = example("stage");
        assert_eq!(v.venue.id, "stage");
        assert_eq!(v.venue.setting, Setting::Indoor);
        let r = v.check();
        assert_eq!(r.errors(), 0, "{:#?}", r.problems);
        assert_eq!(r.problems, vec![], "the designed viewpoints are exact");
        assert_eq!(r.reprojection, Some(0.0));
    }

    #[test]
    fn the_stage_has_exactly_the_three_led_wall_tiles() {
        let v = example("stage");
        let walls: Vec<_> = v
            .displays
            .iter()
            .filter(|d| d.kind == DisplayKind::LedWall)
            .collect();
        assert_eq!(walls.len(), 3, "three tiles, one picture");
        let mut res: Vec<[u32; 2]> = walls.iter().map(|d| d.resolution).collect();
        res.sort();
        assert_eq!(res, [[1024, 256], [1024, 256], [1024, 1536]]);
        // Each tile is its own output, on its own surface.
        let outputs: std::collections::HashSet<_> = walls.iter().map(|d| &d.output).collect();
        assert_eq!(outputs.len(), 3, "each tile a distinct output");
        assert!(walls
            .iter()
            .all(|d| v.walls().iter().any(|w| w.id == d.surface)));
    }

    #[test]
    fn the_stage_wall_tiles_are_equal_width_and_stacked_a_over_b_over_c() {
        let v = example("stage");
        let tile = |id: &str| {
            v.walls()
                .into_iter()
                .find(|w| w.id == id)
                .expect("tile surface")
        };
        for id in ["wall-a", "wall-b", "wall-c"] {
            let w = tile(id);
            assert!(
                (Vec3::from(w.u).length() - 4.0).abs() < 1e-6,
                "{id} is 4 m wide"
            );
        }
        // A sits directly on B, B directly on C: the bottom edge of one is
        // the top edge of the next, so no band overlaps or leaves a gap.
        let (a, b, c) = (tile("wall-a"), tile("wall-b"), tile("wall-c"));
        let top = |w: &Wall| w.at(0.0, 0.0).y;
        let bottom = |w: &Wall| w.at(0.0, 1.0).y;
        assert!((bottom(&a) - top(&b)).abs() < 1e-5, "A over B");
        assert!((bottom(&b) - top(&c)).abs() < 1e-5, "B over C");
        // The heights are in the ratio 256 : 1536 : 256.
        let h = |w: &Wall| Vec3::from(w.v).length();
        assert!(
            (h(&a) - 1.0).abs() < 1e-5 && (h(&b) - 6.0).abs() < 1e-5 && (h(&c) - 1.0).abs() < 1e-5
        );
        assert!((h(&b) / h(&a) - 6.0).abs() < 1e-5, "1536 : 256");
    }

    #[test]
    fn the_stage_composition_covers_all_three_tiles_from_a_designed_shot() {
        let v = example("stage");
        assert_eq!(v.compositions.len(), 1);
        let c = &v.compositions[0];
        let mut ids: Vec<&str> = c.displays.iter().map(String::as_str).collect();
        ids.sort();
        assert_eq!(ids, ["wall-a", "wall-b", "wall-c"]);
        assert!(
            v.shot(&c.seen_from).is_some(),
            "seen from a shot that exists"
        );
        // Every tile gets a window of the one picture, stacked.
        let spans = v.spans(c).unwrap();
        assert_eq!(spans.len(), 3);
        let at = |o: &str| spans.iter().find(|(out, _)| out == o).unwrap().1;
        assert!(
            at("wall-a").y < at("wall-b").y && at("wall-b").y < at("wall-c").y,
            "A above B above C"
        );
        // Together the three windows are the whole picture; the main tile is
        // six times the height of a band (the 1536:256 ratio), and the two
        // bands are equal.
        let left = spans.iter().map(|(_, s)| s.x).fold(f32::MAX, f32::min);
        let right = spans
            .iter()
            .map(|(_, s)| s.x + s.w)
            .fold(f32::MIN, f32::max);
        assert!(
            left.abs() < 1e-3 && (right - 1.0).abs() < 1e-3,
            "{left} {right}"
        );
        let (ha, hb, hc) = (at("wall-a").h, at("wall-b").h, at("wall-c").h);
        assert!(
            (ha + hb + hc - 1.0).abs() < 1e-3,
            "the three fill the picture: {ha} {hb} {hc}"
        );
        assert!(
            (hb / ha - 6.0).abs() < 0.5,
            "main about six times a band: {hb} {ha}"
        );
        assert!((ha - hc).abs() < 0.05, "the two bands match: {ha} {hc}");
    }

    #[test]
    fn the_stage_fixtures_are_bluetooth_lights_and_dmx_moves() {
        let v = example("stage");
        let kind = |k: FixtureKind| v.fixtures.iter().filter(|f| f.kind == k).count();
        assert_eq!(
            kind(FixtureKind::Moving),
            4,
            "four pan/tilt heads on the comb"
        );
        assert_eq!(
            kind(FixtureKind::Backlight),
            3,
            "three RGB backlights on the rear batten"
        );
        assert!(kind(FixtureKind::Wash) >= 5, "keys and fills");
        // The Bluetooth lights name Godox profiles; the moves ride DMX.
        let bt: Vec<_> = v
            .fixtures
            .iter()
            .filter(|f| f.wire == "bluetooth")
            .collect();
        assert!(bt.len() >= 8, "keys, fills and backlights over Bluetooth");
        assert!(bt.iter().all(|f| f
            .profile
            .as_deref()
            .is_some_and(|p| p.starts_with("godox-"))));
        assert!(v
            .fixtures
            .iter()
            .filter(|f| f.kind == FixtureKind::Moving)
            .all(|f| f.wire == "dmx" && f.profile.as_deref() == Some("moving-head-beam")));
        // The comb: five parallel battens above the deck.
        let battens = v.objects.iter().filter(|o| o.kind == "batten").count();
        assert_eq!(battens, 5);
        assert!(v
            .objects
            .iter()
            .filter(|o| o.kind == "batten")
            .all(|o| (o.at[1] - 7.1).abs() < 1e-6));
    }

    // -- H2: the capture sensor and viewer_from ----------------------------

    fn with_sensor() -> Venue {
        let mut v = example("bar");
        v.sensors.push(Sensor {
            id: "cam".into(),
            name: "capture".into(),
            at: [0.0, 1.6, 2.0],
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            fov: 60.0,
            aspect: 16.0 / 9.0,
            capture: Some("/dev/video0".into()),
        });
        v
    }

    #[test]
    fn a_venue_written_before_sensors_loads_with_none() {
        let v = example("bar");
        assert!(v.sensors.is_empty());
        assert!(v.sensor("cam").is_none());
        assert!(v.viewer_from("cam", [0.5, 0.5, 0.5]).is_none());
    }

    #[test]
    fn a_centred_reading_stands_at_the_sensor() {
        let v = with_sensor();
        let at = v.viewer_from("cam", [0.5, 0.5, 0.5]).unwrap();
        assert!(
            (at[0] - 0.0).abs() < 1e-5 && (at[1] - 1.6).abs() < 1e-5 && (at[2] - 2.0).abs() < 1e-5,
            "{at:?}"
        );
    }

    #[test]
    fn a_reading_right_of_centre_moves_the_viewer_to_the_sensor_right() {
        let v = with_sensor();
        let centre = v.viewer_from("cam", [0.5, 0.5, 0.5]).unwrap();
        let right = v.viewer_from("cam", [1.0, 0.5, 0.5]).unwrap();
        // yaw 0 looks down -z, so the sensor's right is +x.
        assert!(
            right[0] > centre[0],
            "a right reading moves +x, got {right:?}"
        );
        assert!(
            (right[2] - centre[2]).abs() < 1e-5,
            "no depth change from x"
        );
    }

    #[test]
    fn a_closer_reading_moves_the_viewer_toward_the_surface() {
        let v = with_sensor();
        let centre = v.viewer_from("cam", [0.5, 0.5, 0.5]).unwrap();
        let near = v.viewer_from("cam", [0.5, 0.5, 1.0]).unwrap();
        // forward is -z from yaw 0, so toward the surface is -z.
        assert!(
            near[2] < centre[2],
            "closer moves along forward, got {near:?}"
        );
    }

    // -- P0: the scene folded in -------------------------------------------

    /// A 2x2 wall at `z = 0` with a projector one metre in front framing it
    /// exactly, plus the hand-written `Escena` the venue is meant to be the
    /// same room as. The two are built from the same numbers so the tests
    /// compare the *representations*, not two independent guesses.
    fn one_room() -> (Venue, Escena) {
        let wall = Wall {
            id: "front".into(),
            name: "frente".into(),
            origin: [-1.0, 1.0, 0.0],
            u: [2.0, 0.0, 0.0],
            v: [0.0, -2.0, 0.0],
            ..Default::default()
        };
        let view = View {
            output: "proj".into(),
            eye: [0.0, 0.0, 1.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov: 90.0,
            aspect: 1.0,
        };
        let points = vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]];
        let venue = Venue {
            venue: Meta {
                id: "room".into(),
                name: "room".into(),
                setting: Setting::Indoor,
                based_on: vec![],
                notes: String::new(),
            },
            shots: vec![],
            sensors: vec![],
            surfaces: vec![Surface {
                id: "front".into(),
                name: "frente".into(),
                origin: [-1.0, 1.0, 0.0],
                u: [2.0, 0.0, 0.0],
                v: [0.0, -2.0, 0.0],
                material: String::new(),
                form: SurfaceForm::Plane,
                sectors: Default::default(),
            }],
            objects: vec![Object {
                id: "table".into(),
                name: "table".into(),
                kind: "table".into(),
                at: [0.0, 0.0, 1.0],
                size: [0.0, 0.0, 0.0],
                yaw: 0.0,
                crown: None,
                occludes: false,
                emits: 0.0,
                faces: vec![],
                description: String::new(),
                form: ObjectForm::Box,
                parent: None,
                attribution: String::new(),
                source: String::new(),
            }],
            projectors: vec![Projector {
                id: "proj".into(),
                name: "proj".into(),
                output: "proj".into(),
                on: "table".into(),
                lens: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
                throw_ratio: 0.5,
                aspect: 1.0,
                lumens: None,
                pins: BTreeMap::new(),
                blend: Default::default(),
            }],
            fixtures: vec![],
            masks: vec![VenueMask {
                id: "m".into(),
                name: "m".into(),
                on: "front".into(),
                points: points.clone(),
                rgb: None,
                content: String::new(),
                parent: None,
                alpha: 1.0,
            }],
            displays: vec![],
            compositions: vec![],
        };
        let scene = Escena {
            walls: vec![wall],
            views: vec![view],
            masks: vec![WallMask {
                id: "m".into(),
                name: "m".into(),
                wall: "front".into(),
                points,
                parent: None,
                alpha: 1.0,
            }],
        };
        (venue, scene)
    }

    fn views_match(a: &View, b: &View) -> bool {
        let close3 = |x: [f32; 3], y: [f32; 3]| (0..3).all(|i| (x[i] - y[i]).abs() < 1e-5);
        a.output == b.output
            && close3(a.eye, b.eye)
            && close3(a.target, b.target)
            && close3(a.up, b.up)
            && (a.fov - b.fov).abs() < 1e-4
            && (a.aspect - b.aspect).abs() < 1e-5
    }

    fn masks_match(a: &[Mask], b: &[Mask]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b.iter()).all(|(x, y)| {
                x.id == y.id
                    && x.parent == y.parent
                    && (x.alpha - y.alpha).abs() < 1e-5
                    && x.points.len() == y.points.len()
                    && x.points
                        .iter()
                        .zip(y.points.iter())
                        .all(|(p, q)| (p[0] - q[0]).abs() < 1e-4 && (p[1] - q[1]).abs() < 1e-4)
            })
    }

    #[test]
    fn project_gives_the_same_contour_as_the_old_escena() {
        let (v, old) = one_room();
        let from_venue = v.project("proj").expect("the venue projects");
        let from_escena = old.project("proj");
        assert!(
            masks_match(&from_venue, &from_escena),
            "{from_venue:?} vs {from_escena:?}"
        );

        // The new `Venue::homography` is the old `Escena::homography` for the
        // same room, and it is the projector's pose (not a pin set) it used.
        let wall = v.walls().into_iter().find(|w| w.id == "front").unwrap();
        let from_venue_h = v.homography("proj", "front").expect("a warp");
        let from_escena_h = old.homography("proj", &wall).expect("a warp");
        assert!(from_venue_h.distance(&from_escena_h).unwrap() < 1e-4);
    }

    #[test]
    fn several_coplanar_fits_solve_a_view() {
        // P2.4: a surface split in two, or a projector that only pinned the
        // neighbouring panel, still gives one camera. Two coplanar surfaces,
        // each pinned from the same pose, decompose back to that pose.
        let surface = |id: &str, ox: f32| Surface {
            id: id.into(),
            name: id.into(),
            origin: [ox, 1.0, 0.0],
            u: [2.0, 0.0, 0.0],
            v: [0.0, -2.0, 0.0],
            material: String::new(),
            form: SurfaceForm::Plane,
            sectors: Default::default(),
        };
        let front = surface("front", -1.0);
        let right = surface("right", 1.0);
        let base = Projector {
            id: "proj".into(),
            name: "proj".into(),
            output: "proj".into(),
            on: "table".into(),
            lens: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            throw_ratio: 0.5,
            aspect: 1.0,
            lumens: None,
            pins: BTreeMap::new(),
            blend: Default::default(),
        };
        let truth = View {
            output: "proj".into(),
            eye: [0.0, 0.0, 3.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov: base.fov(),
            aspect: 1.0,
        };
        let pins_for = |s: &Surface| {
            let w = s.wall();
            SURFACE_CORNERS.map(|[a, b]| {
                let p = truth.project(w.at(a, b)).expect("in front");
                [p.x, p.y]
            })
        };
        let mut pins = BTreeMap::new();
        pins.insert("front".to_string(), pins_for(&front));
        pins.insert("right".to_string(), pins_for(&right));
        let projector = Projector { pins, ..base };
        let venue = Venue {
            venue: Meta {
                id: "two".into(),
                name: "two".into(),
                setting: Setting::Indoor,
                based_on: vec![],
                notes: String::new(),
            },
            shots: vec![],
            sensors: vec![],
            surfaces: vec![front, right],
            objects: vec![Object {
                id: "table".into(),
                name: "table".into(),
                kind: "table".into(),
                at: [0.0, 0.0, 3.0],
                size: [0.0, 0.0, 0.0],
                yaw: 0.0,
                crown: None,
                occludes: false,
                emits: 0.0,
                faces: vec![],
                description: String::new(),
                form: ObjectForm::Box,
                parent: None,
                attribution: String::new(),
                source: String::new(),
            }],
            projectors: vec![projector],
            fixtures: vec![],
            masks: vec![],
            displays: vec![],
            compositions: vec![],
        };
        let wall = venue.walls().into_iter().find(|w| w.id == "front").unwrap();
        let h_truth = Homography::from_view(&truth, &wall);
        // One camera from both fits, and it is the pose the pins came from.
        let got = venue
            .view_from_fits("proj", "front")
            .expect("a view from two fits");
        let h_got = Homography::from_view(&got, &wall);
        assert!(
            h_truth.distance(&h_got).expect("both in front") < 1e-2,
            "two fits gave {got:?}"
        );
        // A single fit still yields the same camera.
        let one = venue
            .view_from_fit("proj", "right")
            .expect("a view from one fit");
        let h_one = Homography::from_view(&one, &wall);
        assert!(h_truth.distance(&h_one).expect("both in front") < 1e-2);
    }

    #[test]
    fn an_escena_round_trips_through_the_venue() {
        let (v, scene) = one_room();
        let back = v.scene().expect("the venue compiles to a scene");

        // The walls come back whole: same ids, same corners, same (default)
        // fit.
        assert_eq!(back.walls, scene.walls, "walls round-trip");
        // The view comes back as the projector's own pose.
        assert_eq!(back.views.len(), scene.views.len());
        assert!(
            views_match(&back.views[0], &scene.views[0]),
            "{:?}",
            back.views[0]
        );
        // The mask keeps its contour and the surface it is on.
        assert_eq!(back.masks.len(), scene.masks.len());
        assert_eq!(back.masks[0].id, scene.masks[0].id);
        assert_eq!(back.masks[0].wall, scene.masks[0].wall);
        assert_eq!(back.masks[0].points, scene.masks[0].points);
        assert_eq!(back.masks[0].alpha, scene.masks[0].alpha);
        // And projecting the round-tripped scene is projecting the venue.
        assert!(masks_match(&back.project("proj"), &scene.project("proj")));
    }

    fn swept_form() -> SurfaceForm {
        SurfaceForm::Swept {
            path: vec![[-2.0, 1.5, -1.0], [0.0, 2.0, -2.0], [2.0, 1.5, -1.0]],
            profile: vec![[-0.6, -0.6], [0.6, -0.6], [0.6, 0.6], [-0.6, 0.6]],
            caps: false,
            closed: true,
            kind: "catmull_rom".into(),
        }
    }

    fn swept_venue() -> Venue {
        Venue {
            venue: Meta {
                id: "curved".into(),
                name: "Curved".into(),
                setting: Setting::Indoor,
                based_on: Vec::new(),
                notes: String::new(),
            },
            shots: Vec::new(),
            surfaces: vec![Surface {
                id: "curtain".into(),
                name: "Curtain".into(),
                origin: [0.0, 0.0, 0.0],
                u: [4.0, 0.0, 0.0],
                v: [0.0, 2.0, 0.0],
                material: String::new(),
                form: swept_form(),
                sectors: Default::default(),
            }],
            objects: vec![Object {
                id: "table".into(),
                name: "table".into(),
                kind: "table".into(),
                at: [0.0, 0.0, 2.0],
                size: [1.0, 0.8, 1.0],
                yaw: 0.0,
                crown: None,
                occludes: false,
                emits: 0.0,
                faces: Vec::new(),
                description: String::new(),
                form: ObjectForm::Box,
                parent: None,
                attribution: String::new(),
                source: String::new(),
            }],
            projectors: vec![Projector {
                id: "proj".into(),
                name: "proj".into(),
                output: "proj".into(),
                on: "table".into(),
                lens: [0.0, 0.2, 0.0],
                yaw: 0.0,
                pitch: 5.0,
                throw_ratio: 0.5,
                aspect: 1.0,
                lumens: None,
                pins: BTreeMap::new(),
                blend: Default::default(),
            }],
            fixtures: Vec::new(),
            masks: Vec::new(),
            displays: Vec::new(),
            compositions: Vec::new(),
            sensors: Vec::new(),
        }
    }

    #[test]
    fn a_surface_written_without_a_form_is_the_old_plane() {
        let text = r#"
[venue]
id = "old"
name = "Old"
setting = "indoor"

[[surface]]
id = "wall"
name = "Wall"
origin = [1.0, 2.0, 3.0]
u = [4.0, 0.0, 0.0]
v = [0.0, -2.0, 0.0]
"#;
        let v = Venue::parse(text).expect("an old venue parses");
        let s = &v.surfaces[0];
        assert!(s.is_planar());
        assert_eq!(s.form, SurfaceForm::Plane);
        for (a, b) in [(0.0, 0.0), (0.25, 0.75), (1.0, 1.0)] {
            let got = s.at(a, b);
            let want = [
                s.origin[0] + a * s.u[0] + b * s.v[0],
                s.origin[1] + a * s.u[1] + b * s.v[1],
                s.origin[2] + a * s.u[2] + b * s.v[2],
            ];
            assert_eq!(got, want);
        }
    }

    #[test]
    fn a_swept_surface_has_a_mesh_and_finite_bounds() {
        let v = swept_venue();
        let s = &v.surfaces[0];
        assert!(!s.is_planar());
        let mesh = s.tessellate(16, 8);
        assert!(!mesh.positions.is_empty(), "a swept surface has vertices");
        assert!(!mesh.indices.is_empty(), "a swept surface has triangles");
        let (min, max) = s.bbox();
        assert!(min.iter().chain(max.iter()).all(|x| x.is_finite()));
        assert!(min[0] < max[0] && min[1] < max[1] && min[2] < max[2]);
    }

    #[test]
    fn project_gives_a_projected_mask_for_a_swept_surface() {
        let v = swept_venue();
        let masks = v.project("proj").expect("the venue projects");
        let m = masks
            .iter()
            .find(|m| m.id == "curtain.projected")
            .expect("the swept surface projects");
        assert!(m.points.len() >= 3);
        assert!(
            m.points
                .iter()
                .all(|p| (-1.0..=2.0).contains(&p[0]) && (-1.0..=2.0).contains(&p[1])),
            "{:?}",
            m.points
        );
        assert!(v.homography("proj", "curtain").is_none());
        assert!(!v.walls().iter().any(|w| w.id == "curtain"));
    }

    #[test]
    fn a_swept_form_round_trips_through_toml() {
        let v = swept_venue();
        let text = toml::to_string(&v).expect("serializes");
        let back = Venue::parse(&text).expect("parses");
        assert_eq!(back.surfaces[0].form, v.surfaces[0].form);
    }

    #[test]
    fn a_planar_surface_has_no_mesh_warp() {
        let mut v = swept_venue();
        v.surfaces[0].form = SurfaceForm::Plane;
        assert!(v.surfaces[0].is_planar());
        assert!(v.warp_mesh("proj", "curtain", 24, 12).is_none());
    }

    #[test]
    fn a_swept_surface_gets_a_mesh_warp_inside_the_frame() {
        let v = swept_venue();
        let mesh = v
            .warp_mesh("proj", "curtain", 24, 12)
            .expect("a swept mesh");
        assert!(!mesh.is_empty());
        let [x, y, w, h] = mesh.bbox().expect("a bbox");
        assert!(w > 0.0 && h > 0.0, "{:?}", mesh.bbox());
        assert!(
            (-0.1..=1.1).contains(&x)
                && (-0.1..=1.1).contains(&y)
                && (-0.1..=1.1).contains(&(x + w))
                && (-0.1..=1.1).contains(&(y + h)),
            "bbox {:?}",
            mesh.bbox()
        );
    }

    #[test]
    fn a_display_mesh_is_the_identity() {
        let mut v = swept_venue();
        v.displays.push(Display {
            id: "screen".into(),
            name: "Screen".into(),
            kind: DisplayKind::Tv,
            output: "screen".into(),
            surface: "curtain".into(),
            resolution: [1920, 1080],
            link: Link::Local,
            node: None,
            notes: String::new(),
        });
        let mesh = v
            .warp_mesh("screen", "curtain", 24, 12)
            .expect("a display mesh");
        for d in [[0.0, 0.0], [0.25, 0.75], [0.5, 0.5], [1.0, 1.0]] {
            let uv = mesh.source_at(d).expect("inside the identity mesh");
            assert!(
                (uv[0] - d[0]).abs() < 1e-5 && (uv[1] - d[1]).abs() < 1e-5,
                "{d:?} -> {uv:?}"
            );
        }
        assert!(mesh.source_at([-0.1, 0.5]).is_none());
    }

    // -- the output's grid: sectors -----------------------------------------

    /// A one-projector, one-wall room: `front` dead ahead of a head-on lens,
    /// so the projected warp is the identity and a cell's polygon is also its
    /// `(s, t)` rectangle — which makes overlap and coverage readable.
    fn sector_venue(source: SectorSource, ratio: f32, points: Vec<[f32; 2]>) -> Venue {
        Venue {
            venue: Meta {
                id: "grid".into(),
                name: "grid".into(),
                setting: Setting::Indoor,
                based_on: vec![],
                notes: String::new(),
            },
            shots: vec![],
            sensors: vec![],
            surfaces: vec![Surface {
                id: "front".into(),
                name: "frente".into(),
                origin: [-1.0, 1.0, 0.0],
                u: [2.0, 0.0, 0.0],
                v: [0.0, -2.0, 0.0],
                material: String::new(),
                form: SurfaceForm::Plane,
                sectors: SectorGrid {
                    source,
                    ratio,
                    points,
                    magnet: false,
                },
            }],
            objects: vec![Object {
                id: "table".into(),
                name: "table".into(),
                kind: "table".into(),
                at: [0.0, 0.0, 1.0],
                size: [0.0, 0.0, 0.0],
                yaw: 0.0,
                crown: None,
                occludes: false,
                emits: 0.0,
                faces: vec![],
                description: String::new(),
                form: ObjectForm::Box,
                parent: None,
                attribution: String::new(),
                source: String::new(),
            }],
            projectors: vec![Projector {
                id: "proj".into(),
                name: "proj".into(),
                output: "proj".into(),
                on: "table".into(),
                lens: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
                throw_ratio: 0.5,
                aspect: 1.0,
                lumens: None,
                // The head-on fit of `escena`'s tests: four corners where the
                // lens throws them, so the warp is the identity and a cell's
                // `(s, t)` polygon is also its device polygon.
                pins: [("front".to_string(), SURFACE_CORNERS)].into_iter().collect(),
                blend: Default::default(),
            }],
            fixtures: vec![],
            masks: vec![],
            displays: vec![],
            compositions: vec![],
        }
    }

    fn polygon_area(poly: &[[f32; 2]]) -> f32 {
        let n = poly.len();
        let mut a = 0.0;
        for i in 0..n {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            a += p[0] * q[1] - q[0] * p[1];
        }
        (a * 0.5).abs()
    }

    /// The exact area of the intersection of two convex polygons: clip `a`
    /// into each of `b`'s edges' half-planes. Zero means they do not overlap.
    fn polygon_overlap(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
        // `b` is counter-clockwise or clockwise; take its winding from its
        // signed area so the inward half-plane is "left" either way.
        let mut poly = a.to_vec();
        let n = b.len();
        let b_area: f32 = (0..n)
            .map(|i| {
                let (p, q) = (b[i], b[(i + 1) % n]);
                p[0] * q[1] - q[0] * p[1]
            })
            .sum();
        let sign = if b_area >= 0.0 { 1.0 } else { -1.0 };
        for i in 0..n {
            let (u, v) = (b[i], b[(i + 1) % n]);
            poly = clip(&poly, |p| sign * ((v[0] - u[0]) * (p[1] - u[1]) - (v[1] - u[1]) * (p[0] - u[0])));
            if poly.len() < 3 {
                return 0.0;
            }
        }
        polygon_area(&poly)
    }

    fn assert_a_split(cells: &[Vec<[f32; 2]>], n: usize) {
        assert_eq!(cells.len(), n, "one cell per slot");
        let total: f32 = cells.iter().map(|c| polygon_area(c)).sum();
        assert!((total - 1.0).abs() < 1e-3, "the cells cover the frame: {total}");
        for i in 0..cells.len() {
            assert!(polygon_area(&cells[i]) > 1e-4, "cell {i} is not empty");
            for j in i + 1..cells.len() {
                let shared = polygon_overlap(&cells[i], &cells[j]);
                assert!(
                    shared < 1e-3,
                    "cells {i} and {j} overlap by {shared}: {:?} / {:?}",
                    cells[i],
                    cells[j]
                );
            }
        }
    }

    #[test]
    fn a_legacy_surface_has_one_cell_that_is_the_whole_surface() {
        let mut v = sector_venue(SectorSource::Projected, 1.0, vec![]);
        v.surfaces[0].sectors = SectorGrid::default();
        let cells = v.sector_splits("proj", "front");
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0], SURFACE_CORNERS.to_vec());
        assert!((polygon_area(&cells[0]) - 1.0).abs() < 1e-6, "the whole plane");
    }

    #[test]
    fn the_projected_split_divides_the_surface_into_n_fan_cells() {
        for (ratio, n) in [(2.0f32, 6usize), (4.0 / 3.0, 4), (8.0 / 3.0, 8)] {
            let v = sector_venue(SectorSource::Projected, ratio, vec![]);
            let cells = v.sector_splits("proj", "front");
            assert_a_split(&cells, n);
        }
    }

    #[test]
    fn the_drawn_streaks_split_the_surface_in_drawn_order() {
        // Two vertical streaks at a third and two thirds: three columns.
        let points = vec![[1.0 / 3.0, 0.0], [1.0 / 3.0, 1.0], [2.0 / 3.0, 0.0], [2.0 / 3.0, 1.0]];
        let v = sector_venue(SectorSource::Points, 1.0, points);
        let cells = v.sector_splits("proj", "front");
        assert_a_split(&cells, 3);
        // The first cell is the left column: `(s, t)` keeps the order drawn.
        assert!(cells[0].iter().all(|p| p[0] <= 1.0 / 3.0 + 1e-4), "{:?}", cells[0]);
        assert!(cells[2].iter().all(|p| p[0] >= 2.0 / 3.0 - 1e-4), "{:?}", cells[2]);
    }

    #[test]
    fn degenerate_strokes_fall_back_to_the_projected_split() {
        // One zero-length pair and one real streak: no drawn split, so the
        // surface's ratio decides instead, exactly as if no points existed.
        // A zero-length streak draws nothing, so the whole drawing is
        // degenerate even though what follows it is a real line.
        let points = vec![[0.5, 0.5], [0.5, 0.5], [0.5, 0.0], [0.5, 1.0]];
        let drawn = sector_venue(SectorSource::Points, 2.0, points);
        let projected = sector_venue(SectorSource::Projected, 2.0, vec![]);
        assert_eq!(
            drawn.sector_splits("proj", "front"),
            projected.sector_splits("proj", "front"),
            "a degenerate drawing is the projected split"
        );
        // A surface the venue cannot map still answers the whole plane — the
        // engine's place/clip then drops the layer (P2.4), never the whole
        // frame. A surface the venue does not have draws nothing at all.
        assert_eq!(drawn.sector_splits("nobody", "front"), vec![SURFACE_CORNERS.to_vec()]);
        assert_eq!(drawn.sector_splits("proj", "nowhere"), vec![Vec::<[f32; 2]>::new()]);
    }

    #[test]
    fn a_venue_written_before_sectors_round_trips_byte_identically() {
        let mut v = sector_venue(SectorSource::Projected, 1.0, vec![]);
        v.surfaces[0].sectors = SectorGrid::default();
        let before = toml::to_string(&v).expect("serializes");
        assert!(!before.contains("sectors"), "a default grid writes nothing: {before}");
        let back = Venue::parse(&before).expect("parses");
        assert_eq!(back.surfaces[0].sectors, SectorGrid::default());
        assert_eq!(toml::to_string(&back).unwrap(), before);

        v.surfaces[0].sectors.ratio = 3.0;
        let after = toml::to_string(&v).expect("serializes");
        assert!(after.contains("sectors"), "a set grid is written");
        let back = Venue::parse(&after).expect("parses");
        assert_eq!(back.surfaces[0].sectors.ratio, 3.0);
        assert_eq!(back.surfaces[0].sectors.source, SectorSource::Projected);
    }
}
