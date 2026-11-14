//! The per-frame maths, as pure functions over one spectrum.
//!
//! Everything here is stateless except the decibel floor, which is the one
//! place the offline digester reaches across the whole file and a live one
//! cannot. That exception is argued where it happens.

/// librosa's `amin` — the floor under the logarithm, so silence is a large
/// negative number instead of minus infinity.
const AMIN: f32 = 1e-10;

/// Periodic Hann, which is what `scipy.signal.get_window("hann", n)` returns
/// with `fftbins=True` and therefore what librosa's STFT uses.
///
/// The symmetric variant — `cos(2πn/(N-1))` — is the one every textbook prints
/// and it is subtly wrong here: it makes the window non-periodic over the FFT
/// length and leaks differently. One character of difference, no error message.
pub fn hann_periodic(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            (0.5 - 0.5 * x.cos()) as f32
        })
        .collect()
}

/// Root mean square of a frame of *raw* samples.
///
/// Note "raw": librosa computes RMS off the time-domain signal and does not
/// apply the analysis window first. Windowing it would attenuate the edges and
/// report every frame quieter than it is.
pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame.iter().map(|&x| (x as f64) * (x as f64)).sum();
    (sum / frame.len() as f64).sqrt() as f32
}

/// Magnitude-weighted mean frequency, in Hz.
pub fn centroid(mag: &[f32], freqs: &[f32]) -> f32 {
    let total: f64 = mag.iter().map(|&m| m as f64).sum();
    if total <= f32::EPSILON as f64 {
        return 0.0;
    }
    let weighted: f64 =
        mag.iter().zip(freqs).map(|(&m, &f)| (m as f64) * (f as f64)).sum();
    (weighted / total) as f32
}

/// Magnitude-weighted spread about the centroid, in Hz — librosa's `p = 2`.
pub fn bandwidth(mag: &[f32], freqs: &[f32], centroid_hz: f32) -> f32 {
    let total: f64 = mag.iter().map(|&m| m as f64).sum();
    if total <= f32::EPSILON as f64 {
        return 0.0;
    }
    let acc: f64 = mag
        .iter()
        .zip(freqs)
        .map(|(&m, &f)| {
            let d = (f - centroid_hz) as f64;
            (m as f64) * d * d
        })
        .sum();
    (acc / total).sqrt() as f32
}

/// In-place `10·log10(max(amin, S))`, then librosa's `top_db` floor.
///
/// # The one thing a live digester cannot copy
///
/// librosa clamps at `S.max() - top_db` where the max is taken over the
/// **entire file**. A stream has no such number: the loudest moment of the set
/// may not have happened yet. So the peak here is a running one, and the floor
/// rises as the night gets louder.
///
/// It matters less than it sounds. The floor only touches bins more than 80 dB
/// below the peak — which is to say, silence — and the onset column is built
/// from a *difference* of these values, where a floor that clamps both sides
/// contributes nothing at all. `ref` cancels in that difference too, which is
/// why the rest of `power_to_db` needed no approximation. The reference test
/// measures what is left rather than asserting it away.
pub fn power_to_db_running(power: &mut [f32], peak_db: &mut f32, top_db: f32) {
    let mut frame_max = f32::NEG_INFINITY;
    for v in power.iter_mut() {
        // ref = 1.0, so the subtraction librosa does here is a subtraction of
        // zero and is simply absent.
        *v = 10.0 * v.max(AMIN).log10();
        if *v > frame_max {
            frame_max = *v;
        }
    }
    if frame_max > *peak_db {
        *peak_db = frame_max;
    }
    let floor = *peak_db - top_db;
    for v in power.iter_mut() {
        if *v < floor {
            *v = floor;
        }
    }
}

/// Rectified mean difference between two decibel spectra — the onset column
/// before it is pooled.
///
/// Rectified because only *increases* in energy are onsets: a note ending is
/// not an attack, and letting it count would put a spike on every release.
pub fn spectral_flux(now: &[f32], before: &[f32]) -> f32 {
    debug_assert_eq!(now.len(), before.len());
    if now.is_empty() {
        return 0.0;
    }
    let sum: f64 = now
        .iter()
        .zip(before)
        .map(|(&a, &b)| ((a - b) as f64).max(0.0))
        .sum();
    (sum / now.len() as f64) as f32
}

