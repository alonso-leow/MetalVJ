//! The two media effects of "The next leap" (2026-09-27): `fx.particles` and
//! `fx.image-pop`.
//!
//! Both are `Effect`-family nodes that work in [`crate::paint::Paint::run`]'s
//! `fx.*` branch rather than inside a generator's post pass — like
//! [`crate::motion::FX_MOVE_KIND`], they act on the *picture* the node is
//! handed, and their parameters are read off the node's own config keys rather
//! than baked into a generator's row. Neither has a `Post` field to land an
//! amount in (`metal_schema::effect_index` returns `None`), so
//! [`crate::resolved_chain`] skips them exactly as it skips `fx.move`.
//!
//! # `fx.particles` — a stateless shader (decision 6)
//!
//! Dust is hashed points from `(seed, i, t)` with drift, jitter and twinkle.
//! There is **no simulation buffer**: the picture at time `t` is a pure
//! function of the ingest and the clock, so the effect survives fan-out and
//! scrubbing and is deterministic for the goldens. On a host with an adapter
//! [`particles::PARTICLES_SHADER`] runs the hashed points in WGSL; the CPU
//! [`particles::dust`] is the stand-in's honest version, the same pairing
//! `fx.move`'s `move.wgsl` and `motion::translate` have.
//!
//! # `fx.image-pop` — reel media over the picture (decision 5)
//!
//! Media (stills and short clips) live in a reel as they do today
//! (`data/reels/<name>/`). The effect draws up to `concurrent` instances at
//! once, each with a position, scale, life and fade, inside the mask like any
//! `fx.*`. A [`image_pop::Atlas`] decodes at most 32 rungs of the reel and
//! cycles them round-robin; unwired, the effect therefore cycles the reel
//! rather than going broken.
//!
//! The atlas is composited on the CPU. The reason is the `fx.forge` effect
//! contract (`metal_render::models::forge`): an effect shader is handed exactly
//! one texture — the picture it is applied to — and there is no second binding
//! an atlas could arrive on. `data/shaders/image_pop.wgsl` is the shader that
//! contract would run once it grows one; today the live path is
//! [`image_pop::compose`], which both painters share so the CPU and GPU builds
//! cannot drift.

pub mod flash;
pub mod image_pop;
pub mod parity;
pub mod particles;
pub mod warp;

/// The lowbias32 finalizer (`mix13`), used by both effects so a point or an
/// instance is a pure function of `(seed, index)` — the property decision 6
/// asks for and the goldens rest on.
pub(crate) fn mix32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// `mix32` of `(seed, i, salt)` as a `0..1` float. `salt` is what gives one
/// instance several independent draws (x, y, phase, size) without a second
/// seed.
pub(crate) fn unit(seed: u32, i: u32, salt: u32) -> f32 {
    let mixed = mix32(seed ^ mix32(i.wrapping_mul(0x9e37_79b9).wrapping_add(salt)));
    mixed as f32 / u32::MAX as f32
}

/// `1 - |x|`, the cheap triangular falloff both effects use for a soft point.
pub(crate) fn soft(x: f32) -> f32 {
    (1.0 - x.abs()).clamp(0.0, 1.0)
}
