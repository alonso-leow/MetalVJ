//! The in-memory tree: what the browser build (and `--features mem`) reads and
//! writes instead of a disk. Behaves like `std::fs` on the calls the engine
//! makes: parents must exist, `read` of a missing file is `NotFound`, and so on.
//!
//! Paths are normalised lexically (no symlinks, no cwd): `a/../b` is `b`, `.`
//! is dropped, and an absolute `/x` is a different key from a relative `x`.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use web_time::{SystemTime, UNIX_EPOCH};

/// The modification time a file has until something writes it: the moment
/// this crate was compiled (`build.rs`). Bundled files all carry it, so a new
/// build reads as new files to any cache keyed on the time.
pub fn build_stamp() -> SystemTime {
    let secs = env!("FW_VFS_BUILD_STAMP").parse::<u64>().unwrap_or(0);
    UNIX_EPOCH + Duration::from_secs(secs)
}

#[derive(Default)]
struct Tree {
    files: BTreeMap<String, Vec<u8>>,
    /// Last write time of each file that has one; a file with no entry reads
    /// as [`build_stamp`]. Kept beside `files` so the bytes stay plain.
    mtimes: BTreeMap<String, SystemTime>,
    /// Every directory that exists, by key. The empty key (`""`, the relative
    /// root) and `"/"` are implicit.
    dirs: BTreeSet<String>,
}

type Hook = Box<dyn FnMut(&str, Option<&[u8]>) + Send>;

static TREE: Mutex<Tree> = Mutex::new(Tree {
    files: BTreeMap::new(),
    mtimes: BTreeMap::new(),
    dirs: BTreeSet::new(),
});

static HOOK: Mutex<Option<Hook>> = Mutex::new(None);

fn tree() -> std::sync::MutexGuard<'static, Tree> {
    TREE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reports a change to the hook: `Some(bytes)` for a write, `None` for a
/// removal. Called after the tree lock is released.
fn notify(key: &str, bytes: Option<&[u8]>) {
    let mut hook = HOOK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = hook.as_mut() {
        f(key, bytes);
    }
}

/// Installs the function that hears every write (`Some(bytes)`) and removal
/// (`None`) with its normalised key. The browser build uses this to mirror
/// the tree to `localStorage`. Replaces any earlier hook.
pub fn set_hook(hook: impl FnMut(&str, Option<&[u8]>) + Send + 'static) {
    *HOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(hook));
}