/// Divide by the largest element — librosa's `norm=inf` along the chroma axis.
///
/// A frame whose largest element is ~0 is left alone rather than amplified
/// into noise, which is what `librosa.util.normalize` does with its threshold
/// and is the difference between a quiet passage and a burst of static.
pub fn normalize_inf(v: &mut [f32]) {
    let peak = v.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    if peak > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= peak;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hann_window_is_periodic_not_symmetric() {
        let w = hann_periodic(8);
        assert!(w[0].abs() < 1e-6, "a Hann window starts at zero");
        // The periodic window does NOT come back to zero at the last sample —
        // that is exactly what distinguishes it from the symmetric one.
        assert!(w[7] > 0.1, "this looks like the symmetric window");
        // Symmetry about the centre still holds for the samples after the
        // first.
        for i in 1..4 {
            assert!((w[i] - w[8 - i]).abs() < 1e-6);
        }
    }

    #[test]
    fn rms_of_a_known_signal_is_the_known_answer() {
        assert!((rms(&[1.0, -1.0, 1.0, -1.0]) - 1.0).abs() < 1e-6);
        assert!(rms(&[0.0; 16]).abs() < 1e-9);
        // A full-scale sine is 1/sqrt(2).
        let sine: Vec<f32> = (0..1000)
            .map(|i| (2.0 * std::f32::consts::PI * i as f32 / 100.0).sin())
            .collect();
        assert!((rms(&sine) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    }

    #[test]
    fn the_centroid_sits_on_a_lone_tone_and_between_two() {
        let freqs: Vec<f32> = (0..5).map(|k| k as f32 * 100.0).collect();
        let mut mag = vec![0.0; 5];
        mag[3] = 1.0;
        assert!((centroid(&mag, &freqs) - 300.0).abs() < 1e-3);
        // Equal energy at 100 and 300 puts the centroid at 200.
        let mag = vec![0.0, 1.0, 0.0, 1.0, 0.0];
        assert!((centroid(&mag, &freqs) - 200.0).abs() < 1e-3);
        assert_eq!(centroid(&vec![0.0; 5], &freqs), 0.0, "silence has no centroid");
    }

    #[test]
    fn bandwidth_is_zero_on_a_pure_tone_and_grows_as_energy_spreads() {
        let freqs: Vec<f32> = (0..5).map(|k| k as f32 * 100.0).collect();
        let mut pure = vec![0.0; 5];
        pure[2] = 1.0;
        let c = centroid(&pure, &freqs);
        assert!(bandwidth(&pure, &freqs, c) < 1e-3, "one tone has no spread");

        let spread = vec![0.0, 1.0, 0.0, 1.0, 0.0];
        let c = centroid(&spread, &freqs);
        assert!((bandwidth(&spread, &freqs, c) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn flux_counts_energy_arriving_and_ignores_it_leaving() {
        let quiet = [0.0f32, 0.0, 0.0, 0.0];
        let loud = [10.0f32, 10.0, 10.0, 10.0];
        assert!((spectral_flux(&loud, &quiet) - 10.0).abs() < 1e-6, "an attack counts");
        assert_eq!(spectral_flux(&quiet, &loud), 0.0, "a release must not");
    }

    #[test]
    fn the_decibel_floor_tracks_the_loudest_thing_heard_so_far() {
        let mut peak = f32::NEG_INFINITY;
        // 1.0 in power is 0 dB; the floor lands 80 below it.
        let mut a = [1.0f32, 1e-12];
        power_to_db_running(&mut a, &mut peak, 80.0);
        assert!((a[0] - 0.0).abs() < 1e-4);
        assert!((a[1] - (-80.0)).abs() < 1e-4, "the quiet bin is floored, not -120");

        // Something louder arrives and drags the floor up with it.
        let mut b = [100.0f32, 1e-12];
        power_to_db_running(&mut b, &mut peak, 80.0);
        assert!((b[0] - 20.0).abs() < 1e-4);
        assert!((b[1] - (-60.0)).abs() < 1e-4, "the floor should have risen to -60");
    }

    #[test]
    fn normalising_leaves_a_silent_frame_silent() {
        let mut v = [0.0f32; 12];
        normalize_inf(&mut v);
        assert!(v.iter().all(|&x| x == 0.0), "silence must not become static");

        let mut v = [0.5f32, 0.25, 0.0];
        normalize_inf(&mut v);
        assert!((v[0] - 1.0).abs() < 1e-6 && (v[1] - 0.5).abs() < 1e-6);
    }
}
