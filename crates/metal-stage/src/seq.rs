//! Sequence state: sparse keys over the stage floats, sampled by tick, and exploded into
//! the per-fixture lanes the sequence editor's fixture pages hold.

use crate::names::{Channel, FloatName};
use crate::patch::Matrix;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One key: a tick and the floats it sets (`None` = this key says nothing about it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub tick: u32,
    pub values: Vec<Option<f32>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageSeq {
    pub rate: f32,
    /// Float names, the matrix's order.
    pub columns: Vec<String>,
    pub keys: Vec<Key>,
}

/// `(tick, value)` keyframes of one fixture channel.
pub type Lane = Vec<(u32, f32)>;

impl StageSeq {
    pub fn new(rate: f32, matrix: &Matrix) -> StageSeq {
        StageSeq { rate, columns: matrix.names(), keys: vec![] }
    }

    /// Set one float at a tick (inserts the key in order; a repeat overwrites).
    pub fn set(&mut self, tick: u32, name: &str, value: f32) -> Result<(), String> {
        let c = self.columns.iter().position(|c| c == name).ok_or_else(|| format!("no stage float `{name}` in this patch"))?;
        let n = self.columns.len();
        let at = match self.keys.binary_search_by_key(&tick, |k| k.tick) {
            Ok(i) => i,
            Err(i) => {
                self.keys.insert(i, Key { tick, values: vec![None; n] });
                i
            }
        };
        self.keys[at].values[c] = Some(value);
        Ok(())
    }

    /// The value of every float at `tick`: linear between its own keys, held outside
    /// them, the channel's rest value where it has no key at all.
    pub fn sample(&self, tick: f32) -> Vec<f32> {
        (0..self.columns.len()).map(|c| self.column_at(c, tick)).collect()
    }

    fn column_at(&self, c: usize, tick: f32) -> f32 {
        let mut prev: Option<(f32, f32)> = None;
        for k in &self.keys {
            let Some(v) = k.values[c] else { continue };
            let t = k.tick as f32;
            if t >= tick {
                return match prev {
                    Some((pt, pv)) if t > pt => pv + (v - pv) * (tick - pt) / (t - pt),
                    _ => v,
                };
            }
            prev = Some((t, v));
        }
        match prev {
            Some((_, v)) => v,
            None => FloatName::parse(&self.columns[c]).map_or(0.0, |f| rest_value(f.channel)),
        }
    }

    /// Per fixture, per channel keyframes — only the ticks a key actually set.
    pub fn lanes(&self, matrix: &Matrix) -> BTreeMap<String, BTreeMap<Channel, Lane>> {
        let mut out: BTreeMap<String, BTreeMap<Channel, Lane>> = BTreeMap::new();
        for (c, drive) in matrix.drives.iter().enumerate() {
            let Some(col) = matrix.columns.get(c) else { continue };
            for k in &self.keys {
                let Some(v) = k.values.get(c).copied().flatten() else { continue };
                for (id, _) in drive {
                    out.entry(id.clone()).or_default().entry(col.channel).or_default().push((k.tick, v));
                }
            }
        }
        out
    }
}

fn rest_value(c: Channel) -> f32 {
    let s = Channel::rest();
    c.get(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::tests::demo;
    #[test]
    fn sparse_keys_interpolate() {
        let m = demo().matrix();
        let name = m.columns.iter().find(|c| c.channel == Channel::Level).unwrap().to_string();
        let mut s = StageSeq::new(32.0, &m);
        s.set(0, &name, 0.0).unwrap();
        s.set(32, &name, 1.0).unwrap();
        let i = s.columns.iter().position(|c| *c == name).unwrap();
        assert!((s.sample(16.0)[i] - 0.5).abs() < 1e-6);
        assert_eq!(s.sample(99.0)[i], 1.0);
        assert!(s.set(1, "l9.x", 0.0).is_err());
        let lanes = s.lanes(&m);
        assert!(lanes.values().any(|l| l[&Channel::Level] == vec![(0, 0.0), (32, 1.0)]));
    }
}
