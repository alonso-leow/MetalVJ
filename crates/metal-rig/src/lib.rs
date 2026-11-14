//! Core motion representation: skeleton topology, the canonical full-body rig,
//! and pose/clip sampling. Was `koreo-core` until 2026-08-17.
//!
//! Everything here is **rotations**. A pose is a local rotation per joint plus
//! one root translation; a phrase is a curve of rotations per joint; a tick is
//! a list of rotations by node. There is no position notation, no chain solver,
//! and no given/carried/silent role — all three were deleted on 2026-08-15, and
//! `docs/CORRECTIONS.md` records what they cost while they were here.
//!
//! `stance` is gone with them. A [`Stance`] was four declared limb terminals
//! and a `[*, 0]` marker meaning *everything else carried* — the role system in
//! its purest form, and unresolvable without the solver that read it. A held
//! shape is now what it always should have been: a [`Frame`].

pub mod contact;
pub mod curve;
pub mod figure;
pub mod frame;
pub mod logic;
pub mod motion;
pub mod node;
pub mod pose;
pub mod region;
pub mod rig;
pub mod skeleton;
pub mod skin;

pub use contact::{Contact, Surface};
pub use curve::{Blend, Curve, EaseKind, Interp, Key};
pub use figure::Figure;
pub use frame::{tensor_width, Frame, Track as FrameTrack};
pub use motion::{Motion, Phrase, KEY_RATE};
pub use node::{NodeId, Nodes};
pub use pose::{from_rotvec, to_rotvec, Clip, Pose, Track};
pub use region::BodyRegion;
pub use skeleton::{mirrored_name, Joint, JointId, Mirror, Side, Skeleton};
pub use skin::{skin_point, skin_rotation, Binding, Skin};
