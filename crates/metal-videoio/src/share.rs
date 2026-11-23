//! P-C: the platform-independent half of the GPU-share senders (Spout on
//! Windows, Syphon on macOS).
//!
//! Both publish a frame to other programs on the same machine under a name.
//! The shared part is the bit that can be tested on any host: how the name is
//! cleaned, when a sender has to be (re)created (first frame, a size change —
//! resolution follow), when it is released (stop, drop, a failed publish), and
//! what the status read says. A platform binds [`ShareBackend`] to its API;
//! the frame goes through the same [`Share`] state machine either way.
//!
//! The frame arrives as RGBA bytes (the pump reads the device's texture back
//! through `AsyncReadback`, one frame late). Each backend uploads those bytes
//! into its own shared surface: a Spout sender copies them into its DX11
//! texture, a Syphon server into an `MTLTexture` it publishes. Zero-copy
//! interop from the wgpu texture is not reached yet.

use crate::Sink;

/// The name used when the target and the screen label are both empty.
pub const DEFAULT_NAME: &str = "MetalVJ";
/// Spout caps a sender name at 256 characters; Syphon takes the same cut.
pub const NAME_MAX_CHARS: usize = 256;

/// The name a sender publishes under: `target` when set, else the screen's
/// `label`. Control characters are dropped, whitespace runs collapse to one
/// space, the result is trimmed and cut to [`NAME_MAX_CHARS`] characters.
pub fn sender_name(target: &str, label: &str) -> String {
    let pick = if target.trim().is_empty() { label } else { target };
    let cleaned: String = pick.chars().filter(|c| !c.is_control()).collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = collapsed.chars().take(NAME_MAX_CHARS).collect();
    if cut.is_empty() {
        DEFAULT_NAME.to_string()
    } else {
        cut
    }
}

/// What a platform does with a published sender. Errors are the operator's
/// sentence (shown in `GET /videoio`).
pub trait ShareBackend: Send {
    /// Creates the sender `name` for `w × h` RGBA frames.
    fn create(&mut self, name: &str, w: u32, h: u32) -> Result<(), String>;
    /// Uploads one `w × h` RGBA frame and publishes it.
    fn publish(&mut self, rgba: &[u8], w: u32, h: u32) -> Result<(), String>;
    /// Tears the sender down. Never fails; called on stop, resize and drop.
    fn release(&mut self);
}

/// Where the sender is in its life.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareState {
    /// Nothing published; the next frame creates the sender.
    Stopped,
    /// Publishing `width × height` frames under the name.
    Live { width: u32, height: u32 },
}

/// One named sender over a [`ShareBackend`].
pub struct Share<B: ShareBackend> {
    backend: B,
    name: String,
    state: ShareState,
    frames: u64,
}

impl<B: ShareBackend> Share<B> {
    pub fn new(name: impl Into<String>, backend: B) -> Share<B> {
        Share { backend, name: name.into(), state: ShareState::Stopped, frames: 0 }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn state(&self) -> &ShareState {
        &self.state
    }

    /// Frames published since the sender was created.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Publishes one frame. The sender is created on the first frame and
    /// again whenever the size changes (the old one is released first). A
    /// failed create leaves the sender stopped; a failed publish releases it,
    /// so the next frame starts clean.
    pub fn push_frame(&mut self, rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
        let want = w as usize * h as usize * 4;
        if w == 0 || h == 0 || rgba.len() != want {
            return Err(format!("cuadro de {}×{} con {} bytes (se esperaban {want})", w, h, rgba.len()));
        }
        let fits = self.state == ShareState::Live { width: w, height: h };
        if !fits {
            self.stop();
            self.backend.create(&self.name, w, h)?;
            self.state = ShareState::Live { width: w, height: h };
        }
        if let Err(e) = self.backend.publish(rgba, w, h) {
            self.stop();
            return Err(e);
        }
        self.frames += 1;
        Ok(())
    }

    /// Releases the sender if it is live. Idempotent.
    pub fn stop(&mut self) {
        if self.state != ShareState::Stopped {
            self.backend.release();
            self.state = ShareState::Stopped;
        }
    }

    /// One line for the status read: name, size, frame count.
    pub fn describe_state(&self) -> String {
        match self.state {
            ShareState::Stopped => format!("«{}» parado", self.name),
            ShareState::Live { width, height } => {
                format!("«{}» {}×{} · {} fotogramas", self.name, width, height, self.frames)
            }
        }
    }
}

impl<B: ShareBackend> Sink for Share<B> {
    fn push(&mut self, rgba: &[u8], w: u32, h: u32, _t: f64) -> Result<(), String> {
        self.push_frame(rgba, w, h)
    }

