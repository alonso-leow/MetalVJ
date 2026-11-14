//! Visual notation for how far a node turns: colour now, staff glyphs later.
//! Rewritten 2026-08-15 when the given/carried/silent roles were deleted — see
//! the `color` module docs.

pub mod color;
pub mod tone;

pub use color::{legend, turn_color, Oklch, Rgb, FULL, NEUTRAL};
pub use tone::{tone_gradient, Palette};
