//! G1 — which half of the engine composes: the GPU or the CPU.
//!
//! Chosen **once**, at startup, from what the adapter can do, and overridable
//! with `METALVJ_BACKEND=cpu|gpu`. Never per frame: the old per-frame
//! competition (`paint::Pick`) paid an upload and a readback to find out which
//! path was faster. The CPU backend is the code that always existed
//! ([`crate::compose`], [`crate::transition`], [`crate::text`]); it stays the
//! fallback for peers, mobile, headless and wasm, and the reference the GPU
//! compose is diffed against.

use std::sync::atomic::{AtomicU8, Ordering};

/// Where the frame is composed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Models draw into textures, compose runs in a shader on the same
    /// device, and pixels are read back only at the sinks that need them.
    Gpu,
    /// Models are read back and everything after them runs on the CPU.
    Cpu,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Gpu => "gpu",
            Backend::Cpu => "cpu",
        }
    }

    pub fn parse(s: &str) -> Option<Backend> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gpu" => Some(Backend::Gpu),
            "cpu" => Some(Backend::Cpu),
            _ => None,
        }
    }

    /// `METALVJ_BACKEND`, when set to something this build understands.
    pub fn from_env() -> Option<Backend> {
        std::env::var("METALVJ_BACKEND").ok().as_deref().and_then(Backend::parse)
    }

    /// The startup decision. The override wins; otherwise any adapter at all
    /// (software, downlevel, GL ES, WebGL2 included) is `Gpu`, and only the
    /// absence of one is `Cpu`. Missing features degrade inside the GPU path.
    pub fn choose(over: Option<Backend>, has_adapter: bool) -> Backend {
        over.unwrap_or(if has_adapter { Backend::Gpu } else { Backend::Cpu })
    }
}

const UNSET: u8 = 0;
static ACTIVE: AtomicU8 = AtomicU8::new(UNSET);

/// Records the backend the painter chose, for `GET /perf` and logs.
pub fn set_active(b: Backend) {
    ACTIVE.store(if b == Backend::Gpu { 1 } else { 2 }, Ordering::Relaxed);
}

/// The backend the running painter chose; `None` before one exists.
pub fn active() -> Option<Backend> {
    match ACTIVE.load(Ordering::Relaxed) {
        1 => Some(Backend::Gpu),
        2 => Some(Backend::Cpu),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_wins_and_a_software_adapter_is_cpu() {
        assert_eq!(Backend::choose(None, true), Backend::Gpu);
        assert_eq!(Backend::choose(None, false), Backend::Cpu);
        // a software adapter is still an adapter
        assert_eq!(Backend::choose(None, true), Backend::Gpu);
        assert_eq!(Backend::choose(Some(Backend::Cpu), true), Backend::Cpu);
        assert_eq!(Backend::choose(Some(Backend::Gpu), false), Backend::Gpu);
        assert_eq!(Backend::parse(" GPU "), Some(Backend::Gpu));
        assert_eq!(Backend::parse("x"), None);
    }
}
