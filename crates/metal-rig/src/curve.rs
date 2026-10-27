//! Keyframe curves with per-segment interpolation.
//!
//! Every key owns the interpolation used to reach the *next* key, so a single
//! curve can hold a sustained ease into one key and a hard cut out of it.
//! Two keys at the same time are legal and mean an instantaneous jump.

use serde::{Deserialize, Serialize};

/// Values that can be interpolated. Implemented for scalars, points, and
/// rotations, so joint rotations and any other declared curve share one type.
pub trait Blend: Clone {
    fn blend(a: &Self, b: &Self, t: f32) -> Self;
}

impl Blend for f32 {
    fn blend(a: &Self, b: &Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

/// Booleans do not interpolate; they hold and then cut. A curve of them is a
/// sequence of intervals written as keys, which is what discrete state — a foot
/// on the floor or off it — actually is. There is no half-contact.
impl Blend for bool {
    fn blend(a: &Self, b: &Self, t: f32) -> Self {
        if t >= 1.0 {
            *b
        } else {
            *a
        }
    }
}

impl Blend for glam::Vec3 {
    fn blend(a: &Self, b: &Self, t: f32) -> Self {
        a.lerp(*b, t)
    }
}

impl Blend for glam::Quat {
    fn blend(a: &Self, b: &Self, t: f32) -> Self {
        a.slerp(*b, t)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EaseKind {
    In,
    Out,
    InOut,
}

/// How a value travels from one key to the next.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum Interp {
    /// No travel at all: the value holds, then cuts on the next key. This is
    /// how an abrupt change of intention is written.
    Hold,
    Linear,
    /// Symmetric ease. `power` is the sharpness: 1.0 is linear, 2.0 quadratic,
    /// higher is slower to leave and faster to arrive.
    Ease { kind: EaseKind, power: f32 },
    /// Cubic Bézier easing with the two control points in normalised
    /// time/value space, as in CSS `cubic-bezier()`. Use for eases that are
    /// not symmetric — a slow gather into a sudden strike.
    Bezier { x1: f32, y1: f32, x2: f32, y2: f32 },
}

impl Default for Interp {
    fn default() -> Self {
        Interp::Linear
    }
}

impl Interp {
    /// Maps normalised progress `0..=1` between two keys to a blend weight.
    pub fn warp(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match *self {
            Interp::Hold => 0.0,
            Interp::Linear => t,
            Interp::Ease { kind, power } => {
                let p = power.max(1e-3);
                match kind {
                    EaseKind::In => t.powf(p),
                    EaseKind::Out => 1.0 - (1.0 - t).powf(p),
                    EaseKind::InOut => {
                        if t < 0.5 {
                            0.5 * (2.0 * t).powf(p)
                        } else {
                            1.0 - 0.5 * (2.0 - 2.0 * t).powf(p)
                        }
                    }
                }
            }
            Interp::Bezier { x1, y1, x2, y2 } => bezier_ease(x1, y1, x2, y2, t),
        }
    }
}

/// Polynomial coefficients of a cubic Bézier with endpoints fixed at 0 and 1.
fn coeffs(a: f32, b: f32) -> (f32, f32, f32) {
    let c = 3.0 * a;
    let bb = 3.0 * (b - a) - c;
    (1.0 - c - bb, bb, c)
}

fn eval(a: f32, b: f32, t: f32) -> f32 {
    let (aa, bb, c) = coeffs(a, b);
    ((aa * t + bb) * t + c) * t
}

fn slope(a: f32, b: f32, t: f32) -> f32 {
    let (aa, bb, c) = coeffs(a, b);
    (3.0 * aa * t + 2.0 * bb) * t + c
}

fn bezier_ease(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    // Control points must stay in the unit interval on the time axis or the
    // curve stops being a function of time.
    let (x1, x2) = (x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
    if x1 == y1 && x2 == y2 {
        return x;
    }
    eval(y1, y2, solve_t(x1, x2, x))
}

/// Inverts `x(t)` by Newton-Raphson, falling back to bisection where the
/// curve goes flat and the derivative carries no information.
fn solve_t(x1: f32, x2: f32, x: f32) -> f32 {
    let mut t = x;
    for _ in 0..8 {
        let err = eval(x1, x2, t) - x;
        if err.abs() < 1e-6 {
            return t;
        }
        let d = slope(x1, x2, t);
        if d.abs() < 1e-6 {
            break;
        }
        t = (t - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut t = x.clamp(0.0, 1.0);
    for _ in 0..32 {
        let v = eval(x1, x2, t);
        if (v - x).abs() < 1e-6 {
            break;
        }
        if v > x {
            hi = t;
        } else {
            lo = t;
        }
        t = 0.5 * (lo + hi);
    }
    t
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Key<T> {
    /// Seconds from the start of the clip.
    pub t: f64,
    pub value: T,
    #[serde(default)]
    pub interp: Interp,
}

impl<T> Key<T> {
    pub fn new(t: f64, value: T) -> Self {
        Self {
            t,
            value,
            interp: Interp::default(),
        }
    }

    pub fn with(t: f64, value: T, interp: Interp) -> Self {
        Self { t, value, interp }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Curve<T> {
    keys: Vec<Key<T>>,
}

impl<T> Default for Curve<T> {
    fn default() -> Self {
        Self { keys: Vec::new() }
    }
}

impl<T: Blend> Curve<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// A curve that reads the same value at every time.
    pub fn constant(value: T) -> Self {
        Self {
            keys: vec![Key::new(0.0, value)],
        }
    }

    pub fn from_keys(mut keys: Vec<Key<T>>) -> Self {
        keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        Self { keys }
    }

    pub fn keys(&self) -> &[Key<T>] {
        &self.keys
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Inserts a key, keeping the curve sorted. Later keys win ties, so
    /// inserting at an existing time creates the jump rather than replacing.
    pub fn insert(&mut self, key: Key<T>) {
        let at = self.keys.partition_point(|k| k.t <= key.t);
        self.keys.insert(at, key);
    }

    pub fn remove(&mut self, index: usize) -> Option<Key<T>> {
        (index < self.keys.len()).then(|| self.keys.remove(index))
    }

    pub fn start(&self) -> Option<f64> {
        self.keys.first().map(|k| k.t)
    }

    pub fn end(&self) -> Option<f64> {
        self.keys.last().map(|k| k.t)
    }

    /// Reads the curve at `t`. Outside the key range the curve holds its first
    /// or last value rather than extrapolating.
    pub fn sample(&self, t: f64) -> Option<T> {
        let first = self.keys.first()?;
        if t <= first.t {
            return Some(first.value.clone());
        }
        let last = self.keys.last()?;
        if t >= last.t {
            return Some(last.value.clone());
        }
        let i = self.keys.partition_point(|k| k.t <= t) - 1;
        let (a, b) = (&self.keys[i], &self.keys[i + 1]);
        let span = b.t - a.t;
        if span <= 0.0 {
            // Coincident keys: an instantaneous jump to the later value.
            return Some(b.value.clone());
        }
        let u = ((t - a.t) / span) as f32;
        Some(T::blend(&a.value, &b.value, a.interp.warp(u)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(interp: Interp) -> Curve<f32> {
        Curve::from_keys(vec![Key::with(0.0, 0.0, interp), Key::new(1.0, 1.0)])
    }

    #[test]
    fn linear_reads_its_midpoint() {
        assert!((ramp(Interp::Linear).sample(0.5).unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hold_cuts_instead_of_travelling() {
        let c = ramp(Interp::Hold);
        assert_eq!(c.sample(0.5).unwrap(), 0.0);
        assert_eq!(c.sample(0.999).unwrap(), 0.0);
        assert_eq!(c.sample(1.0).unwrap(), 1.0);
    }

    #[test]
    fn ease_in_lags_and_ease_out_leads() {
        let power = 3.0;
        let ein = ramp(Interp::Ease {
            kind: EaseKind::In,
            power,
        });
        let eout = ramp(Interp::Ease {
            kind: EaseKind::Out,
            power,
        });
        assert!(ein.sample(0.5).unwrap() < 0.5);
        assert!(eout.sample(0.5).unwrap() > 0.5);
    }

    #[test]
    fn ease_power_one_matches_linear() {
        let c = ramp(Interp::Ease {
            kind: EaseKind::InOut,
            power: 1.0,
        });
        assert!((c.sample(0.3).unwrap() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn bezier_hits_its_endpoints_and_stays_monotone() {
        // A slow gather into a sudden arrival.
        let c = ramp(Interp::Bezier {
            x1: 0.9,
            y1: 0.0,
            x2: 1.0,
            y2: 0.4,
        });
        assert!(c.sample(0.0).unwrap().abs() < 1e-5);
        assert!((c.sample(1.0).unwrap() - 1.0).abs() < 1e-5);
        let mut prev = f32::NEG_INFINITY;
        for i in 0..=100 {
            let v = c.sample(i as f64 / 100.0).unwrap();
            assert!(v >= prev - 1e-4, "not monotone at {i}: {v} < {prev}");
            prev = v;
        }
        // Front-loaded control points keep the value low for most of the span.
        assert!(c.sample(0.5).unwrap() < 0.25);
    }

    #[test]
    fn bezier_identity_controls_are_linear() {
        let c = ramp(Interp::Bezier {
            x1: 0.25,
            y1: 0.25,
            x2: 0.75,
            y2: 0.75,
        });
        assert!((c.sample(0.4).unwrap() - 0.4).abs() < 1e-5);
    }

    #[test]
    fn coincident_keys_jump() {
        let c = Curve::from_keys(vec![
            Key::new(0.0, 0.0),
            Key::new(1.0, 0.2),
            Key::new(1.0, 0.9),
            Key::new(2.0, 1.0),
        ]);
        assert!((c.sample(0.5).unwrap() - 0.1).abs() < 1e-6);
        // Immediately after the shared time the later key is in force.
        assert!(c.sample(1.0001).unwrap() > 0.85);
    }

    #[test]
    fn outside_the_range_the_curve_holds() {
        let c = ramp(Interp::Linear);
        assert_eq!(c.sample(-5.0).unwrap(), 0.0);
        assert_eq!(c.sample(50.0).unwrap(), 1.0);
    }

    #[test]
    fn empty_curve_reads_nothing() {
        assert!(Curve::<f32>::new().sample(0.0).is_none());
    }

    #[test]
    fn insert_keeps_the_curve_sorted() {
        let mut c = Curve::new();
        c.insert(Key::new(2.0, 1.0));
        c.insert(Key::new(0.0, 0.0));
        c.insert(Key::new(1.0, 0.5));
        let times: Vec<f64> = c.keys().iter().map(|k| k.t).collect();
        assert_eq!(times, vec![0.0, 1.0, 2.0]);
    }
}
