//! The paint surface, re-exported from `metal-perform` (LAY.9).
//!
//! `Paint`, `RunOutput`, `painter()`, `encode_jpeg`, `Gpu` and `Stand` moved
//! into `metal-perform` when the execute → output half of the engine did. This
//! module stays so every `crate::paint::…` path in the app keeps reading the
//! same way it always did, and so the choice of painter remains the one line
//! in `main` that prints what it is.

pub use metal_perform::paint::*;
