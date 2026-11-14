//! A [`Source`] that loops a recorded run of digest rows.
//!
//! The rows are whatever an offline digester wrote out, played back at the
//! rate they were digested at and wrapped so the loop has no hard edge. Nothing
//! here listens to anything, so [`Source::is_real`] is `false` and
//! [`Source::beats`] is `None`: a recording carries no tempo clock of its own.

use crate::digest::{Frame, Source, FEATURES, MUSIC, VOICE};

/// A digest that loops a recorded run of rows.
pub struct Replay {
    /// The rows, each already fitted to exactly [`FEATURES`] columns.
    rows: Vec<[f32; FEATURES]>,
    /// The rate the rows were digested at, in rows per second.
    rate_hz: f32,
    /// Whether every row carries an analysed voice block. False for an empty
    /// recording and for any row narrower than the voice block, so a face
    /// model rests rather than animating a voice that was never measured (see
    /// [`Frame::voice`]).
    voice: bool,
}

impl Replay {
    /// Builds a looping replay from `rows`, each of which is padded with zeros
    /// up to [`FEATURES`] columns or truncated down to it. An older 16-wide
    /// recording therefore plays back with its voice and tone columns at zero.
    ///
    /// `rate_hz` is the rate the rows were digested at, so the loop lasts
    /// `rows.len() / rate_hz` seconds.
    pub fn new(rows: Vec<Vec<f32>>, rate_hz: f32) -> Replay {
        let voice = !rows.is_empty() && rows.iter().all(|r| r.len() >= MUSIC + VOICE);
        let rows = rows
            .iter()
            .map(|r| {
                let mut row = [0.0f32; FEATURES];
                let n = r.len().min(FEATURES);
                row[..n].copy_from_slice(&r[..n]);
                row
            })
            .collect();
        Replay { rows, rate_hz, voice }
    }
}

impl Source for Replay {
    /// The row for `time`, looping over the recording.
    ///
    /// The stepped row is the one whose slot `time` falls in. The interpolated
    /// row moves from the previous row toward it across that slot, so it
    /// reaches the stepped row as the slot closes, exactly as `Live::poll`
    /// does. At the seam the "previous" row for the first slot is the last row,
    /// so the loop wraps as a lerp from last to first rather than a jump.
    ///
    /// A function of `time` alone: the same instant always gives the same frame.
    fn poll(&mut self, time: f32) -> Frame {
        let n = self.rows.len();
        if n == 0 {
            return Frame {
                f: [0.0; FEATURES],
                raw: [0.0; FEATURES],
                rate: metal_schema::TickRate::Base,
                voice: false,
            };
        }

        // Position in rows, in f64 so the slot index stays exact over a long
        // session. `rem_euclid` keeps the index in range for a negative time too.
        let pos = time as f64 * self.rate_hz as f64;
        let whole = pos.floor();
        let phase = (pos - whole) as f32;
        let cur = (whole as i64).rem_euclid(n as i64) as usize;
        let prev = (cur + n - 1) % n;

        let raw = self.rows[cur];
        let before = self.rows[prev];
        let mut f = [0.0f32; FEATURES];
        for i in 0..FEATURES {
            f[i] = before[i] + (raw[i] - before[i]) * phase;
        }
        Frame {
            f,
            raw,
            rate: metal_schema::TickRate::Base,
            voice: self.voice,
        }
    }

    /// A recording has no tempo clock, so it cannot say how many beats have
    /// gone by.
    fn beats(&self, _time: f32) -> Option<f32> {
        None
    }

    fn describe(&self) -> String {
        format!("replay · {} rows at {:.1} Hz", self.rows.len(), self.rate_hz)
    }

    /// False: this is a recording looped back, not audio being analysed now.
    fn is_real(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` rows where row `k` is filled with the value `k`, so a frame's
    /// stepped row names the slot it came from.
    fn ramp(n: usize, rate_hz: f32) -> Replay {
        let rows = (0..n).map(|k| vec![k as f32; FEATURES]).collect();
        Replay::new(rows, rate_hz)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_five_second_loop_repeats_its_rows() {
        // 40 rows at 8 Hz is exactly five seconds.
        let mut r = ramp(40, 8.0);
        let a = r.poll(1.03);
        let b = r.poll(6.03);
        assert_eq!(a.raw, b.raw, "the stepped row must repeat after 5 s");
        for i in 0..FEATURES {
            assert!(close(a.f[i], b.f[i]), "column {i} drifted across the loop");
        }
        assert_eq!(a.raw[0], 8.0, "1.03 s at 8 Hz is slot 8");
    }

    #[test]
    fn the_seam_is_a_lerp_from_the_last_row_to_the_first() {
        // Slot 39 is the last; at 5.0625 s (pos 40.5) the interpolated row is
        // halfway from row 39 back to row 0.
        let mut r = ramp(40, 8.0);
        let frame = r.poll(5.0625);
        assert_eq!(frame.raw[0], 0.0, "the stepped row has wrapped to the first");
        assert!(close(frame.f[0], 19.5), "expected 19.5, got {}", frame.f[0]);
    }

    #[test]
    fn determinism_same_time_same_frame_regardless_of_history() {
        let mut a = ramp(40, 8.0);
        let mut b = ramp(40, 8.0);
        let _ = b.poll(9.9);
        let _ = b.poll(0.1);
        let fa = a.poll(3.3);
        let fb = b.poll(3.3);
        assert_eq!(fa.raw, fb.raw);
        assert_eq!(fa.f, fb.f);
        assert_eq!(a.poll(3.3).f, fa.f, "polling the same instant twice must agree");
    }

    #[test]
    fn the_interpolated_row_is_the_midpoint_between_two_rows() {
        // Two rows at 2 Hz; the slot for row 1 runs from 0.5 s to 1.0 s.
        let mut r = Replay::new(vec![vec![0.0; FEATURES], vec![1.0; FEATURES]], 2.0);
        let frame = r.poll(0.75);
        assert_eq!(frame.raw[0], 1.0, "stepped row is the held one");
        assert!(close(frame.f[0], 0.5), "midpoint expected, got {}", frame.f[0]);
        assert!(close(frame.f[FEATURES - 1], 0.5));
    }

    #[test]
    fn empty_input_is_a_zero_frame() {
        let mut r = Replay::new(Vec::new(), 8.0);
        let frame = r.poll(1.0);
        assert_eq!(frame.f, [0.0; FEATURES]);
        assert_eq!(frame.raw, [0.0; FEATURES]);
        assert!(!frame.voice, "nothing was measured, so no voice block");
    }

    #[test]
    fn short_rows_pad_with_zero_and_long_rows_truncate() {
        // A 16-wide row (no voice or tone block) is padded, and carries no voice.
        let mut short = Replay::new(vec![vec![0.5; MUSIC]], 8.0);
        let frame = short.poll(0.0);
        assert_eq!(frame.raw.len(), FEATURES);
        assert!(frame.raw[..MUSIC].iter().all(|&v| v == 0.5));
        assert!(frame.raw[MUSIC..].iter().all(|&v| v == 0.0));
        assert!(!frame.voice);

        // A row wider than FEATURES is cut down, and a full-width row has voice.
        let mut long = Replay::new(vec![vec![0.25; FEATURES + 5]], 8.0);
        let frame = long.poll(0.0);
        assert!(frame.raw.iter().all(|&v| v == 0.25));
        assert!(frame.voice);
    }

    #[test]
    fn a_recording_follows_no_tempo_and_is_not_real() {
        let r = ramp(4, 8.0);
        assert_eq!(r.beats(1.0), None);
        assert!(!r.is_real());
    }
}
