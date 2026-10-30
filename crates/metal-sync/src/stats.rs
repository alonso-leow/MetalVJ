//! The peer monitor's data: what one machine reports about itself every
//! [`TICK`], and how a receiver judges how fresh a peer's report is.
//!
//! **Unknown is not zero.** Every reading is an `Option`: a value this machine
//! cannot read (no `/proc`, no thermal zone, no paint backend yet) is `None`,
//! which serialises as `null` and shows as unknown. A missing number never
//! becomes `0`.
//!
//! **Stale, never blank.** A report older than [`STALE_AFTER`] is still shown,
//! marked stale, with its last values. Time is passed in as an [`Instant`], so
//! the 20 s rule is tested with an injected clock.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How often a machine sends its own stats to its peers.
pub const TICK: Duration = Duration::from_secs(5);

/// A peer's report older than this is shown as stale.
pub const STALE_AFTER: Duration = Duration::from_secs(20);

/// One machine's self-report. Every reading may be unknown.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PeerStats {
    pub cpu_pct: Option<f32>,
    pub gpu_pct: Option<f32>,
    pub temp_c: Option<f32>,
    pub uptime_s: Option<u64>,
    pub paint_backend: Option<String>,
    pub fps_paint: Option<f32>,
}

impl PeerStats {
    /// The wire form: one JSON object.
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// The inverse of [`PeerStats::encode`]; `None` for anything that is not a
    /// stats object.
    pub fn decode(bytes: &[u8]) -> Option<PeerStats> {
        serde_json::from_slice(bytes).ok()
    }
}

/// How fresh a peer's last report is at a given moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Heard from within [`STALE_AFTER`].
    Fresh,
    /// Heard from, but longer ago than [`STALE_AFTER`].
    Stale,
    /// Never heard from.
    Unknown,
}

impl Freshness {
    pub fn word(self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::Stale => "stale",
            Freshness::Unknown => "unknown",
        }
    }
}

/// The last report heard from each peer, and when.
#[derive(Debug, Default)]
pub struct StatsBook {
    seen: BTreeMap<String, (PeerStats, Instant)>,
}

impl StatsBook {
    /// Keeps `stats` as `name`'s latest report, heard at `now`.
    pub fn record(&mut self, name: &str, stats: PeerStats, now: Instant) {
        self.seen.insert(name.to_string(), (stats, now));
    }

    /// Whether `name`'s last report is fresh, stale or absent at `now`.
    pub fn freshness(&self, name: &str, now: Instant) -> Freshness {
        match self.seen.get(name) {
            None => Freshness::Unknown,
            Some((_, heard)) if now.saturating_duration_since(*heard) < STALE_AFTER => Freshness::Fresh,
            Some(_) => Freshness::Stale,
        }
    }

