//! A real patch → placements → the matrix that maps each stage float to fixture outputs.

use crate::grid::{Col, Frame, Level, Point, Row};
use crate::names::{Channel, FloatName, Kind, Variant};
use metal_schema::Signal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
    /// A pixel bar's pixel count (0: driven as one colour). Its cell gains
    /// [`crate::common::PIXEL_SEGMENTS`] segment floats at most, each driving the
    /// output key `"<id>#<segment>"`.
    #[serde(default)]
    pub pixels: u16,
    /// Pin the fixture to one variant of its cell instead of its turn along the place.
    #[serde(default)]
    pub variant: Option<Variant>,
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
    /// A point, or a line the fixture hangs along.
    pub point: Point,
    /// Its voice in the cell: dealt α β γ δ along the place.
    pub variant: Variant,
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

    /// Each fixture's place and variant. A fixture drops into its nearest point; then
    /// fixtures of one type that hang in a row become a line: three or more on one row
    /// spread over two or more columns become the line across it (`l2.pro.x`); three or
    /// more on one column spread over two or more rows (back, mid, pro) become a run
    /// (`l3.run.c`), and two or more at a stage edge become a side-wall run
    /// (`l1.run.wl`). A fixture that would join both takes the longer line (a side wall
    /// always wins); a venue `stage_point` pins it instead (`PatchFixture::point`). Last, every
    /// cell deals its fixtures into the variants in order along the place.
    pub fn placements(&self) -> Vec<Placement> {
        let points: Vec<Point> = self.fixtures.iter().map(|f| f.point.unwrap_or_else(|| self.frame.nearest(f.at))).collect();
        let free = |i: usize| self.fixtures[i].point.is_none();
        let wall = |at: [f32; 3]| {
            if at[0] <= -self.frame.width * 0.4 {
                Some(Col::Wl)
            } else if at[0] >= self.frame.width * 0.4 {
                Some(Col::Wr)
            } else {
                None
            }
        };
        let mut across: BTreeMap<(Kind, Level, Row), Vec<usize>> = BTreeMap::new();
        let mut runs: BTreeMap<(Kind, Level, Col), Vec<usize>> = BTreeMap::new();
        for (i, (f, p)) in self.fixtures.iter().zip(&points).enumerate() {
            if !free(i) {
                continue;
            }
            across.entry((f.kind, p.level, p.row)).or_default().push(i);
            if p.row != Row::House {
                runs.entry((f.kind, p.level, wall(f.at).unwrap_or(p.col))).or_default().push(i);
            }
        }
        let spread = |ids: &[usize], key: fn(&Point) -> u8| ids.iter().map(|&i| key(&points[i])).collect::<BTreeSet<_>>().len();
        let mut line: Vec<Option<(Point, usize)>> = vec![None; self.fixtures.len()];
        for ((_, level, row), ids) in &across {
            if ids.len() >= 3 && spread(ids, |p| p.col as u8) >= 2 {
                for &i in ids {
                    line[i] = Some((Point { level: *level, row: *row, col: Col::X }, ids.len()));
                }
            }
        }
        for ((_, level, col), ids) in &runs {
            let walls = matches!(col, Col::Wl | Col::Wr);
            let rows = spread(ids, |p| p.row as u8);
            let p = Point { level: *level, row: Row::Run, col: *col };
            if p.valid() && ((walls && ids.len() >= 2) || (ids.len() >= 3 && rows >= 2)) {
                for &i in ids {
                    // A side wall always wins: fixtures at the stage edge belong to the wall.
                    if walls || line[i].is_none_or(|(_, n)| ids.len() > n) {
                        line[i] = Some((p, ids.len()));
                    }
                }
            }
        }
        let placed: Vec<Point> = points.iter().zip(&line).map(|(p, l)| l.map_or(*p, |(q, _)| q)).collect();
        // Deal the variants: per cell, in order along the place, pinned ones first.
        let mut variant = vec![Variant::Alpha; self.fixtures.len()];
        let mut cells: BTreeMap<(Point, Kind), Vec<usize>> = BTreeMap::new();
        for (i, f) in self.fixtures.iter().enumerate() {
            cells.entry((placed[i], f.kind)).or_default().push(i);
        }
        for ((p, _), mut ids) in cells {
            ids.sort_by(|&a, &b| {
                let (fa, fb) = (&self.fixtures[a], &self.fixtures[b]);
                Frame::along(p, fa.at).total_cmp(&Frame::along(p, fb.at)).then(fa.at[2].total_cmp(&fb.at[2])).then(a.cmp(&b))
            });
            let mut k = 0;
            for i in ids {
                variant[i] = match self.fixtures[i].variant {
                    Some(v) => v,
                    None => {
                        k += 1;
                        Variant::nth(k - 1)
                    }
                };
            }
        }
        self.fixtures
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let point = placed[i];
                let (yaw, pitch) = f.pose.unwrap_or_else(|| self.frame.orientation(point));
                Placement { fixture: f.id.clone(), point, variant: variant[i], yaw, pitch, tuned: f.pose.is_some() }
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
    /// Every fixture's place in its wave, per `(column, fixture)`.
    pub members: Vec<WaveMember>,
    /// Per `(group, channel, lane)`: the bounded history the wave is read from. A lane is
    /// one repeat of a wrapped wave (`0` when the wave does not wrap).
    pub rings: BTreeMap<(String, Channel, usize), DelayRing>,
}

