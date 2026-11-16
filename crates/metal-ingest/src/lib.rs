//! The parts of the ingest that are not about two camera angles.
//!
//! This crate was a binary and nothing else: two videos of one take in, a shot
//! bundle out. [`video`] is the half of it that has no opinion about takes —
//! `ffprobe` and `ffmpeg`, wrapped just enough — and `metal-reel` needs exactly
//! that half to index a folder of footage.
//!
//! So it is a library now, with the shot-building left in `main.rs` where it
//! was. The alternative was a second ffprobe wrapper in the workspace, and the
//! first one already carries a measured 20 ms lesson in its header
//! ([`video::extract`]) that a fresh copy would not have.
//!
//! `sync` stays private to the binary: audio alignment is about pairing angles,
//! and nothing else here has two of anything.
//!
//! [`camera`] is the third leg, added for H.5a: live capture from a v4l2
//! device rather than a file already on disk. It sits beside [`video`]
//! rather than inside it because it produces an [`metal_sense::Image`], not a
//! file on disk — [`video`]'s whole shape is "point ffmpeg at a path", and a
//! camera is a device node feeding a pixel buffer, not a take to index.

pub mod audio;
pub mod camera;
pub mod export;
pub mod video;