    /// Every peer heard from, as JSON: `{name, status, age_s, ...stats}`. A
    /// stale peer keeps its last values; an unreadable value stays `null`.
    pub fn view(&self, now: Instant) -> Vec<serde_json::Value> {
        self.seen
            .iter()
            .map(|(name, (stats, heard))| {
                let age = now.saturating_duration_since(*heard).as_secs_f32();
                let mut v = serde_json::to_value(stats).unwrap_or_default();
                v["name"] = serde_json::json!(name);
                v["status"] = serde_json::json!(self.freshness(name, now).word());
                v["age_s"] = serde_json::json!(age);
                v
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// readings this machine makes about itself (Linux; `None` elsewhere)
// ---------------------------------------------------------------------------

/// The `(idle, total)` jiffies of the aggregate `cpu` line of `/proc/stat`.
/// Idle counts `idle` and `iowait`.
pub fn cpu_totals(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line.split_whitespace().skip(1).map(|f| f.parse().ok()).collect::<Option<_>>()?;
    if fields.len() < 4 {
        return None;
    }
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some((idle, fields.iter().sum()))
}

/// CPU busy percentage between two `/proc/stat` readings. The first reading
/// only primes the sampler and gives `None`.
#[derive(Debug, Default)]
pub struct CpuSampler {
    prev: Option<(u64, u64)>,
}

impl CpuSampler {
    pub fn sample(&mut self, stat: &str) -> Option<f32> {
        let now = cpu_totals(stat)?;
        let prev = self.prev.replace(now)?;
        let total = now.1.checked_sub(prev.1)?;
        let idle = now.0.checked_sub(prev.0)?;
        if total == 0 {
            return None;
        }
        Some((1.0 - idle as f32 / total as f32) * 100.0)
    }
}

/// Whole seconds from the first field of `/proc/uptime`.
pub fn uptime_from(proc_uptime: &str) -> Option<u64> {
    let secs: f64 = proc_uptime.split_whitespace().next()?.parse().ok()?;
    Some(secs as u64)
}

/// A thermal zone's reading: the kernel writes millidegrees Celsius.
pub fn temp_from(millidegrees: &str) -> Option<f32> {
    let m: i64 = millidegrees.trim().parse().ok()?;
    Some(m as f32 / 1000.0)
}

/// This machine's own stats, read now. `sampler` keeps the CPU reading between
/// ticks. Missing readings are `None`; nothing here is ever guessed.
pub fn read_self(sampler: &mut CpuSampler) -> PeerStats {
    #[cfg(target_os = "linux")]
    {
        PeerStats {
            cpu_pct: std::fs::read_to_string("/proc/stat").ok().and_then(|s| sampler.sample(&s)),
            gpu_pct: None,
            temp_c: read_temp(),
            uptime_s: std::fs::read_to_string("/proc/uptime").ok().and_then(|s| uptime_from(&s)),
            paint_backend: None,
            fps_paint: None,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = sampler;
        PeerStats::default()
    }
}

/// The first readable `/sys/class/thermal/thermal_zone*/temp`, in Celsius.
#[cfg(target_os = "linux")]
fn read_temp() -> Option<f32> {
    let dir = std::fs::read_dir("/sys/class/thermal").ok()?;
    let mut zones: Vec<_> = dir
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("thermal_zone"))
        .map(|e| e.path().join("temp"))
        .collect();
    zones.sort();
    zones.iter().find_map(|p| std::fs::read_to_string(p).ok().and_then(|s| temp_from(&s)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> PeerStats {
        PeerStats {
            cpu_pct: Some(12.5),
            gpu_pct: Some(40.0),
            temp_c: Some(51.25),
            uptime_s: Some(3600),
            paint_backend: Some("vulkan".into()),
            fps_paint: Some(59.9),
        }
    }

    #[test]
    fn stats_round_trip_through_their_wire_form() {
        let s = full();
        assert_eq!(PeerStats::decode(&s.encode()), Some(s));
        let partial = PeerStats { cpu_pct: Some(3.0), ..PeerStats::default() };
        assert_eq!(PeerStats::decode(&partial.encode()), Some(partial));
        assert_eq!(PeerStats::decode(b"not json"), None);
    }

    #[test]
    fn a_report_is_fresh_until_twenty_seconds_then_stale_with_its_values() {
        let t0 = Instant::now();
        let mut book = StatsBook::default();
        book.record("pi", full(), t0);
        assert_eq!(book.freshness("pi", t0 + Duration::from_secs(19)), Freshness::Fresh);
        assert_eq!(book.freshness("pi", t0 + STALE_AFTER), Freshness::Stale);
        let view = book.view(t0 + Duration::from_secs(25));
        assert_eq!(view[0]["status"], "stale");
        assert_eq!(view[0]["cpu_pct"], serde_json::json!(12.5), "stale keeps its last value");
        assert_eq!(book.freshness("nobody", t0), Freshness::Unknown);
    }

    #[test]
    fn a_missing_reading_is_null_not_zero() {
        let t0 = Instant::now();
        let mut book = StatsBook::default();
        book.record("pi", PeerStats { cpu_pct: Some(0.0), ..PeerStats::default() }, t0);
        let view = book.view(t0);
        assert!(view[0]["gpu_pct"].is_null(), "unknown GPU is null");
        assert!(view[0]["temp_c"].is_null());
        assert_eq!(view[0]["cpu_pct"], serde_json::json!(0.0), "a measured zero stays zero");
    }

    #[test]
    fn cpu_is_the_busy_share_between_two_readings() {
        let a = "cpu  100 0 100 800 100 0 0 0\n";
        let b = "cpu  150 0 150 850 100 0 0 0\n";
        assert_eq!(cpu_totals(a), Some((900, 1100)));
        let mut sampler = CpuSampler::default();
        assert_eq!(sampler.sample(a), None, "the first reading only primes");
        // 150 jiffies passed, 50 of them idle: two thirds busy.
        let busy = sampler.sample(b).unwrap();
        assert!((busy - 200.0 / 3.0).abs() < 0.01, "{busy}");
    }

    #[test]
    fn uptime_and_temperature_parse_their_kernel_formats() {
        assert_eq!(uptime_from("3601.27 12000.1\n"), Some(3601));
        assert_eq!(uptime_from(""), None);
        assert_eq!(temp_from("51250\n"), Some(51.25));
        assert_eq!(temp_from("hot"), None);
    }
}
