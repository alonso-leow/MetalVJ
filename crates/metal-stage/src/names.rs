//! Float names: `<place>.<type>[.<variant>][.p<n>].<channel>`, e.g.
//! `l2.back.l.wash.level` (the whole cell), `l2.pro.x.moving.beta.pan` (one variant of
//! the proscenium line), `l3.house.x.led_bar.gamma.p3.hue` (a segment of a bar).
//! A cell name with no variant speaks for all four variants. They sit beside the
//! shader `sig_*` rows; the fixture AI emits only these.

use crate::grid::Point;
use metal_schema::Signal;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Fixture type words: the venue's `FixtureKind` spellings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Wash,
    Spot,
    Moving,
    Backlight,
    Panel,
    Continuous,
    LedBar,
    LedStrip,
    Matrix,
}
impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Wash => "wash",
            Kind::Spot => "spot",
            Kind::Moving => "moving",
            Kind::Backlight => "backlight",
            Kind::Panel => "panel",
            Kind::Continuous => "continuous",
            Kind::LedBar => "led_bar",
            Kind::LedStrip => "led_strip",
            Kind::Matrix => "matrix",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        use Kind::*;
        [Wash, Spot, Moving, Backlight, Panel, Continuous, LedBar, LedStrip, Matrix].into_iter().find(|k| k.word() == s)
    }
    pub const ALL: [Kind; 9] = [
        Kind::Wash, Kind::Spot, Kind::Moving, Kind::Backlight, Kind::Panel, Kind::Continuous,
        Kind::LedBar, Kind::LedStrip, Kind::Matrix,
    ];
    /// Types whose fixtures carry pixels: their cells get per-segment floats.
    pub fn pixelled(self) -> bool {
        matches!(self, Kind::LedBar | Kind::LedStrip | Kind::Matrix)
    }
    /// The types a float of this type falls back to when a venue has none of it,
    /// nearest first (a backlight is a spot behind the band, a bar a strip).
    pub fn stand_ins(self) -> &'static [Kind] {
        use Kind::*;
        match self {
            Wash => &[Panel, Continuous, Spot],
            Spot => &[Backlight, Moving, Wash],
            Moving => &[Spot, Backlight],
            Backlight => &[Spot, Moving, Wash],
            Panel => &[Wash, Continuous],
            Continuous => &[Wash, Panel],
            LedBar => &[LedStrip, Matrix, Wash],
            LedStrip => &[LedBar, Matrix, Wash],
            Matrix => &[LedBar, LedStrip, Panel],
        }
    }
    /// A camera is a sensor, not an emitter: no word, no floats.
    /// The channels a type can be asked for when its profile does not say (a registered
    /// profile narrows this to its own).
    pub fn default_channels(self) -> &'static [Channel] {
        use Channel::*;
        match self {
            Kind::Moving => &[Level, Hue, Sat, Strobe, Pan, Tilt, Zoom, Focus, Iris, Frost],
            Kind::Spot | Kind::Backlight => &[Level, Hue, Sat, Strobe, Cct],
            Kind::Continuous => &[Level, Cct],
            _ => &[Level, Hue, Sat, Strobe],
        }
    }
}

/// The `Signal` fields a fixture float can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Level,
    Hue,
    Sat,
    Strobe,
    Pan,
    Tilt,
    Cct,
    Zoom,
    Focus,
    Iris,
    Frost,
}
impl Channel {
    pub const ALL: [Channel; 11] = [
        Channel::Level, Channel::Hue, Channel::Sat, Channel::Strobe, Channel::Pan, Channel::Tilt,
        Channel::Cct, Channel::Zoom, Channel::Focus, Channel::Iris, Channel::Frost,
    ];
    pub fn word(self) -> &'static str {
        match self {
            Channel::Level => "level",
            Channel::Hue => "hue",
            Channel::Sat => "sat",
            Channel::Strobe => "strobe",
            Channel::Pan => "pan",
            Channel::Tilt => "tilt",
            Channel::Cct => "cct",
            Channel::Zoom => "zoom",
            Channel::Focus => "focus",
            Channel::Iris => "iris",
            Channel::Frost => "frost",
        }
    }
    pub fn parse(s: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.word() == s)
    }
    /// Declared range of the float (the `sig_*` ranges); the AI writes inside it.
    pub fn range(self) -> [f32; 2] {
        match self {
            Channel::Pan => [-270.0, 270.0],
            Channel::Tilt => [-135.0, 135.0],
            Channel::Cct => [1800.0, 10000.0],
            _ => [0.0, 1.0],
        }
    }
    pub fn get(self, s: &Signal) -> f32 {
        match self {
            Channel::Level => s.level,
            Channel::Hue => s.hue,
            Channel::Sat => s.sat,
            Channel::Strobe => s.strobe,
            Channel::Pan => s.pan,
            Channel::Tilt => s.tilt,
            Channel::Cct => s.cct,
            Channel::Zoom => s.zoom,
            Channel::Focus => s.focus,
            Channel::Iris => s.iris,
            Channel::Frost => s.frost,
        }
    }
    pub fn set(self, s: &mut Signal, v: f32) {
        match self {
            Channel::Level => s.level = v,
            Channel::Hue => s.hue = v,
            Channel::Sat => s.sat = v,
            Channel::Strobe => s.strobe = v,
            Channel::Pan => s.pan = v,
            Channel::Tilt => s.tilt = v,
            Channel::Cct => s.cct = v,
            Channel::Zoom => s.zoom = v,
            Channel::Focus => s.focus = v,
            Channel::Iris => s.iris = v,
            Channel::Frost => s.frost = v,
        }
    }
    /// A dark, white, centred signal: what a fixture holds before anything is asked.
    pub fn rest() -> Signal {
        Signal {
            level: 0.0, hue: 0.0, sat: 0.0, strobe: 0.0, pan: 0.0, tilt: 0.0, a: 0.0, b: 0.0,
            cct: 5600.0, zoom: 0.5, focus: 0.5, iris: 1.0, frost: 0.0,
        }
    }
}

