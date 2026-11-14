//! The common matrix: one fixed layout of stage floats that every venue fits in.
//!
//! - [`layout`]: every place (24 points, 23 lines) × every type × its four variants
//!   (α β γ δ) × that type's channels, plus the pixel segments of the pixel types.
//!   It never depends on a venue, so a scene or a dataset example written against it
//!   plays on any patch.
//! - [`retarget`]: common floats → the floats one patch answers. A cell float (no
//!   variant) speaks for all four variants; a variant float wins over its cell. A
//!   variant the cell lacks folds back (γ → α, δ → β); a cell the venue lacks lands
//!   on a stand-in type at the same place, else the nearest spot holding the type; a
//!   pixel segment the venue lacks lands on its bar.
//! - [`dedup`]: the patch read as cells. Fixtures of one type at one place are dealt
//!   into the four variants; the report says how far the patch shrinks and which
//!   waves (groups) the cells regroup into.

use crate::grid::{Frame, Point};
use crate::names::{Channel, FloatName, Kind, Variant};
use crate::patch::{Matrix, Patch, PatchFixture};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The most segments a pixel cell is split into. A bar with more pixels spreads
/// each segment over a run of them; one with fewer gets one segment per pixel.
pub const PIXEL_SEGMENTS: usize = 8;

/// How many segment floats a fixture's cell carries.
pub fn segments(f: &PatchFixture) -> usize {
    if f.kind.pixelled() { (f.pixels as usize).min(PIXEL_SEGMENTS) } else { 0 }
}

/// The output key a segment of a fixture drives: `"<id>#<segment>"`.
pub fn pixel_key(id: &str, segment: usize) -> String {
    format!("{id}#{segment}")
}

/// A key's fixture id (`"bar-1#3"` → `"bar-1"`).
pub fn base_id(key: &str) -> &str {
    key.split_once('#').map_or(key, |(id, _)| id)
}

/// The pixels a segment covers, of a bar with `pixels` pixels: an even split, the
/// remainder to the first segments.
pub fn segment_pixels(pixels: usize, segment: usize) -> std::ops::Range<usize> {
    let n = pixels.min(PIXEL_SEGMENTS).max(1);
    let (base, extra) = (pixels / n, pixels % n);
    let start = segment * base + segment.min(extra);
    start..start + base + usize::from(segment < extra)
}

/// Every float of the common matrix, in a fixed order.
pub fn layout() -> Vec<FloatName> {
    let mut out = Vec::new();
    for point in Point::all() {
        for kind in Kind::ALL {
            for v in Variant::ALL {
                for &channel in kind.default_channels() {
                    out.push(FloatName::new(point, kind, channel).with_variant(v));
                }
                if kind.pixelled() {
                    for seg in 0..PIXEL_SEGMENTS {
                        for &channel in kind.default_channels() {
                            out.push(FloatName { pixel: Some(seg as u8), ..FloatName::new(point, kind, channel).with_variant(v) });
                        }
                    }
                }
            }
        }
    }
    out
}

/// Where each common float went on one patch.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Retarget {
    /// The patch's floats (averaged where several common floats land on one).
    pub floats: BTreeMap<String, f32>,
    /// `(asked, landed)` for every float that did not land on itself.
    pub moved: Vec<(String, String)>,
    /// Names that are not floats, or that no fixture of the patch can take.
    pub dropped: Vec<String>,
}

/// The column of the patch a common float lands on. A cell float (no variant) lands
/// as its α; [`land_all`] spreads it over every variant.
pub fn land(m: &Matrix, frame: &Frame, want: &FloatName) -> Option<FloatName> {
    let want = &FloatName { variant: Some(want.variant.unwrap_or(Variant::Alpha)), ..*want };
    let has = |n: &FloatName| m.index(n).is_some();
    if has(want) {
        return Some(*want);
    }
    // A segment the venue lacks drives its whole bar.
    if want.pixel.is_some() {
        return land(m, frame, &want.cell());
    }
    let v = want.variant.unwrap_or(Variant::Alpha);
    // The same place: the type, then its stand-ins, folding a missing variant back.
    let kinds: Vec<Kind> = std::iter::once(want.kind).chain(want.kind.stand_ins().iter().copied()).collect();
    for &kind in &kinds {
        let vs = m.variants(want.point, kind, want.channel);
        if !vs.is_empty() {
            return Some(FloatName { kind, variant: Some(vs[v.index() % vs.len()]), ..*want });
        }
    }
    // The nearest spot holding the type, then its stand-ins.
    kinds.iter().find_map(|&kind| m.fallback(frame, &FloatName { kind, ..*want }))
}

