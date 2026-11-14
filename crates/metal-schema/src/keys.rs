//! Keyframe interpolation, one float at a time.
//!
//! A keyframe stores a model's whole row, but an author keys only the floats
//! that change there: the rest of the row is each float's own interpolated
//! value at that tick, written in so the row is complete. Easing the whole row
//! from keyframe to keyframe would make every float stop at every other
//! float's keyframe (smoothstep has zero slope at both ends). A slow glide
//! then turns into a staircase of pauses and surges, and a speed float (phase =
//! time × speed) shows it as a stutter.
//!
//! So each float is eased only between its **own** keys. The keys are
//! recovered from the rows, with no extra field on disk: a keyframe whose
//! value for a float lies on the ease between that float's neighbouring keys
//! is a pass-through for it, not a key. This is the timeline's, the trainer's
//! (`metal-dash` `dense::interpolate`) and `example-proof`'s one interpolation.

/// Smoothstep, `3t² - 2t³`: eases out of one key and into the next.
pub fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// How far a value may sit from the ease and still count as on it: the
/// dataset builder rounds rows to 5 decimals.
fn on_curve(v: f32, expected: f32) -> bool {
    (v - expected).abs() <= 1e-4 * (1.0 + expected.abs())
}

/// Float `at`'s own keys among `ticks` (sorted), as indices into `ticks`.
/// Greedy: from each key, the next key is the furthest one such that every
/// keyframe in between lies on the ease between the two.
pub fn own_keys(ticks: &[usize], value: impl Fn(usize) -> f32) -> Vec<usize> {
    let n = ticks.len();
    if n == 0 {
        return Vec::new();
    }
    let mut kept = vec![0];
    let mut p = 0;
    while p + 1 < n {
        let mut q = p + 1;
        while q + 1 < n {
            let (a, b) = (ticks[p] as f32, ticks[q + 1] as f32);
            let (va, vb) = (value(p), value(q + 1));
            let fits = (p + 1..=q).all(|j| on_curve(value(j), va + (vb - va) * ease((ticks[j] as f32 - a) / (b - a))));
            if !fits {
                break;
            }
            q += 1;
        }
        kept.push(q);
        p = q;
    }
    kept
}

/// One float's track: its own keys as `(tick, value)`, in order.
pub fn track(ticks: &[usize], value: impl Fn(usize) -> f32) -> Vec<(usize, f32)> {
    own_keys(ticks, &value).into_iter().map(|i| (ticks[i], value(i))).collect()
}

/// A track's value at `tick`: eased between its keys, held flat past either
/// end. `None` for an empty track.
pub fn track_at(track: &[(usize, f32)], tick: f32) -> Option<f32> {
    let first = track.first()?;
    let i = track.partition_point(|(t, _)| (*t as f32) <= tick);
    if i == 0 {
        return Some(first.1);
    }
    if i == track.len() {
        return Some(track[i - 1].1);
    }
    let ((a, va), (b, vb)) = (track[i - 1], track[i]);
    Some(va + (vb - va) * ease((tick - a as f32) / (b - a) as f32))
}

/// Every float's track for keyframes of `width`-wide rows.
pub fn tracks<'a>(keys: impl IntoIterator<Item = (usize, &'a [f32])>, width: usize) -> Vec<Vec<(usize, f32)>> {
    let mut keys: Vec<(usize, &[f32])> = keys.into_iter().filter(|(_, f)| f.len() == width).collect();
    keys.sort_by_key(|(t, _)| *t);
    let ticks: Vec<usize> = keys.iter().map(|(t, _)| *t).collect();
    (0..width).map(|i| track(&ticks, |k| keys[k].1[i])).collect()
}

/// The row at `tick` from [`tracks`]. `None` when nothing is keyed.
pub fn row_at(tracks: &[Vec<(usize, f32)>], tick: f32) -> Option<Vec<f32>> {
    tracks.iter().map(|t| track_at(t, tick)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A float keyed at 0 and 100 keeps gliding through a keyframe at 50
    /// that only another float named.
    #[test]
    fn a_pass_through_keyframe_does_not_stop_the_glide() {
        let ticks = [0usize, 50, 100];
        let vals = [0.0f32, 0.5, 1.0]; // 0.5 = ease(0.5): on the 0 → 100 glide
        assert_eq!(own_keys(&ticks, |k| vals[k]), vec![0, 2]);
        let t = track(&ticks, |k| vals[k]);
        // Row-to-row easing would give 0.5 + 0.5·ease(0.2) = 0.552 at 60;
        // the float's own glide gives ease(0.6) = 0.648.
        assert!((track_at(&t, 60.0).unwrap() - ease(0.6)).abs() < 1e-6);
    }

    #[test]
    fn a_real_change_and_a_hold_are_kept() {
        let ticks = [0usize, 40, 80, 120];
        let vals = [0.2f32, 0.2, 0.9, 0.9];
        assert_eq!(own_keys(&ticks, |k| vals[k]), vec![0, 1, 2, 3]);
        let t = track(&ticks, |k| vals[k]);
        assert_eq!(track_at(&t, 20.0), Some(0.2));
        assert_eq!(track_at(&t, 200.0), Some(0.9));
        assert_eq!(track_at(&t, -5.0), Some(0.2));
    }

    #[test]
    fn rows_resolve_per_float() {
        let a = [0.0f32, 1.0];
        let b = [0.5f32, 1.0];
        let c = [1.0f32, 0.0];
        let tr = tracks([(0, &a[..]), (50, &b[..]), (100, &c[..])], 2);
        assert_eq!(tr[0].len(), 2, "float 0 passes through 50");
        assert_eq!(tr[1].len(), 3, "float 1 holds to 50, then falls");
        assert_eq!(row_at(&tr, 50.0), Some(vec![0.5, 1.0]));
        assert_eq!(row_at(&[], 0.0), Some(vec![]));
    }
}
