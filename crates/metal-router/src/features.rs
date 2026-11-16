//! A stretch of digest rows → a fixed description of what kind of music it is.
//!
//! # Why not feed the model the sixteen columns
//!
//! Because the question is not "what is happening in this 125 ms" — it is
//! "what kind of passage is this", and those want different inputs. Sixteen
//! columns × a thousand ticks is a hundred and sixty thousand numbers per
//! example, against a corpus that will realistically be a few dozen songs
//! authored by one person. A model over that is a model that memorises which
//! song it is looking at.
//!
//! These twelve are statistics *over a window*: how loud, how much it breathes,
//! how bright, how hard it hits, how often, how regularly, how tonal, how fast
//! the harmony moves. They are the properties an operator would actually name
//! if you asked why a visual suits a track, they are invariant to which song it
//! is, and there are few enough of them that a linear model over a handful of
//! songs is a fair thing to fit.
//!
//! Every one comes out in 0..1, because [`crate::learn`] standardises them
//! anyway and a feature that could run to the thousands would dominate the
//! initial gradient before it got the chance to. (The digest itself makes the
//! same promise and does not keep it for two of its columns — see
//! `metal-dash::digest::Frame`. Anything handing rows to this module owes it the
//! same scaling `metal-dash::live` does.)

/// One digest row: the music prefix then the voice block. Declared once in
/// `metal_schema::FEATURES`; every statistic below reads the first
/// [`MUSIC`](metal_schema::MUSIC) columns and ignores the voice.
pub const COLUMNS: usize = metal_schema::FEATURES;

/// How many numbers describe a passage.
pub const FEATURES: usize = 12;

/// What each one is. Used by the studio to label the bars, so the operator sees
/// "pulso" and not "f7".
pub const NAMES: [&str; FEATURES] = [
    "energy",
    "dinamica",
    "shine",
    "width",
    "hit",
    "pico",
    "density",
    "pulse",
    "tonalidad",
    "armonia",
    "lleno",
    "variation",
];

/// The base tick rate — the bottom rung of the ladder (`metal_schema::BASE_HZ`),
/// and the rate the trained descriptor contract is defined on. [`describe_at`]
/// accepts a finer rate and normalises per second so a descriptor means the
/// same at every level; this constant is the "per base tick" denominator.
pub const RATE: f32 = metal_schema::BASE_HZ as f32;

/// A transient counts as a hit above this. Not a tuned constant: it is the
/// same 0.28 the studio's lane already draws its beat suggestions at, and the
/// two must agree or the lane's marks and the model's `densidad` disagree
/// about what a hit is.
const HIT: f32 = 0.28;

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.iter().sum::<f32>() / v.len() as f32
}

/// The value `q` of the way through the sorted values. Interpolation-free: a
/// percentile of a few hundred samples does not need it, and the nearest-rank
/// form has no edge cases at the ends.
fn percentile(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() - 1) as f32 * q.clamp(0.0, 1.0)).round() as usize;
    sorted[at]
}

/// Describes `rows` — one passage of a song, on the base grid.
///
/// Fewer than four rows is half a second, which is not a passage; it returns
/// zeros rather than a description built on nothing.
pub fn describe(rows: &[[f32; COLUMNS]]) -> [f32; FEATURES] {
    describe_at(rows, metal_schema::TickRate::Base)
}