/// Every column a common float lands on: a cell float lands once per variant.
pub fn land_all(m: &Matrix, frame: &Frame, want: &FloatName) -> Vec<FloatName> {
    let asked: Vec<FloatName> = match want.variant {
        Some(_) => vec![*want],
        None => Variant::ALL.iter().map(|&v| want.with_variant(v)).collect(),
    };
    let mut out: Vec<FloatName> = asked.iter().filter_map(|n| land(m, frame, n)).collect();
    out.sort();
    out.dedup();
    out
}

/// Common floats → the patch's floats.
pub fn retarget(m: &Matrix, frame: &Frame, floats: &BTreeMap<String, f32>) -> Retarget {
    // Per column: (sum, n) from variant floats, and from cell floats.
    let mut exact: BTreeMap<FloatName, (f32, u32)> = BTreeMap::new();
    let mut broad: BTreeMap<FloatName, (f32, u32)> = BTreeMap::new();
    let mut out = Retarget::default();
    for (name, v) in floats {
        let Some(want) = FloatName::parse(name) else {
            out.dropped.push(name.clone());
            continue;
        };
        let landed = land_all(m, frame, &want);
        if landed.is_empty() {
            out.dropped.push(name.clone());
            continue;
        }
        for to in &landed {
            // A cell float filling its own variants has not moved.
            let home = want.variant.is_none() && to.point == want.point && to.kind == want.kind && to.pixel == want.pixel;
            if !home && to.to_string() != *name {
                out.moved.push((name.clone(), to.to_string()));
            }
            let e = if want.variant.is_some() { exact.entry(*to) } else { broad.entry(*to) }.or_default();
            e.0 += v;
            e.1 += 1;
        }
    }
    for (n, e) in broad {
        exact.entry(n).or_insert(e);
    }
    out.floats = exact.into_iter().map(|(n, (s, k))| (n.to_string(), s / k as f32)).collect();
    out
}

/// One cell of a patch: a type at a point and the fixtures it fans out to.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cell {
    pub point: String,
    pub kind: Kind,
    pub fixtures: Vec<String>,
    /// The venue groups its fixtures came in with.
    pub groups: Vec<String>,
    pub segments: usize,
    /// How many of the four variants the cell's fixtures fill.
    pub variants: usize,
}

/// The patch read as cells.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dedup {
    pub fixtures: usize,
    pub cells: Vec<Cell>,
    /// Rows the patch asks for one per fixture and channel, against one per cell.
    pub fixture_rows: usize,
    pub cell_rows: usize,
    /// The venue's groups and the cell waves they fold into.
    pub groups_before: usize,
    pub groups_after: usize,
    /// `(fixture, old group, new group)` for every fixture whose wave changes.
    pub regroup: Vec<(String, String, String)>,
    /// Floats of the common layout this patch answers itself, of [`layout`]'s total.
    pub covered: usize,
    pub common: usize,
}

/// The wave a fixture belongs to once the patch is deduped: its type on its level,
/// so a wave runs across every position of one type at one height.
pub fn wave_of(kind: Kind, point: Point) -> String {
    format!("{}.{}", kind.word(), point.level.word())
}

pub fn dedup(patch: &Patch) -> Dedup {
    let places = patch.placements();
    let mut cells: BTreeMap<(Point, Kind), Cell> = BTreeMap::new();
    let mut fixture_rows = 0;
    let mut regroup = Vec::new();
    let mut before = BTreeSet::new();
    let mut after = BTreeSet::new();
    for (f, p) in patch.fixtures.iter().zip(&places) {
        let ch = f.channels().len();
        fixture_rows += ch * (1 + segments(f));
        let old = f.group.clone().unwrap_or_else(|| f.kind.word().to_string());
        let new = wave_of(f.kind, p.point);
        if old != new {
            regroup.push((f.id.clone(), old.clone(), new.clone()));
        }
        before.insert(old.clone());
        after.insert(new);
        let c = cells.entry((p.point, f.kind)).or_insert_with(|| Cell {
            point: p.point.word(),
            kind: f.kind,
            fixtures: vec![],
            groups: vec![],
            segments: 0,
            variants: 0,
        });
        c.fixtures.push(f.id.clone());
        c.variants = c.fixtures.len().min(Variant::ALL.len());
        if !c.groups.contains(&old) {
            c.groups.push(old);
        }
        c.segments = c.segments.max(segments(f));
    }
    let m = patch.matrix();
    let common = layout();
    let have: BTreeSet<&FloatName> = m.columns.iter().collect();
    Dedup {
        fixtures: patch.fixtures.len(),
        cells: cells.into_values().collect(),
        fixture_rows,
        cell_rows: m.columns.len(),
        groups_before: before.len(),
        groups_after: after.len(),
        regroup,
        covered: common.iter().filter(|n| have.contains(n)).count(),
        common: common.len(),
    }
}

