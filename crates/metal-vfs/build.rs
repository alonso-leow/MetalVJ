// The browser tree seeds every bundled file's modification time with this
// stamp (`mem::BUILD_STAMP`), so a rebuild of the sources changes it and
// anything that caches by (length, mtime), such as metal-reel's index, sees the
// new files as new. Seconds since the epoch at compile time.
fn main() {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    println!("cargo:rustc-env=FW_VFS_BUILD_STAMP={secs}");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
}