/// Samples one delay ring keeps. The oldest sample is dropped first, so a long show
/// never grows the history.
pub const RING_CAP: usize = 256;

/// One fixture's place in a wave. A point shared by several fixtures is one column but
/// several members, so each fixture keeps its own index, lag and last value.
#[derive(Clone, Debug, PartialEq)]
pub struct WaveMember {
    /// The matrix column this member reads its value from.
    pub column: usize,
    pub fixture: String,
    pub channel: Channel,
    pub weight: f32,
    /// The wave the member belongs to: the fixture's `group`, else its kind word.
    pub group: String,
    /// Position among the members of the same `(group, channel)`, in patch order.
    pub index_in_group: usize,
    pub group_len: usize,
    /// The position (a distinct point) this member stands at, among the positions of its
    /// `(group, channel)` in patch order. Fixtures on one position share it, and with it
    /// the same offset.
    pub bucket: usize,
    /// How many positions the member's `(group, channel)` has.
    pub buckets: usize,
    /// The value the member produced on the last expand.
    pub last: f32,
    /// The lag (in the caller's time unit) the member read at on the last delayed expand.
    pub phase: f32,
}

/// Which way a wave runs across a group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelayDir {
    /// Index 0 leads; the wave runs to the last fixture.
    #[default]
    Forward,
    /// The last fixture leads.
    Reverse,
    /// The middle leads; the wave runs out to both ends.
    CenterOut,
}
impl DelayDir {
    pub fn parse(s: &str) -> Option<DelayDir> {
        Some(match s {
            "forward" => DelayDir::Forward,
            "reverse" => DelayDir::Reverse,
            "center_out" | "center-out" => DelayDir::CenterOut,
            _ => return None,
        })
    }
}

/// One channel's delay: `step` per fixture (in the caller's time unit), the direction and
/// an optional wrap (`wrap > 0` repeats the wave every `wrap` fixtures).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DelaySpec {
    pub step: f32,
    #[serde(default)]
    pub dir: DelayDir,
    #[serde(default)]
    pub wrap: u32,
}
impl DelaySpec {
    /// How many steps behind the lead fixture `i` of `n` in its group runs.
    pub fn steps(&self, i: usize, n: usize) -> f32 {
        let m = if self.wrap > 0 { self.wrap as usize } else { n.max(1) };
        let k = i % m.max(1);
        let last = (m.max(1) - 1) as f32;
        match self.dir {
            DelayDir::Forward => k as f32,
            DelayDir::Reverse => last - k as f32,
            DelayDir::CenterOut => (2.0 * k as f32 - last).abs() / 2.0,
        }
    }
}

/// A bounded history of `(time, value)` samples, read back with a hold: the value
/// of the latest sample at or before the asked time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DelayRing {
    samples: VecDeque<(f32, f32)>,
}
impl DelayRing {
    pub fn push(&mut self, t: f32, v: f32) {
        if let Some(last) = self.samples.back_mut() {
            if last.0 == t {
                last.1 = v;
                return;
            }
        }
        if self.samples.len() == RING_CAP {
            self.samples.pop_front();
        }
        self.samples.push_back((t, v));
    }
    pub fn len(&self) -> usize {
        self.samples.len()
    }
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
    /// The value held at `t`. Before the oldest sample it is the oldest value.
    pub fn hold(&self, t: f32) -> Option<f32> {
        let mut out = self.samples.front()?.1;
        for &(st, sv) in &self.samples {
            if st <= t {
                out = sv;
            } else {
                break;
            }
        }
        Some(out)
    }
}

