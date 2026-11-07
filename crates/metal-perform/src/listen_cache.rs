//! Listen cache: short snippets of the digest rows, kept so previews can be
//! rendered with real audio later.
//!
//! The digest produces one row of `FEATURES` floats per tick at the digest rate
//! (`digest::RATE`, 8 Hz). A [`Recorder`] keeps the last two minutes of those
//! rows and cuts a [`SNIPPET_SECS`]-long window out of them on request,
//! skipping silence. A [`Store`] keeps a small set of those snippets on disk so
//! a preview can pick one deterministically.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Length of one snippet, in seconds.
pub const SNIPPET_SECS: f32 = 5.0;
/// Rows per second the caller pushes into a [`Recorder`].
pub const ROW_RATE: f32 = 8.0;

/// Rows per snippet: `SNIPPET_SECS * ROW_RATE`.
const SNIPPET_ROWS: usize = 40;
/// Rows the recorder keeps: two minutes at [`ROW_RATE`].
const WINDOW_ROWS: usize = 960;
/// Snippets a [`Store`] keeps before it starts replacing the oldest.
pub const STORE_CAP: usize = 12;
/// A snippet whose column-0 mean (rms) is below this is silence.
const SILENCE_RMS: f32 = 0.02;
/// File name inside the store directory.
const FILE_NAME: &str = "snippets.json";

/// One cut of digest rows, with the moment it was heard.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    /// Consecutive digest rows, oldest first.
    pub rows: Vec<Vec<f32>>,
    /// Rows per second the rows were recorded at.
    pub rate_hz: f32,
    /// Unix seconds of the last row the recorder had when the snippet was cut.
    pub heard_at: u64,
}

/// Rolling window of digest rows.
#[derive(Debug, Default)]
pub struct Recorder {
    rows: VecDeque<Vec<f32>>,
    last_unix: u64,
}

impl Recorder {
    /// An empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one digest row, dropping the oldest once the window is full.
    pub fn push(&mut self, row: &[f32], now_unix: u64) {
        if self.rows.len() == WINDOW_ROWS {
            self.rows.pop_front();
        }
        self.rows.push_back(row.to_vec());
        self.last_unix = now_unix;
    }

    /// Cuts a [`SNIPPET_SECS`]-long window at a position chosen from `seed`.
    ///
    /// `None` when the window holds fewer than a snippet's worth of rows, or
    /// when the chosen window is silent (column-0 mean below the threshold).
    pub fn take_snippet(&mut self, seed: u64) -> Option<Snippet> {
        if self.rows.len() < SNIPPET_ROWS {
            return None;
        }
        let starts = self.rows.len() - SNIPPET_ROWS + 1;
        let start = index_for(seed, starts);
        let rows: Vec<Vec<f32>> = self
            .rows
            .iter()
            .skip(start)
            .take(SNIPPET_ROWS)
            .cloned()
            .collect();
        let mean = rows
            .iter()
            .map(|row| row.first().copied().unwrap_or(0.0))
            .sum::<f32>()
            / SNIPPET_ROWS as f32;
        if mean < SILENCE_RMS {
            return None;
        }
        Some(Snippet {
            rows,
            rate_hz: ROW_RATE,
            heard_at: self.last_unix,
        })
    }
}

/// Snippets kept on disk under one directory.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    snippets: Vec<Snippet>,
}

