//! Float names: `<level>.<row>.<col>.<type>.<channel>`, e.g. `l2.back.l.wash.level`.
//! They sit beside the shader `sig_*` rows; the fixture AI emits only these.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FloatName {
    pub point: Point,
    pub kind: Kind,
    pub channel: Channel,
}
impl fmt::Display for FloatName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.point.word(), self.kind.word(), self.channel.word())
    }
}
impl FloatName {
    pub fn parse(s: &str) -> Option<FloatName> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 5 {
            return None;
        }
        Some(FloatName {
            point: Point::parse(&parts[..3].join("."))?,
            kind: Kind::parse(parts[3])?,
            channel: Channel::parse(parts[4])?,
        })
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
    }
}