/// One member per `(column, fixture)` pair, indexed within its `(group, channel)` in the
/// patch's own fixture order.
fn wave_members(patch: &Patch, columns: &[FloatName], drives: &[Vec<(String, f32)>]) -> Vec<WaveMember> {
    let pos: BTreeMap<&str, usize> = patch.fixtures.iter().enumerate().map(|(i, f)| (f.id.as_str(), i)).collect();
    let group_of: BTreeMap<&str, String> = patch
        .fixtures
        .iter()
        .map(|f| (f.id.as_str(), f.group.clone().unwrap_or_else(|| f.kind.word().to_string())))
        .collect();
    let mut members: Vec<WaveMember> = Vec::new();
    for (c, (col, drive)) in columns.iter().zip(drives).enumerate() {
        for (id, w) in drive {
            let group = group_of.get(crate::common::base_id(id)).cloned().unwrap_or_else(|| col.kind.word().to_string());
            members.push(WaveMember {
                column: c,
                fixture: id.clone(),
                channel: col.channel,
                weight: *w,
                group,
                index_in_group: 0,
                group_len: 0,
                bucket: 0,
                buckets: 0,
                last: 0.0,
                phase: 0.0,
            });
        }
    }
    members.sort_by_key(|m| (m.group.clone(), m.channel, pos.get(crate::common::base_id(&m.fixture)).copied().unwrap_or(usize::MAX), m.column));
    let mut total: BTreeMap<(String, Channel), usize> = BTreeMap::new();
    for m in &members {
        *total.entry((m.group.clone(), m.channel)).or_default() += 1;
    }
    let mut seen: BTreeMap<(String, Channel), usize> = BTreeMap::new();
    for m in &mut members {
        let k = (m.group.clone(), m.channel);
        let e = seen.entry(k.clone()).or_default();
        m.index_in_group = *e;
        *e += 1;
        m.group_len = total[&k];
    }
    // Positions: the distinct spots (place and variant) of each (group, channel),
    // numbered in patch order — so a wave runs α → δ along a line.
    let mut points: BTreeMap<(String, Channel), Vec<(Point, Option<Variant>)>> = BTreeMap::new();
    for m in &mut members {
        let pts = points.entry((m.group.clone(), m.channel)).or_default();
        let p = (columns[m.column].point, columns[m.column].variant);
        m.bucket = match pts.iter().position(|q| *q == p) {
            Some(i) => i,
            None => {
                pts.push(p);
                pts.len() - 1
            }
        };
    }
    for m in &mut members {
        m.buckets = points[&(m.group.clone(), m.channel)].len();
    }
    members
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
                let name = FloatName::new(p.point, f.kind, c).with_variant(p.variant);
                cols.entry(name).or_default().push((f.id.clone(), 1.0));
                for seg in 0..crate::common::segments(f) {
                    let name = FloatName { pixel: Some(seg as u8), ..name };
                    cols.entry(name).or_default().push((crate::common::pixel_key(&f.id, seg), 1.0));
                }
            }
        }
        let (columns, drives): (Vec<FloatName>, Vec<Vec<(String, f32)>>) = cols.into_iter().unzip();
        let members = wave_members(patch, &columns, &drives);
        Matrix { columns, drives, orphans, members, rings: BTreeMap::new() }
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
        let per_member: Vec<f32> = self.members.iter().map(|m| values[m.column]).collect();
        accumulate(&self.members, &per_member)
    }

    /// [`Matrix::expand`] with the waves. A wave runs over the positions of a group, not
    /// its fixtures: every fixture on one position shares one offset, and the offset of
    /// position `k` is the channel's step times the steps from the lead to `k` (a running
    /// sum, one step per position before it). A member whose channel has a [`DelaySpec`]
    /// in `delays`, on a group of more than one position, reads its lead's value from that
    /// offset earlier. Wrapping counts positions and repeats the wave every `wrap`
    /// positions, each repeat with its own lead. The lead is the position with the fewest
    /// steps; its ring is fed its raw value each call, so `now` must move forward.
    /// Channels with no delay, or a zero step, keep their own value, so an empty `delays`
    /// is exactly [`Matrix::expand`].
    pub fn expand_delayed(
        &mut self,
        values: &[f32],
        now: f32,
        delays: &BTreeMap<Channel, DelaySpec>,
    ) -> BTreeMap<String, Signal> {
        // Per member: the spec, its steps from the lead and its lane, when it is in a wave.
        let waves: Vec<Option<(DelaySpec, f32, usize)>> = self
            .members
            .iter()
            .map(|m| {
                delays.get(&m.channel).copied().filter(|s| s.step > 0.0 && m.buckets > 1).map(|s| {
                    let lane = if s.wrap > 0 { m.bucket / s.wrap as usize } else { 0 };
                    (s, s.steps(m.bucket, m.buckets), lane)
                })
            })
            .collect();
        // Feed each lane's ring once: the raw value of its lead position.
        let mut lead: BTreeMap<(String, Channel, usize), (f32, f32)> = BTreeMap::new();
        for (m, w) in self.members.iter().zip(&waves) {
            if let Some((_, steps, lane)) = w {
                let e = lead.entry((m.group.clone(), m.channel, *lane)).or_insert((f32::INFINITY, 0.0));
                if *steps < e.0 {
                    *e = (*steps, values[m.column]);
                }
            }
        }
        for (key, (_, v)) in &lead {
            self.rings.entry(key.clone()).or_default().push(now, *v);
        }
        let mut per_member = Vec::with_capacity(self.members.len());
        for (m, w) in self.members.iter_mut().zip(&waves) {
            let own = values[m.column];
            let v = match w {
                Some((spec, steps, lane)) => {
                    let lag = steps * spec.step;
                    m.phase = lag;
                    self.rings
                        .get(&(m.group.clone(), m.channel, *lane))
                        .and_then(|r| r.hold(now - lag))
                        .unwrap_or(own)
                }
                None => {
                    m.phase = 0.0;
                    own
                }
            };
            m.last = v;
            per_member.push(v);
        }
        accumulate(&self.members, &per_member)
    }

    /// A float that no fixture of the patch answers: the nearest spot holding the same
    /// type and channel (to suggest where a request for an absent spot would land).
    pub fn fallback(&self, frame: &Frame, want: &FloatName) -> Option<FloatName> {
        let at = frame.spot(want.point, want.variant.map(Variant::index));
        let d = |c: &FloatName| {
            let q = frame.spot(c.point, c.variant.map(Variant::index));
            (q[0] - at[0]).powi(2) + (q[1] - at[1]).powi(2) + (q[2] - at[2]).powi(2)
        };
        self.columns
            .iter()
            .filter(|c| c.kind == want.kind && c.channel == want.channel && c.pixel.is_none())
            .min_by(|a, b| d(a).total_cmp(&d(b)).then(a.cmp(b)))
            .copied()
    }
    /// The variants one cell answers for a channel, α first.
    pub fn variants(&self, point: Point, kind: Kind, channel: Channel) -> Vec<Variant> {
        let mut v: Vec<Variant> = self
            .columns
            .iter()
            .filter(|c| c.point == point && c.kind == kind && c.channel == channel && c.pixel.is_none())
            .filter_map(|c| c.variant)
            .collect();
        v.dedup();
        v
    }
}