    fn describe(&self) -> String {
        self.describe_state()
    }
}

impl<B: ShareBackend> Drop for Share<B> {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
        fail_create: bool,
        fail_publish: bool,
        releases: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ShareBackend for Fake {
        fn create(&mut self, name: &str, w: u32, h: u32) -> Result<(), String> {
            self.log.push(format!("create {name} {w}x{h}"));
            if self.fail_create {
                Err("no hay driver".into())
            } else {
                Ok(())
            }
        }
        fn publish(&mut self, _rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
            self.log.push(format!("publish {w}x{h}"));
            if self.fail_publish {
                Err("textura perdida".into())
            } else {
                Ok(())
            }
        }
        fn release(&mut self) {
            self.log.push("release".into());
            self.releases.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn frame(w: u32, h: u32) -> Vec<u8> {
        vec![0; w as usize * h as usize * 4]
    }

    #[test]
    fn names_come_from_the_target_then_the_label_and_are_cleaned() {
        assert_eq!(sender_name("Salida  principal", "Pantalla 1"), "Salida principal");
        assert_eq!(sender_name("  ", "Pantalla 1"), "Pantalla 1");
        assert_eq!(sender_name("", ""), DEFAULT_NAME);
        assert_eq!(sender_name("a\u{7}b", ""), "ab");
        assert_eq!(sender_name(" a  b\n c ", ""), "a b c");
        let long = "x".repeat(400);
        assert_eq!(sender_name(&long, "").chars().count(), NAME_MAX_CHARS);
    }

    #[test]
    fn the_first_frame_creates_then_later_frames_only_publish() {
        let mut s = Share::new("MetalVJ", Fake::default());
        s.push_frame(&frame(4, 2), 4, 2).unwrap();
        s.push_frame(&frame(4, 2), 4, 2).unwrap();
        assert_eq!(s.backend.log, ["create MetalVJ 4x2", "publish 4x2", "publish 4x2"]);
        assert_eq!(s.state(), &ShareState::Live { width: 4, height: 2 });
        assert_eq!(s.frames(), 2);
    }

    #[test]
    fn a_size_change_releases_and_recreates_the_sender() {
        let mut s = Share::new("out", Fake::default());
        s.push_frame(&frame(4, 2), 4, 2).unwrap();
        s.push_frame(&frame(8, 4), 8, 4).unwrap();
        assert_eq!(
            s.backend.log,
            ["create out 4x2", "publish 4x2", "release", "create out 8x4", "publish 8x4"]
        );
    }

    #[test]
    fn stop_releases_once_and_the_next_frame_starts_again() {
        let mut s = Share::new("out", Fake::default());
        s.stop();
        assert!(s.backend.log.is_empty(), "stopping a stopped sender does nothing");
        s.push_frame(&frame(2, 2), 2, 2).unwrap();
        s.stop();
        s.stop();
        assert_eq!(s.state(), &ShareState::Stopped);
        s.push_frame(&frame(2, 2), 2, 2).unwrap();
        assert_eq!(s.backend.log.iter().filter(|l| *l == "release").count(), 1);
        assert_eq!(s.backend.log.iter().filter(|l| l.starts_with("create")).count(), 2);
    }

    #[test]
    fn a_failed_create_stays_stopped_and_says_why() {
        let mut s = Share::new("out", Fake { fail_create: true, ..Fake::default() });
        assert_eq!(s.push_frame(&frame(2, 2), 2, 2), Err("no hay driver".into()));
        assert_eq!(s.state(), &ShareState::Stopped);
        assert_eq!(s.backend.log, ["create out 2x2"], "no publish without a sender");
    }

    #[test]
    fn a_failed_publish_releases_so_the_next_frame_starts_clean() {
        let mut s = Share::new("out", Fake { fail_publish: true, ..Fake::default() });
        assert_eq!(s.push_frame(&frame(2, 2), 2, 2), Err("textura perdida".into()));
        assert_eq!(s.state(), &ShareState::Stopped);
        assert_eq!(s.backend.log, ["create out 2x2", "publish 2x2", "release"]);
        assert_eq!(s.frames(), 0);
    }

    #[test]
    fn a_frame_of_the_wrong_length_is_refused_before_the_backend() {
        let mut s = Share::new("out", Fake::default());
        assert!(s.push_frame(&frame(2, 2)[..10], 2, 2).is_err());
        assert!(s.push_frame(&[], 0, 0).is_err());
        assert!(s.backend.log.is_empty());
    }

    #[test]
    fn dropping_a_live_sender_releases_it_and_a_stopped_one_does_not() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let live = Arc::new(AtomicUsize::new(0));
        let mut s = Share::new("out", Fake { releases: Arc::clone(&live), ..Fake::default() });
        s.push_frame(&frame(2, 2), 2, 2).unwrap();
        drop(s);
        assert_eq!(live.load(Ordering::SeqCst), 1);
        let idle = Arc::new(AtomicUsize::new(0));
        drop(Share::new("out", Fake { releases: Arc::clone(&idle), ..Fake::default() }));
        assert_eq!(idle.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_status_line_names_the_sender_and_its_state() {
        let mut s = Share::new("Salida", Fake::default());
        assert_eq!(s.describe_state(), "«Salida» parado");
        s.push_frame(&frame(16, 9), 16, 9).unwrap();
        assert_eq!(s.describe_state(), "«Salida» 16×9 · 1 fotogramas");
    }
}
