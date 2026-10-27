//! P-F: edge blending between projectors that share one venue surface.
//!
//! Two projectors pinned to the same surface (`Projector::pins`) overlap; with
//! no blend both draw full brightness and the overlap is a bright band. Here
//! the venue works out, per surface, where each projector's footprint lies in
//! the surface's own `(s, t)`, and gives every device pixel a gain:
//!
//! - **ramp**: at a surface point covered by projectors `P₁…Pₙ`, each one's
//!   distance to the edge of its own frame (`dᵢ`, in that device's 0..1) is
//!   normalised, `aᵢ = dᵢ / Σd`, shaped by the [`Curve`] and normalised again,
//!   so the weights `wᵢ` sum to 1 **in linear light**;
//! - **gamma**: the pixel is multiplied by `w^(1/gamma)`, so after the
//!   projector's own `gamma` the light it emits is `w` times the signal and the
//!   overlap adds up to one projector's worth;
//! - **black lift**: outside the overlap (but on the surface) the black floor is
//!   raised by `black_lift`, so the doubled black of the overlap does not show
//!   as a grey band on a dark frame.
//!
//! Config is [`Blend`] on the projector in the venue (never a `Mask` field).
//! Only projectors with `blend.enabled` and pins on the surface take part; a
//! venue where none is enabled yields no [`BlendMap`] and the frame is not
//! touched — byte-identical to before.

use serde::{Deserialize, Serialize};

use crate::escena::Homography;
use crate::venue::Venue;

/// The shape of the ramp across the overlap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    Linear,
    #[default]
    Smoothstep,
    Cosine,
}

impl Curve {
    pub fn from_word(word: &str) -> Option<Curve> {
        match word {
            "linear" => Some(Curve::Linear),
            "smoothstep" => Some(Curve::Smoothstep),
            "cosine" => Some(Curve::Cosine),
            _ => None,
        }
    }

    /// `a` in 0..1 shaped; every curve keeps `c(a) + c(1 − a) = 1`.
    pub fn shape(self, a: f32) -> f32 {
        let a = a.clamp(0.0, 1.0);
        match self {
            Curve::Linear => a,
            Curve::Smoothstep => a * a * (3.0 - 2.0 * a),
            Curve::Cosine => 0.5 - 0.5 * (std::f32::consts::PI * a).cos(),
        }
    }
}

/// `blend { enabled, gamma, curve, black_lift }` on a venue projector.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Blend {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_gamma")]
    pub gamma: f32,
    #[serde(default)]
    pub curve: Curve,
    /// 0..1 of full scale, added to the black of the non-overlap.
    #[serde(default)]
    pub black_lift: f32,
}

fn default_gamma() -> f32 {
    2.2
}

impl Default for Blend {
    fn default() -> Blend {
        Blend {
            enabled: false,
            gamma: default_gamma(),
            curve: Curve::default(),
            black_lift: 0.0,
        }
    }
}

impl Blend {
    pub fn is_default(&self) -> bool {
        *self == Blend::default()
    }
}

/// `H · (x, y)` with the projective divide; `None` when `w` vanishes.
fn project(h: &Homography, p: [f32; 2]) -> Option<[f32; 2]> {
    let [x, y, w] = h.apply(p[0] as f64, p[1] as f64);
    if w.abs() < 1e-12 {
        return None;
    }
    let out = [(x / w) as f32, (y / w) as f32];
    (out[0].is_finite() && out[1].is_finite()).then_some(out)
}

const UNIT: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

fn inside_unit(p: [f32; 2]) -> bool {
    (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1])
}

/// Distance to the edge of the unit square, 0 outside.
fn edge_distance(p: [f32; 2]) -> f32 {
    if !inside_unit(p) {
        return 0.0;
    }
    p[0].min(1.0 - p[0]).min(p[1]).min(1.0 - p[1])
}

/// One projector taking part in a surface's blend.
struct Member {
    output: String,
    /// surface `(s, t)` → device 0..1.
    to_device: Homography,
    /// device 0..1 → surface `(s, t)`.
    to_surface: Homography,
    blend: Blend,
}

