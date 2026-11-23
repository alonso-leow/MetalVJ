//! `S.U3`: the adapter/backend probe. One test forces a backend selection
//! that cannot succeed on any hardware (`wgpu::Backends::empty()`) and
//! asserts `Backend::Glow` — deterministic, not gated on this machine's
//! GPU, and not a test that vacuously skips (rule 10 of `plan-sonnet.md`).
//! The other asserts `Backend::Wgpu` against the real primary backends,
//! which is the same adapter path every other test in this crate already
//! depends on via `Gpu::new_headless`.

use metal_render::gpu::{probe, Backend};

#[test]
fn probe_forced_empty_backends_reports_glow() {
    assert_eq!(probe(wgpu::Backends::empty()), Backend::Glow);
}

#[test]
fn probe_primary_backends_reports_wgpu_on_this_machine() {
    assert_eq!(probe(wgpu::Backends::PRIMARY), Backend::Wgpu);
}
