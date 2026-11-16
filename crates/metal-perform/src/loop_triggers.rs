//! Looper trigger counts per output device and source (taste/DMX lote, Phase 2).
//!
//! Every time the [`crate::looper::Looper`] engages a loop (a manual lock or a
//! detector lock) while a source is on air on an output device, that device's
//! count for that source goes up by one. The counts are the operator's own
//! history of what the loop was played over, so they are persisted: one small
//! JSON under the data root, `loop_triggers.json`.
//!
//! The same store holds two operator settings, because they are read with the
//! counts and belong in the same file:
//! - `sort`: per device, whether its sources are listed by trigger count
//!   (highest first) instead of by name;
//! - `factor` (0..1): how much the trigger counts weigh on the router's
//!   automatic source selection (see [`crate::router::Router`]).
//!
//! Tests never touch the real `data/`: the caller passes the path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file name under the data root.
pub const FILE: &str = "loop_triggers.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Store {
    /// device id → source id → trigger count.
    #[serde(default)]
    pub counts: BTreeMap<String, BTreeMap<String, u64>>,
    /// device id → list its sources by trigger count.
    #[serde(default)]
    pub sort: BTreeMap<String, bool>,
    /// How much trigger counts weigh on automatic source selection, 0..1.
    #[serde(default)]
    pub factor: f32,
}

impl Store {
    /// The file under `data_root`.
    pub fn path(data_root: &Path) -> PathBuf {
        data_root.join(FILE)
    }

    /// Read the store. A missing or unreadable file is an empty store: the
    /// counts are a history, not a reason to refuse to start.
    pub fn load(path: &Path) -> Store {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("loop triggers: {} unreadable, starting empty: {e}", path.display());
                Store::default()
            }),
            Err(_) => Store::default(),
        }
    }

    /// Write the store, through a temp file so a crash never leaves half a JSON.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Add `n` triggers of `source` on `device`.
    pub fn record(&mut self, device: &str, source: &str, n: u64) {
        if n == 0 {
            return;
        }
        *self
            .counts
            .entry(device.to_string())
            .or_default()
            .entry(source.to_string())
            .or_insert(0) += n;
    }

    /// The count of `source` on `device`, 0 when none.
    pub fn count(&self, device: &str, source: &str) -> u64 {
        self.counts
            .get(device)
            .and_then(|m| m.get(source))
            .copied()
            .unwrap_or(0)
    }

    /// Whether `device` lists its sources by trigger count.
    pub fn sorted(&self, device: &str) -> bool {
        self.sort.get(device).copied().unwrap_or(false)
    }

    /// The sources of `device` in the order it wants them: by count, highest
    /// first, ties by name, when sorted; by name otherwise.
    pub fn listing(&self, device: &str) -> Vec<(String, u64)> {
        let mut rows: Vec<(String, u64)> = self
            .counts
            .get(device)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default();
        if self.sorted(device) {
            rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        }
        rows
    }

    /// Trigger totals per source, summed over every device.
    pub fn totals(&self) -> BTreeMap<String, u64> {
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        for sources in self.counts.values() {
            for (source, n) in sources {
                *out.entry(source.clone()).or_insert(0) += n;
            }
        }
        out
    }

    /// Set the factor, clamped to 0..1. NaN is 0.
    pub fn set_factor(&mut self, factor: f32) {
        self.factor = if factor.is_nan() { 0.0 } else { factor.clamp(0.0, 1.0) };
    }

    /// The per-source selection weight, in the order of `ids`: each source's
    /// total over the largest total, 0..1. All zero when nothing has fired.
    pub fn weights(&self, ids: &[String]) -> Vec<f32> {
        let totals = self.totals();
        let max = totals.values().copied().max().unwrap_or(0);
        ids.iter()
            .map(|id| {
                if max == 0 {
                    0.0
                } else {
                    totals.get(id).copied().unwrap_or(0) as f32 / max as f32
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory per test, removed on drop. Tests never use `data/`.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!(
                "fw-loop-triggers-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn counts_are_per_device_and_per_source() {
        let mut s = Store::default();
        s.record("wall", "spiral", 2);
        s.record("wall", "spiral", 1);
        s.record("floor", "spiral", 5);
        assert_eq!(s.count("wall", "spiral"), 3);
        assert_eq!(s.count("floor", "spiral"), 5);
        assert_eq!(s.count("wall", "bars"), 0);
        assert_eq!(s.totals()["spiral"], 8);
    }

    #[test]
    fn sorted_listing_is_highest_first_and_unsorted_is_by_name() {
        let mut s = Store::default();
        s.record("wall", "bars", 1);
        s.record("wall", "spiral", 4);
        s.record("wall", "dots", 4);
        assert_eq!(
            s.listing("wall"),
            vec![("bars".into(), 1), ("dots".into(), 4), ("spiral".into(), 4)],
            "unsorted is by name"
        );
        s.sort.insert("wall".into(), true);
        assert_eq!(
            s.listing("wall"),
            vec![("dots".into(), 4), ("spiral".into(), 4), ("bars".into(), 1)],
            "sorted is by count, ties by name"
        );
    }

    #[test]
    fn weights_are_relative_to_the_largest_total() {
        let mut s = Store::default();
        s.record("wall", "a", 2);
        s.record("floor", "b", 8);
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(s.weights(&ids), vec![0.25, 1.0, 0.0]);
        assert_eq!(Store::default().weights(&ids), vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn factor_is_clamped_to_the_unit_interval() {
        let mut s = Store::default();
        s.set_factor(3.0);
        assert_eq!(s.factor, 1.0);
        s.set_factor(-1.0);
        assert_eq!(s.factor, 0.0);
        s.set_factor(f32::NAN);
        assert_eq!(s.factor, 0.0);
    }

    #[test]
    fn the_store_round_trips_through_its_json_file() {
        let dir = Scratch::new("roundtrip");
        let path = Store::path(&dir.0);
        let mut s = Store::default();
        s.record("wall", "spiral", 7);
        s.sort.insert("wall".into(), true);
        s.set_factor(0.5);
        s.save(&path).expect("save");
        assert_eq!(Store::load(&path), s);
    }

    #[test]
    fn a_missing_or_broken_file_loads_as_empty() {
        let dir = Scratch::new("broken");
        let path = Store::path(&dir.0);
        assert_eq!(Store::load(&path), Store::default());
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Store::load(&path), Store::default());
    }
}
