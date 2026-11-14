//! The grid. Venue frame: metres, +y up, the front edge of the stage (the proscenium)
//! at z = 0, the back wall at z = -depth, the audience at z > 0. x < 0 is the left
//! column seen from the audience. yaw 0 looks at the back wall (-z), +90 looks +x;
//! pitch > 0 looks up (the venue's own convention).
//!
//! Levels 1 (floor) and 2 (rig height) are 2 columns × 3 rows (`back`, `pro`, `house`).
//! Level 3 (zenith) adds a middle row between proscenium and back and a centre column,
//! and keeps the house row: the trusses hung over the audience. 3 × 4.
//!
//! Besides the points, a level has **lines**: places a row of fixtures hangs along.
//! - `<level>.<row>.x`: across the width on a row — the proscenium truss (`l2.pro.x`),
//!   the back wall (`l2.back.x`), a truss over the audience (`l3.house.x`), a floor
//!   strip at the stage edge (`l1.pro.x`).
//! - `<level>.run.<col>`: along the depth, back to proscenium, on a column — a truss
//!   over the stage (`l3.run.c`), a side tower (`l2.run.l`), or a side wall
//!   (`wl`/`wr`, at the stage edges).
//!
//! A line spreads its fixtures over four variants (α β γ δ, see `names::Variant`),
//! in order along it.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    Floor,
    Rig,
    Zenith,
}
impl Level {
    pub const ALL: [Level; 3] = [Level::Floor, Level::Rig, Level::Zenith];
    pub fn word(self) -> &'static str {
        match self {
            Level::Floor => "l1",
            Level::Rig => "l2",
            Level::Zenith => "l3",
        }
    }
    pub fn parse(s: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.word() == s)
    }
}

/// `Mid` exists on the zenith level only. `Run` is no row: it names a line along the
/// depth (back → proscenium) on a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Row {
    Back,
    Mid,
    Pro,
    House,
    Run,
}
impl Row {
    pub fn word(self) -> &'static str {
        match self {
            Row::Back => "back",
            Row::Mid => "mid",
            Row::Pro => "pro",
            Row::House => "house",
            Row::Run => "run",
        }
    }
    pub const ALL: [Row; 5] = [Row::Back, Row::Mid, Row::Pro, Row::House, Row::Run];
    pub fn parse(s: &str) -> Option<Row> {
        Row::ALL.into_iter().find(|r| r.word() == s)
    }
}

/// `C` exists on the zenith level only. `X` is the whole row (a line across); `Wl`/`Wr`
/// are the side walls, at the stage edges, and only take a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Col {
    L,
    C,
    R,
    X,
    Wl,
    Wr,
}
impl Col {
    pub fn word(self) -> &'static str {
        match self {
            Col::L => "l",
            Col::C => "c",
            Col::R => "r",
            Col::X => "x",
            Col::Wl => "wl",
            Col::Wr => "wr",
        }
    }
    pub const ALL: [Col; 6] = [Col::L, Col::C, Col::R, Col::X, Col::Wl, Col::Wr];
    pub fn parse(s: &str) -> Option<Col> {
        Col::ALL.into_iter().find(|c| c.word() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Point {
    pub level: Level,
    pub row: Row,
    pub col: Col,
}

impl Point {
    pub fn new(level: Level, row: Row, col: Col) -> Result<Point, String> {
        let p = Point { level, row, col };
        if p.valid() { Ok(p) } else { Err(format!("{} is not a point of the grid", p.word())) }
    }
    pub fn valid(&self) -> bool {
        let zenith = self.level == Level::Zenith;
        match (self.row, self.col) {
            // A line along the depth: a column or a side wall.
            (Row::Run, Col::X) => false,
            (Row::Run, Col::C) => zenith,
            (Row::Run, _) => true,
            // The walls take a run only.
            (_, Col::Wl | Col::Wr) => false,
            // A point, or a line across its row.
            (Row::Mid, _) => zenith,
            (_, Col::C) => zenith,
            _ => true,
        }
    }
    /// A line (across a row or along the depth), not a single point.
    pub fn is_line(&self) -> bool {
        self.row == Row::Run || self.col == Col::X
    }
    /// `l2.back.l`
    pub fn word(&self) -> String {
        format!("{}.{}.{}", self.level.word(), self.row.word(), self.col.word())
    }
    pub fn parse(s: &str) -> Option<Point> {
        let mut it = s.split('.');
        let p = Point { level: Level::parse(it.next()?)?, row: Row::parse(it.next()?)?, col: Col::parse(it.next()?)? };
        (it.next().is_none() && p.valid()).then_some(p)
    }
    /// Every place of the grid: the 24 points, then the 23 lines.
    pub fn all() -> Vec<Point> {
        let mut v = Point::points();
        v.extend(Point::lines());
        v
    }
    /// The single points: 6 + 6 + 12 = 24.
    pub fn points() -> Vec<Point> {
        Point::every().filter(|p| !p.is_line()).collect()
    }
    /// The lines: across each row (3 + 3 + 4) and along the depth (4 + 4 + 5).
    pub fn lines() -> Vec<Point> {
        Point::every().filter(|p| p.is_line()).collect()
    }
    fn every() -> impl Iterator<Item = Point> {
        Level::ALL.into_iter().flat_map(|level| {
            Row::ALL.into_iter().flat_map(move |row| Col::ALL.into_iter().map(move |col| Point { level, row, col }))
        }).filter(|p| p.valid())
    }
}

/// An edge: the line between two points of one level. Orientation of a fixture hung on
/// an edge is perpendicular to it, toward the stage centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub a: Point,
    pub b: Point,
}
impl Edge {
    pub fn new(a: Point, b: Point) -> Result<Edge, String> {
        if a.level != b.level || a == b {
            return Err("an edge joins two different points of one level".into());
        }
        Ok(Edge { a, b })
    }
}

/// The room's size: turns grid points into metres. Derived from a patch, or set by hand
/// and fine-tuned later (the grid is the first orientation, never the last).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// Stage width, x ∈ [-width/2, width/2].
    pub width: f32,
    /// Stage depth: the back wall is at z = -depth.
    pub depth: f32,
    /// Distance from the proscenium to the back of the public.
    pub house: f32,
    /// Height of the rig level (l2) and of the zenith level (l3).
    pub rig_y: f32,
    pub zenith_y: f32,
}
impl Default for Frame {
    fn default() -> Frame {
        Frame { width: 10.0, depth: 6.0, house: 8.0, rig_y: 4.0, zenith_y: 6.0 }
    }
}

