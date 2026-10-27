//! Counting beats — the second output of the same analysis.
//!
//! `docs/AUDIO_FEATURES.md` settled the shape of this: a beat is **not** a
//! seventeenth column. The row stays sixteen wide because that width is shared
//! with the notation, the corpus and `digest_audio.py`, and a beat is not a
//! per-frame feature anyway. A real tracker emits a tempo and beat positions on
//! their own clock, which is what this is.
//!
//! `metal-dash`'s Router switches every sixteen beats and had nothing to count
//! with. Counting onsets was the cheap alternative and is wrong in a way an
//! operator sees immediately — eight onsets is two beats on a busy bar and
//! thirty seconds on a breakdown.
//!
//! # How
//!
//! Autocorrelate the onset flux over the last ten seconds, at lags that
//! correspond to plausible tempos, and take the peak. Then run a phase-locked
//! loop against it so the beat count advances smoothly between analyses instead
//! of jumping whenever the estimate is refreshed.
//!
//! **The count only ever goes forward.** A router built on a number that went
//! backwards would switch back to an animation it had just left, so corrections
//! are applied to the *rate* and never to the position. Locking on takes a
//! little longer that way and it is the right trade.
//!
//! # Following a DJ (re-lock)
//!
//! Small disagreements between the estimate and the running period are eased
//! in (25 % per analysis), which is what keeps a busy bar from making the count
//! stutter. A *different tempo* is not a small disagreement, and easing toward
//! it took the better part of the ten-second window — long enough for every
//! loop built on the clock to be audibly off. So a second estimate is taken on
//! the most recent half of the window only, which hears a new record after a
//! few seconds instead of ten, and when that estimate keeps naming the same
//! new period for [`RELOCK_VOTES`] analyses in a row the period is *replaced*,
//! not glided ([`Tempo::changes`] counts these). One analysis is never enough:
//! a fill or a breakdown can name a stray period once.
//!
//! # The octave guard
//!
//! Autocorrelation hears 87 and 174 as the same pulse. Once locked, an
//! estimate an octave away from the running period is folded back into the
//! running octave as long as the running octave still correlates at least
//! [`OCTAVE_KEEP`] as well as the estimate does — so a hi-hat pattern cannot
//! flip a 87 BPM count into 174 and double every loop's speed, while a track
//! that genuinely has no pulse left at the old octave is still followed.

/// Native STFT frames per second — the clock the flux arrives on.
pub const FRAME_RATE: f64 = crate::SR as f64 / crate::HOP as f64;

/// How much history the tempo estimate looks at. Long enough for eight beats
/// of a slow track, short enough to follow a DJ changing record.
const HISTORY: usize = 512;
/// Re-estimate this often. Every frame would be waste; the tempo of a room
/// does not change at 43 Hz.
const ANALYSE_EVERY: u32 = 16;

/// Agreeing analyses a new period needs before it replaces the old one:
/// 4 × [`ANALYSE_EVERY`] frames ≈ 1.5 s of the new record saying the same
/// thing, on top of the time the recent half-window needs to hear it.
pub const RELOCK_VOTES: u32 = 4;
/// Relative distance past which an estimate is a *different tempo* (voted on)
/// rather than a correction (glided toward).
const RELOCK_DIFF: f64 = 0.04;
/// Relative tolerance for two estimates to count as the same vote, and for a
/// ratio to count as an octave.
const AGREE: f64 = 0.03;
/// The running octave is kept while its correlation is at least this fraction
/// of the octave-away estimate's.
const OCTAVE_KEEP: f64 = 0.5;
/// Beats of recent flux the phase fit looks at — see `lock_phase`.
const PHASE_BEATS: f64 = 8.0;

/// How much better (bar-supported) a 3:4 / 2:3 neighbour of the best lag
/// must score to replace it — see the tresillo resolve in `estimate`.
const TRESILLO_MARGIN: f64 = 1.1;

const MIN_BPM: f64 = 70.0;
const MAX_BPM: f64 = 180.0;

