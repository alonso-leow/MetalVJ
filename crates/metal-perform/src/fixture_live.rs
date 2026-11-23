//! The live fixture store: what each venue fixture was last driven with.
//!
//! The engine records the [`Signal`] a venue fixture's head got this frame
//! ([`record`]); `GET /venue/state` reads it back ([`snapshot`]) and turns it
//! into a commanded [`Pose`] ([`pose_of`]), which a [`Tracker`] slews the
//! actual head toward. A signal older than [`FRESH_SECS`] counts as no signal:
//! the fixture falls back to its authored pose.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use metal_schema::Signal;
use serde::Serialize;
use web_time::Instant;

use crate::fixture::{self, Selector};
use crate::venue::{Fixture as VenueFixture, FixtureState, Pose, Venue};

/// A signal older than this many seconds is no longer live.
pub const FRESH_SECS: f32 = 1.0;

struct Entry {
    signal: Signal,
    at: Instant,
}

static LIVE: OnceLock<Mutex<BTreeMap<String, Entry>>> = OnceLock::new();

fn store() -> std::sync::MutexGuard<'static, BTreeMap<String, Entry>> {
    LIVE.get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Remember the signal a venue fixture was driven with, stamped now. The
/// key is allocated only the first time a fixture is seen.
pub fn record(fixture_id: &str, signal: &Signal) {
    let now = Instant::now();
    let mut live = store();
    match live.get_mut(fixture_id) {
        Some(e) => {
            e.signal = *signal;
            e.at = now;
        }
        None => {
            live.insert(fixture_id.to_string(), Entry { signal: *signal, at: now });
        }
    }
}

/// Every recorded fixture with its signal and its age in seconds.
pub fn snapshot() -> BTreeMap<String, (Signal, f32)> {
    snapshot_at(Instant::now())
}

fn snapshot_at(now: Instant) -> BTreeMap<String, (Signal, f32)> {
    store()
        .iter()
        .map(|(id, e)| (id.clone(), (e.signal, now.saturating_duration_since(e.at).as_secs_f32())))
        .collect()
}

/// The range's midpoint.
fn mid(r: [f32; 2]) -> f32 {
    (r[0] + r[1]) * 0.5
}

/// Map a `0..1` signal value onto a travel range.
fn lerp(r: [f32; 2], t: f32) -> f32 {
    r[0] + (r[1] - r[0]) * t
}

/// The pose a fixture is commanded to by one signal. `profile` supplies the
/// physical travel: with it, signal 0.5 is the authored aim, 0 and 1 are the
/// range ends, and zoom follows the profile's zoom channel (else the authored
/// beam). Without a profile, pan and tilt stay at the authored aim.
pub fn pose_of(f: &VenueFixture, profile: Option<&fixture::Fixture>, s: &Signal) -> Pose {
    let mut pose = FixtureState::new(f).commanded;
    pose.dimmer = s.level;
    pose.rgb = s.rgb();
    let Some(p) = profile else { return pose };
    let l = &p.limits;
    pose.pan = f.yaw + lerp(l.pan, s.pan) - mid(l.pan);
    pose.tilt = f.pitch + lerp(l.tilt, s.tilt) - mid(l.tilt);
    if p.selectors().contains(&Selector::Zoom) {
        pose.zoom = lerp(l.zoom, s.zoom);
    }
    pose
}

/// One fixture as `GET /venue/state` reports it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveState {
    pub fixture: String,
    pub commanded: Pose,
    pub actual: Pose,
    pub strobe: f32,
    pub live: bool,
}

/// Per-venue slew state, kept between polls so `actual` moves toward
/// `commanded` over time.
#[derive(Debug, Default)]
pub struct Tracker {
    states: BTreeMap<String, FixtureState>,
    last: Option<Instant>,
}

impl Tracker {
    /// Advance every fixture of `venue` to now and report it.
    pub fn update(&mut self, venue: &Venue, root: &Path) -> Vec<LiveState> {
        self.update_at(venue, root, Instant::now())
    }

