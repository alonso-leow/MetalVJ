//! **Transmissions** — the models that do not draw a picture, they modify one.
//!
//! ```text
//!   a folder of videos                 the music
//!         │                                │
//!    index::build                     the digest
//!    ladder + descriptors             (16 columns, 8 Hz)
//!         │                            │        │
//!         │      ┌─────────────────────┘        │
//!         ▼      ▼                              │
//!    curate::Curator  ── which clip, and where in it
//!         │                                     │
//!         │  a Cue, held across a passage       ▼
//!         │                            grade::Colourist ──► a float row
//!         │                                                       │
//!         ▼                                                       ▼
//!    the rungs on screen ───────────────────────────► metal_render::Model::render
//! ```
//!
//! **Only one arrow crosses from the footage into a model, and it points at the
//! Curator.** That was not true until 2026-08-25: the Colourist read the frame
//! on screen as well, which made the row a function of the picture it was about
//! to be applied to. [`grade`]'s header has why that went.
//!
//! # What is new here, in one sentence
//!
//! Thirty renderers turn a row of floats into a frame; a transmission turns a
//! row of floats into *a modification of a frame that already exists*. The
//! contract downstream is unchanged — it is still `Schema` in, pixels out — and
//! that is deliberate: a transmission sits in the same registry, on the same
//! lane, under the same Router, and `metal-dash` selects it with the same
//! `/select`.
//!
//! # Two models, because there are two questions
//!
//! > *"for the new kind of transmissions we use 2 models, one to choose the
//! > source and the second to combine image + music to apply custom params to
//! > the final effect."*
//!
//! | | | input | |
//! |---|---|---|---|
//! | [`curate`] | **the Curator** | music **+ the library** | which footage, out of an open set |
//! | [`grade`] | **the Colourist** | music | how it is graded |
//!
//! They are named after the two people who would do these jobs, and the names
//! are load-bearing: a curator picks from what exists and a colourist works on
//! what they are handed. Neither could do the other's job and neither model can
//! either.
//!
//! **The Curator is the only model in the suite that reads anything but the
//! music, and that is now the whole difference between the two.** It has to:
//! choosing from an open set means comparing what the music wants against what
//! is in the folder, and there is no way to do that without looking in the
//! folder. Nothing else here has that excuse, and the Colourist's attempt at one
//! was withdrawn.
//!
//! # The Curator is the Router one level down, and it is a different kind of
//! model for one reason
//!
//! `metal_router` picks one of a **closed set** — thirty renderers, known when the
//! binary is built — so it is a multinomial logistic regression over classes,
//! and an animation nobody authored for scores low everywhere.
//!
//! A library is not a closed set. It is whatever folder an operator dropped on
//! the laptop this afternoon, and the classes change per venue. A classifier
//! over clips would have to be retrained before the first clip could play,
//! which is a system that does not work on the night it is needed.
//!
//! So the Curator learns the **relationship**, not the classes: music → what
//! kind of image this music wants, in [`read`]'s twelve columns, and then the
//! nearest passage in whatever library is loaded wins. A folder indexed five
//! minutes ago is usable by a model trained on a different folder last month.
//! That property is the whole reason for the shape.
//!
//! # The Colourist replaces one function and inherits every guarantee
//!
//! `metal-dash::drive::row` maps the digest onto any model's floats by assigning
//! each float a digest column round-robin (`column_for`) — by *counting*,
//! knowing nothing about what the float means. Everything else it does —
//! modulating *around the operator's row* rather than sweeping the declared
//! range, damping structural floats by `FloatSpec::drive`, clamping to the
//! range, never touching a camera — is safety that was each a bug once.
//!
//! [`grade::row`] changes **only the assignment**. The excursion a float gets is
//! still `SWING` of its range at most, still scaled by its own `drive` weight,
//! still clamped. A Colourist that has learnt something absurd can therefore
//! only ever produce a row a slider could have produced, which is the property
//! that lets an untrained default ship.
//!
//! # Nothing here has a GPU in it
//!
//! Same rule as `metal-router` and for the same reason: this runs at a venue inside
//! `metal-dash`, and `metal-dash` has a build with no adapter at all. The renderer
//! that consumes a `Cue` is in `metal-render`; the deciding is here.

pub mod curate;
pub mod grade;
pub mod hap;
pub mod index;
pub mod clip;
pub mod read;
pub mod scenes;

pub use curate::{Curator, Cue};
pub use grade::Colourist;
pub use index::{Clip, Library, Passage};

/// The footage half of a transmission: the seam between the app, which decides
/// what is on, and the renderer, which draws it.
///
/// Deliberately small — a library and a cue, and nothing else. Everything about
/// *where in the footage* a frame comes from is arithmetic on the cue and the
/// clock, done inside the model (`metal_render::models::estela::playhead`), so
/// nothing here is a playhead that could be advanced or a state that could be
/// out of sync between two screens of one venue. Both screens are handed the
/// same cue and read the same clock, and that is the whole synchronisation.
///
/// **It lives in this crate rather than in `metal-render`** because `metal-dash`'s
/// `Paint` trait has to name it on both builds, and the build with no adapter
/// has no `metal-render` in its tree at all — the same reason `Schema` was lifted
/// out into `metal-schema`.
pub trait Reel {
    /// Point the model at an indexed library, or at none.
    ///
    /// Detaching must throw away anything decoded from the old one: two folders
    /// can hold the same filename, and a cached frame surviving the swap is a
    /// frame of the wrong footage on somebody's wall.
    fn attach(&mut self, library: Option<std::sync::Arc<Library>>);

    /// The name of the attached library, for the page to print.
    fn attached(&self) -> Option<&str>;

    /// What the Curator decided. `None` renders the model's own slate, which
    /// says on its face that there is no footage.
    fn cue(&mut self, cue: Option<Cue>);

    fn cued(&self) -> Option<&Cue>;
}

/// The musical digest's width — the prefix the corpus, the Colourist and the
/// Curator are defined on. Declared once in `metal_schema::MUSIC`; the voice
/// block that follows it is not this crate's business.
pub const MUSIC: usize = metal_schema::MUSIC;