/// [`describe`] at an explicit tick level.
///
/// **Rate-invariant by per-second normalisation.** Every statistic that is a
/// count or a mean *per row* — `golpe`, `densidad`, `armonia`, `variacion` — is
/// scaled by `hz / RATE`, so the factor is exactly `1` at the base level (the
/// trained 8 Hz contract is unchanged) and `4` at the fine level, where the
/// same events are spread over four times as many rows. Statistics that are
/// already dimensionless over the window (`dinamica`, `pulso`, `tonalidad`,
/// `lleno`) need no conversion. `densidad` reads a hit count over `seconds`,
/// which is `rows / hz` and therefore already per second. This is
/// the voice design notes line 270: *"`describe` normalizes densities per second, so a
/// descriptor means the same at both levels."*
pub fn describe_at(rows: &[[f32; COLUMNS]], rate: metal_schema::TickRate) -> [f32; FEATURES] {
    let hz = rate.hz() as f32;
    // 1.0 at base, 4.0 at fine: turns a per-row density into a per-base-row
    // equivalent, so the two levels land on the same numbers.
    let density = hz / RATE;
    let mut out = [0.0f32; FEATURES];
    if rows.len() < 4 {
        return out;
    }
    // Every read below stays inside the music prefix — the descriptor is the
    // twelve statistics over what the corpus and the Colourist are defined on,
    // and the voice block must not leak into a model trained without it.
    let music = metal_schema::MUSIC;

    let rms: Vec<f32> = rows.iter().map(|r| r[0]).collect();
    let onset: Vec<f32> = rows.iter().map(|r| r[2]).collect();
    let mut rms_sorted = rms.clone();
    rms_sorted.sort_by(f32::total_cmp);
    let mut onset_sorted = onset.clone();
    onset_sorted.sort_by(f32::total_cmp);

    out[0] = mean(&rms);
    out[1] = percentile(&rms_sorted, 0.9) - percentile(&rms_sorted, 0.1);
    out[2] = mean(&rows.iter().map(|r| r[3]).collect::<Vec<_>>());
    out[3] = mean(&rows.iter().map(|r| r[1]).collect::<Vec<_>>());
    // `golpe` is a mean over rows and onset is a per-event transient: one spike
    // per hit at either level, so 32 Hz would divide it by four. Per-second
    // normalisation is the `density` factor (1 at base, 4 at fine).
    out[4] = (mean(&onset) * density).clamp(0.0, 1.0);
    out[5] = percentile(&onset_sorted, 0.95);

    // Where the hits are. A hit is a local maximum over the threshold, which is
    // what the studio's lane already marks — counting *every* tick over the
    // threshold would make a long swell read as a fast passage.
    let mut hits: Vec<usize> = Vec::new();
    for i in 1..onset.len() - 1 {
        if onset[i] >= HIT && onset[i] > onset[i - 1] && onset[i] >= onset[i + 1] {
            hits.push(i);
        }
    }
    // Already per second: `rows / hz` is the same duration at either level, so
    // the hit *rate* is invariant without a further factor.
    let seconds = rows.len() as f32 / hz;
    // Eight a second is about as busy as music gets; past that it is noise and
    // the model has nothing more to learn from the difference.
    out[6] = (hits.len() as f32 / seconds.max(1e-3) / 8.0).clamp(0.0, 1.0);

    // How evenly spaced they are. A drum machine is near 1, a rubato piano near
    // 0 — which is one of the few things about a track a linear model can use
    // to tell a genre from a mood.
    if hits.len() >= 4 {
        let gaps: Vec<f32> =
            hits.windows(2).map(|w| (w[1] - w[0]) as f32).collect();
        let m = mean(&gaps);
        let var = gaps.iter().map(|g| (g - m) * (g - m)).sum::<f32>() / gaps.len() as f32;
        out[7] = (1.0 - var.sqrt() / m.max(1e-3)).clamp(0.0, 1.0);
    }

    // How concentrated the harmony is, as one minus the normalised entropy of
    // the chroma. A single held chord is near 1; a wash or a drum break is
    // near 0, because twelve pitch classes all lit is the same as none of them.
    let mut tonal = 0.0f32;
    let mut motion = 0.0f32;
    let mut prev: Option<[f32; 12]> = None;
    for r in rows {
        let mut c = [0.0f32; 12];
        let total: f32 = r[4..music].iter().sum::<f32>().max(1e-6);
        for k in 0..12 {
            c[k] = r[4 + k] / total;
        }
        let h: f32 = c.iter().filter(|p| **p > 1e-6).map(|p| -p * p.ln()).sum();
        tonal += 1.0 - (h / 12f32.ln()).clamp(0.0, 1.0);
        if let Some(p) = prev {
            // L1 between two distributions tops out at 2.
            motion += c.iter().zip(p.iter()).map(|(a, b)| (a - b).abs()).sum::<f32>() * 0.5;
        }
        prev = Some(c);
    }
    out[8] = tonal / rows.len() as f32;
    // Harmony motion is a sum over adjacent rows; the same chord changes over
    // four times the rows would read four times lower, so normalise per second.
    out[9] = (motion / (rows.len() - 1) as f32 * 4.0 * density).clamp(0.0, 1.0);

    // How much of the passage is at full tilt, against its own loudest moment —
    // so a quiet track that never lets up still reads as full, which is what
    // the word means to somebody choosing a visual.
    let loudest = rms_sorted[rms_sorted.len() - 1].max(1e-6);
    out[10] = rms.iter().filter(|v| **v > loudest * 0.5).count() as f32 / rms.len() as f32;

    let churn: f32 =
        rms.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / (rms.len() - 1) as f32;
    // As `armonia`: total level churn per second, not per row.
    out[11] = (churn * 4.0 * density).clamp(0.0, 1.0);

    for v in out.iter_mut() {
        if !v.is_finite() {
            *v = 0.0;
        }
        *v = v.clamp(0.0, 1.0);
    }
    out
}

