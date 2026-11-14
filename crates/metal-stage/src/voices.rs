//! Synthetic voices: fill every cell of a matrix that a look leaves empty, so a scene
//! written for one rig uses every place and type of another.
//!
//! The four variants carry an implicit hierarchy:
//! - **alpha, the lead**: the look's main hue at its level, the mean aim.
//! - **beta, the second**: the look's second hue at 80 %, its pan mirrored.
//! - **gamma, the accent**: the complement of the main hue at 55 %, carries the strobe,
//!   in sync with the lead.
//! - **delta, the support**: the main hue half-saturated at 35 %, mirrored.
//!
//! Pixel segments run from their voice's hue to the next voice's, so a bar reads as one
//! gradient across the four voices of its line. Floats the look wrote itself are never
//! touched: [`fill`] only answers the columns it is handed as empty.

use crate::names::{Channel, FloatName, Variant};
use crate::patch::Matrix;
use std::collections::BTreeMap;

/// What a look says, summed over the floats it wrote.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub hue: f32,
    pub second: f32,
    pub sat: f32,
    pub level: f32,
    pub strobe: f32,
    pub pan: f32,
    pub tilt: f32,
}

impl Palette {
    /// The palette of a set of floats: hues averaged on the circle by level, the second
    /// hue the strongest one at least 30° from the first (else the first + 30°).
    pub fn of(floats: &BTreeMap<FloatName, f32>) -> Palette {
        let get = |n: &FloatName, c: Channel| floats.get(&FloatName { channel: c, ..*n }).copied();
        let mut hues: Vec<(f32, f32)> = Vec::new();
        let (mut sat, mut sw, mut level, mut ln, mut strobe) = (0.0, 0.0, 0.0f32, 0, 0.0f32);
        let (mut pan, mut tilt, mut pn) = (0.0, 0.0, 0);
        for (n, v) in floats {
            match n.channel {
                Channel::Level => {
                    level += v;
                    ln += 1;
                    if let Some(h) = get(n, Channel::Hue) {
                        hues.push((h, v.max(0.05)));
                        sat += get(n, Channel::Sat).unwrap_or(1.0) * v.max(0.05);
                        sw += v.max(0.05);
                    }
                }
                Channel::Strobe => strobe = strobe.max(*v),
                Channel::Pan => {
                    pan += v;
                    pn += 1;
                    tilt += get(n, Channel::Tilt).unwrap_or(0.0);
                }
                _ => {}
            }
        }
        let hue = circular_mean(&hues).unwrap_or(0.08);
        let second = hues
            .iter()
            .filter(|(h, _)| hue_dist(*h, hue) >= 30.0 / 360.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or((hue + 30.0 / 360.0).fract(), |(h, _)| *h);
        Palette {
            hue,
            second,
            sat: if sw > 0.0 { sat / sw } else { 0.8 },
            level: if ln > 0 { (level / ln as f32).max(0.3) } else { 0.7 },
            strobe,
            pan: if pn > 0 { pan / pn as f32 } else { 0.0 },
            tilt: if pn > 0 { tilt / pn as f32 } else { -30.0 },
        }
    }

    /// One voice's value for a channel (`segment`: which of 8 pixel segments, if any).
    pub fn voice(&self, v: Variant, channel: Channel, segment: Option<u8>) -> f32 {
        let (hue, sat, level) = match v {
            Variant::Alpha => (self.hue, self.sat, self.level),
            Variant::Beta => (self.second, self.sat, self.level * 0.8),
            Variant::Gamma => ((self.hue + 0.5).fract(), self.sat, self.level * 0.55),
            Variant::Delta => (self.hue, self.sat * 0.5, self.level * 0.35),
        };
        let hue = match segment {
            Some(s) => {
                let next = self.voice(Variant::nth(v.index() + 1), Channel::Hue, None);
                lerp_hue(hue, next, s as f32 / crate::common::PIXEL_SEGMENTS as f32)
            }
            None => hue,
        };
        let mirrored = matches!(v, Variant::Beta | Variant::Delta);
        match channel {
            Channel::Level => level.clamp(0.0, 1.0),
            Channel::Hue => hue,
            Channel::Sat => sat.clamp(0.0, 1.0),
            Channel::Strobe => if v == Variant::Gamma { self.strobe } else { 0.0 },
            Channel::Pan => if mirrored { -self.pan } else { self.pan },
            Channel::Tilt => self.tilt,
            Channel::Cct => 5600.0,
            Channel::Zoom | Channel::Focus => 0.5,
            Channel::Iris => 1.0,
            Channel::Frost => 0.0,
        }
    }
}

/// Values for `empty` columns of `m` from the palette of `written`.
pub fn fill(m: &Matrix, written: &BTreeMap<FloatName, f32>, empty: &[usize]) -> Vec<(usize, f32)> {
    let p = Palette::of(written);
    empty
        .iter()
        .map(|&i| {
            let c = m.columns[i];
            (i, p.voice(c.variant.unwrap_or(Variant::Alpha), c.channel, c.pixel))
        })
        .collect()
}

fn circular_mean(h: &[(f32, f32)]) -> Option<f32> {
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for (hue, w) in h {
        let a = hue * std::f32::consts::TAU;
        x += a.cos() * w;
        y += a.sin() * w;
    }
    (x.abs() + y.abs() > 1e-6).then(|| (y.atan2(x) / std::f32::consts::TAU).rem_euclid(1.0))
}

fn hue_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(1.0);
    d.min(1.0 - d)
}