fn members(venue: &Venue, surface: &str) -> Vec<Member> {
    venue
        .projectors
        .iter()
        .filter(|p| p.blend.enabled)
        .filter_map(|p| {
            let pins = p.pins.get(surface)?;
            let to_device = Homography::from_corners(&UNIT, pins)?;
            let to_surface = to_device.inverse()?;
            Some(Member { output: p.output.clone(), to_device, to_surface, blend: p.blend })
        })
        .collect()
}

/// The blend weight of `members[me]` at surface point `st`, and whether `st`
/// lies in an overlap (≥ 2 projectors cover it).
fn weight(members: &[Member], me: usize, st: [f32; 2]) -> (f32, bool) {
    let d: Vec<f32> = members
        .iter()
        .map(|m| project(&m.to_device, st).map(edge_distance).unwrap_or(0.0))
        .collect();
    let covering = d.iter().filter(|x| **x > 0.0).count();
    let sum: f32 = d.iter().sum();
    if sum <= 0.0 || d[me] <= 0.0 {
        return (0.0, covering >= 2);
    }
    if covering < 2 {
        return (1.0, false);
    }
    let curve = members[me].blend.curve;
    let shaped: Vec<f32> = d.iter().map(|x| curve.shape(x / sum)).collect();
    let total: f32 = shaped.iter().sum();
    (if total > 0.0 { shaped[me] / total } else { 0.0 }, true)
}

/// The surfaces every enabled projector of `venue` shares with another.
fn blended_surfaces(venue: &Venue) -> Vec<String> {
    let mut out: Vec<String> = venue.surfaces.iter().map(|s| s.id.clone()).collect();
    out.retain(|s| members(venue, s).len() >= 2);
    out
}

/// The per-pixel blend of one device: `gain[i]` multiplies pixel `i`,
/// `lift[i]` (0..1) raises its black.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendMap {
    pub width: u32,
    pub height: u32,
    pub gain: Vec<f32>,
    pub lift: Vec<f32>,
}

/// The [`BlendMap`] for `output` at `w × h`, or `None` when the device takes
/// part in no blend — the frame is then left exactly as it was.
pub fn blend_map(venue: &Venue, output: &str, w: u32, h: u32) -> Option<BlendMap> {
    let surfaces: Vec<(String, Vec<Member>, usize)> = blended_surfaces(venue)
        .into_iter()
        .filter_map(|s| {
            let m = members(venue, &s);
            let me = m.iter().position(|x| x.output == output)?;
            Some((s, m, me))
        })
        .collect();
    if surfaces.is_empty() || w == 0 || h == 0 {
        return None;
    }
    let n = (w as usize) * (h as usize);
    let mut map = BlendMap { width: w, height: h, gain: vec![1.0; n], lift: vec![0.0; n] };
    for y in 0..h {
        for x in 0..w {
            let uv = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
            let i = (y as usize) * (w as usize) + x as usize;
            for (_, m, me) in &surfaces {
                let Some(st) = project(&m[*me].to_surface, uv) else { continue };
                if !inside_unit(st) {
                    continue;
                }
                let b = m[*me].blend;
                let (wt, overlap) = weight(m, *me, st);
                map.gain[i] *= wt.max(0.0).powf(1.0 / b.gamma.max(0.1));
                if !overlap {
                    map.lift[i] = map.lift[i].max(b.black_lift.clamp(0.0, 1.0));
                }
            }
        }
    }
    Some(map)
}

