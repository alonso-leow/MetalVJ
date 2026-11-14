//! The `std::fs` subset the engine uses, behind one door.
//!
//! Native builds re-export `std::fs`, byte for byte. The browser build
//! (`wasm32-unknown-unknown`, and any build with the `mem` feature) uses an
//! in-memory tree instead: `wasm32` has no file system, and `std::fs` there
//! returns errors, so every read of a bundled file would silently fail.
//!
//! The tree is seeded with [`mount`] (bundled files), and every write or
//! removal is reported to the hook installed with [`set_hook`], which is how
//! the browser keeps a user's edits across a reload (`localStorage`).
//!
//! Callers name the functions the way they name `std::fs`: `metal_vfs::read`,
//! `metal_vfs::write`, `metal_vfs::File::open`. `std::path::Path` methods that touch
//! the disk (`is_file`, `is_dir`, `exists`, `canonicalize`) are here too, as
//! [`is_file`], [`is_dir`], [`exists`] and [`canonicalize`], because on the
//! browser they would not see the tree.

pub mod mem;

#[cfg(any(target_arch = "wasm32", feature = "mem"))]
pub use mem::*;

#[cfg(not(any(target_arch = "wasm32", feature = "mem")))]
mod native;
#[cfg(not(any(target_arch = "wasm32", feature = "mem")))]
pub use native::*;

// Seeding and the write hook are meaningful only for the in-memory tree; on
// native they still exist (and do nothing visible) so callers compile the same.
pub use mem::{mount, set_hook};