/// The four fixed variants of every cell. A venue deals the fixtures of one type at one
/// place into them in order along the place (left to right, back to front), so the AI
/// always has four voices to compose with: four colours, pan/tilt mirrored or in sync,
/// a wave that runs α → δ. A cell with fewer fixtures folds the missing ones back
/// (γ → α, δ → β on two; all on α on one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Variant {
    Alpha,
    Beta,
    Gamma,
    Delta,
}
impl Variant {
    pub const ALL: [Variant; 4] = [Variant::Alpha, Variant::Beta, Variant::Gamma, Variant::Delta];
    pub fn word(self) -> &'static str {
        match self {
            Variant::Alpha => "alpha",
            Variant::Beta => "beta",
            Variant::Gamma => "gamma",
            Variant::Delta => "delta",
        }
    }
    pub fn parse(s: &str) -> Option<Variant> {
        Variant::ALL.into_iter().find(|v| v.word() == s)
    }
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn nth(i: usize) -> Variant {
        Variant::ALL[i % 4]
    }
}

/// A place and, optionally, one variant of it: `l2.pro.x.beta`, or `l2.back.l`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Spot {
    pub point: Point,
    pub variant: Option<Variant>,
}
impl Spot {
    pub fn word(&self) -> String {
        match self.variant {
            Some(v) => format!("{}.{}", self.point.word(), v.word()),
            None => self.point.word(),
        }
    }
    pub fn parse(s: &str) -> Option<Spot> {
        if let Some(p) = Point::parse(s) {
            return Some(Spot { point: p, variant: None });
        }
        let (head, v) = s.rsplit_once('.')?;
        Some(Spot { point: Point::parse(head)?, variant: Some(Variant::parse(v)?) })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FloatName {
    pub point: Point,
    pub kind: Kind,
    /// One variant of the cell (`None`: the cell, all of its variants).
    #[serde(default)]
    pub variant: Option<Variant>,
    /// The segment of a pixel bar's cell (`None`: the whole bar).
    #[serde(default)]
    pub pixel: Option<u8>,
    pub channel: Channel,
}
impl fmt::Display for FloatName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.point.word(), self.kind.word())?;
        if let Some(v) = self.variant {
            write!(f, ".{}", v.word())?;
        }
        if let Some(p) = self.pixel {
            write!(f, ".p{p}")?;
        }
        write!(f, ".{}", self.channel.word())
    }
}
impl FloatName {
    pub fn new(point: Point, kind: Kind, channel: Channel) -> FloatName {
        FloatName { point, kind, variant: None, pixel: None, channel }
    }
    pub fn with_variant(self, v: Variant) -> FloatName {
        FloatName { variant: Some(v), ..self }
    }
    pub fn parse(s: &str) -> Option<FloatName> {
        let parts: Vec<&str> = s.split('.').collect();
        if !(5..=7).contains(&parts.len()) {
            return None;
        }
        let kind = Kind::parse(parts[3])?;
        let (mut variant, mut pixel) = (None, None);
        for t in &parts[4..parts.len() - 1] {
            if let Some(v) = Variant::parse(t) {
                if variant.is_some() || pixel.is_some() {
                    return None;
                }
                variant = Some(v);
            } else {
                let n: u8 = t.strip_prefix('p')?.parse().ok()?;
                if pixel.is_some() || !kind.pixelled() || (n as usize) >= crate::common::PIXEL_SEGMENTS {
                    return None;
                }
                pixel = Some(n);
            }
        }
        Some(FloatName {
            point: Point::parse(&parts[..3].join("."))?,
            kind,
            variant,
            pixel,
            channel: Channel::parse(parts[parts.len() - 1])?,
        })
    }
    /// The same float on the whole bar (no segment).
    pub fn cell(self) -> FloatName {
        FloatName { pixel: None, ..self }
    }
    pub fn spot(&self) -> Spot {
        Spot { point: self.point, variant: self.variant }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip() {
        let n = FloatName::parse("l2.back.l.led_bar.level").unwrap();
        assert_eq!(n.to_string(), "l2.back.l.led_bar.level");
        assert!(FloatName::parse("l2.back.l.wash").is_none());
        assert!(FloatName::parse("l1.mid.l.wash.level").is_none());
        let p = FloatName::parse("l3.house.c.led_bar.p3.hue").unwrap();
        assert_eq!(p.pixel, Some(3));
        assert_eq!(p.to_string(), "l3.house.c.led_bar.p3.hue");
        assert!(FloatName::parse("l2.back.l.wash.p3.hue").is_none(), "a wash has no pixels");
        assert!(FloatName::parse("l2.back.l.led_bar.p99.hue").is_none());
        let v = FloatName::parse("l2.pro.x.moving.beta.pan").unwrap();
        assert_eq!((v.variant, v.point.word()), (Some(Variant::Beta), "l2.pro.x".into()));
        let vp = FloatName::parse("l3.house.x.led_bar.gamma.p3.hue").unwrap();
        assert_eq!((vp.variant, vp.pixel), (Some(Variant::Gamma), Some(3)));
        assert_eq!(vp.to_string(), "l3.house.x.led_bar.gamma.p3.hue");
        assert!(FloatName::parse("l3.house.x.led_bar.p3.gamma.hue").is_none(), "variant before segment");
        assert_eq!(Spot::parse("l2.pro.x.beta").unwrap().word(), "l2.pro.x.beta");
        assert_eq!(Spot::parse("l2.back.l").unwrap().variant, None);
    }
}
