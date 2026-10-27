//! Native: `std::fs`, unchanged.

use std::path::Path;

pub use std::fs::{
    canonicalize, copy, create_dir, create_dir_all, metadata, read, read_dir, read_to_string,
    remove_dir, remove_dir_all, remove_file, rename, set_permissions, write, DirEntry, File,
    FileType, Metadata, OpenOptions, Permissions, ReadDir,
};

pub fn exists<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref().exists()
}

pub fn is_file<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref().is_file()
}

pub fn is_dir<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref().is_dir()
}