fn lag_to_bpm(lag: f64) -> f64 {
    60.0 * FRAME_RATE / lag
}
fn bpm_to_lag(bpm: f64) -> f64 {
    60.0 * FRAME_RATE / bpm
}

pub struct Tempo {
    hist: Vec<f32>,
    head: usize,
    filled: usize,
    since: u32,

    /// Frames per beat.
    period: f64,
    locked: bool,
    confidence: f32,

    /// Monotonic, fractional, and the only thing anyone outside reads.
    beats: f64,
    /// PLL correction, multiplying the nominal rate. Clamped so a bad phase
    /// estimate cannot stall or sprint the count.
    rate: f64,

    /// A candidate new period and how many analyses in a row have named it.
    pending: Option<(f64, u32)>,
    /// Re-locks since the tracker first locked — see [`Tempo::changes`].
    changes: u32,
    /// Integral phase correction — see `lock_phase`.
    trim: f64,
}

/// One autocorrelation pass: the best period (fractional frames) and the raw
/// correlation of every lag, so the octave guard can ask about another one.
struct Estimate {
    period: f64,
    r: f64,
    raw: Vec<f64>,
    min_lag: usize,
    max_lag: usize,
}

impl Estimate {
    /// Bare correlation at a fractional lag, nearest integer; `None` outside
    /// the searched range.
    fn r_at(&self, lag: f64) -> Option<f64> {
        let l = lag.round() as usize;
        (l >= self.min_lag && l <= self.max_lag).then(|| self.raw[l])
    }
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a / b - 1.0).abs() < tol
}

impl Default for Tempo {
    fn default() -> Self {
        Self::new()
    }
}

impl Tempo {
    pub fn new() -> Tempo {
        Tempo {
            hist: vec![0.0; HISTORY],
            head: 0,
            filled: 0,
            since: 0,
            period: bpm_to_lag(120.0),
            locked: false,
            confidence: 0.0,
            beats: 0.0,
            rate: 1.0,
            pending: None,
            changes: 0,
            trim: 0.0,
        }
    }

    /// One native frame of onset flux.
    pub fn push(&mut self, flux: f32) {
        self.hist[self.head] = flux;
        self.head = (self.head + 1) % HISTORY;
        self.filled = (self.filled + 1).min(HISTORY);

        if self.locked {
            self.beats += self.rate / self.period;
        }

        self.since += 1;
        if self.since >= ANALYSE_EVERY && self.filled >= HISTORY / 2 {
            self.since = 0;
            self.analyse();
        }
    }

    /// Beats since the tracker locked on, fractional. `None` until it has.
    ///
    /// A caller polling faster than the audio arrives sees the same number
    /// twice, which is correct: this advances with the music and not with how
    /// often it is asked.
    pub fn beats(&self) -> Option<f32> {
        self.locked.then_some(self.beats as f32)
    }

    pub fn bpm(&self) -> Option<f32> {
        self.locked.then(|| lag_to_bpm(self.period) as f32)
    }

    /// How many times the tracker has *re-locked* — replaced its period with a
    /// confirmed new tempo rather than easing toward it. Starts at 0 on the
    /// first lock; a caller that wants a "tempo changed" event compares it with
    /// the value it saw last. The beat count is untouched by a re-lock (it
    /// carries on at the new speed from where it was).
    pub fn changes(&self) -> u32 {
        self.changes
    }

    /// Frames per beat at [`FRAME_RATE`], once locked.
    pub fn period(&self) -> Option<f64> {
        self.locked.then_some(self.period)
    }

    /// 0..1 — how much the autocorrelation peak stands out. Low means the
    /// music has no steady pulse, not that the tracker is broken.
    pub fn confidence(&self) -> f32 {
        self.confidence
    }

    /// Oldest-to-newest view of the ring.
    fn ordered(&self, out: &mut Vec<f32>) {
        out.clear();
        if self.filled < HISTORY {
            out.extend_from_slice(&self.hist[..self.filled]);
        } else {
            out.extend_from_slice(&self.hist[self.head..]);
            out.extend_from_slice(&self.hist[..self.head]);
        }
    }

