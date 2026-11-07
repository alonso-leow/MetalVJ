//! A real patch → placements → the matrix that maps each stage float to fixture outputs.

use crate::grid::{Frame, Point};
use crate::names::{Channel, FloatName, Kind};
use metal_schema::Signal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the stage needs to know about one venue fixture. `metal-perform` builds these
/// from `venue::Fixture` + its profile (`channels` = the profile's selectors).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatchFixture {
    pub id: String,
    pub kind: Kind,
    pub at: [f32; 3],
    #[serde(default)]
    pub group: Option<String>,
    /// The channels its profile drives; empty means the type's defaults.
    #[serde(default)]
    pub channels: Vec<Channel>,
    /// Pin the fixture to a hitbox point instead of the nearest one.
    #[serde(default)]
    pub point: Option<Point>,
    /// The venue pose (yaw, pitch) was fine-tuned by hand: it wins over the point's.
    #[serde(default)]
    pub pose: Option<(f32, f32)>,
}
impl PatchFixture {
    pub fn channels(&self) -> Vec<Channel> {
        if self.channels.is_empty() { self.kind.default_channels().to_vec() } else { self.channels.clone() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Patch {
    pub name: String,
    pub frame: Frame,
    pub fixtures: Vec<PatchFixture>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Placement {
    pub fixture: String,
    pub point: Point,
    /// Aim in degrees: the fine-tuned pose, else the hitbox default.
    pub yaw: f32,
    pub pitch: f32,
    pub tuned: bool,
}

impl Patch {
    pub fn new(name: &str, fixtures: Vec<PatchFixture>) -> Patch {
        let ats: Vec<[f32; 3]> = fixtures.iter().map(|f| f.at).collect();
        Patch { name: name.into(), frame: Frame::fit(&ats), fixtures }
    }

    pub fn placements(&self) -> Vec<Placement> {
        self.fixtures
            .iter()
            .map(|f| {
                let point = f.point.unwrap_or_else(|| self.frame.nearest(f.at));
                let (yaw, pitch) = f.pose.unwrap_or_else(|| self.frame.orientation(point));
                Placement { fixture: f.id.clone(), point, yaw, pitch, tuned: f.pose.is_some() }
            })
            .collect()
    }

    /// Fixtures per `(point, type)` cell: the amount, type and position the view shows.
    pub fn cells(&self) -> BTreeMap<(Point, Kind), Vec<String>> {
        let mut m: BTreeMap<(Point, Kind), Vec<String>> = BTreeMap::new();
        for (f, p) in self.fixtures.iter().zip(self.placements()) {
            m.entry((p.point, f.kind)).or_default().push(f.id.clone());
        }
        m
    }

    pub fn matrix(&self) -> Matrix {
        Matrix::build(self)
    }
}

/// The common matrix: the ordered stage floats of one patch, and per float the fixtures
/// (id, weight) it drives. A float exists only where the patch has a fixture of that type
/// at that point able to take that channel — so the AI's output is never padded with dead
/// columns.
#[derive(Clone, Debug, PartialEq)]
pub struct Matrix {
    pub columns: Vec<FloatName>,
    pub drives: Vec<Vec<(String, f32)>>,
    /// Fixtures that no float reaches (a camera, a type with no channels).
    pub orphans: Vec<String>,
}

impl Matrix {
    fn build(patch: &Patch) -> Matrix {
        let places = patch.placements();
        let mut cols: BTreeMap<FloatName, Vec<(String, f32)>> = BTreeMap::new();
        let mut orphans = Vec::new();
        for (f, p) in patch.fixtures.iter().zip(&places) {
            let ch = f.channels();
            if ch.is_empty() {
                orphans.push(f.id.clone());
            }
            for c in ch {
                cols.entry(FloatName { point: p.point, kind: f.kind, channel: c }).or_default().push((f.id.clone(), 1.0));
            }
        }
        let (columns, drives) = cols.into_iter().unzip();
        Matrix { columns, drives, orphans }
    }

    pub fn names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.to_string()).collect()
    }
    pub fn index(&self, name: &FloatName) -> Option<usize> {
        self.columns.iter().position(|c| c == name)
    }

    /// Stage floats → one signal per fixture. Several floats landing on one channel
    /// average (weights are 1 today; the field is where a fine-tuned spread goes).
    pub fn expand(&self, values: &[f32]) -> BTreeMap<String, Signal> {
        let mut out: BTreeMap<String, Signal> = BTreeMap::new();
        let mut acc: BTreeMap<(String, Channel), (f32, f32)> = BTreeMap::new();
        for ((col, drive), v) in self.columns.iter().zip(&self.drives).zip(values) {
            for (id, w) in drive {
                out.entry(id.clone()).or_insert_with(Channel::rest);
                let e = acc.entry((id.clone(), col.channel)).or_default();
                e.0 += v * w;
                e.1 += w;
            }
        }
        for ((id, ch), (sum, w)) in acc {
            if w > 0.0 {
                let [lo, hi] = ch.range();
                ch.set(out.get_mut(&id).unwrap(), (sum / w).clamp(lo, hi));
            }
        }
        out
    }

    /// A float that no fixture of the patch answers: the nearest point holding the same
    /// type and channel (to suggest where a request for an absent point would land).
    pub fn fallback(&self, frame: &Frame, want: &FloatName) -> Option<FloatName> {
        let at = frame.at(want.point);
        self.columns
            .iter()
            .filter(|c| c.kind == want.kind && c.channel == want.channel)
            .min_by(|a, b| {
                let d = |p: Point| {
                    let q = frame.at(p);
                    (q[0] - at[0]).powi(2) + (q[1] - at[1]).powi(2) + (q[2] - at[2]).powi(2)
                };
                d(a.point).total_cmp(&d(b.point))
            })
            .copied()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn pf(id: &str, kind: Kind, at: [f32; 3]) -> PatchFixture {
        PatchFixture { id: id.into(), kind, at, group: None, channels: vec![], point: None, pose: None }
    }
    pub fn demo() -> Patch {
        Patch::new(
            "demo",
            vec![
                pf("key-1", Kind::Wash, [-3.0, 6.7, -0.8]),
                pf("key-2", Kind::Wash, [3.0, 6.7, -0.8]),
                pf("move-1", Kind::Moving, [-2.0, 5.0, -5.0]),
                pf("back-1", Kind::Backlight, [0.0, 4.0, -5.5]),
            ],
        )
    }
    #[test]
    fn places_and_expands() {
        let p = demo();
        let m = p.matrix();
        assert!(!m.columns.is_empty());
        let names = m.names();
        assert!(names.iter().all(|n| FloatName::parse(n).is_some()));
        let mut v = vec![0.0; m.columns.len()];
        let i = m.columns.iter().position(|c| c.kind == Kind::Moving && c.channel == Channel::Level).unwrap();
        v[i] = 0.75;
        let out = m.expand(&v);
        assert_eq!(out["move-1"].level, 0.75);
        assert_eq!(out["key-1"].level, 0.0);
    }
    #[test]
    fn tuned_pose_wins() {
        let mut p = demo();
        p.fixtures[0].pose = Some((12.0, -20.0));
        let pl = p.placements();
        assert!(pl[0].tuned && pl[0].yaw == 12.0 && !pl[1].tuned);
    }
}