/// Normalises a path to its tree key: components joined by `/`, a leading `/`
/// kept for absolute paths, `.` dropped, `..` resolved where it can be.
pub fn key(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let mut segs: Vec<String> = Vec::new();
    let mut absolute = false;
    for c in path.components() {
        match c {
            Component::RootDir => absolute = true,
            Component::Prefix(_) | Component::CurDir => {}
            Component::ParentDir => {
                if segs.last().is_some_and(|s| s != "..") {
                    segs.pop();
                } else if !absolute {
                    segs.push("..".into());
                }
            }
            Component::Normal(s) => segs.push(s.to_string_lossy().into_owned()),
        }
    }
    let joined = segs.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// The key of a file's parent directory, or `None` at the root.
fn parent(key: &str) -> Option<String> {
    if key.is_empty() || key == "/" {
        return None;
    }
    match key.rfind('/') {
        Some(0) => Some("/".into()),
        Some(i) => Some(key[..i].to_string()),
        None => Some(String::new()),
    }
}

/// Every ancestor directory of `key`, outermost first, not including `key`.
fn ancestors(key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = parent(key);
    while let Some(p) = cur {
        cur = parent(&p);
        out.push(p);
    }
    out.reverse();
    out.retain(|p| !p.is_empty() && p != "/");
    out
}

fn is_root(key: &str) -> bool {
    key.is_empty() || key == "/"
}

fn is_dir_in(t: &Tree, key: &str) -> bool {
    is_root(key) || t.dirs.contains(key)
}

fn not_found(key: &str) -> io::Error {
    io::Error::new(ErrorKind::NotFound, format!("{key}: no such file or directory"))
}

fn parent_must_exist(t: &Tree, key: &str) -> io::Result<()> {
    match parent(key) {
        Some(p) if !is_dir_in(t, &p) => Err(not_found(key)),
        _ => Ok(()),
    }
}

/// Seeds the tree with `files`, each `(path, bytes)` placed under `prefix`
/// (directories made as needed). Not a write: the hook does not hear it.
/// A later mount or write of the same path replaces the bytes.
pub fn mount<'a, I>(prefix: impl AsRef<Path>, files: I)
where
    I: IntoIterator<Item = (&'a str, &'a [u8])>,
{
    let base = key(prefix);
    let mut t = tree();
    if !is_root(&base) {
        for a in ancestors(&base) {
            t.dirs.insert(a);
        }
        t.dirs.insert(base.clone());
    }
    for (rel, bytes) in files {
        let full = if base.is_empty() {
            key(rel)
        } else {
            format!("{base}/{}", key(rel))
        };
        for a in ancestors(&full) {
            t.dirs.insert(a);
        }
        t.mtimes.insert(full.clone(), build_stamp());
        t.files.insert(full, bytes.to_vec());
    }
}

/// Every file under the tree as `(key, bytes)`, in key order. Used by tests
/// and by a host that wants to persist the whole tree at once.
pub fn snapshot() -> Vec<(String, Vec<u8>)> {
    tree().files.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

// ---- functions, named as in `std::fs` ----

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let k = key(path);
    let t = tree();
    match t.files.get(&k) {
        Some(b) => Ok(b.clone()),
        None if is_dir_in(&t, &k) => Err(io::Error::new(ErrorKind::IsADirectory, k)),
        None => Err(not_found(&k)),
    }
}

pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let bytes = read(path)?;
    String::from_utf8(bytes).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let k = key(path);
    let bytes = contents.as_ref();
    {
        let mut t = tree();
        parent_must_exist(&t, &k)?;
        if is_dir_in(&t, &k) {
            return Err(io::Error::new(ErrorKind::IsADirectory, k));
        }
        t.files.insert(k.clone(), bytes.to_vec());
        t.mtimes.insert(k.clone(), SystemTime::now());
    }
    notify(&k, Some(bytes));
    Ok(())
}

pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let k = key(path);
    let mut t = tree();
    parent_must_exist(&t, &k)?;
    if is_dir_in(&t, &k) || t.files.contains_key(&k) {
        return Err(io::Error::new(ErrorKind::AlreadyExists, k));
    }
    t.dirs.insert(k);
    Ok(())
}

pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let k = key(path);
    let mut t = tree();
    let mut chain = ancestors(&k);
    chain.push(k.clone());
    for d in chain {
        if d.is_empty() || d == "/" {
            continue;
        }
        if t.files.contains_key(&d) {
            return Err(io::Error::new(ErrorKind::AlreadyExists, d));
        }
        t.dirs.insert(d);
    }
    Ok(())
}

pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let k = key(path);
    {
        let mut t = tree();
        t.mtimes.remove(&k);
        if t.files.remove(&k).is_none() {
            return Err(if is_dir_in(&t, &k) {
                io::Error::new(ErrorKind::IsADirectory, k)
            } else {
                not_found(&k)
            });
        }
    }
    notify(&k, None);
    Ok(())
}

pub fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let k = key(path);
    let mut t = tree();
    if !t.dirs.contains(&k) {
        return Err(not_found(&k));
    }
    let prefix = format!("{k}/");
    if t.dirs.iter().any(|d| d.starts_with(&prefix))
        || t.files.keys().any(|f| f.starts_with(&prefix))
    {
        return Err(io::Error::new(ErrorKind::DirectoryNotEmpty, k));
    }
    t.dirs.remove(&k);
    Ok(())
}

