//! The on-disk cache of rendered preview pictures: one animated GIF per key,
//! with a JSON sidecar that says how and when it was made.
//!
//! Layout, under a root such as `~/.cache/metalvj/previews`:
//!
//! ```text
//! <root>/<space>/<key>.gif    the picture (a looping GIF, the human-viewable part)
//! <root>/<space>/<key>.json   its [`Meta`], pretty-printed, read by hand when debugging
//! ```
//!
//! Both files are written together through a temporary name and a rename, so
//! a reader never sees a half-written picture. A key is any text (a node id,
//! a sequence name); it is reduced to plain filename characters the same way
//! [`super::gallery_anim::file_name`] does. The cache decides only whether a
//! picture must be rendered again; the rendering itself lives elsewhere.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The sidecar `input_hash` of a stand-in picture (drawn while no snippet has
/// been heard). A real picture's hash is a digest of its floats, which is not
/// 0 in practice; the first real snippet replaces the stand-in.
pub const STAND_IN_HASH: u64 = 0;

/// Which kind of preview a picture is. Each space is its own subdirectory, so
/// keys never collide across spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// The gallery's listened pictures: a card's picture after its song was heard.
    Gallery,
    /// A sequence's pictures in the sequence editor.
    Sequence,
}

impl Space {
    /// The subdirectory name of this space under the cache root.
    pub fn dir_name(self) -> &'static str {
        match self {
            Space::Gallery => "gallery-listened",
            Space::Sequence => "sequence",
        }
    }
}

/// What the sidecar records about one cached picture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// When the picture was rendered, in seconds since the Unix epoch.
    pub rendered_at: u64,
    /// A hash of the inputs the picture was drawn from (the floats or the
    /// snippet). A caller that finds a different hash knows the picture is out
    /// of date even when it is not yet old.
    pub input_hash: u64,
    /// How many frames the GIF has.
    pub frames: usize,
    /// The picture's width and height in pixels.
    pub size: [usize; 2],
}

/// A cache of preview pictures rooted at one directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewCache {
    root: PathBuf,
}

impl PreviewCache {
    /// A cache rooted at `root`. Nothing is created until a picture is written.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The default root, `$HOME/.cache/metalvj/previews`, or `None` without a
    /// home directory.
    pub fn default_root() -> Option<PathBuf> {
        let home = metal_config::home_dir();
        (!home.as_os_str().is_empty() && home != Path::new("."))
            .then(|| home.join(".cache/metalvj/previews"))
    }

    /// The GIF path for `key` in `space`.
    pub fn path(&self, space: Space, key: &str) -> PathBuf {
        self.dir(space).join(file_name(key, "gif"))
    }

    /// The sidecar path for `key` in `space`.
    fn meta_path(&self, space: Space, key: &str) -> PathBuf {
        self.dir(space).join(file_name(key, "json"))
    }

    fn dir(&self, space: Space) -> PathBuf {
        self.root.join(space.dir_name())
    }