    /// [`Tracker::update`] at an explicit time (tests drive it with a fake
    /// clock). The step is the time since the last call, clamped to 0..0.5 s.
    pub fn update_at(&mut self, venue: &Venue, root: &Path, now: Instant) -> Vec<LiveState> {
        let dt = self
            .last
            .map(|t| now.saturating_duration_since(t).as_secs_f32())
            .unwrap_or(0.0)
            .clamp(0.0, 0.5);
        self.last = Some(now);
        let snap = snapshot_at(now);

        let mut out = Vec::with_capacity(venue.fixtures.len());
        let mut kept = BTreeMap::new();
        for f in &venue.fixtures {
            let profile = f.profile.as_deref().and_then(|m| fixture::get(root, m));
            let live = snap.get(&f.id).filter(|(_, age)| *age < FRESH_SECS);
            let (commanded, strobe, is_live) = match live {
                Some((s, _)) => (pose_of(f, profile.as_ref(), s), s.strobe, true),
                None => (FixtureState::new(f).commanded, 0.0, false),
            };
            let mut state = self.states.remove(&f.id).unwrap_or_else(|| FixtureState {
                fixture: f.id.clone(),
                commanded,
                actual: commanded,
            });
            state.commanded = commanded;
            let limits = profile.map(|p| p.limits).unwrap_or_default();
            state.step(&limits, dt);
            out.push(LiveState {
                fixture: f.id.clone(),
                commanded,
                actual: state.actual,
                strobe,
                live: is_live,
            });
            kept.insert(f.id.clone(), state);
        }
        self.states = kept;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Channel, WireKind};
    use crate::venue::FixtureKind;
    use std::time::Duration;

    fn fixture(id: &str) -> VenueFixture {
        let mut f = VenueFixture::new(id.into(), id.into(), FixtureKind::Wash);
        f.yaw = 30.0;
        f.pitch = -10.0;
        f.beam = 25.0;
        f
    }

    fn profile(zoom: bool) -> fixture::Fixture {
        let mut limits = fixture::Limits::default();
        limits.pan = [-270.0, 270.0];
        limits.tilt = [-135.0, 135.0];
        limits.zoom = [10.0, 40.0];
        let channels = if zoom {
            vec![Channel { signal: Selector::Zoom, coarse: Some(1), fine: None, range: None, physical: None }]
        } else {
            Vec::new()
        };
        fixture::Fixture {
            model: "test".into(),
            label: "test".into(),
            wire: WireKind::Dmx,
            bluetooth: None,
            universe: 1,
            address: 1,
            channels,
            limits,
        }
    }

    fn sig(pan: f32, tilt: f32, zoom: f32) -> Signal {
        Signal {
            level: 0.8,
            hue: 0.0,
            sat: 0.0,
            strobe: 0.0,
            pan,
            tilt,
            a: 0.0,
            b: 0.0,
            cct: 0.0,
            zoom,
            focus: 0.0,
            iris: 0.0,
            frost: 0.0,
        }
    }

    fn venue_with(fixtures: Vec<VenueFixture>) -> Venue {
        let mut v = Venue::parse("[venue]\nid = \"live-test\"\nname = \"live test\"\nsetting = \"indoor\"\n").expect("minimal venue");
        v.fixtures = fixtures;
        v
    }

