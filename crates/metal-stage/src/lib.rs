//! The stage hitbox: a coarse grid of named points the fixture AI speaks in.
//!
//! - [`grid`]: levels, rows, columns, points, edges and default orientation.
//! - [`names`]: the float names `<level>.<row>.<col>.<type>.<channel>`.
//! - [`patch`]: a real patch → placements → the matrix float → fixture outputs.
//! - [`seq`]: sparse keyed sequence state and its per-fixture lanes.
//! - [`dataset`]: audio ticks → authoring examples, validation, status.
//! - [`model`]: the `ai.stage` ridge map from the music row to a patch's stage floats.

pub mod dataset;
pub mod grid;
pub mod model;
pub mod names;
pub mod patch;
pub mod seq;

pub use grid::{Col, Edge, Frame, Level, Point, Row};
pub use names::{Channel, FloatName, Kind};
pub use model::StageModel;
pub use patch::{Matrix, Patch, PatchFixture, Placement};