impl Store {
    /// Opens the store in `dir`. A missing, unreadable or corrupt
    /// `snippets.json` gives an empty store rather than an error.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let loaded: Vec<Snippet> = std::fs::read(dir.join(FILE_NAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let mut store = Self {
            dir,
            snippets: Vec::with_capacity(STORE_CAP),
        };
        for snippet in loaded {
            store.add(snippet);
        }
        store
    }

    /// Adds a snippet. Once the store holds [`STORE_CAP`] snippets, the one
    /// with the smallest `heard_at` is replaced.
    pub fn add(&mut self, snippet: Snippet) {
        if self.snippets.len() < STORE_CAP {
            self.snippets.push(snippet);
            return;
        }
        let oldest = self
            .snippets
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| s.heard_at)
            .map(|(index, _)| index);
        if let Some(index) = oldest {
            self.snippets[index] = snippet;
        }
    }

    /// A snippet chosen deterministically from `seed`, or `None` when empty.
    pub fn pick(&self, seed: u64) -> Option<&Snippet> {
        if self.snippets.is_empty() {
            return None;
        }
        self.snippets.get(index_for(seed, self.snippets.len()))
    }

    /// Number of snippets held.
    pub fn len(&self) -> usize {
        self.snippets.len()
    }

    /// True when the store holds no snippets.
    pub fn is_empty(&self) -> bool {
        self.snippets.is_empty()
    }

    /// Writes the store as pretty JSON to `snippets.json`, via a temp file and
    /// a rename so a crash never leaves a torn file. Creates the directory.
    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_string_pretty(&self.snippets)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::atomic::write(self.dir.join(FILE_NAME), json)
    }
}

/// Where the listen cache lives: `$HOME/.cache/metalvj/listen`.
pub fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".cache")
            .join("metalvj")
            .join("listen"),
    )
}

/// Scrambles a seed into a non-zero xorshift state (splitmix64 finalizer).
fn scramble(seed: u64) -> u64 {
    let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    if x == 0 {
        1
    } else {
        x
    }
}