    #[test]
    fn pose_of_maps_signal_onto_travel() {
        let f = fixture("pose-a");
        let p = profile(true);
        let mid = pose_of(&f, Some(&p), &sig(0.5, 0.5, 0.5));
        assert!((mid.pan - 30.0).abs() < 1e-4, "0.5 is the authored yaw");
        assert!((mid.tilt + 10.0).abs() < 1e-4, "0.5 is the authored pitch");
        let lo = pose_of(&f, Some(&p), &sig(0.0, 0.0, 0.0));
        assert!((lo.pan - (30.0 - 270.0)).abs() < 1e-3);
        assert!((lo.tilt - (-10.0 - 135.0)).abs() < 1e-3);
        assert!((lo.zoom - 10.0).abs() < 1e-4, "zoom 0 is the range start");
        let hi = pose_of(&f, Some(&p), &sig(1.0, 1.0, 1.0));
        assert!((hi.pan - (30.0 + 270.0)).abs() < 1e-3);
        assert!((hi.tilt - (-10.0 + 135.0)).abs() < 1e-3);
        assert!((hi.zoom - 40.0).abs() < 1e-4, "zoom 1 is the range end");
        assert_eq!(hi.dimmer, 0.8);
    }

    #[test]
    fn pose_of_without_limits_keeps_authored_aim() {
        let f = fixture("pose-b");
        let p = pose_of(&f, None, &sig(0.0, 1.0, 1.0));
        assert_eq!(p.pan, 30.0);
        assert_eq!(p.tilt, -10.0);
        assert_eq!(p.zoom, 25.0, "no profile: authored beam");
    }

    #[test]
    fn pose_of_keeps_beam_without_zoom_channel() {
        let f = fixture("pose-c");
        let p = pose_of(&f, Some(&profile(false)), &sig(0.5, 0.5, 1.0));
        assert_eq!(p.zoom, 25.0);
    }

    #[test]
    fn tracker_slews_actual_toward_commanded() {
        let root = std::env::temp_dir().join("metal-live-no-profiles");
        let mut f = fixture("trk-slew-1");
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        // Nothing recorded: authored pose, actual equal to it.
        let first = tr.update_at(&venue_with(vec![f.clone()]), &root, t0);
        assert!(!first[0].live);
        assert_eq!(first[0].actual.pan, 30.0);
        // The authored aim moves; the commanded pan jumps and actual lags.
        f.yaw = 130.0;
        let v = venue_with(vec![f]);
        let a = tr.update_at(&v, &root, t0 + Duration::from_millis(100));
        assert_eq!(a[0].commanded.pan, 130.0);
        assert!(a[0].actual.pan > 30.0 && a[0].actual.pan < 130.0, "moves toward 130");
        let b = tr.update_at(&v, &root, t0 + Duration::from_millis(200));
        assert!(b[0].actual.pan > a[0].actual.pan, "keeps moving");
        // Given enough time it arrives and stops there.
        let mut last = b[0].actual.pan;
        for i in 3..400u64 {
            last = tr.update_at(&v, &root, t0 + Duration::from_millis(i * 100))[0].actual.pan;
        }
        assert!((last - 130.0).abs() < 1e-3);
    }

    #[test]
    fn tracker_stale_signal_falls_back_to_authored() {
        let root = std::env::temp_dir().join("metal-live-no-profiles");
        let id = "trk-stale-1";
        record(id, &sig(1.0, 1.0, 1.0));
        let v = venue_with(vec![fixture(id)]);
        let now = Instant::now();
        let fresh = Tracker::default().update_at(&v, &root, now);
        assert!(fresh[0].live);
        assert_eq!(fresh[0].strobe, 0.0);
        let stale = Tracker::default().update_at(&v, &root, now + Duration::from_secs(2));
        assert!(!stale[0].live);
        assert_eq!(stale[0].commanded.pan, 30.0, "authored again");
        assert_eq!(stale[0].commanded.dimmer, 1.0);
    }

    #[test]
    fn record_then_snapshot_round_trips() {
        let id = "live-roundtrip-1";
        let s = Signal { level: 0.25, ..sig(0.75, 0.0, 0.0) };
        record(id, &s);
        let (got, age) = snapshot().get(id).copied().expect("recorded");
        assert_eq!(got, s);
        assert!((0.0..1.0).contains(&age));
        let s2 = Signal { level: 0.5, ..sig(0.0, 0.0, 0.0) };
        record(id, &s2);
        assert_eq!(snapshot()[id].0, s2);
    }
}
