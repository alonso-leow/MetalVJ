//! LPC formants — the jaw and the lips, estimated from the spectrum's shape.
//!
//! `the voice design notes` decision 12: pre-emphasis, a Hamming window, order-10
//! autocorrelation LPC and **root-free peak picking on the LPC envelope**.
//! [`Formants::f1`] is the jaw-openness proxy (F1 rises from /i/ to /a/) and
//! [`Formants::f2`] is spread vs round.
//!
//! This is a heuristic. It is allowed to be wrong; the face only needs a
//! plausible mouth, so a formant that is 15 % off is not a bug as long as it
//! moves the right way. The tests pin the direction, not the exact Hz.

/// Samples the LPC sees: 32 ms at 16 kHz.
pub const LPC_WIN: usize = 512;
/// Filter order. Ten poles is five formants, which is the usual speech choice.
pub const ORDER: usize = 10;
/// Pre-emphasis coefficient, the standard `1 − 0.97 z⁻¹`.
const PREEMPH: f32 = 0.97;
/// How finely the envelope is scanned, in Hz.
const STEP_HZ: f32 = 10.0;

/// F1 and F2 in Hz; zero when no peak was found there.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Formants {
    pub f1: f32,
    pub f2: f32,
    /// True when the frame carried enough energy to solve at all.
    pub present: bool,
}

/// The autocorrelation LPC solver. Reusable buffers, one per analyser.
pub struct Lpc {
    /// Autocorrelation `r[0..=ORDER]`.
    r: [f32; ORDER + 1],
    /// `a[0..=ORDER]`, `a[0] = 1`.
    a: [f32; ORDER + 1],
    /// Pre-emphasised, windowed frame.
    frame: Vec<f32>,
    window: Vec<f32>,
    /// Levinson–Durbin working copy.
    work: [f32; ORDER + 1],
}

impl Lpc {
    pub fn new() -> Lpc {
        let window = (0..LPC_WIN)
            .map(|i| {
                let t = i as f32 / (LPC_WIN - 1) as f32;
                0.54 - 0.46 * (std::f32::consts::TAU * t).cos()
            })
            .collect();
        Lpc {
            r: [0.0; ORDER + 1],
            a: [0.0; ORDER + 1],
            frame: vec![0.0; LPC_WIN],
            window,
            work: [0.0; ORDER + 1],
        }
    }

    /// F1/F2 from the most recent [`LPC_WIN`] samples of `last`.
    pub fn analyze(&mut self, last: &[f32]) -> Formants {
        let w = LPC_WIN.min(last.len());
        if w < ORDER * 2 {
            return Formants::default();
        }
        let x = &last[last.len() - w..];

        let mut energy = 0.0f32;
        for i in 0..w {
            let pre = if i == 0 { x[0] } else { x[i] - PREEMPH * x[i - 1] };
            let v = pre * self.window[i];
            self.frame[i] = v;
            energy += v * v;
        }
        if energy < 1e-8 {
            return Formants::default();
        }

        let n = w as f32;
        for lag in 0..=ORDER {
            let mut acc = 0.0f32;
            for i in lag..w {
                acc += self.frame[i] * self.frame[i - lag];
            }
            self.r[lag] = acc / n;
        }
        if self.r[0] <= 0.0 {
            return Formants::default();
        }

        // Levinson–Durbin. `work` starts as a[0]=1 and the reflection steps
        // fill a[1..]; the classic update order matters, hence the temporary.
        self.work[0] = 1.0;
        let mut err = self.r[0];
        for i in 1..=ORDER {
            let mut acc = self.r[i];
            for j in 1..i {
                acc += self.work[j] * self.r[i - j];
            }
            let k = -acc / err;
            self.work[i] = k;
            let prev = self.work;
            for j in 1..i {
                self.work[j] = prev[j] + k * prev[i - j];
            }
            err *= 1.0 - k * k;
            if err <= 0.0 {
                return Formants::default();
            }
        }
        self.a = self.work;
        self.a[0] = 1.0;

        // Root-free peak picking: the LPC envelope is 1/|A(e^{jω})|; scan it
        // and take the first two local maxima above 90 Hz, far enough apart.
        let mut peaks: Vec<(f32, f32)> = Vec::new(); // (hz, height)
        let mut prev = (0.0f32, 0.0f32); // (hz, value)
        let mut cur = (0.0f32, 0.0f32);
        let mut hz = STEP_HZ;
        let fmax = crate::voice::yin::VOICE_SR as f32 / 2.0;
        while hz < fmax {
            let w = std::f32::consts::TAU * hz / crate::voice::yin::VOICE_SR as f32;
            let (mut re, mut im) = (self.a[0], 0.0f32);
            for k in 1..=ORDER {
                re += self.a[k] * (k as f32 * w).cos();
                im -= self.a[k] * (k as f32 * w).sin();
            }
            let mag = (re * re + im * im).sqrt().max(1e-9);
            let env = 1.0 / mag;
            if cur.1 > prev.1
                && cur.1 > env
                && cur.0 > 90.0
                && peaks.last().is_none_or(|p| cur.0 - p.0 > 120.0)
            {
                peaks.push(cur);
            }
            prev = cur;
            cur = (hz, env);
            hz += STEP_HZ;
        }

        let mut f = Formants { present: true, ..Default::default() };
        if let Some(&(p, _)) = peaks.first() {
            f.f1 = p;
        }
        if let Some(&(p, _)) = peaks.get(1) {
            f.f2 = p;
        }
        f
    }
}

impl Default for Lpc {
    fn default() -> Self {
        Lpc::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-formant synthetic vowel: an impulse train through two 2-pole
    /// resonators at `f1` and `f2`, which is the standard source–filter toy.
    pub fn vowel(f0: f32, f1: f32, f2: f32, secs: f32) -> Vec<f32> {
        let sr = crate::voice::yin::VOICE_SR as f32;
        let n = (sr * secs) as usize;
        let period = (sr / f0).round() as usize;
        let mut glottal = vec![0.0f32; n];
        let mut at = 0;
        while at < n {
            glottal[at] = 1.0;
            at += period.max(1);
        }
        // Two resonators, each y[n] = x[n] - a1 y[n-1] - a2 y[n-2] with the
        // poles at radius r and angle 2πf/sr.
        let mut out = glottal;
        for &(freq, bw) in &[(f1, 80.0f32), (f2, 100.0)] {
            let r = (-std::f32::consts::PI * bw / sr).exp();
            let theta = std::f32::consts::TAU * freq / sr;
            let a1 = -2.0 * r * theta.cos();
            let a2 = r * r;
            let mut y = vec![0.0f32; n];
            for i in 0..n {
                let x = out[i];
                let y1 = if i >= 1 { y[i - 1] } else { 0.0 };
                let y2 = if i >= 2 { y[i - 2] } else { 0.0 };
                y[i] = x - a1 * y1 - a2 * y2;
            }
            out = y;
        }
        out
    }

    #[test]
    fn f1_rises_from_i_to_a() {
        let mut lpc = Lpc::new();
        // /i/: low F1, high F2. /a/: high F1, low F2.
        let i = vowel(120.0, 270.0, 2290.0, 0.5);
        let a = vowel(120.0, 730.0, 1090.0, 0.5);
        let fi = lpc.analyze(&i);
        let fa = lpc.analyze(&a);
        assert!(fi.present && fa.present, "both vowels must solve");
        assert!(
            fa.f1 > fi.f1 + 150.0,
            "/a/ F1 {} should sit well above /i/ F1 {}",
            fa.f1,
            fi.f1
        );
    }
}