/// Removes a directory and everything under it. Each removed file is reported
/// to the hook.
pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let k = key(path);
    let prefix = format!("{k}/");
    let mut removed = Vec::new();
    {
        let mut t = tree();
        if !t.dirs.contains(&k) {
            return Err(not_found(&k));
        }
        t.dirs.retain(|d| *d != k && !d.starts_with(&prefix));
        let doomed: Vec<String> = t
            .files
            .keys()
            .filter(|f| **f == k || f.starts_with(&prefix))
            .cloned()
            .collect();
        for f in doomed {
            t.files.remove(&f);
            removed.push(f);
        }
    }
    for f in removed {
        notify(&f, None);
    }
    Ok(())
}

pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let (a, b) = (key(from), key(to));
    let mut moved = Vec::new();
    {
        let mut t = tree();
        parent_must_exist(&t, &b)?;
        if let Some(bytes) = t.files.remove(&a) {
            let when = t.mtimes.remove(&a).unwrap_or(build_stamp());
            t.mtimes.insert(b.clone(), when);
            t.files.insert(b.clone(), bytes.clone());
            moved.push((a.clone(), None));
            moved.push((b.clone(), Some(bytes)));
        } else if t.dirs.contains(&a) {
            let prefix = format!("{a}/");
            let dirs: Vec<String> = t
                .dirs
                .iter()
                .filter(|d| **d == a || d.starts_with(&prefix))
                .cloned()
                .collect();
            for d in dirs {
                t.dirs.remove(&d);
                t.dirs.insert(format!("{b}{}", &d[a.len()..]));
            }
            let files: Vec<String> = t.files.keys().filter(|f| f.starts_with(&prefix)).cloned().collect();
            for f in files {
                let bytes = t.files.remove(&f).unwrap_or_default();
                let when = t.mtimes.remove(&f).unwrap_or(build_stamp());
                let nf = format!("{b}{}", &f[a.len()..]);
                moved.push((f, None));
                moved.push((nf.clone(), Some(bytes.clone())));
                t.mtimes.insert(nf.clone(), when);
                t.files.insert(nf, bytes);
            }
            t.dirs.insert(b.clone());
        } else {
            return Err(not_found(&a));
        }
    }
    for (k, bytes) in moved {
        notify(&k, bytes.as_deref());
    }
    Ok(())
}

pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let bytes = read(from)?;
    write(to, &bytes)?;
    Ok(bytes.len() as u64)
}

/// The key as an absolute-or-relative path. The tree has no symlinks, so this
/// only checks that the path exists.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let k = key(&path);
    let t = tree();
    if t.files.contains_key(&k) || is_dir_in(&t, &k) {
        Ok(PathBuf::from(k))
    } else {
        Err(not_found(&k))
    }
}

pub fn exists(path: impl AsRef<Path>) -> bool {
    let k = key(path);
    let t = tree();
    t.files.contains_key(&k) || is_dir_in(&t, &k)
}

pub fn is_file(path: impl AsRef<Path>) -> bool {
    tree().files.contains_key(&key(path))
}

pub fn is_dir(path: impl AsRef<Path>) -> bool {
    is_dir_in(&tree(), &key(path))
}

pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    let k = key(path);
    let t = tree();
    if let Some(b) = t.files.get(&k) {
        let modified = t.mtimes.get(&k).copied().unwrap_or(build_stamp());
        Ok(Metadata { is_dir: false, len: b.len() as u64, modified })
    } else if is_dir_in(&t, &k) {
        Ok(Metadata { is_dir: true, len: 0, modified: build_stamp() })
    } else {
        Err(not_found(&k))
    }
}

pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    let k = key(path);
    let t = tree();
    if t.files.contains_key(&k) {
        return Err(io::Error::new(ErrorKind::NotADirectory, k));
    }
    if !is_dir_in(&t, &k) {
        return Err(not_found(&k));
    }
    let prefix = if k.is_empty() {
        String::new()
    } else if k == "/" {
        "/".to_string()
    } else {
        format!("{k}/")
    };
    let mut entries = Vec::new();
    let mut push = |full: &str, is_dir: bool| {
        let rest = &full[prefix.len()..];
        if rest.is_empty() || rest.contains('/') {
            return;
        }
        entries.push(DirEntry {
            path: PathBuf::from(full),
            name: OsString::from(rest),
            is_dir,
        });
    };
    for (f, _) in t.files.range(prefix.clone()..) {
        if !f.starts_with(&prefix) {
            break;
        }
        push(f, false);
    }
    for d in t.dirs.range(prefix.clone()..) {
        if !d.starts_with(&prefix) {
            break;
        }
        push(d, true);
    }
    Ok(ReadDir { inner: entries.into_iter() })
}

pub fn set_permissions(path: impl AsRef<Path>, _perm: Permissions) -> io::Result<()> {
    if exists(path) {
        Ok(())
    } else {
        Err(io::Error::new(ErrorKind::NotFound, "no such file"))
    }
}

// ---- types ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    is_dir: bool,
    len: u64,
    modified: SystemTime,
}

impl Metadata {
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }
    pub fn is_file(&self) -> bool {
        !self.is_dir
    }
    pub fn file_type(&self) -> FileType {
        FileType { is_dir: self.is_dir }
    }
    pub fn permissions(&self) -> Permissions {
        Permissions::default()
    }
    /// The last write (or [`build_stamp`] for a file nothing has written).
    pub fn modified(&self) -> io::Result<SystemTime> {
        Ok(self.modified)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileType {
    is_dir: bool,
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }
    pub fn is_file(&self) -> bool {
        !self.is_dir
    }
    pub fn is_symlink(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    readonly: bool,
}

impl Permissions {
    pub fn readonly(&self) -> bool {
        self.readonly
    }
    pub fn set_readonly(&mut self, readonly: bool) {
        self.readonly = readonly;
    }
}

#[derive(Debug)]
pub struct DirEntry {
    path: PathBuf,
    name: OsString,
    is_dir: bool,
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn file_name(&self) -> OsString {
        self.name.clone()
    }
    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(FileType { is_dir: self.is_dir })
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        metadata(&self.path)
    }
}

#[derive(Debug)]
pub struct ReadDir {
    inner: std::vec::IntoIter<DirEntry>,
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(Ok)
    }
}

#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn read(&mut self, v: bool) -> &mut Self {
        self.read = v;
        self
    }
    pub fn write(&mut self, v: bool) -> &mut Self {
        self.write = v;
        self
    }
    pub fn append(&mut self, v: bool) -> &mut Self {
        self.append = v;
        self
    }
    pub fn truncate(&mut self, v: bool) -> &mut Self {
        self.truncate = v;
        self
    }
    pub fn create(&mut self, v: bool) -> &mut Self {
        self.create = v;
        self
    }
    pub fn create_new(&mut self, v: bool) -> &mut Self {
        self.create_new = v;
        self
    }
    pub fn open(&self, path: impl AsRef<Path>) -> io::Result<File> {
        let k = key(path);
        let writable = self.write || self.append;
        // Creating or truncating a file changes the tree, so the hook hears the
        // empty file here, not only once the handle is flushed (a file created and
        // never written would otherwise vanish on reload).
        let mut emptied = false;
        {
            let mut t = tree();
            parent_must_exist(&t, &k)?;
            if is_dir_in(&t, &k) {
                return Err(io::Error::new(ErrorKind::IsADirectory, k));
            }
            let exists = t.files.contains_key(&k);
            if exists && self.create_new {
                return Err(io::Error::new(ErrorKind::AlreadyExists, k));
            }
            if !exists && !(self.create || self.create_new) {
                return Err(not_found(&k));
            }
            if !exists {
                t.files.insert(k.clone(), Vec::new());
                t.mtimes.insert(k.clone(), SystemTime::now());
                emptied = true;
            } else if self.truncate && writable {
                t.files.insert(k.clone(), Vec::new());
                t.mtimes.insert(k.clone(), SystemTime::now());
                emptied = true;
            }
        }
        if emptied {
            notify(&k, Some(&[]));
        }
        let pos = if self.append {
            tree().files.get(&k).map_or(0, |b| b.len() as u64)
        } else {
            0
        };
        Ok(File { key: k, pos, read: self.read, write: writable, dirty: false })
    }
}