    /// The sidecar for `key`, or `None` when it is missing, unreadable or not
    /// valid JSON. A picture without a usable sidecar counts as not cached.
    pub fn meta(&self, space: Space, key: &str) -> Option<Meta> {
        let text = fs::read_to_string(self.meta_path(space, key)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// The GIF bytes for `key`, or `None` when no picture is stored.
    pub fn read_gif(&self, space: Space, key: &str) -> Option<Vec<u8>> {
        fs::read(self.path(space, key)).ok()
    }

    /// Stores `gif` and its `meta` for `key`, replacing any earlier pair.
    ///
    /// Both files are written under temporary names first and then renamed
    /// into place, the GIF before the sidecar. If the second rename fails the
    /// new GIF is in place with the old sidecar, which reads as out of date and
    /// is rendered again; the temporary files are removed on any failure.
    pub fn write(&self, space: Space, key: &str, gif: &[u8], meta: &Meta) -> io::Result<()> {
        fs::create_dir_all(self.dir(space))?;
        let meta_json = serde_json::to_vec_pretty(meta)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let gif_path = self.path(space, key);
        let meta_path = self.meta_path(space, key);
        let token = unique_token();
        let gif_tmp = tmp_path(&gif_path, token);
        let meta_tmp = tmp_path(&meta_path, token);
        let result = write_then_rename(gif, &gif_tmp, &gif_path, &meta_json, &meta_tmp, &meta_path);
        if result.is_err() {
            let _ = fs::remove_file(&gif_tmp);
            let _ = fs::remove_file(&meta_tmp);
        }
        result
    }

    /// Whether `key` must be rendered now.
    ///
    /// - A picture or sidecar that is missing or unreadable: always `true`.
    /// - Otherwise `true` only when `selected` (the node is on screen as the
    ///   user's selection) and the picture is [`is_stale`] under `cooldown_secs`.
    /// - Anything else: `false`, so a stale picture that nobody is looking at
    ///   is left as it is.
    pub fn should_render(
        &self,
        space: Space,
        key: &str,
        now: u64,
        cooldown_secs: u64,
        selected: bool,
    ) -> bool {
        if !self.path(space, key).is_file() {
            return true;
        }
        match self.meta(space, key) {
            None => true,
            Some(meta) => selected && is_stale(&meta, now, cooldown_secs),
        }
    }
}

/// Whether a picture rendered at `meta.rendered_at` is older than
/// `cooldown_secs` at time `now`. A picture stamped in the future (a clock that
/// moved back) has age zero, so it is stale only when the cooldown is zero.
pub fn is_stale(meta: &Meta, now: u64, cooldown_secs: u64) -> bool {
    now.saturating_sub(meta.rendered_at) >= cooldown_secs
}

/// The file name for `key` with `ext` appended: the key with anything that is
/// not an ASCII letter, digit, `.`, `-` or `_` replaced by `_`. An empty key
/// becomes `_`, so the name is never a bare extension.
fn file_name(key: &str, ext: &str) -> String {
    let mut safe: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if safe.is_empty() {
        safe.push('_');
    }
    format!("{safe}.{ext}")
}

/// A temporary sibling of `path`, unique to this write, so two writers of one
/// key never share a temporary file.
fn tmp_path(path: &Path, token: u64) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{name}.{token}.tmp"))
}

fn unique_token() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    (u64::from(std::process::id()) << 32) | (n & 0xffff_ffff)
}

