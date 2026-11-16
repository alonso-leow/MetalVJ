//! The facial rig of the singing monsters: controls, the shared face
//! topology, species, deformation, visemes and coarticulation.
//!
//! # What this crate is, and what it is not
//!
//! `the voice design notes` decisions 8–11 describe a face that sings, raps, speaks and
//! whispers along with the voice in the music. Two programs need the same
//! rig to do that: the **face solver** in metal-perform, which turns the voice
//! columns into control values every tick, and the **`monster` model** in
//! metal-render, a raymarched SDF head whose shader draws the posed landmarks.
//! This crate is the part they share, and only that part: pure CPU maths,
//! no GPU, no audio, no I/O, no engine. Estimating visemes from audio, the
//! upper-face "secondary motion" mapping (decision 9) and the engine hook
//! (decision 11) live in metal-perform; the WGSL lives in metal-render.
//!
//! It is also **not** `metal-rig`. That crate is a 36-joint body skeleton made of
//! rotations; a face is not a skeleton. The jaw is the only real joint here.
//! Lips, lids, cheeks and the tongue are shapes, so the rig is landmarks moved
//! by named controls — the way ARKit's blendshapes or a FACS rig work — and
//! nothing from metal-rig applies.
//!
//! # The layers, bottom up
//!
//! - [`control`]: the 23 speech controls (decision 8) plus the appended
//!   expression muscles (FACS, per side), their ranges, rest values and
//!   springs, and [`Controls`], a value of all of them. The control names
//!   become model float names, so they are append-only (plan rule 2).
//! - [`topology`]: the landmarks every species shares (decision 10) — jaw,
//!   lip ring, cavity, teeth, a 4-capsule tongue, eyes and lids, brows,
//!   nostrils, cheeks — in a head-local frame.
//! - [`species`]: the five species as proportions. A species moves and scales
//!   landmarks; it never adds, removes or renames one.
//! - [`mod@pose`]: the rig proper. Controls move the landmarks, and
//!   [`Posed::pack`] flattens the result into `vec4`s in a documented order a
//!   WGSL struct mirrors.
//! - [`expression`]: whole-face gestures (rage, terror, sneer, wink, smirk,
//!   surprise, joy, grief) as control presets.
//! - [`viseme`]: the fourteen mouth shapes as control poses, and [`blend`].
//! - [`springs`]: per-control critically damped springs (coarticulation) and
//!   the singing [`Style`] that bends them.
//!
//! A frame of the face, end to end, is: viseme weights → [`blend`] → the
//! solver's secondary motion on the upper face → [`Springs::step`] →
//! [`pose()`] → [`Posed::pack`] → the shader.

pub mod control;
pub mod expression;
pub mod pose;
pub mod species;
pub mod springs;
pub mod topology;
pub mod viseme;

pub use control::{ControlSpec, Controls, FaceControl, Spring};
pub use expression::{Expression, EXPRESSIONS};
pub use pose::{pose, Posed, LAYOUT, PACKED, VEC4S};
pub use species::{Kind, Species};
pub use springs::{Springs, Style};
pub use topology::{Ball, Capsule, Ellipsoid, Eye, Topology};
pub use viseme::{blend, Viseme};

#[cfg(test)]
mod tests;