    fn analyse(&mut self) {
        let mut x = Vec::with_capacity(HISTORY);
        self.ordered(&mut x);
        let n = x.len();

        // Remove the mean so a loud passage does not correlate with itself at
        // every lag simply for being loud.
        let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
        let x: Vec<f64> = x.iter().map(|&v| v as f64 - mean).collect();

        let Some(full) = estimate(&x) else {
            self.confidence = 0.0;
            return;
        };
        self.confidence = full.r.clamp(0.0, 1.0) as f32;

        if !self.locked {
            self.period = full.period;
            self.locked = true;
            self.beats = 0.0;
            self.rate = 1.0;
        } else {
            let p = self.period;
            let whole = octave_guard(&full, p);
            if close(whole, p, RELOCK_DIFF) {
                // Ease toward the new estimate. Jumping to it makes the count
                // stutter every time a busy bar nudges the autocorrelation.
                self.period += (whole - p) * 0.25;
            }
            // The re-lock vote listens to the recent half only: the whole
            // window still remembers the old record for ten seconds, and is
            // exactly why gliding was slow. Its mean is its own, for the same
            // reason the whole window's is.
            let half = &x[n / 2..];
            let m = half.iter().sum::<f64>() / half.len() as f64;
            let half: Vec<f64> = half.iter().map(|v| v - m).collect();
            match estimate(&half).map(|e| octave_guard(&e, p)) {
                Some(recent) if !close(recent, p, RELOCK_DIFF) => {
                    let votes = match self.pending {
                        Some((cand, v)) if close(recent, cand, AGREE) => v + 1,
                        _ => 1,
                    };
                    if votes >= RELOCK_VOTES {
                        // Replace, not glide. Position is untouched: `beats`
                        // carries on from where it was at the new speed.
                        self.period = recent;
                        self.changes += 1;
                        self.pending = None;
                        self.trim = 0.0;
                    } else {
                        self.pending = Some((recent, votes));
                    }
                }
                _ => self.pending = None,
            }
        }

        self.lock_phase(&x);
    }

    /// Nudge the rate so the count's downbeat drifts toward the music's.
    fn lock_phase(&mut self, x: &[f64]) {
        let p = self.period;
        if p < 2.0 {
            return;
        }
        let n = x.len();

        // Where does a pulse train of this period fit the flux best? Only the
        // last [`PHASE_BEATS`] beats are asked: a period a fraction of a
        // percent off drifts across the whole window, and the fit then names
        // the phase of the window's *middle* — a quarter of a beat late at
        // 174 BPM, which is a loop audibly behind the kick.
        let steps = 32;
        let mut best = (0.0f64, f64::NEG_INFINITY);
        for s in 0..steps {
            let phi = s as f64 * p / steps as f64;
            let mut acc = 0.0;
            let mut k = 0.0;
            while (phi + k * p) < n as f64 && k < PHASE_BEATS {
                let idx = n as f64 - 1.0 - (phi + k * p);
                if idx < 0.0 {
                    break;
                }
                acc += x[idx as usize];
                k += 1.0;
            }
            if k > 0.0 {
                let score = acc / k;
                if score > best.1 {
                    best = (phi, score);
                }
            }
        }

        // `best.0` is how many frames back the last beat was, so the music is
        // `best.0 / p` of the way into its beat now. The count's own idea of
        // that is its fractional part.
        //
        // 2026-10-03: this was `-best.0 / p`, which locked the count to the
        // *mirror* of the music's phase — the tempo was right and the beats
        // landed anywhere inside the beat. Nothing read the phase until the
        // downbeat detector and the tempo sidecar did.
        let detected = (best.0 / p).rem_euclid(1.0);
        let ours = self.beats.rem_euclid(1.0);
        let mut err = detected - ours;
        if err > 0.5 {
            err -= 1.0;
        }
        if err < -0.5 {
            err += 1.0;
        }

        // Rate, never position — see the module note. A correction big enough
        // to be heard as a tempo change is not applied at all in one go.
        //
        // `trim` is the integral half of the loop: the period estimate carries
        // a small bias of its own (an autocorrelation of frame-sampled pulses
        // cannot do better than a fraction of a percent), and a proportional
        // correction alone settles with the count a tenth of a beat behind.
        // The trim learns that bias slowly and cancels it.
        self.trim = (self.trim + 0.03 * err).clamp(-0.05, 0.05);
        self.rate = ((1.0 + self.trim) * (1.0 + 0.15 * err)).clamp(0.85, 1.15);
    }
}