/// How far apart two descriptions are, 0..1. The novelty curve in
/// [`crate::segment`] is built out of this, and so is the live "has the song
/// changed" test in `metal-dash`.
pub fn distance(a: &[f32; FEATURES], b: &[f32; FEATURES]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum::<f32>() / FEATURES as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A steady four-on-the-floor at `bpm`, loud and tonal.
    fn beat(ticks: usize, bpm: f32, level: f32) -> Vec<[f32; COLUMNS]> {
        let per_beat = 60.0 / bpm * RATE;
        (0..ticks)
            .map(|i| {
                let mut r = [0.0f32; COLUMNS];
                let phase = (i as f32 / per_beat).fract();
                r[2] = if phase < 1.0 / per_beat { 0.9 } else { 0.02 };
                r[0] = level;
                r[1] = 0.4;
                r[3] = 0.35;
                r[4] = 0.9; // one pitch class, held
                for k in 1..12 {
                    r[4 + k] = 0.02;
                }
                r
            })
            .collect()
    }

    /// A wash: no transients, no key, drifting level.
    fn wash(ticks: usize) -> Vec<[f32; COLUMNS]> {
        (0..ticks)
            .map(|i| {
                let mut r = [0.0f32; COLUMNS];
                r[0] = 0.35 + 0.1 * (i as f32 * 0.03).sin();
                r[1] = 0.7;
                r[2] = 0.03;
                r[3] = 0.6;
                for k in 0..12 {
                    r[4 + k] = 0.5 + 0.02 * ((i + k) as f32).sin();
                }
                r
            })
            .collect()
    }

    #[test]
    fn every_feature_is_named_and_lands_in_the_unit_range() {
        // `learn` standardises, so a feature in the thousands would still
        // train — but it would dominate the first hundred epochs and the
        // failure would look like "the model ignores everything else".
        assert_eq!(NAMES.len(), FEATURES);
        for rows in [beat(400, 128.0, 0.7), wash(400), beat(30, 90.0, 0.1)] {
            for (i, v) in describe(&rows).iter().enumerate() {
                assert!(
                    (0.0..=1.0).contains(v) && v.is_finite(),
                    "{} left the unit range: {v}",
                    NAMES[i]
                );
            }
        }
    }

    #[test]
    fn nothing_at_all_describes_as_nothing_rather_than_as_noise() {
        assert_eq!(describe(&[]), [0.0; FEATURES]);
        assert_eq!(describe(&[[0.0; COLUMNS]; 2]), [0.0; FEATURES]);
    }

    #[test]
    fn a_steady_beat_is_regular_and_a_wash_is_not() {
        // `pulso` is the one feature that carries "is this machine music", and
        // it is the whole reason the router can tell house from a ballad.
        let steady = describe(&beat(600, 128.0, 0.7));
        let drift = describe(&wash(600));
        assert!(steady[7] > 0.85, "a machine beat read as irregular: {}", steady[7]);
        assert!(drift[7] < 0.2, "a wash read as regular: {}", drift[7]);
        assert!(steady[6] > drift[6], "a wash was busier than a beat");
    }

    #[test]
    fn one_held_chord_is_tonal_and_twelve_lit_pitch_classes_are_not() {
        let tonal = describe(&beat(400, 120.0, 0.6));
        let flat = describe(&wash(400));
        assert!(tonal[8] > 0.6, "a single held pitch class read as atonal: {}", tonal[8]);
        assert!(flat[8] < 0.2, "an even chroma read as tonal: {}", flat[8]);
    }

    #[test]
    fn faster_is_denser() {
        let slow = describe(&beat(800, 70.0, 0.6));
        let fast = describe(&beat(800, 170.0, 0.6));
        assert!(fast[6] > slow[6] * 1.5, "170 BPM did not read as denser than 70");
    }

    #[test]
    fn two_kinds_of_music_are_further_apart_than_two_passages_of_one() {
        // The property the segmenter is built on. If this did not hold, the
        // novelty curve would be noise and every boundary would be a guess.
        let a = describe(&beat(600, 128.0, 0.7));
        let b = describe(&beat(600, 128.0, 0.62));
        let c = describe(&wash(600));
        assert!(
            distance(&a, &c) > distance(&a, &b) * 3.0,
            "a beat and a wash ({}) were not much further apart than two beats ({})",
            distance(&a, &c),
            distance(&a, &b)
        );
    }

    #[test]
    fn describing_the_same_rows_twice_gives_the_same_description() {
        let rows = beat(500, 124.0, 0.66);
        assert_eq!(describe(&rows), describe(&rows));
    }

    /// The same passage at base and at fine describes the same way. The fine
    /// rows are a faithful 4→1 representation of the base rows: every
    /// continuous column repeats, and each onset event lands in exactly one of
    /// the four fine rows (which is what max-pooling a transient back to base
    /// does). Per-second normalisation then has to make the two agree, which is
    /// what `density` is for.
    ///
    /// Hits are isolated and non-uniformly spaced (a 4,4,4,3 gap pattern), so
    /// `pulso` is below 1 and the test is not trivially satisfied by uniform
    /// spacing. The ×4 upsampling makes every base gap exactly four fine gaps,
    /// so the coefficient of variation — and `pulso` — is unchanged.
    #[test]
    fn router_descriptors_agree_across_levels() {
        use metal_schema::TickRate;
        let n = 640usize;
        let mut hit = vec![false; n];
        let mut at = 4usize;
        // Walk the 4,4,4,3 pattern explicitly.
        let gaps = [4usize, 4, 4, 3];
        let mut g = 0;
        while at < n - 1 {
            hit[at] = true;
            at += gaps[g % gaps.len()];
            g += 1;
        }
        let base_rows: Vec<[f32; COLUMNS]> = (0..n)
            .map(|i| {
                let mut r = [0.0f32; COLUMNS];
                r[0] = 0.6 + 0.1 * (i as f32 * 0.05).sin();
                r[1] = 0.4;
                r[2] = if hit[i] { 0.9 } else { 0.0 };
                r[3] = 0.3;
                r[4] = 0.9; // one pitch class, held
                for k in 1..12 {
                    r[4 + k] = 0.02;
                }
                r
            })
            .collect();
        let fine_rows: Vec<[f32; COLUMNS]> = base_rows
            .iter()
            .flat_map(|r| {
                (0..4).map(move |j| {
                    let mut f = *r;
                    // The event survives pooling by max, so it sits in one
                    // fine row; the other three are silent.
                    f[2] = if j == 0 { r[2] } else { 0.0 };
                    f
                })
            })
            .collect();

        let base = describe_at(&base_rows, TickRate::Base);
        let fine = describe_at(&fine_rows, TickRate::Fine);
        for i in 0..FEATURES {
            assert!(
                (base[i] - fine[i]).abs() < 0.1,
                "{}: base {:.4} vs fine {:.4}",
                NAMES[i],
                base[i],
                fine[i]
            );
        }
    }

    /// Per-second normalisation is the identity at the base level: `describe`
    /// and `describe_at(.., Base)` are the same numbers the trained contract is
    /// defined on.
    #[test]
    fn describe_at_base_is_describe() {
        let rows = beat(400, 120.0, 0.6);
        assert_eq!(describe(&rows), describe_at(&rows, metal_schema::TickRate::Base));
    }
}