/// An open file in the tree. Writes land in the tree at once; the hook hears
/// about them on `flush` or when the file is dropped.
#[derive(Debug)]
pub struct File {
    key: String,
    pos: u64,
    read: bool,
    write: bool,
    dirty: bool,
}

impl File {
    pub fn open(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().read(true).open(path)
    }
    pub fn create(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().write(true).create(true).truncate(true).open(path)
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        metadata(&self.key)
    }
    pub fn sync_all(&self) -> io::Result<()> {
        Ok(())
    }
    pub fn set_len(&self, size: u64) -> io::Result<()> {
        let bytes = {
            let mut t = tree();
            let b = t.files.get_mut(&self.key).ok_or_else(|| not_found(&self.key))?;
            b.resize(size as usize, 0);
            b.clone()
        };
        notify(&self.key, Some(&bytes));
        Ok(())
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.read {
            return Err(io::Error::new(ErrorKind::PermissionDenied, "file not open for reading"));
        }
        let t = tree();
        let Some(b) = t.files.get(&self.key) else {
            return Err(not_found(&self.key));
        };
        let start = (self.pos as usize).min(b.len());
        let n = buf.len().min(b.len() - start);
        buf[..n].copy_from_slice(&b[start..start + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.write {
            return Err(io::Error::new(ErrorKind::PermissionDenied, "file not open for writing"));
        }
        {
            let mut t = tree();
            let Some(b) = t.files.get_mut(&self.key) else {
                return Err(not_found(&self.key));
            };
            let at = self.pos as usize;
            if b.len() < at {
                b.resize(at, 0);
            }
            let end = at + buf.len();
            if b.len() < end {
                b.resize(end, 0);
            }
            b[at..end].copy_from_slice(buf);
            self.pos = end as u64;
            let key = self.key.clone();
            t.mtimes.insert(key, SystemTime::now());
        }
        self.dirty = true;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.report();
        Ok(())
    }
}

impl File {
    /// Tells the hook the file's bytes once, not once per `write`: a host that
    /// mirrors to `localStorage` would otherwise re-encode the whole file for
    /// every chunk. Runs on `flush` and on drop.
    fn report(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let bytes = tree().files.get(&self.key).cloned();
        if let Some(b) = bytes {
            notify(&self.key, Some(&b));
        }
    }
}

impl Drop for File {
    fn drop(&mut self) {
        self.report();
    }
}

impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let len = tree().files.get(&self.key).map_or(0, |b| b.len() as u64);
        let next = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        match next {
            Some(n) => {
                self.pos = n;
                Ok(n)
            }
            None => Err(io::Error::new(ErrorKind::InvalidInput, "seek before start")),
        }
    }
}

#[cfg(test)]
mod tests {
    //! The tree is one process-wide static, so each test works under its own
    //! top-level name (`t_<test>`) and checks only its own keys. The hook is
    //! also global: tests that inspect it filter by their own prefix.

    use super::*;
    use std::io::{Read as _, Seek as _, Write as _};
    use std::sync::Arc;

    type Log = Arc<Mutex<Vec<(String, Option<Vec<u8>>)>>>;

    /// Installs a hook that records every change, and returns the log.
    fn record() -> Log {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        set_hook(move |k, b| sink.lock().unwrap().push((k.to_string(), b.map(<[u8]>::to_vec))));
        log
    }