/// The best tempo period in `x` (mean already removed), searched between
/// [`MIN_BPM`] and [`MAX_BPM`]. `None` for silence or a window too short to
/// hold two beats of the slowest tempo.
fn estimate(x: &[f64]) -> Option<Estimate> {
    let n = x.len();
    let energy: f64 = x.iter().map(|v| v * v).sum();
    if energy <= 1e-9 {
        return None;
    }
    let min_lag = bpm_to_lag(MAX_BPM).floor().max(2.0) as usize;
    let max_lag = bpm_to_lag(MIN_BPM).ceil().min((n / 2) as f64) as usize;
    if max_lag <= min_lag + 1 {
        return None;
    }

    let variance = energy / n as f64;
    // lag, prior-weighted score, and the bare correlation behind it
    let mut best = (0usize, f64::NEG_INFINITY, 0.0f64);
    let mut raw = vec![0.0f64; max_lag + 1];
    for lag in min_lag..=max_lag {
        let mut acc = 0.0;
        for i in lag..n {
            acc += x[i] * x[i - lag];
        }
        // Normalised into a correlation coefficient in −1..1, so "how
        // strongly periodic is this" means the same thing in a breakdown as
        // it does in a drop.
        //
        // It used to be the raw autocorrelation, and confidence was then
        // measured against the *mean* of those across all lags — which for
        // a mean-removed signal is about zero, so the ratio was garbage and
        // every reading came out 0.0000. Locking still worked, which is
        // what made it easy to miss: the number an operator reads to decide
        // whether to trust the beat light was the only thing broken.
        let r = acc / (n - lag) as f64 / variance;
        // A log-normal prior about 120 BPM. Autocorrelation cannot tell a
        // tempo from half of it — every other peak is also a peak — and
        // without this the tracker settles on 70 for a 140 track about as
        // often as not.
        let bpm = lag_to_bpm(lag as f64);
        let prior = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp();
        let score = r * prior;
        raw[lag] = r;
        if score > best.1 {
            best = (lag, score, r);
        }
    }
    if best.1 <= 0.0 {
        return None;
    }

    // Tresillo resolve (2026-10-03, T17 real audio). Dembow and dancehall
    // put a 3+3+2 figure over the kick, and its three-quarter-beat spacing
    // out-correlated the beat on a third of the reggaeton edits tried (95 →
    // 127, 100 → 134, 108 → 144). The beat and that figure are told apart by
    // the bar: a beat repeats four of itself later, three quarters of a beat
    // does not. Only 3:4 and 2:3 neighbours are weighed this way — octaves
    // are the prior's and the octave guard's business, and a bar term would
    // favour every subdivision.
    let corr = |l: usize| -> f64 {
        if l == 0 || l + 8 >= n {
            return 0.0;
        }
        let a: f64 = (l..n).map(|i| x[i] * x[i - l]).sum();
        a / (n - l) as f64 / variance
    };
    let prior = |l: usize| (-0.5 * ((lag_to_bpm(l as f64) / 120.0).log2() / 0.9).powi(2)).exp();
    let metric = |l: usize| (corr(l) + corr(4 * l)) * prior(l);
    let mut pick = best.0;
    for f in [4.0 / 3.0, 3.0 / 4.0, 3.0 / 2.0, 2.0 / 3.0] {
        let centre = (best.0 as f64 * f).round() as usize;
        let Some(alt) = (centre.saturating_sub(1)..=centre + 1)
            .filter(|l| (min_lag..=max_lag).contains(l))
            .max_by(|&a, &b| raw[a].total_cmp(&raw[b]))
        else {
            continue;
        };
        if raw[alt] > 0.0 && metric(alt) > metric(pick) * TRESILLO_MARGIN {
            pick = alt;
        }
    }
    if pick != best.0 {
        best = (pick, best.1, raw[pick]);
    }

    // Parabolic interpolation about the peak. Integer lags are far too
    // coarse on their own — one frame at lag 20 is five percent of the
    // tempo, which is a beat and a half of drift over a sixteen-beat group.
    // On the bare correlation, not the prior-weighted score: the prior's
    // slope tilts the parabola toward 120.
    let l = best.0;
    let period = if l > min_lag && l < max_lag {
        let (a, b, c) = (raw[l - 1], raw[l], raw[l + 1]);
        let denom = a - 2.0 * b + c;
        let shift = if denom.abs() > 1e-12 { 0.5 * (a - c) / denom } else { 0.0 };
        l as f64 + shift.clamp(-0.5, 0.5)
    } else {
        l as f64
    };
    Some(Estimate { period, r: best.2, raw, min_lag, max_lag })
}