/// One xorshift64 step. Never maps a non-zero state to zero.
fn xorshift(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

/// An index in `0..n` derived from `seed`. `n` must be non-zero.
fn index_for(seed: u64, n: usize) -> usize {
    (xorshift(scramble(seed)) % n as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row whose column 0 is `level` and column 1 is `index`, so a
    /// snippet's place in the stream shows in its contents.
    fn row(index: usize, level: f32) -> Vec<f32> {
        vec![level, index as f32, 0.5]
    }

    /// Pushes `count` rows at `level`, numbered from `first`.
    fn feed(rec: &mut Recorder, first: usize, count: usize, level: f32) {
        for i in first..first + count {
            rec.push(&row(i, level), 1_000 + i as u64);
        }
    }

    fn snippet_at(heard_at: u64) -> Snippet {
        Snippet {
            rows: vec![row(0, 0.5)],
            rate_hz: ROW_RATE,
            heard_at,
        }
    }

    /// A per-test directory under the system temp dir, cleared before use.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fw-listen-cache-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn constants_describe_forty_rows() {
        assert_eq!(SNIPPET_SECS * ROW_RATE, SNIPPET_ROWS as f32);
        assert_eq!(WINDOW_ROWS as f32, 120.0 * ROW_RATE);
    }

    #[test]
    fn snippet_needs_a_full_forty_row_window() {
        let mut rec = Recorder::new();
        feed(&mut rec, 0, 39, 0.5);
        assert!(rec.take_snippet(1).is_none());

        rec.push(&row(39, 0.5), 2_000);
        let snippet = rec.take_snippet(1).expect("40 loud rows make a snippet");
        assert_eq!(snippet.rows.len(), SNIPPET_ROWS);
        assert_eq!(snippet.rate_hz, ROW_RATE);
        assert_eq!(snippet.heard_at, 2_000);
    }

    #[test]
    fn window_is_capped_at_two_minutes_and_snippets_are_contiguous() {
        let mut rec = Recorder::new();
        feed(&mut rec, 0, 1_200, 0.5);
        for seed in 0..50 {
            let snippet = rec.take_snippet(seed).expect("loud window");
            let first = snippet.rows[0][1] as usize;
            assert!(first >= 1_200 - WINDOW_ROWS, "row {first} is older than the window");
            assert!(first + SNIPPET_ROWS <= 1_200, "row {first} runs past the newest row");
            for (k, r) in snippet.rows.iter().enumerate() {
                assert_eq!(r[1] as usize, first + k);
            }
        }
    }

    #[test]
    fn silent_window_is_rejected() {
        let mut rec = Recorder::new();
        feed(&mut rec, 0, 200, 0.0);
        for seed in 0..16 {
            assert!(rec.take_snippet(seed).is_none());
        }

        let mut quiet = Recorder::new();
        feed(&mut quiet, 0, 200, 0.019);
        assert!(quiet.take_snippet(3).is_none());

        let mut audible = Recorder::new();
        feed(&mut audible, 0, 200, 0.5);
        assert!(audible.take_snippet(3).is_some());
    }

    #[test]
    fn same_seed_gives_the_same_snippet() {
        let mut rec = Recorder::new();
        feed(&mut rec, 0, 300, 0.5);
        assert_eq!(rec.take_snippet(42), rec.take_snippet(42));

        let starts: std::collections::BTreeSet<u64> = (0..16)
            .map(|seed| rec.take_snippet(seed).expect("loud window").rows[0][1] as u64)
            .collect();
        assert!(starts.len() > 1, "different seeds should cut different windows");
    }

    #[test]
    fn pick_is_deterministic_per_seed() {
        let mut store = Store::open(scratch("pick"));
        for heard_at in 0..5 {
            store.add(snippet_at(heard_at));
        }
        assert_eq!(store.pick(7), store.pick(7));

        let heard: std::collections::BTreeSet<u64> = (0..32)
            .map(|seed| store.pick(seed).expect("non-empty").heard_at)
            .collect();
        assert!(heard.len() > 1);

        let empty = Store::open(scratch("pick-empty"));
        assert!(empty.pick(7).is_none());
    }

    #[test]
    fn add_fills_then_replaces_the_oldest() {
        let mut store = Store::open(scratch("cap"));
        for heard_at in 10..10 + STORE_CAP as u64 {
            store.add(snippet_at(heard_at));
        }
        assert_eq!(store.len(), STORE_CAP);

        store.add(snippet_at(100));
        assert_eq!(store.len(), STORE_CAP);
        let heard: Vec<u64> = store.snippets.iter().map(|s| s.heard_at).collect();
        assert!(!heard.contains(&10), "the oldest snippet is replaced");
        assert!(heard.contains(&100));
        assert!(heard.contains(&11));

        store.add(snippet_at(101));
        let heard: Vec<u64> = store.snippets.iter().map(|s| s.heard_at).collect();
        assert_eq!(heard.len(), STORE_CAP);
        assert!(!heard.contains(&11));
        assert!(heard.contains(&101));
    }

    #[test]
    fn save_and_open_round_trip() {
        let dir = scratch("roundtrip");
        let mut store = Store::open(&dir);
        assert!(store.is_empty());
        for heard_at in [3, 1, 2] {
            store.add(snippet_at(heard_at));
        }
        store.save().expect("save");

        let json = std::fs::read_to_string(dir.join(FILE_NAME)).expect("file written");
        assert!(json.contains("\n  "), "pretty JSON");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from(FILE_NAME)]);

        let reopened = Store::open(&dir);
        assert_eq!(reopened.snippets, store.snippets);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_opens_as_an_empty_store() {
        let dir = scratch("corrupt");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(FILE_NAME), b"{not json").expect("write");

        let mut store = Store::open(&dir);
        assert_eq!(store.len(), 0);
        store.add(snippet_at(5));
        store.save().expect("save over corrupt file");
        assert_eq!(Store::open(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_directory_opens_empty_and_save_creates_it() {
        let dir = scratch("missing").join("nested");
        let mut store = Store::open(&dir);
        assert!(store.is_empty());
        store.add(snippet_at(1));
        store.save().expect("save creates the directory");
        assert_eq!(Store::open(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(dir.parent().expect("parent"));
    }

    #[test]
    fn default_dir_sits_under_home_cache() {
        if let Some(dir) = default_dir() {
            assert!(dir.ends_with(Path::new(".cache").join("metalvj").join("listen")));
        }
    }
}
