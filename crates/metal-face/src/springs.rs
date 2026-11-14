//! Coarticulation: every control chases its target through its own spring,
//! and the singing style bends the springs.
//!
//! # Why springs
//!
//! Real articulators overlap: the lips round for the *u* in "tu" while the
//! tongue is still on the *t*. Snapping each control to its target per frame
//! gives a mouth that pops between shapes; one shared smoothing gives a mouth
//! that slurs every closure away. A spring **per control**, with that
//! control's own speed, gets both: the lips can shut for a p in 50 ms while
//! the jaw is still on its way to the next vowel, and the overlap reads as
//! speech.
//!
//! # Exact, critically damped, no overshoot
//!
//! Each step integrates the spring **exactly** over `dt` (the closed form of
//! `x'' = ω²(target − x) − 2ω x'` with the target held over the step), not
//! with an Euler step. That makes it stable for any `dt` — a 32 Hz tick, a
//! 60 fps frame or a dropped frame — and, being critically damped, a value
//! that starts at rest moves monotonically toward a held target and never
//! passes it. A mouth that overshoots a closure opens again, which is the
//! one artefact speech cannot hide.
//!
//! Attack or release is chosen by the **target**: a target further from rest
//! than the current value is attack (the control is being pushed), anything
//! else is release. Deciding by the target rather than by the velocity keeps
//! the frequency steady through a whole approach, so the closed form holds.
//!
//! # Style
//!
//! [`Style`] is the four soft style scores from the voice block (`sing`,
//! `rap`, `speak`, `whisper`), used as weights. Each style is a multiplier on
//! the mouth springs and, for some, a reshaping of the target:
//!
//! - **speak**: neutral, the reference.
//! - **rap**: stiffer (mouth springs × 1.5) and sharper (mouth targets pushed
//!   15 % further from rest).
//! - **sing**: the jaw slows (× 0.6) and the lips a little (× 0.85): sung
//!   vowels are held, and a singer's jaw glides.
//! - **whisper**: the jaw is capped at 0.3, the lips stay tight (a stretch
//!   floor) and the eyes squint.
//!
//! Mixed scores mix linearly. Scores summing to zero mean speak.
//!
//! Two parts of decision 8 are deliberately **not** here because they need
//! inputs this crate does not have: the singer's vibrato at the measured F0
//! modulation rate, and teeth shown on hiss. Both are target changes the
//! solver in metal-perform makes from the voice columns before calling
//! [`Springs::step`].

use serde::{Deserialize, Serialize};

use crate::control::{Controls, FaceControl, FaceControl as C};

/// Soft style weights. Not normalized; [`Style::shares`] does that.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub sing: f32,
    pub rap: f32,
    pub speak: f32,
    pub whisper: f32,
}

/// Jaw cap under a full whisper.
pub const WHISPER_JAW: f32 = 0.3;
/// Squint floor under a full whisper.
pub const WHISPER_SQUINT: f32 = 0.5;
/// Stretch floor under a full whisper ("lips tight").
pub const WHISPER_STRETCH: f32 = 0.3;

impl Style {
    pub const SPEAK: Style = Style {
        sing: 0.0,
        rap: 0.0,
        speak: 1.0,
        whisper: 0.0,
    };
    pub const SING: Style = Style {
        sing: 1.0,
        rap: 0.0,
        speak: 0.0,
        whisper: 0.0,
    };
    pub const RAP: Style = Style {
        sing: 0.0,
        rap: 1.0,
        speak: 0.0,
        whisper: 0.0,
    };
    pub const WHISPER: Style = Style {
        sing: 0.0,
        rap: 0.0,
        speak: 0.0,
        whisper: 1.0,
    };

    /// `[sing, rap, speak, whisper]` normalized to sum 1; all-zero (or
    /// garbage) input is pure speak.
    pub fn shares(&self) -> [f32; 4] {
        let raw = [self.sing, self.rap, self.speak, self.whisper].map(|x| {
            if x.is_finite() {
                x.max(0.0)
            } else {
                0.0
            }
        });
        let total: f32 = raw.iter().sum();
        if total <= 1e-6 {
            [0.0, 0.0, 1.0, 0.0]
        } else {
            raw.map(|x| x / total)
        }
    }

    /// Multiplier on control `c`'s spring frequencies.
    pub fn rate(&self, c: FaceControl) -> f32 {
        let [sing, rap, speak, whisper] = self.shares();
        let sing_m = match c {
            C::JawOpen | C::JawSide => 0.6,
            c if c.is_mouth() => 0.85,
            _ => 1.0,
        };
        let rap_m = if c.is_mouth() { 1.5 } else { 1.0 };
        sing * sing_m + rap * rap_m + speak + whisper
    }

    /// The target after the style's reshaping (rap sharpening, whisper caps
    /// and floors), clamped.
    pub fn shape(&self, target: &Controls) -> Controls {
        let [_, rap, _, whisper] = self.shares();
        let mut out = *target;
        if rap > 0.0 {
            for c in FaceControl::ALL.iter().filter(|c| c.is_mouth()) {
                let rest = c.spec().rest;
                out[*c] = rest + (out[*c] - rest) * (1.0 + 0.15 * rap);
            }
        }
        if whisper > 0.0 {
            let hi = C::JawOpen.spec().hi;
            let cap = hi - (hi - WHISPER_JAW) * whisper;
            out[C::JawOpen] = out[C::JawOpen].min(cap);
            out[C::Squint] = out[C::Squint].max(WHISPER_SQUINT * whisper);
            out[C::LipStretch] = out[C::LipStretch].max(WHISPER_STRETCH * whisper);
        }
        out.clamped()
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::SPEAK
    }
}

/// The springs' state: a value and a velocity per control, plus the style.
#[derive(Clone, Debug, PartialEq)]
pub struct Springs {
    value: Controls,
    velocity: [f32; FaceControl::COUNT],
    pub style: Style,
}

impl Default for Springs {
    fn default() -> Self {
        Self::new()
    }
}

impl Springs {
    /// At rest, speaking.
    pub fn new() -> Self {
        Self::with_style(Style::SPEAK)
    }

    pub fn with_style(style: Style) -> Self {
        Springs {
            value: Controls::rest(),
            velocity: [0.0; FaceControl::COUNT],
            style,
        }
    }

    /// The current values.
    pub fn value(&self) -> Controls {
        self.value
    }

    /// Advances every control `dt` seconds toward `target` (reshaped by the
    /// style) and returns the new values, clamped to their ranges.
    pub fn step(&mut self, target: &Controls, dt: f32) -> Controls {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let target = self.style.shape(target);
        for &c in FaceControl::ALL {
            let spec = c.spec();
            let i = c.index();
            let x = self.value.0[i];
            let goal = target.0[i];
            let attack = (goal - spec.rest).abs() > (x - spec.rest).abs();
            let w = if attack {
                spec.spring.attack
            } else {
                spec.spring.release
            } * self.style.rate(c);
            // Closed form of the critically damped spring, target held.
            let y0 = x - goal;
            let v0 = self.velocity[i];
            let e = (-w * dt).exp();
            let k = v0 + w * y0;
            let y = (y0 + k * dt) * e;
            let v = (v0 - w * k * dt) * e;
            let nx = spec.clamp(goal + y);
            // A clamped value has stopped against its range; so has its speed.
            self.velocity[i] = if nx != goal + y { 0.0 } else { v };
            self.value.0[i] = nx;
        }
        self.value
    }
}