/// The estimate's period, folded into the octave of the running period `p`
/// when it lands an octave away and the running octave still correlates —
/// see the module note. Anything that is not an octave passes through.
///
/// 2026-10-03 (the loops design notes T17, real audio): also the 3:4 / 3:2 relations.
/// Dembow and dancehall (3+3+2 tresillo over the kick) correlate strongly at
/// three quarters of a beat, and on real reggaeton edits the re-lock vote
/// walked from the right tempo to 4/3 of it (95 → 127, 100 → 134, 108 →
/// 144). The same rule as an octave: kept while the running period still
/// correlates.
fn octave_guard(e: &Estimate, p: f64) -> f64 {
    for factor in [2.0, 0.5, 4.0 / 3.0, 3.0 / 4.0, 3.0 / 2.0, 2.0 / 3.0] {
        if close(e.period * factor, p, AGREE * 2.0) {
            let kept = e.period * factor;
            if e.r_at(kept).is_some_and(|r| r >= OCTAVE_KEEP * e.r) {
                return kept;
            }
        }
    }
    e.period
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flux envelope like a four-on-the-floor at `bpm`.
    fn pulses(bpm: f64, secs: f64) -> Vec<f32> {
        let n = (FRAME_RATE * secs) as usize;
        let period = bpm_to_lag(bpm);
        (0..n)
            .map(|i| {
                let phase = (i as f64).rem_euclid(period);
                // A sharp attack that decays over a couple of frames, which is
                // what a kick actually looks like in this column.
                (-phase / 1.5).exp() as f32
            })
            .collect()
    }

    #[test]
    fn it_finds_the_tempo_of_a_steady_pulse() {
        for target in [90.0, 120.0, 128.0, 174.0] {
            let mut t = Tempo::new();
            for v in pulses(target, 20.0) {
                t.push(v);
            }
            let got = t.bpm().expect("should have locked on a metronome") as f64;
            assert!(
                (got - target).abs() / target < 0.03,
                "wanted {target} BPM, got {got}"
            );
        }
    }

    #[test]
    fn the_count_never_goes_backwards() {
        let mut t = Tempo::new();
        let mut last = f32::NEG_INFINITY;
        // Change tempo halfway, which is what a DJ does and is exactly when a
        // naive phase correction would rewind the count.
        for v in pulses(128.0, 15.0).into_iter().chain(pulses(140.0, 15.0)) {
            t.push(v);
            if let Some(b) = t.beats() {
                assert!(b >= last, "beat count went backwards: {last} -> {b}");
                last = b;
            }
        }
        assert!(last > 0.0, "it never counted anything");
    }

    #[test]
    fn beats_advance_at_about_the_right_speed() {
        let bpm = 120.0;
        let secs = 30.0;
        let mut t = Tempo::new();
        for v in pulses(bpm, secs) {
            t.push(v);
        }
        let counted = t.beats().unwrap() as f64;
        // It cannot count the seconds before it locked on, so this is a floor
        // rather than an equality.
        let ideal = bpm / 60.0 * secs;
        assert!(
            counted > ideal * 0.5 && counted < ideal * 1.05,
            "counted {counted} beats, a metronome would have given about {ideal}"
        );
    }

    #[test]
    fn silence_never_claims_a_tempo() {
        let mut t = Tempo::new();
        for _ in 0..(FRAME_RATE * 20.0) as usize {
            t.push(0.0);
        }
        assert!(t.beats().is_none(), "silence must not lock");
        assert_eq!(t.confidence(), 0.0);
    }

    /// A DJ cuts from 124 to 140: the period is replaced (one re-lock, not a
    /// ten-second glide), within a few seconds, and the count never stepped
    /// back while it happened.
    #[test]
    fn it_relocks_on_a_new_tempo_within_a_few_seconds() {
        let mut t = Tempo::new();
        for v in pulses(124.0, 20.0) {
            t.push(v);
        }
        assert!((t.bpm().unwrap() as f64 - 124.0).abs() < 124.0 * 0.03);
        assert_eq!(t.changes(), 0, "a steady track must not re-lock");

        let mut last = t.beats().unwrap();
        let mut followed_at = None;
        for (i, v) in pulses(140.0, 15.0).into_iter().enumerate() {
            t.push(v);
            let b = t.beats().unwrap();
            assert!(b >= last, "beat count went backwards: {last} -> {b}");
            last = b;
            if followed_at.is_none() && (t.bpm().unwrap() as f64 - 140.0).abs() < 140.0 * 0.03 {
                followed_at = Some(i as f64 / FRAME_RATE);
            }
        }
        let secs = followed_at.expect("never followed the new record");
        assert!(secs < 6.0, "took {secs:.1} s to follow 124 -> 140");
        assert!(t.changes() >= 1, "the change should have been a re-lock");
        assert!((t.bpm().unwrap() as f64 - 140.0).abs() < 140.0 * 0.03, "ended at {:?}", t.bpm());
    }

    /// A steady 87 with off-beat hats and noise — the classic bait for an
    /// octave flip to 174 — stays at 87 for a whole minute.
    #[test]
    fn a_steady_87_never_flips_an_octave() {
        let period = bpm_to_lag(87.0);
        let mut seed = 0x2545_f491u32;
        let mut t = Tempo::new();
        let n = (FRAME_RATE * 60.0) as usize;
        let mut seen = false;
        for i in 0..n {
            let ph = (i as f64).rem_euclid(period);
            let half = (i as f64 + period / 2.0).rem_euclid(period);
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed % 1000) as f64 / 1000.0 * 0.25;
            let v = (-ph / 1.5).exp() + 0.6 * (-half / 1.0).exp() + noise;
            t.push(v as f32);
            if let Some(bpm) = t.bpm() {
                if i as f64 > FRAME_RATE * 15.0 {
                    seen = true;
                    assert!(
                        (bpm as f64 - 87.0).abs() < 87.0 * 0.05,
                        "flipped to {bpm} at {:.1} s",
                        i as f64 / FRAME_RATE
                    );
                }
            }
        }
        assert!(seen, "never locked");
        assert_eq!(t.changes(), 0);
    }

    /// The count's integer beats land on the pulses, not merely at their
    /// rate: once settled, the count reads close to a whole number on every
    /// attack. (The phase was once locked to its mirror image.)
    #[test]
    fn beats_land_on_the_pulses() {
        for bpm in [90.0, 128.0, 174.0] {
            let period = bpm_to_lag(bpm);
            let mut t = Tempo::new();
            let mut worst = 0.0f64;
            for (i, v) in pulses(bpm, 30.0).into_iter().enumerate() {
                t.push(v);
                let on_attack = (i as f64).rem_euclid(period) < 1.0;
                if on_attack && i as f64 > FRAME_RATE * 20.0 {
                    let b = t.beats().unwrap() as f64;
                    worst = worst.max((b - b.round()).abs());
                }
            }
            // A frame is 0.07 of a beat at 174, so this is about a frame and
            // a half.
            assert!(worst < 0.12, "{bpm} BPM: count is {worst} of a beat off the pulses");
        }
    }
}