impl Frame {
    /// A frame that holds every `at` of a patch, with the default house.
    pub fn fit(points: &[[f32; 3]]) -> Frame {
        let mut f = Frame::default();
        let mx = points.iter().fold(0.0f32, |m, p| m.max(p[0].abs()));
        let mz = points.iter().fold(0.0f32, |m, p| m.min(p[2]));
        let my = points.iter().fold(0.0f32, |m, p| m.max(p[1]));
        if mx > 0.0 {
            f.width = (mx * 2.0).max(4.0);
        }
        if mz < 0.0 {
            f.depth = (-mz).max(3.0);
        }
        if my > 0.0 {
            f.zenith_y = my.max(3.0);
            f.rig_y = (my * 0.66).max(2.0);
        }
        f
    }
    pub fn at(&self, p: Point) -> [f32; 3] {
        let x = match p.col {
            Col::L => -self.width * 0.25,
            Col::C | Col::X => 0.0,
            Col::R => self.width * 0.25,
            Col::Wl => -self.width * 0.5,
            Col::Wr => self.width * 0.5,
        };
        let z = match p.row {
            Row::Back => -self.depth,
            Row::Mid | Row::Run => -self.depth * 0.5,
            Row::Pro => 0.0,
            Row::House => self.house,
        };
        let y = match p.level {
            Level::Floor => 0.0,
            Level::Rig => self.rig_y,
            Level::Zenith => self.zenith_y,
        };
        [x, y, z]
    }
    /// Where variant `i` of 4 sits: a point is one spot; a line spreads its four
    /// variants evenly along it (across: left to right; a run: back to front).
    pub fn spot(&self, p: Point, i: Option<usize>) -> [f32; 3] {
        let mut q = self.at(p);
        let Some(i) = i.filter(|_| p.is_line()) else { return q };
        let t = (i as f32 + 0.5) / 4.0;
        if p.col == Col::X {
            q[0] = (t - 0.5) * self.width * LINE_SPAN;
        } else {
            q[2] = -self.depth * (1.0 - t);
        }
        q
    }
    /// The coordinate that orders fixtures along a line (x across, z along the depth).
    pub fn along(p: Point, at: [f32; 3]) -> f32 {
        if p.row == Row::Run { at[2] } else { at[0] }
    }
    /// The point nearest a position (the hitbox the fixture is dropped into).
    pub fn nearest(&self, at: [f32; 3]) -> Point {
        Point::points()
            .into_iter()
            .min_by(|a, b| self.dist(*a, at).total_cmp(&self.dist(*b, at)))
            .expect("the grid is not empty")
    }
    fn dist(&self, p: Point, at: [f32; 3]) -> f32 {
        let q = self.at(p);
        // Up on the zenith level means "over the stage": height counts double so a
        // 6 m light does not fall to the floor row, and a floor light stays on the floor.
        let d = [q[0] - at[0], (q[1] - at[1]) * 2.0, q[2] - at[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
    }
    /// Default aim `(yaw, pitch)` degrees at a point: toward the stage centre; the
    /// back row looks at the public (backlight); the zenith looks down. An edge point
    /// pair aims perpendicular to the edge, toward the stage centre.
    pub fn orientation(&self, p: Point) -> (f32, f32) {
        let from = self.at(p);
        let target = match (p.level, p.row) {
            // A truss over the audience looks back at the stage.
            (Level::Zenith, Row::House) => [0.0, 1.2, -self.depth * 0.5],
            (Level::Zenith, _) => [from[0], 0.0, from[2]],
            (_, Row::Back) => [0.0, 1.2, self.house * 0.5],
            _ => [0.0, 1.2, -self.depth * 0.5],
        };
        aim(from, target)
    }
    pub fn edge_orientation(&self, e: Edge) -> (f32, f32) {
        let (a, b) = (self.at(e.a), self.at(e.b));
        let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5];
        let along = [b[0] - a[0], b[2] - a[2]];
        let mut n = [-along[1], along[0]];
        let to_centre = [-mid[0], -self.depth * 0.5 - mid[2]];
        if n[0] * to_centre[0] + n[1] * to_centre[1] < 0.0 {
            n = [-n[0], -n[1]];
        }
        let yaw = n[0].atan2(-n[1]).to_degrees();
        (yaw, aim(mid, [0.0, 1.2, -self.depth * 0.5]).1)
    }
}

/// The share of the stage width a line across spans (a truss stops short of the walls).
pub const LINE_SPAN: f32 = 0.8;

/// yaw 0 → -z, +90 → +x; pitch > 0 up.
pub fn aim(from: [f32; 3], to: [f32; 3]) -> (f32, f32) {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let flat = d[0].hypot(d[2]);
    (d[0].atan2(-d[2]).to_degrees(), d[1].atan2(flat).to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn twenty_four_points_and_twenty_three_lines() {
        assert_eq!(Point::lines().len(), 23);
        assert_eq!(Point::all().len(), 47);
        for p in Point::all() {
            assert_eq!(Point::parse(&p.word()), Some(p));
        }
        for w in ["l2.pro.x", "l3.house.x", "l3.run.c", "l1.run.wl", "l2.back.x"] {
            assert!(Point::parse(w).is_some_and(|p| p.is_line()), "{w}");
        }
        for w in ["l2.run.c", "l2.run.x", "l2.pro.wl", "l1.mid.x"] {
            assert!(Point::parse(w).is_none(), "{w}");
        }
        let f = Frame::default();
        let a = f.spot(Point::parse("l2.pro.x").unwrap(), Some(0));
        let d = f.spot(Point::parse("l2.pro.x").unwrap(), Some(3));
        assert!(a[0] < 0.0 && d[0] > 0.0, "alpha left, delta right");
        let back = f.spot(Point::parse("l3.run.c").unwrap(), Some(0));
        assert!(back[2] < -f.depth * 0.5, "alpha of a run at the back");
        let all = Point::points();
        assert_eq!(all.len(), 24);
        assert_eq!(all.iter().filter(|p| p.level == Level::Floor).count(), 6);
        assert_eq!(all.iter().filter(|p| p.level == Level::Zenith).count(), 12);
        for p in all {
            assert_eq!(Point::parse(&p.word()), Some(p));
        }
        assert!(Point::parse("l1.mid.l").is_none());
    }
    #[test]
    fn nearest_and_aim() {
        let f = Frame::default();
        assert_eq!(f.nearest([-3.0, 4.0, 0.2]).word(), "l2.pro.l");
        assert_eq!(f.nearest([0.2, 6.0, -3.0]).word(), "l3.mid.c");
        let (yaw, _) = f.orientation(Point::parse("l2.house.r").unwrap());
        assert!(yaw < 0.0 && yaw > -90.0, "house-right looks back and to the left: {yaw}");
        let (_, pitch) = f.orientation(Point::parse("l3.mid.c").unwrap());
        assert!(pitch < -80.0);
        // Over the audience: looks back at the stage, down.
        let (yaw, pitch) = f.orientation(Point::parse("l3.house.c").unwrap());
        assert!(yaw.abs() < 1.0 && pitch < 0.0, "{yaw} {pitch}");
    }
}