impl Patch {
    /// The patch with every fixture's group set to its cell wave ([`wave_of`]).
    pub fn deduped(&self) -> Patch {
        let mut p = self.clone();
        for (f, pl) in p.fixtures.iter_mut().zip(self.placements()) {
            f.group = Some(wave_of(f.kind, pl.point));
        }
        p
    }
}

/// The float names of a set that the common layout does not hold.
pub fn outside(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let all: BTreeSet<String> = layout().iter().map(|n| n.to_string()).collect();
    names.into_iter().filter(|n| !all.contains(n)).collect()
}

/// A channel's rest value, for a float a patch drops.
pub fn rest(channel: Channel) -> f32 {
    channel.get(&Channel::rest())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::tests::{demo, pf};

    #[test]
    fn the_layout_is_fixed_and_round_trips() {
        let l = layout();
        let per_point: usize = Kind::ALL
            .iter()
            .map(|k| k.default_channels().len() * (1 + if k.pixelled() { PIXEL_SEGMENTS } else { 0 }))
            .sum();
        assert_eq!(l.len(), Point::all().len() * per_point * Variant::ALL.len());
        let set: BTreeSet<&FloatName> = l.iter().collect();
        assert_eq!(set.len(), l.len(), "no float twice");
        for n in &l {
            assert_eq!(FloatName::parse(&n.to_string()), Some(*n), "{n}");
        }
        assert!(outside(demo().matrix().names()).is_empty(), "every patch float is a common float");
    }

    #[test]
    fn segments_split_pixels_evenly() {
        let r: Vec<_> = (0..8).map(|s| segment_pixels(20, s)).collect();
        assert_eq!(r[0], 0..3);
        assert_eq!(r[3], 9..12);
        assert_eq!(r[4], 12..14);
        assert_eq!(r[7].end, 20);
        assert_eq!(segment_pixels(3, 2), 2..3);
        assert_eq!(base_id(&pixel_key("bar", 4)), "bar");
    }

    #[test]
    fn a_pixel_bar_gets_segment_columns_driving_its_pixels() {
        let mut bar = pf("bar", Kind::LedBar, [0.0, 4.0, -5.0]);
        bar.pixels = 12;
        let p = Patch::new("px", vec![bar]);
        let m = p.matrix();
        let segs = m.columns.iter().filter(|c| c.pixel.is_some() && c.channel == Channel::Level).count();
        assert_eq!(segs, PIXEL_SEGMENTS);
        let mut v = vec![0.0; m.columns.len()];
        let i = m.columns.iter().position(|c| c.pixel == Some(2) && c.channel == Channel::Hue).unwrap();
        v[i] = 0.4;
        let out = m.expand(&v);
        assert_eq!(out["bar#2"].hue, 0.4);
        assert_eq!(out["bar#1"].hue, 0.0);
        assert!(out.contains_key("bar"), "the whole bar still has its own signal");
    }

    #[test]
    fn retarget_lands_on_stand_ins_and_nearest_points() {
        let p = demo();
        let m = p.matrix();
        let col = |pred: &dyn Fn(&FloatName) -> bool| *m.columns.iter().find(|c| pred(c)).unwrap();
        let back = col(&|c| c.kind == Kind::Backlight && c.channel == Channel::Level);
        // A spot asked where only a backlight hangs lands on the backlight.
        let spot = FloatName { kind: Kind::Spot, ..back };
        // A wash pixel-less bar asked on the zenith over the audience lands somewhere real.
        let far = FloatName::parse("l3.house.c.led_bar.p5.level").unwrap();
        let floats: BTreeMap<String, f32> =
            [(spot.to_string(), 0.6), (back.to_string(), 0.2), (far.to_string(), 0.3), ("nope".into(), 1.0)].into_iter().collect();
        let r = retarget(&m, &p.frame, &floats);
        assert_eq!(r.floats[&back.to_string()], 0.4, "two floats on one column average");
        assert!(r.moved.iter().any(|(a, b)| a == &spot.to_string() && b == &back.to_string()));
        assert!(r.moved.iter().any(|(a, _)| a == &far.to_string()), "{:?}", r.moved);
        assert_eq!(r.dropped, vec!["nope".to_string()]);
        assert!(r.floats.keys().all(|k| m.index(&FloatName::parse(k).unwrap()).is_some()));
    }

    #[test]
    fn dedup_folds_fixtures_into_cells_and_waves() {
        let mut fx: Vec<PatchFixture> = (0..4)
            .map(|i| {
                let mut f = pf(&format!("par-{i}"), Kind::Wash, [-3.0, 6.7, -0.8]);
                f.group = Some(["abj", "abk", "bc", "bcw"][i].into());
                f
            })
            .collect();
        fx.push(pf("par-far", Kind::Wash, [3.0, 6.7, -0.8]));
        let p = Patch { frame: Frame::default(), ..Patch::new("frag", fx) };
        let d = dedup(&p);
        assert_eq!(d.fixtures, 5);
        assert_eq!(d.cells.len(), 1, "five washes on one row are one line");
        assert!(d.cells[0].point.ends_with(".x"), "{}", d.cells[0].point);
        assert_eq!(d.cells[0].variants, 4);
        assert!(d.cell_rows < d.fixture_rows);
        assert_eq!(d.groups_before, 5);
        assert_eq!(d.groups_after, 1, "one wave along the line");
        let deduped = p.deduped();
        let m = deduped.matrix();
        assert!(m.members.iter().all(|x| x.buckets == 4), "the wave runs alpha to delta");
    }

    /// A truss of heads: the line across, dealt left to right.
    fn truss(n: usize) -> Patch {
        let fx = (0..n).map(|i| pf(&format!("h{i}"), Kind::Moving, [-4.0 + 8.0 * i as f32 / (n - 1).max(1) as f32, 4.0, 0.0])).collect();
        Patch { frame: Frame::default(), ..Patch::new("truss", fx) }
    }

    #[test]
    fn a_row_of_heads_is_a_line_dealt_alpha_to_delta_left_to_right() {
        let p = truss(6);
        let pl = p.placements();
        assert!(pl.iter().all(|x| x.point.word() == "l2.pro.x"), "{:?}", pl.iter().map(|x| x.point.word()).collect::<Vec<_>>());
        let v: Vec<Variant> = pl.iter().map(|x| x.variant).collect();
        use Variant::*;
        assert_eq!(v, vec![Alpha, Beta, Gamma, Delta, Alpha, Beta]);
    }

    #[test]
    fn a_cell_float_fills_every_variant_and_a_variant_float_wins() {
        let p = truss(4);
        let m = p.matrix();
        let floats: BTreeMap<String, f32> =
            [("l2.pro.x.moving.pan".to_string(), 30.0), ("l2.pro.x.moving.beta.pan".to_string(), -30.0)].into_iter().collect();
        let r = retarget(&m, &p.frame, &floats);
        assert_eq!(r.floats["l2.pro.x.moving.alpha.pan"], 30.0);
        assert_eq!(r.floats["l2.pro.x.moving.beta.pan"], -30.0, "mirrored voice");
        assert_eq!(r.floats["l2.pro.x.moving.delta.pan"], 30.0);
        assert!(r.moved.is_empty(), "{:?}", r.moved);
    }

    #[test]
    fn missing_variants_fold_back_and_a_line_lands_on_points() {
        // Two heads: alpha and beta only; gamma folds to alpha, delta to beta.
        let p = Patch { frame: Frame::default(), ..Patch::new("pair", vec![pf("a", Kind::Moving, [-3.0, 4.0, 0.0]), pf("b", Kind::Moving, [3.0, 4.0, 0.0])]) };
        let m = p.matrix();
        let g = land(&m, &p.frame, &FloatName::parse("l2.pro.l.moving.gamma.tilt").unwrap()).unwrap();
        assert_eq!(g.to_string(), "l2.pro.l.moving.alpha.tilt");
        // The proscenium line asked of a venue with two points: alpha lands left, delta right.
        let a = land(&m, &p.frame, &FloatName::parse("l2.pro.x.moving.alpha.tilt").unwrap()).unwrap();
        let d = land(&m, &p.frame, &FloatName::parse("l2.pro.x.moving.delta.tilt").unwrap()).unwrap();
        assert_eq!((a.point.word(), d.point.word()), ("l2.pro.l".into(), "l2.pro.r".into()));
    }

    #[test]
    fn bars_at_a_stage_edge_are_a_side_wall() {
        let mut fx: Vec<PatchFixture> = (0..3).map(|i| pf(&format!("w{i}"), Kind::LedBar, [-4.9, 2.0, -1.0 - 2.0 * i as f32])).collect();
        fx.push(pf("far", Kind::Wash, [4.0, 4.0, -6.0]));
        let mut p = Patch::new("walls", fx);
        p.frame.width = 10.0;
        let pl = p.placements();
        assert!(pl[..3].iter().all(|x| x.point.col == crate::grid::Col::Wl && x.point.row == crate::grid::Row::Run), "{:?}", pl);
        // Back to front: the deepest bar is alpha.
        assert_eq!(pl[2].variant, Variant::Alpha);
    }
}