    /// `write` with its parent directories made first, the way a test wants a fixture.
    fn put(path: &str, bytes: impl AsRef<[u8]>) -> io::Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            create_dir_all(parent)?;
        }
        write(path, bytes)
    }

    fn events_under(log: &Log, prefix: &str) -> Vec<(String, Option<Vec<u8>>)> {
        log.lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .cloned()
            .collect()
    }

    #[test]
    fn keys_are_lexically_normalised() {
        assert_eq!(key("a/./b/../c"), "a/c");
        assert_eq!(key("/x/y/"), "/x/y");
        assert_eq!(key("../up"), "../up");
        assert_ne!(key("/x"), key("x"));
        assert_eq!(key("a//b"), "a/b");
    }

    #[test]
    fn write_then_read_round_trips() {
        put("t_rw/a.txt", b"hello").unwrap();
        assert_eq!(read("t_rw/a.txt").unwrap(), b"hello");
        assert_eq!(read_to_string("t_rw/./a.txt").unwrap(), "hello");
        assert!(is_file("t_rw/a.txt"));
        assert!(exists("t_rw/a.txt"));
        assert!(!is_dir("t_rw/a.txt"));
    }

    #[test]
    fn missing_file_is_not_found_like_std() {
        let err = read("t_missing/nope.txt").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(!exists("t_missing/nope.txt"));
        assert!(!is_file("t_missing/nope.txt"));
    }

    #[test]
    fn write_needs_its_parent_directory() {
        let err = write("t_parent/x/y.txt", b"1").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        create_dir_all("t_parent/x").unwrap();
        write("t_parent/x/y.txt", b"1").unwrap();
        assert_eq!(read("t_parent/x/y.txt").unwrap(), b"1");
    }

    #[test]
    fn create_dir_refuses_an_existing_path() {
        create_dir_all("t_mk/a/b").unwrap();
        assert_eq!(create_dir("t_mk/a").unwrap_err().kind(), ErrorKind::AlreadyExists);
        assert!(is_dir("t_mk/a/b"));
        create_dir("t_mk/fresh").unwrap();
        assert!(is_dir("t_mk/fresh"));
    }

    #[test]
    fn reading_a_directory_is_an_error() {
        create_dir_all("t_isdir/d").unwrap();
        assert_eq!(read("t_isdir/d").unwrap_err().kind(), ErrorKind::IsADirectory);
        put("t_isdir/f", b"x").unwrap();
        assert_eq!(read_dir("t_isdir/f").unwrap_err().kind(), ErrorKind::NotADirectory);
    }

    #[test]
    fn remove_dir_refuses_a_non_empty_directory() {
        put("t_rmdir/d/f", b"x").unwrap();
        assert_eq!(remove_dir("t_rmdir/d").unwrap_err().kind(), ErrorKind::DirectoryNotEmpty);
        remove_file("t_rmdir/d/f").unwrap();
        remove_dir("t_rmdir/d").unwrap();
        assert!(!exists("t_rmdir/d"));
        assert_eq!(remove_file("t_rmdir/d/f").unwrap_err().kind(), ErrorKind::NotFound);
    }

    #[test]
    fn remove_dir_all_takes_the_subtree_and_reports_each_file() {
        let log = record();
        put("t_rmall/keep.txt", b"k").unwrap();
        put("t_rmall/sub/one.txt", b"1").unwrap();
        put("t_rmall/sub/deep/two.txt", b"2").unwrap();
        remove_dir_all("t_rmall/sub").unwrap();
        assert!(!exists("t_rmall/sub"));
        assert!(!exists("t_rmall/sub/deep/two.txt"));
        assert!(is_file("t_rmall/keep.txt"));
        let removed: Vec<String> = events_under(&log, "t_rmall/sub")
            .into_iter()
            .filter(|(_, b)| b.is_none())
            .map(|(k, _)| k)
            .collect();
        assert!(removed.contains(&"t_rmall/sub/one.txt".to_string()));
        assert!(removed.contains(&"t_rmall/sub/deep/two.txt".to_string()));
    }

    #[test]
    fn rename_moves_a_file_and_a_directory() {
        put("t_mv/src.txt", b"data").unwrap();
        rename("t_mv/src.txt", "t_mv/dst.txt").unwrap();
        assert!(!exists("t_mv/src.txt"));
        assert_eq!(read("t_mv/dst.txt").unwrap(), b"data");

        put("t_mv/dir/inner/leaf.txt", b"leaf").unwrap();
        rename("t_mv/dir", "t_mv/moved").unwrap();
        assert!(!exists("t_mv/dir"));
        assert_eq!(read("t_mv/moved/inner/leaf.txt").unwrap(), b"leaf");
        assert_eq!(rename("t_mv/nothing", "t_mv/x").unwrap_err().kind(), ErrorKind::NotFound);
    }

    #[test]
    fn copy_duplicates_bytes_and_returns_the_length() {
        put("t_cp/a", b"abcd").unwrap();
        assert_eq!(copy("t_cp/a", "t_cp/b").unwrap(), 4);
        assert_eq!(read("t_cp/b").unwrap(), b"abcd");
    }

    #[test]
    fn read_dir_lists_direct_children_only() {
        put("t_ls/a.txt", b"1").unwrap();
        put("t_ls/sub/b.txt", b"2").unwrap();
        create_dir_all("t_ls/empty").unwrap();
        let mut names: Vec<(String, bool)> = read_dir("t_ls")
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.file_name().to_string_lossy().into_owned(), e.file_type().unwrap().is_dir())
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![("a.txt".into(), false), ("empty".into(), true), ("sub".into(), true)]
        );
    }

    #[test]
    fn metadata_reports_length_and_kind() {
        put("t_meta/f.bin", &[0u8; 7]).unwrap();
        let m = metadata("t_meta/f.bin").unwrap();
        assert!(m.is_file() && !m.is_dir());
        assert_eq!(m.len(), 7);
        let d = metadata("t_meta").unwrap();
        assert!(d.is_dir() && !d.is_file());
        assert_eq!(metadata("t_meta/none").unwrap_err().kind(), ErrorKind::NotFound);
    }

    #[test]
    fn canonicalize_needs_the_path_to_exist() {
        put("t_canon/x", b"").unwrap();
        assert_eq!(canonicalize("t_canon/./x").unwrap(), PathBuf::from("t_canon/x"));
        assert!(canonicalize("t_canon/gone").is_err());
    }

    #[test]
    fn file_handles_read_write_and_seek() {
        let mut f = File::create("t_fh/f.dat").unwrap_or_else(|_| {
            create_dir_all("t_fh").unwrap();
            File::create("t_fh/f.dat").unwrap()
        });
        f.write_all(b"0123456789").unwrap();
        f.flush().unwrap();
        drop(f);

        let mut r = File::open("t_fh/f.dat").unwrap();
        r.seek(SeekFrom::Start(3)).unwrap();
        let mut buf = [0u8; 4];
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"3456");
        assert_eq!(r.seek(SeekFrom::End(-2)).unwrap(), 8);
        let mut tail = String::new();
        r.read_to_string(&mut tail).unwrap();
        assert_eq!(tail, "89");
        assert_eq!(r.metadata().unwrap().len(), 10);
    }

    #[test]
    fn open_options_follow_std_rules() {
        create_dir_all("t_oo").unwrap();
        assert_eq!(File::open("t_oo/none").unwrap_err().kind(), ErrorKind::NotFound);
        OpenOptions::new().write(true).create_new(true).open("t_oo/new").unwrap();
        assert_eq!(
            OpenOptions::new().write(true).create_new(true).open("t_oo/new").unwrap_err().kind(),
            ErrorKind::AlreadyExists
        );
        {
            let mut a = OpenOptions::new().append(true).open("t_oo/new").unwrap();
            a.write_all(b"ab").unwrap();
        }
        {
            let mut a = OpenOptions::new().append(true).create(true).open("t_oo/new").unwrap();
            a.write_all(b"cd").unwrap();
        }
        assert_eq!(read("t_oo/new").unwrap(), b"abcd");
        File::create("t_oo/new").unwrap();
        assert_eq!(read("t_oo/new").unwrap(), b"");
    }

    #[test]
    fn writes_are_reported_to_the_hook_and_mounts_are_not() {
        let log = record();
        mount("t_hook/seed", [("a.txt", b"seed".as_slice())]);
        assert_eq!(read("t_hook/seed/a.txt").unwrap(), b"seed");
        assert!(events_under(&log, "t_hook/seed").is_empty());

        put("t_hook/w.txt", b"one").unwrap();
        remove_file("t_hook/w.txt").unwrap();
        let ev = events_under(&log, "t_hook/w.txt");
        assert_eq!(
            ev,
            vec![
                ("t_hook/w.txt".to_string(), Some(b"one".to_vec())),
                ("t_hook/w.txt".to_string(), None),
            ]
        );
    }

    #[test]
    fn a_file_handle_reports_once_on_flush_and_creation_reports_at_once() {
        let log = record();
        create_dir_all("t_once").unwrap();
        {
            let mut f = File::create("t_once/log.txt").unwrap();
            assert_eq!(events_under(&log, "t_once/log.txt").len(), 1, "creation is reported");
            for _ in 0..5 {
                f.write_all(b"xy").unwrap();
            }
            assert_eq!(events_under(&log, "t_once/log.txt").len(), 1, "writes wait for flush");
        }
        let ev = events_under(&log, "t_once/log.txt");
        assert_eq!(ev.len(), 2, "one report on drop");
        assert_eq!(ev[1].1.as_deref(), Some(b"xyxyxyxyxy".as_slice()));
    }

    #[test]
    fn writes_need_a_parent_and_reject_directories() {
        create_dir_all("t_bad/dir").unwrap();
        assert_eq!(write("t_bad/dir", b"x").unwrap_err().kind(), ErrorKind::IsADirectory);
        assert_eq!(
            write("t_bad/nope/child", b"x").unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn set_permissions_only_checks_existence() {
        put("t_perm/p", b"").unwrap();
        let p = metadata("t_perm/p").unwrap().permissions();
        set_permissions("t_perm/p", p).unwrap();
        assert!(set_permissions("t_perm/none", metadata("t_perm/p").unwrap().permissions()).is_err());
    }

    #[test]
    fn std_fs_style_callers_compile_against_the_same_names() {
        // The engine names these as `std::fs` does; this keeps the surface honest.
        assert!(read_to_string("t_names/none").is_err());
        put("t_names/w", b"ok").unwrap();
        assert!(exists("t_names/w"));
        let _ = OpenOptions::new().read(true).open("t_names/none");
    }
}

#[cfg(test)]
mod mtime_tests {
    use super::*;

    #[test]
    fn modified_times_are_seeded_then_follow_writes_and_renames() {
        mount("t_mtime", [("seed.bin", &b"x"[..])]);
        let seeded = metadata("t_mtime/seed.bin").unwrap().modified().unwrap();
        assert_eq!(seeded, build_stamp());

        write("t_mtime/new.bin", b"y").unwrap();
        let written = metadata("t_mtime/new.bin").unwrap().modified().unwrap();
        assert!(written >= seeded, "a write stamps the time it happened");

        rename("t_mtime/new.bin", "t_mtime/moved.bin").unwrap();
        let moved = metadata("t_mtime/moved.bin").unwrap().modified().unwrap();
        assert_eq!(moved, written, "a rename keeps the file's time");

        let listed = read_dir("t_mtime").unwrap().flatten().next().unwrap();
        assert!(listed.metadata().unwrap().modified().is_ok());
    }
}