/// Per fixture, the weighted mean of its members' values for each channel; several members
/// landing on one channel average (weights are 1 today).
fn accumulate(members: &[WaveMember], values: &[f32]) -> BTreeMap<String, Signal> {
    let mut out: BTreeMap<String, Signal> = BTreeMap::new();
    let mut acc: BTreeMap<(String, Channel), (f32, f32)> = BTreeMap::new();
    for (m, v) in members.iter().zip(values) {
        out.entry(m.fixture.clone()).or_insert_with(Channel::rest);
        let e = acc.entry((m.fixture.clone(), m.channel)).or_default();
        e.0 += v * m.weight;
        e.1 += m.weight;
    }
    for ((id, ch), (sum, w)) in acc {
        if w > 0.0 {
            let [lo, hi] = ch.range();
            ch.set(out.get_mut(&id).unwrap(), (sum / w).clamp(lo, hi));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn pf(id: &str, kind: Kind, at: [f32; 3]) -> PatchFixture {
        PatchFixture { id: id.into(), kind, at, group: None, channels: vec![], point: None, pose: None, pixels: 0, variant: None }
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

#[cfg(test)]
mod delay_tests {
    use super::*;
    use crate::grid::{Col, Level, Row};

    /// Five moving heads in one group, each on its own point (so none share a column).
    fn bar() -> Patch {
        let pt = |l, r, c| Point::new(l, r, c).ok();
        let spots = [
            pt(Level::Rig, Row::Back, Col::L),
            pt(Level::Rig, Row::Back, Col::R),
            pt(Level::Rig, Row::Pro, Col::L),
            pt(Level::Rig, Row::Pro, Col::R),
            pt(Level::Zenith, Row::Back, Col::C),
        ];
        let fixtures = spots
            .iter()
            .enumerate()
            .map(|(i, p)| PatchFixture {
                id: format!("bar-{i}"),
                kind: Kind::Moving,
                at: [i as f32, 5.0, 0.0],
                group: Some("bar".into()),
                channels: vec![],
                point: *p,
                pose: None,
                pixels: 0,
                variant: None,
            })
            .collect();
        Patch::new("bar", fixtures)
    }

    fn tilt_cols(m: &Matrix) -> Vec<usize> {
        (0..m.columns.len()).filter(|&c| m.columns[c].channel == Channel::Tilt).collect()
    }

    /// Drives every tilt column with `v(t)`, then reads the expanded tilt per fixture.
    fn run(m: &mut Matrix, delays: &BTreeMap<Channel, DelaySpec>, t: f32, v: f32) -> BTreeMap<String, Signal> {
        let mut vals = vec![0.0; m.columns.len()];
        for c in tilt_cols(m) {
            vals[c] = v;
        }
        m.expand_delayed(&vals, t, delays)
    }

    #[test]
    fn no_delay_is_the_old_expand() {
        let mut m = bar().matrix();
        let v: Vec<f32> = (0..m.columns.len()).map(|c| c as f32 * 0.01).collect();
        let plain = m.expand(&v);
        assert_eq!(m.expand_delayed(&v, 3.0, &BTreeMap::new()), plain);
        let zero = BTreeMap::from([(Channel::Tilt, DelaySpec { step: 0.0, dir: DelayDir::Forward, wrap: 0 })]);
        assert_eq!(m.expand_delayed(&v, 4.0, &zero), plain);
    }

    #[test]
    fn a_step_reaches_fixture_i_at_i_times_delay() {
        let mut m = bar().matrix();
        let delays = BTreeMap::from([(Channel::Tilt, DelaySpec { step: 0.5, dir: DelayDir::Forward, wrap: 0 })]);
        for k in 0..=12 {
            let t = k as f32 * 0.25;
            let out = run(&mut m, &delays, t, if t >= 1.0 { 1.0 } else { 0.0 });
            for mem in m.members.iter().filter(|x| x.channel == Channel::Tilt) {
                let id = &mem.fixture;
                let on = t >= 1.0 + 0.5 * mem.index_in_group as f32;
                assert_eq!(Channel::Tilt.get(&out[id]), if on { 1.0 } else { 0.0 }, "{id} at t={t}");
            }
        }
    }

    #[test]
    fn center_out_responds_from_the_middle_outwards() {
        let mut m = bar().matrix();
        let delays = BTreeMap::from([(Channel::Tilt, DelaySpec { step: 1.0, dir: DelayDir::CenterOut, wrap: 0 })]);
        // The middle head (index 2) leads and answers at 1.0; 1 and 3 at 2.0; 0 and 4 at 3.0.
        let mut first_on: BTreeMap<usize, f32> = BTreeMap::new();
        for k in 0..=40 {
            let t = k as f32 * 0.125;
            let out = run(&mut m, &delays, t, if t >= 1.0 { 1.0 } else { 0.0 });
            for mem in m.members.iter().filter(|x| x.channel == Channel::Tilt) {
                if Channel::Tilt.get(&out[&mem.fixture]) > 0.5 {
                    first_on.entry(mem.index_in_group).or_insert(t);
                }
            }
        }
        assert_eq!(first_on[&2], 1.0);
        assert_eq!(first_on[&1], 2.0);
        assert_eq!(first_on[&3], 2.0);
        assert_eq!(first_on[&0], 3.0);
        assert_eq!(first_on[&4], 3.0);
    }

    #[test]
    fn steps_by_direction_and_wrap() {
        let fwd = DelaySpec { step: 1.0, dir: DelayDir::Forward, wrap: 0 };
        let steps = |s: DelaySpec| (0..5).map(|i| s.steps(i, 5)).collect::<Vec<f32>>();
        assert_eq!(steps(fwd), vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        assert_eq!(steps(DelaySpec { dir: DelayDir::Reverse, ..fwd }), vec![4.0, 3.0, 2.0, 1.0, 0.0]);
        assert_eq!(steps(DelaySpec { dir: DelayDir::CenterOut, ..fwd }), vec![2.0, 1.0, 0.0, 1.0, 2.0]);
        assert_eq!(steps(DelaySpec { wrap: 2, ..fwd }), vec![0.0, 1.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn the_ring_is_bounded_and_holds() {
        let mut r = DelayRing::default();
        for i in 0..(RING_CAP + 100) {
            r.push(i as f32, i as f32);
        }
        assert_eq!(r.len(), RING_CAP);
        // The oldest kept sample is 100; asking earlier than that holds it.
        assert_eq!(r.hold(10.0), Some(100.0));
        assert_eq!(r.hold(150.5), Some(150.0));
        assert_eq!(r.hold(1e9), Some((RING_CAP + 99) as f32));
        let mut same = DelayRing::default();
        same.push(1.0, 0.25);
        same.push(1.0, 0.75);
        assert_eq!(same.len(), 1);
        assert_eq!(same.hold(1.0), Some(0.75));
        assert_eq!(DelayRing::default().hold(0.0), None);
    }

    #[test]
    fn groups_and_indices_are_counted_per_channel() {
        let m = bar().matrix();
        assert!(m.members.iter().all(|st| st.group == "bar" && st.group_len == 5));
        let mut idx: Vec<usize> = m.members.iter().filter(|x| x.channel == Channel::Tilt).map(|x| x.index_in_group).collect();
        idx.sort();
        assert_eq!(idx, vec![0, 1, 2, 3, 4]);
    }
}

#[cfg(test)]
mod shared_point_tests {
    use super::*;
    use crate::grid::{Col, Level, Row};

    #[test]
    fn fixtures_on_one_point_are_one_column_but_separate_members() {
        let at = Point::new(Level::Rig, Row::Back, Col::L).ok();
        let fixtures = (0..3)
            .map(|i| PatchFixture {
                id: format!("wash-{i}"),
                kind: Kind::Moving,
                at: [0.0, 5.0, i as f32],
                group: Some("heads".into()),
                channels: vec![],
                point: at,
                pose: None,
                pixels: 0,
                // Pinned to one voice: several fixtures share one column.
                variant: Some(Variant::Alpha),
            })
            .collect();
        let mut m = Patch::new("shared", fixtures).matrix();
        let tilt_cols = m.columns.iter().filter(|c| c.channel == Channel::Tilt).count();
        assert_eq!(tilt_cols, 1, "one point, one tilt column");
        let tilt: Vec<&WaveMember> = m.members.iter().filter(|x| x.channel == Channel::Tilt).collect();
        assert_eq!(tilt.len(), 3, "three fixtures, three members");
        let mut idx: Vec<usize> = tilt.iter().map(|x| x.index_in_group).collect();
        idx.sort();
        assert_eq!(idx, vec![0, 1, 2]);
        assert!(tilt.iter().all(|x| x.group_len == 3));
        // One position is one offset: with a step, the three share it, so they answer together.
        let delays = BTreeMap::from([(Channel::Tilt, DelaySpec { step: 1.0, dir: DelayDir::Forward, wrap: 0 })]);
        let mut vals = vec![0.0; m.columns.len()];
        let tc = m.columns.iter().position(|c| c.channel == Channel::Tilt).unwrap();
        m.expand_delayed(&vals, 0.0, &delays);
        vals[tc] = 1.0;
        let out = m.expand_delayed(&vals, 1.0, &delays);
        for id in ["wash-0", "wash-1", "wash-2"] {
            assert_eq!(Channel::Tilt.get(&out[id]), 1.0, "{id} answers with the position");
        }
    }
}

/// Five fixtures on one position (patch-first) and one on each of three others, so the
/// group has four positions. Every `Tilt` and `Hue` column is driven from one value.
#[cfg(test)]
mod position_wave_tests {
    use super::*;
    use crate::grid::{Col, Level, Row};

    fn stacked() -> Patch {
        let pts = [
            Point::new(Level::Rig, Row::Back, Col::L).ok(),
            Point::new(Level::Rig, Row::Back, Col::R).ok(),
            Point::new(Level::Rig, Row::Pro, Col::L).ok(),
            Point::new(Level::Rig, Row::Pro, Col::R).ok(),
        ];
        // Patch order: the five on position 0 first, then one each on 1, 2 and 3.
        let spots: Vec<Option<Point>> = [0, 0, 0, 0, 0, 1, 2, 3].iter().map(|&k| pts[k]).collect();
        let fixtures = spots
            .iter()
            .enumerate()
            .map(|(i, p)| PatchFixture {
                id: format!("wash-{i}"),
                kind: Kind::Moving,
                at: [i as f32, 5.0, 0.0],
                group: Some("wash".into()),
                channels: vec![Channel::Tilt, Channel::Hue],
                point: *p,
                pose: None,
                pixels: 0,
                // Pinned to one voice: several fixtures share one column.
                variant: Some(Variant::Alpha),
            })
            .collect();
        Patch::new("wash", fixtures)
    }

    /// Drives every column of `ch` with `v` and expands; returns the per-fixture values.
    fn drive(m: &mut Matrix, delays: &BTreeMap<Channel, DelaySpec>, t: f32, ch: Channel, v: f32) -> BTreeMap<String, Signal> {
        let mut vals = vec![0.0; m.columns.len()];
        for (c, col) in m.columns.clone().iter().enumerate() {
            if col.channel == ch {
                vals[c] = v;
            }
        }
        m.expand_delayed(&vals, t, delays)
    }

    /// The first time each fixture's channel reads on, stepping `ch` on at `t0` (sampled
    /// every 0.01 s, so a read is within one sample of the true time).
    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.02
    }

    /// The first time each fixture's channel reads on, stepping `ch` on at `t0`.
    fn first_on(ch: Channel, delays: &BTreeMap<Channel, DelaySpec>, m: &mut Matrix, t0: f32) -> BTreeMap<String, f32> {
        let mut on: BTreeMap<String, f32> = BTreeMap::new();
        for k in 0..=400 {
            let t = k as f32 * 0.01;
            let out = drive(m, delays, t, ch, if t >= t0 { 1.0 } else { 0.0 });
            for (id, s) in &out {
                if ch.get(s) > 0.5 {
                    on.entry(id.clone()).or_insert(t);
                }
            }
        }
        on
    }

    #[test]
    fn offsets_are_per_position_cumulative_and_per_channel() {
        let delays = BTreeMap::from([
            (Channel::Tilt, DelaySpec { step: 0.2, dir: DelayDir::Forward, wrap: 0 }),
            (Channel::Hue, DelaySpec { step: 0.5, dir: DelayDir::Forward, wrap: 0 }),
        ]);
        // One matrix per sweep: a sweep restarts the clock, and each ring needs monotonic time.
        let tilt = first_on(Channel::Tilt, &delays, &mut stacked().matrix(), 1.0);
        let hue = first_on(Channel::Hue, &delays, &mut stacked().matrix(), 1.0);
        let at = |on: &BTreeMap<String, f32>, i: usize| on[&format!("wash-{i}")];
        // Five on position 0 share one offset (none): all answer at 1.0.
        for i in 0..5 {
            assert!(near(at(&tilt, i), 1.0), "tilt wash-{i}");
            assert!(near(at(&hue, i), 1.0), "hue wash-{i}");
        }
        // Positions 1, 2, 3 sit one, two and three steps behind, per channel.
        assert!(near(at(&tilt, 5), 1.2) && near(at(&tilt, 6), 1.4) && near(at(&tilt, 7), 1.6));
        assert!(near(at(&hue, 5), 1.5) && near(at(&hue, 6), 2.0) && near(at(&hue, 7), 2.5));
    }

    #[test]
    fn wrap_counts_positions_and_each_repeat_leads() {
        let mut m = stacked().matrix();
        let delays = BTreeMap::from([(Channel::Tilt, DelaySpec { step: 1.0, dir: DelayDir::Forward, wrap: 2 })]);
        let tilt = first_on(Channel::Tilt, &delays, &mut m, 1.0);
        let at = |i: usize| tilt[&format!("wash-{i}")];
        // Positions 0,1,0,1: the five on position 0 lead, position 1 follows a step later.
        assert!(near(at(0), 1.0));
        assert!(near(at(5), 2.0), "position 1 waits one step behind its repeat's lead");
        // Positions 2 and 3 are the second repeat: the lead of position 2 answers at once.
        assert!(near(at(6), 1.0), "a new repeat has its own lead, not position 0's");
        assert!(near(at(7), 2.0));
    }
}
