//! `webrtc-vad`, owned by the analyser.
//!
//! The plan chose `webrtc-vad` (the binding of WebRTC's GMM VAD over six
//! bands): BSD C, no model file, speech/no-speech per 10/20/30 ms frame. V0
//! measured that it fires on almost every second of a full mix (synth leads,
//! horns and guitars all read as speech-*like*), which is exactly why
//! [`crate::voice`] fuses it with YIN and the speech band rather than trusting
//! it alone. When stems land, the same wrapper runs on the `vocals` stem and
//! most of that goes away.
//!
//! # Send, but not Sync
//!
//! `webrtc_vad::Vad` wraps `*mut fvad::Fvad`, so it is `!Send` and `!Sync` by
//! default (V0 confirmed this with a compile assertion). The type is a
//! single-owner handle with no thread affinity: moving it to the listener
//! thread is safe, sharing it is not. Hence `unsafe impl Send` and **no**
//! `Sync`. The wrapper keeps the buffer and the mode, so the C object is only
//! ever touched from one thread at a time.

/// VAD operating mode, from WebRTC. More aggressive means fewer false
/// positives but more missed speech.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Quality,
    LowBitrate,
    Aggressive,
    VeryAggressive,
}

#[cfg(not(target_arch = "wasm32"))]
impl Mode {
    fn as_webrtc(self) -> webrtc_vad::VadMode {
        match self {
            Mode::Quality => webrtc_vad::VadMode::Quality,
            Mode::LowBitrate => webrtc_vad::VadMode::LowBitrate,
            Mode::Aggressive => webrtc_vad::VadMode::Aggressive,
            Mode::VeryAggressive => webrtc_vad::VadMode::VeryAggressive,
        }
    }
}

/// A 16 kHz VAD over 10 ms frames.
#[cfg(not(target_arch = "wasm32"))]
pub struct Vad {
    inner: webrtc_vad::Vad,
    mode: Mode,
    buf: Vec<i16>,
}

// SAFETY: `inner` is a single-owner `*mut Fvad` with no thread affinity. The
// analyser that owns a `Vad` is moved between threads, never shared, and no
// method takes `&self` across a call to the C library. `Sync` is deliberately
// not implemented.
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Send for Vad {}

#[cfg(not(target_arch = "wasm32"))]
impl Vad {
    /// 16 kHz, [`Mode::Aggressive`] (V0 found the modes differ only slightly on
    /// music, so the middle-ground aggressive one is the default).
    pub fn new() -> Vad {
        Vad::with_mode(Mode::Aggressive)
    }

    pub fn with_mode(mode: Mode) -> Vad {
        Vad {
            inner: webrtc_vad::Vad::new_with_rate_and_mode(
                webrtc_vad::SampleRate::Rate16kHz,
                mode.as_webrtc(),
            ),
            mode,
            buf: vec![0i16; 160],
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// One decision for a 10 ms (160-sample) 16 kHz frame.
    ///
    /// Shorter or longer frames are accepted by the C library at 8/16/32/48 kHz
    /// as 10/20/30 ms, but this wrapper is fixed at 10 ms because that is the
    /// analyser's frame: a longer one would hide a plosive closure.
    pub fn is_voice(&mut self, frame: &[f32]) -> bool {
        let n = frame.len().min(self.buf.len());
        for (i, &s) in frame[..n].iter().enumerate() {
            self.buf[i] = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
        }
        self.inner.is_voice_segment(&self.buf[..n]).unwrap_or(false)
    }

    /// Clear the detector's internal state. The frame count is not a VAD
    /// concern; this is only for a discontinuity such as a seek.
    pub fn reset(&mut self) {
        self.inner.reset();
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for Vad {
    fn default() -> Self {
        Vad::new()
    }
}

/// On wasm32 `webrtc-vad` (a C core) does not build, so the gate is an energy
/// floor instead: a frame is voice when its RMS clears a level that rises with
/// the mode. Coarser than WebRTC's GMM, and no capture runs in the browser build
/// to feel the difference.
#[cfg(target_arch = "wasm32")]
pub struct Vad {
    mode: Mode,
}

#[cfg(target_arch = "wasm32")]
impl Vad {
    pub fn new() -> Vad {
        Vad::with_mode(Mode::Aggressive)
    }

    pub fn with_mode(mode: Mode) -> Vad {
        Vad { mode }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn is_voice(&mut self, frame: &[f32]) -> bool {
        if frame.is_empty() {
            return false;
        }
        let floor = match self.mode {
            Mode::Quality => 0.01,
            Mode::LowBitrate => 0.02,
            Mode::Aggressive => 0.03,
            Mode::VeryAggressive => 0.05,
        };
        let power = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
        power.sqrt() > floor
    }

    pub fn reset(&mut self) {}
}

#[cfg(target_arch = "wasm32")]
impl Default for Vad {
    fn default() -> Self {
        Vad::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_not_voice() {
        let mut vad = Vad::new();
        assert!(!vad.is_voice(&vec![0.0f32; 160]));
    }

    #[test]
    fn the_vad_can_be_moved_to_another_thread() {
        // The `Send` bound is the point of the unsafe impl; this makes it a
        // test rather than a comment.
        let vad = Vad::new();
        let handle = std::thread::spawn(move || {
            let mut vad = vad;
            vad.is_voice(&vec![0.0f32; 160])
        });
        assert!(!handle.join().unwrap());
    }
}