impl BlendMap {
    /// Multiply `frame` (RGBA8, `width × height`) by the gain, then lift the
    /// black: `out = lift·255 + in·gain·(1 − lift)`. Alpha untouched.
    pub fn apply(&self, frame: &mut [u8]) {
        for (i, px) in frame.chunks_exact_mut(4).enumerate().take(self.gain.len()) {
            let (g, l) = (self.gain[i], self.lift[i]);
            if g == 1.0 && l == 0.0 {
                continue;
            }
            for c in px.iter_mut().take(3) {
                let v = l * 255.0 + (*c as f32) * g * (1.0 - l);
                *c = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// The overlap polygons on `surface`, in its `(s, t)`: one per pair of
/// enabled-or-not projectors pinned to it — what the venue editor hatches.
/// Every projector with pins counts here (the editor shows the overlap before
/// the toggle is on).
pub fn overlaps(venue: &Venue, surface: &str) -> Vec<(String, String, Vec<[f32; 2]>)> {
    let foot: Vec<(String, Vec<[f32; 2]>)> = venue
        .projectors
        .iter()
        .filter_map(|p| Some((p.id.clone(), footprint(p.pins.get(surface)?)?)))
        .collect();
    let mut out = Vec::new();
    for i in 0..foot.len() {
        for j in i + 1..foot.len() {
            let poly = clip(&foot[i].1, &foot[j].1);
            if poly.len() >= 3 && area(&poly).abs() > 1e-6 {
                out.push((foot[i].0.clone(), foot[j].0.clone(), poly));
            }
        }
    }
    out
}

/// A projector's footprint on a surface in `(s, t)`: its frame's four corners
/// mapped back through the pins, clipped to the surface.
pub fn footprint(pins: &[[f32; 2]; 4]) -> Option<Vec<[f32; 2]>> {
    let to_surface = Homography::from_corners(&UNIT, pins)?.inverse()?;
    let quad: Vec<[f32; 2]> = UNIT.iter().map(|c| project(&to_surface, *c)).collect::<Option<_>>()?;
    let poly = clip(&quad, &UNIT);
    (poly.len() >= 3).then_some(poly)
}

fn area(p: &[[f32; 2]]) -> f32 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let (q, r) = (p[i], p[(i + 1) % p.len()]);
        a += q[0] * r[1] - r[0] * q[1];
    }
    a * 0.5
}

/// Sutherland–Hodgman: `subject` clipped by the convex `clipper`.
fn clip(subject: &[[f32; 2]], clipper: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let orient = area(clipper).signum();
    let mut out: Vec<[f32; 2]> = subject.to_vec();
    for i in 0..clipper.len() {
        if out.is_empty() {
            break;
        }
        let (a, b) = (clipper[i], clipper[(i + 1) % clipper.len()]);
        let side = |p: [f32; 2]| orient * ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]));
        let input = std::mem::take(&mut out);
        for k in 0..input.len() {
            let (p, q) = (input[k], input[(k + 1) % input.len()]);
            let (sp, sq) = (side(p), side(q));
            if sp >= 0.0 {
                out.push(p);
            }
            if (sp >= 0.0) != (sq >= 0.0) {
                let t = sp / (sp - sq);
                out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venue::{Projector, Surface};

    fn projector(id: &str, pins: [[f32; 2]; 4], enabled: bool) -> Projector {
        let mut p: Projector = toml::from_str(&format!(
            "id = \"{id}\"\nname = \"{id}\"\noutput = \"{id}\"\non = \"table\"\nyaw = 0.0\npitch = 0.0\nthrow_ratio = 1.0\n"
        ))
        .unwrap();
        p.pins.insert("wall".into(), pins);
        p.blend.enabled = enabled;
        p
    }

    /// Surface 0..1 wide; A covers s 0..0.6, B covers s 0.4..1 — so each
    /// device's 0..1 spans 0.6 of the surface.
    fn side_by_side(enabled: bool) -> Venue {
        let mut v: Venue = toml::from_str("[venue]\nid = \"t\"\nname = \"t\"\nsetting = \"indoor\"\n").unwrap();
        v.surfaces.push(Surface {
            id: "wall".into(),
            name: "wall".into(),
            origin: [0.0, 0.0, 0.0],
            u: [4.0, 0.0, 0.0],
            v: [0.0, 2.0, 0.0],
            material: String::new(),
            form: Default::default(),
            sectors: Default::default(),
        });
        // A: s ∈ [0, 0.6] ↦ u ∈ [0, 1]: s=1 lands at u = 1/0.6.
        let a = 1.0 / 0.6;
        v.projectors.push(projector("a", [[0.0, 0.0], [a, 0.0], [a, 1.0], [0.0, 1.0]], enabled));
        // B: s ∈ [0.4, 1] ↦ u ∈ [0, 1]: s=0 lands at u = -0.4/0.6.
        let b0 = -0.4 / 0.6;
        v.projectors.push(projector("b", [[b0, 0.0], [1.0, 0.0], [1.0, 1.0], [b0, 1.0]], enabled));
        v
    }

    #[test]
    fn overlap_of_two_side_by_side_projectors() {
        let v = side_by_side(false);
        let o = overlaps(&v, "wall");
        assert_eq!(o.len(), 1);
        let poly = &o[0].2;
        let (min_s, max_s) = poly.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
        assert!((min_s - 0.4).abs() < 1e-3 && (max_s - 0.6).abs() < 1e-3, "{poly:?}");
        assert!((area(poly).abs() - 0.2).abs() < 1e-3);
    }

    #[test]
    fn weights_sum_to_one_in_linear_light() {
        let v = side_by_side(true);
        let gamma = 2.2f32;
        for curve in [Curve::Linear, Curve::Smoothstep, Curve::Cosine] {
            let mut v = v.clone();
            v.projectors.iter_mut().for_each(|p| p.blend.curve = curve);
            let (w, h) = (300, 4);
            let ma = blend_map(&v, "a", w, h).unwrap();
            let mb = blend_map(&v, "b", w, h).unwrap();
            let ha = Homography::from_corners(&UNIT, &v.projectors[0].pins["wall"]).unwrap();
            let hb = Homography::from_corners(&UNIT, &v.projectors[1].pins["wall"]).unwrap();
            let gain_at = |m: &BlendMap, uv: [f32; 2]| {
                let x = ((uv[0] * w as f32) as u32).min(w - 1);
                let y = ((uv[1] * h as f32) as u32).min(h - 1);
                m.gain[(y * w + x) as usize]
            };
            for k in 1..20 {
                let st = [0.41 + 0.18 * k as f32 / 20.0, 0.5];
                let la = gain_at(&ma, project(&ha, st).unwrap()).powf(gamma);
                let lb = gain_at(&mb, project(&hb, st).unwrap()).powf(gamma);
                assert!((la + lb - 1.0).abs() < 0.05, "{curve:?} at {st:?}: {la} + {lb}");
            }
            // Outside the overlap each projector is at full gain.
            assert!((gain_at(&ma, project(&ha, [0.2, 0.5]).unwrap()) - 1.0).abs() < 1e-6);
            assert!((gain_at(&mb, project(&hb, [0.8, 0.5]).unwrap()) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn disabled_blend_leaves_the_frame_alone() {
        let v = side_by_side(false);
        assert!(blend_map(&v, "a", 64, 36).is_none());
        // One enabled alone has no partner: still nothing.
        let mut one = v.clone();
        one.projectors[0].blend.enabled = true;
        assert!(blend_map(&one, "a", 64, 36).is_none());
        // And a default blend is not written into the venue file.
        let text = toml::to_string(&v.projectors[0]).unwrap();
        assert!(!text.contains("blend"), "{text}");
    }

    #[test]
    fn black_lift_only_outside_the_overlap() {
        let mut v = side_by_side(true);
        v.projectors.iter_mut().for_each(|p| p.blend.black_lift = 0.1);
        let m = blend_map(&v, "a", 100, 2).unwrap();
        // u = 0.1 → s = 0.06 (no overlap); u = 0.95 → s = 0.57 (overlap).
        assert!((m.lift[10] - 0.1).abs() < 1e-6);
        assert_eq!(m.lift[95], 0.0);
        let mut frame = vec![0u8; 100 * 2 * 4];
        m.apply(&mut frame);
        assert_eq!(frame[10 * 4], 26);
        assert_eq!(frame[95 * 4], 0);
    }
}
