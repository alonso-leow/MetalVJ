//! Imported desk scenes (`.scex`, parsed to JSON by `scripts/scex_import.py`)
//! → the hitbox matrix floats `<point>.<type>.<channel>`.
//!
//! A scene names fixtures by desk id; `placements` puts each id on a hitbox
//! point and `kinds` gives its type. Fixtures that share a point and a type
//! are one row (they always receive the same output), so their values are
//! averaged into one float. Fixtures without a point or a type are dropped
//! (listed in [`Clip::dropped`]).
//!
//! Step `length` is milliseconds. A step whose channels all say `fade: false`
//! holds (snaps) until the next step; otherwise it fades into it.

use crate::names::{Channel, FloatName, Kind, Spot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Value {
    #[serde(default)]
    pub norm: f32,
    #[serde(default)]
    pub deg: Option<f32>,
    #[serde(default)]
    pub fade: bool,
    // Raw desk bytes, kept so an edited scene writes back what it read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dmx: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StepFixture {
    pub id: u64,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub channels: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Milliseconds.
    pub length: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub fixtures: Vec<StepFixture>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub name: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

impl Scene {
    pub fn length_ms(&self) -> u32 {
        self.steps.iter().map(|s| s.length.max(1)).sum()
    }
}

/// The desk's fixture type words → hitbox kinds. `other` has no kind.
pub fn kind_of_desk_type(t: &str) -> Option<Kind> {
    match t {
        "beam" | "moving" => Some(Kind::Moving),
        "par" | "spot" => Some(Kind::Spot),
        "wash" => Some(Kind::Wash),
        "bar" | "led_bar" => Some(Kind::LedBar),
        "backlight" => Some(Kind::Backlight),
        _ => Kind::parse(t),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub ms: u32,
    pub floats: Vec<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// `FloatName`s, sorted.
    pub columns: Vec<String>,
    pub keys: Vec<Key>,
    pub length_ms: u32,
    /// Desk ids with no point or kind.
    pub dropped: Vec<u64>,
}

fn rgb_to_hs(r: f32, g: f32, b: f32) -> (f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if max <= 0.0 || d <= 1e-6 {
        return (0.0, 0.0);
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, d / max)
}

/// One fixture's channel map → the hitbox channels it answers.
pub fn channels_of(ch: &BTreeMap<String, Value>) -> BTreeMap<Channel, f32> {
    let mut out = BTreeMap::new();
    let n = |k: &str| ch.get(k).map(|v| v.norm.clamp(0.0, 1.0));
    if let Some(v) = n("level").or_else(|| n("dimmer")) {
        out.insert(Channel::Level, v);
    }
    for (k, c) in [("pan", Channel::Pan), ("tilt", Channel::Tilt)] {
        if let Some(v) = ch.get(k) {
            let [lo, hi] = c.range();
            out.insert(c, v.deg.unwrap_or(lo + v.norm * (hi - lo)));
        }
    }
    // Desk strobe: fully open (≈1) or closed (≈0) means no strobe.
    if let Some(v) = n("strobe").or_else(|| n("shutter")) {
        out.insert(Channel::Strobe, if (0.04..0.96).contains(&v) { v } else { 0.0 });
    }
    let (r, g, b) = (n("colour:red"), n("colour:green"), n("colour:blue"));
    if r.is_some() || g.is_some() || b.is_some() {
        let (mut r, mut g, b) = (r.unwrap_or(0.0), g.unwrap_or(0.0), b.unwrap_or(0.0));
        if let Some(a) = n("colour:amber") {
            r = (r + a).min(1.0);
            g = (g + a * 0.5).min(1.0);
        }
        let (h, mut s) = rgb_to_hs(r, g, b);
        if let Some(w) = n("colour:white") {
            s *= 1.0 - w;
        }
        out.insert(Channel::Hue, h);
        out.insert(Channel::Sat, s);
    }
    for (k, c) in [("zoom", Channel::Zoom), ("focus", Channel::Focus), ("iris", Channel::Iris), ("FROST", Channel::Frost), ("frost", Channel::Frost)] {
        if let Some(v) = n(k) {
            out.insert(c, v);
        }
    }
    out
}

/// The scene as matrix keyframes. A held step adds a second key 1 ms before
/// the next step so the row snaps. Each desk fixture writes to its spot: a place and
/// its variant keeps its own voice; a place alone writes the whole cell, and fixtures
/// sharing one spot average.
pub fn to_clip(scene: &Scene, placements: &HashMap<u64, Spot>, kinds: &HashMap<u64, Kind>) -> Clip {
    let mut dropped = BTreeSet::new();
    // Per step: float → (sum, n).
    let mut steps: Vec<(BTreeMap<FloatName, (f32, u32)>, bool)> = Vec::new();
    let mut names = BTreeSet::new();
    for step in &scene.steps {
        let mut acc: BTreeMap<FloatName, (f32, u32)> = BTreeMap::new();
        let mut any_fade = false;
        for f in &step.fixtures {
            let (Some(spot), Some(kind)) = (placements.get(&f.id), kinds.get(&f.id)) else {
                dropped.insert(f.id);
                continue;
            };
            any_fade |= f.channels.values().any(|v| v.fade);
            for (channel, v) in channels_of(&f.channels) {
                let name = FloatName { variant: spot.variant, ..FloatName::new(spot.point, *kind, channel) };
                names.insert(name);
                let e = acc.entry(name).or_insert((0.0, 0));
                e.0 += v;
                e.1 += 1;
            }
        }
        steps.push((acc, !any_fade && !step.fixtures.is_empty()));
    }
    let names: Vec<FloatName> = names.into_iter().collect();
    let mut keys = Vec::new();
    let mut last: Vec<f32> = names.iter().map(|n| rest_of(n.channel)).collect();
    let mut t = 0u32;
    for ((acc, hold), step) in steps.iter().zip(&scene.steps) {
        let row: Vec<f32> = names
            .iter()
            .zip(&last)
            .map(|(n, prev)| acc.get(n).map_or(*prev, |(s, c)| s / *c as f32))
            .collect();
        keys.push(Key { ms: t, floats: row.clone() });
        let len = step.length.max(1);
        if *hold && len > 1 {
            keys.push(Key { ms: t + len - 1, floats: row.clone() });
        }
        last = row;
        t += len;
    }
    if let Some(k) = keys.last().cloned() {
        if k.ms < t {
            keys.push(Key { ms: t, floats: k.floats });
        }
    }
    Clip { columns: names.iter().map(|n| n.to_string()).collect(), keys, length_ms: t, dropped: dropped.into_iter().collect() }
}

fn rest_of(c: Channel) -> f32 {
    c.get(&Channel::rest())
}

/// Tags a scene for the gallery's sections: what it mostly does.
pub fn tags(scene: &Scene) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut moves = 0;
    let mut strobes = 0;
    let mut colours = BTreeSet::new();
    let mut prev: HashMap<u64, (f32, f32)> = HashMap::new();
    for s in &scene.steps {
        for f in &s.fixtures {
            let ch = channels_of(&f.channels);
            if ch.get(&Channel::Strobe).copied().unwrap_or(0.0) > 0.0 {
                strobes += 1;
            }
            if let Some(h) = ch.get(&Channel::Hue) {
                if ch.get(&Channel::Sat).copied().unwrap_or(0.0) > 0.2 {
                    colours.insert((h * 12.0) as i32);
                }
            }
            if let (Some(p), Some(t)) = (ch.get(&Channel::Pan), ch.get(&Channel::Tilt)) {
                if let Some((pp, pt)) = prev.insert(f.id, (*p, *t)) {
                    if (pp - p).abs() + (pt - t).abs() > 10.0 {
                        moves += 1;
                    }
                }
            }
        }
    }
    if scene.steps.len() <= 1 {
        out.push("static");
    }
    if moves > 0 {
        out.push("movement");
    }
    if strobes > 0 {
        out.push("strobe");
    }
    if colours.len() > 1 {
        out.push("colour");
    }
    if out.is_empty() {
        out.push("other");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(norm: f32, fade: bool) -> Value {
        Value { norm, fade, ..Value::default() }
    }

    fn scene() -> Scene {
        let fx = |id: u64, level: f32, fade: bool| StepFixture {
            id,
            model: "x".into(),
            channels: [("level".to_string(), v(level, fade))].into_iter().collect(),
        };
        Scene {
            name: "t".into(),
            steps: vec![
                Step { length: 500, name: None, fixtures: vec![fx(1, 1.0, false), fx(2, 0.0, false), fx(9, 1.0, false)] },
                Step { length: 250, name: None, fixtures: vec![fx(1, 0.5, true), fx(2, 0.5, true)] },
            ],
            ..Scene::default()
        }
    }

    fn maps() -> (HashMap<u64, Spot>, HashMap<u64, Kind>) {
        let p = Spot::parse("l2.back.l").unwrap();
        ([(1, p), (2, p)].into_iter().collect(), [(1, Kind::Wash), (2, Kind::Wash)].into_iter().collect())
    }

    #[test]
    fn same_point_and_type_is_one_averaged_row_and_unplaced_drop() {
        let (p, k) = maps();
        let c = to_clip(&scene(), &p, &k);
        assert_eq!(c.columns, vec!["l2.back.l.wash.level"]);
        assert_eq!(c.dropped, vec![9]);
        assert_eq!(c.length_ms, 750);
        // Held first step: keys at 0 and 499, then the faded step at 500, end at 750.
        let ms: Vec<u32> = c.keys.iter().map(|k| k.ms).collect();
        assert_eq!(ms, vec![0, 499, 500, 750]);
        assert!((c.keys[0].floats[0] - 0.5).abs() < 1e-6);
        assert!((c.keys[2].floats[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_variant_keeps_its_own_voice() {
        let (mut p, k) = maps();
        p.insert(1, Spot::parse("l2.pro.x.alpha").unwrap());
        p.insert(2, Spot::parse("l2.pro.x.beta").unwrap());
        let c = to_clip(&scene(), &p, &k);
        assert_eq!(c.columns, vec!["l2.pro.x.wash.alpha.level", "l2.pro.x.wash.beta.level"]);
        assert_eq!((c.keys[0].floats[0], c.keys[0].floats[1]), (1.0, 0.0));
    }

    #[test]
    fn rgb_becomes_hue_and_sat() {
        let ch: BTreeMap<String, Value> =
            [("colour:red", 0.0), ("colour:green", 0.0), ("colour:blue", 1.0)].iter().map(|(k, n)| (k.to_string(), v(*n, false))).collect();
        let out = channels_of(&ch);
        assert!((out[&Channel::Hue] - 2.0 / 3.0).abs() < 1e-4);
        assert!((out[&Channel::Sat] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn open_shutter_is_not_a_strobe() {
        let ch: BTreeMap<String, Value> = [("strobe".to_string(), v(1.0, false))].into_iter().collect();
        assert_eq!(channels_of(&ch)[&Channel::Strobe], 0.0);
    }

    #[test]
    fn unknown_fields_round_trip() {
        let s: Scene = serde_json::from_str(r#"{"name":"a","format":"scex","steps":[]}"#).unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap()["format"], "scex");
    }
}