fn write_then_rename(
    gif: &[u8],
    gif_tmp: &Path,
    gif_path: &Path,
    meta_json: &[u8],
    meta_tmp: &Path,
    meta_path: &Path,
) -> io::Result<()> {
    fs::write(gif_tmp, gif)?;
    fs::write(meta_tmp, meta_json)?;
    fs::rename(gif_tmp, gif_path)?;
    fs::rename(meta_tmp, meta_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir()
                .join(format!("mvj-preview-cache-{name}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&dir).expect("scratch dir");
            Scratch(dir)
        }

        fn cache(&self) -> PreviewCache {
            PreviewCache::new(self.0.join("previews"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn meta(rendered_at: u64) -> Meta {
        Meta { rendered_at, input_hash: 0xfeed_beef, frames: 3, size: [64, 32] }
    }

    #[test]
    fn keys_are_sanitised_into_plain_file_names() {
        assert_eq!(file_name("gen.bloom", "gif"), "gen.bloom.gif");
        assert_eq!(file_name("fx/odd name", "gif"), "fx_odd_name.gif");
        assert_eq!(file_name("../../etc", "json"), ".._.._etc.json");
        assert_eq!(file_name("", "gif"), "_.gif");
        assert_eq!(file_name("café", "gif"), "caf_.gif");
    }

    #[test]
    fn a_hostile_key_stays_inside_its_space_directory() {
        let cache = PreviewCache::new("/root/previews");
        let path = cache.path(Space::Sequence, "../../escape");
        assert_eq!(path.parent(), Some(Path::new("/root/previews/sequence")));
    }

    #[test]
    fn the_two_spaces_use_their_own_directories() {
        let cache = PreviewCache::new("/root/previews");
        assert_eq!(cache.path(Space::Gallery, "k"), Path::new("/root/previews/gallery-listened/k.gif"));
        assert_eq!(cache.path(Space::Sequence, "k"), Path::new("/root/previews/sequence/k.gif"));
    }

    #[test]
    fn the_default_root_is_under_home() {
        if let Some(root) = PreviewCache::default_root() {
            assert!(root.ends_with(".cache/metalvj/previews"), "{}", root.display());
        }
    }

    #[test]
    fn a_written_picture_reads_back_with_its_meta() {
        let scratch = Scratch::new("round-trip");
        let cache = scratch.cache();
        let gif = b"GIF89a-not-really-but-bytes".to_vec();
        cache.write(Space::Gallery, "gen.bloom", &gif, &meta(1_000)).expect("writes");

        assert_eq!(cache.read_gif(Space::Gallery, "gen.bloom"), Some(gif));
        assert_eq!(cache.meta(Space::Gallery, "gen.bloom"), Some(meta(1_000)));
        assert_eq!(cache.read_gif(Space::Sequence, "gen.bloom"), None, "spaces are separate");
    }

    #[test]
    fn the_sidecar_is_pretty_json_a_person_can_read() {
        let scratch = Scratch::new("pretty");
        let cache = scratch.cache();
        cache.write(Space::Sequence, "seq 1", b"x", &meta(7)).expect("writes");
        let text = fs::read_to_string(cache.dir(Space::Sequence).join("seq_1.json")).expect("sidecar");
        assert!(text.contains('\n'), "pretty-printed");
        assert!(text.contains("\"rendered_at\": 7"));
    }

    #[test]
    fn a_second_write_replaces_the_first() {
        let scratch = Scratch::new("replace");
        let cache = scratch.cache();
        cache.write(Space::Gallery, "k", b"first", &meta(1)).expect("writes");
        cache.write(Space::Gallery, "k", b"second", &meta(2)).expect("writes");
        assert_eq!(cache.read_gif(Space::Gallery, "k"), Some(b"second".to_vec()));
        assert_eq!(cache.meta(Space::Gallery, "k"), Some(meta(2)));
    }

    #[test]
    fn an_atomic_write_leaves_no_temporary_files() {
        let scratch = Scratch::new("atomic");
        let cache = scratch.cache();
        cache.write(Space::Gallery, "k", b"gif", &meta(1)).expect("writes");
        cache.write(Space::Gallery, "k", b"gif2", &meta(2)).expect("writes");
        let names: Vec<String> = fs::read_dir(cache.dir(Space::Gallery))
            .expect("dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["k.gif".to_string(), "k.json".to_string()]);
    }

    #[test]
    fn a_missing_sidecar_reads_as_no_meta_and_renders_again() {
        let scratch = Scratch::new("no-meta");
        let cache = scratch.cache();
        let gif_path = cache.path(Space::Gallery, "k");
        fs::create_dir_all(gif_path.parent().expect("parent")).expect("dir");
        fs::write(&gif_path, b"gif").expect("picture without a sidecar");

        assert_eq!(cache.meta(Space::Gallery, "k"), None);
        assert_eq!(cache.read_gif(Space::Gallery, "k"), Some(b"gif".to_vec()));
        assert!(cache.should_render(Space::Gallery, "k", 10, 60, false), "no sidecar, render");
    }

    #[test]
    fn a_corrupt_sidecar_reads_as_no_meta() {
        let scratch = Scratch::new("corrupt");
        let cache = scratch.cache();
        cache.write(Space::Gallery, "k", b"gif", &meta(1)).expect("writes");
        fs::write(cache.meta_path(Space::Gallery, "k"), b"{ not json").expect("corrupt");
        assert_eq!(cache.meta(Space::Gallery, "k"), None);
        assert!(cache.should_render(Space::Gallery, "k", 10, 60, false));
    }

    #[test]
    fn the_cache_is_stale_only_after_the_cooldown() {
        let m = meta(100);
        assert!(!is_stale(&m, 100, 60), "age 0");
        assert!(!is_stale(&m, 159, 60), "age 59 is inside the cooldown");
        assert!(is_stale(&m, 160, 60), "age 60 is the cooldown");
        assert!(is_stale(&m, 500, 60));
        assert!(!is_stale(&m, 50, 60), "a clock that moved back is not stale");
        assert!(is_stale(&m, 50, 0), "zero cooldown is always stale");
    }

    #[test]
    fn should_render_follows_the_table() {
        let scratch = Scratch::new("table");
        let cache = scratch.cache();
        let (space, key) = (Space::Sequence, "seq");

        // Missing picture: render, selected or not, fresh or not.
        assert!(cache.should_render(space, key, 100, 60, false));
        assert!(cache.should_render(space, key, 100, 60, true));

        cache.write(space, key, b"gif", &meta(100)).expect("writes");

        // Fresh (age 10 < cooldown 60): never render.
        assert!(!cache.should_render(space, key, 110, 60, false));
        assert!(!cache.should_render(space, key, 110, 60, true));

        // Stale (age 60 >= cooldown 60): render only when selected.
        assert!(!cache.should_render(space, key, 160, 60, false), "stale but not selected");
        assert!(cache.should_render(space, key, 160, 60, true), "stale and selected");
    }
}