fn lerp_hue(a: f32, b: f32, t: f32) -> f32 {
    let mut d = (b - a).rem_euclid(1.0);
    if d > 0.5 {
        d -= 1.0;
    }
    (a + d * t).rem_euclid(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> FloatName {
        FloatName::parse(s).unwrap()
    }

    #[test]
    fn the_voices_keep_their_hierarchy() {
        let written: BTreeMap<FloatName, f32> = [
            (n("l2.pro.l.wash.alpha.level"), 1.0),
            (n("l2.pro.l.wash.alpha.hue"), 0.6),
            (n("l2.pro.l.wash.alpha.sat"), 1.0),
            (n("l2.pro.r.wash.alpha.level"), 0.5),
            (n("l2.pro.r.wash.alpha.hue"), 0.1),
            (n("l2.pro.r.moving.alpha.pan"), 40.0),
            (n("l2.pro.r.moving.alpha.tilt"), -20.0),
        ]
        .into_iter()
        .collect();
        let p = Palette::of(&written);
        assert!(hue_dist(p.hue, 0.6) < 0.15, "the lead leans to the brighter hue: {}", p.hue);
        assert!(hue_dist(p.second, 0.1) < 1e-4);
        let lv = |v| p.voice(v, Channel::Level, None);
        assert!(lv(Variant::Alpha) > lv(Variant::Beta) && lv(Variant::Beta) > lv(Variant::Gamma) && lv(Variant::Gamma) > lv(Variant::Delta));
        assert!(hue_dist(p.voice(Variant::Gamma, Channel::Hue, None), (p.hue + 0.5).fract()) < 1e-4, "the accent is the complement");
        assert_eq!(p.voice(Variant::Beta, Channel::Pan, None), -40.0, "beta mirrors");
        assert_eq!(p.voice(Variant::Gamma, Channel::Pan, None), 40.0, "gamma follows the lead");
        let s0 = p.voice(Variant::Alpha, Channel::Hue, Some(0));
        assert!(hue_dist(s0, p.hue) < 1e-4, "segment 0 starts on its voice");
    }

    #[test]
    fn an_empty_look_still_lights_the_rig() {
        let p = Palette::of(&BTreeMap::new());
        assert!(p.voice(Variant::Alpha, Channel::Level, None) > 0.0);
    }
}
