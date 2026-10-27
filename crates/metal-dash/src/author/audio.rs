//! Where audio enters the corpus, and the queue that digests it.
//!
//! `AUDIO_FEATURES.md` says there is **exactly one extractor** and that
//! training and serving must see the same distribution, so nothing here
//! computes a feature. Digestion is `data/digest/digest_audio.py`, run as a
//! subprocess, and if librosa is not installed the add flow **says so** rather
//! than writing a matrix with the wrong numbers in it — a corpus is only worth
//! what its inputs agree on, and a silently different digest is the one failure
//! that would not show up until a model trained on it behaved oddly. The
//! subprocess resolves its interpreter as `FW_PYTHON` → the project venv
//! `scripts/setup-digest` creates → the system `python3`, and when the failure
//! is a missing librosa the card carries the one command that fixes it beside
//! the verbatim traceback line.
//!
//! # A file and a microphone are the same thing here
//!
//! The ingest takes **chunks**. A file upload is one chunk that closes the
//! stream; a microphone is many chunks that close it later. That is the whole
//! of what "structure the ingestion layer for streaming" buys, and it is
//! structural rather than aspirational: there is no path through this module
//! that assumes the whole file is present, so the mic route is a caller that
//! calls [`Ingest::chunk`] more than once.
//!
//! What is *not* built is real-time digestion — `digest_audio.py`'s own
//! `digest_chunk` is a flagged placeholder, and pooling a rolling buffer is a
//! latency-budget question nobody has answered. So a stream is digested when it
//! **closes**. A mic take lands as an example the moment you stop recording,
//! which is the honest version of first-class, and it needs no promise about
//! windows that are still open.
//!
//! # Why a queue and not a thread per file
//!
//! the operator will add dozens of songs at once, and dozens of concurrent librosa
//! processes is a box that stops responding rather than a library that fills
//! up quickly. One worker, one song at a time, and the panel watches the
//! statuses change.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use metal_schema::platform::Command;

use serde_json::{json, Value};

/// The digester. Not reimplemented here, on purpose — see the module docs.
const DIGESTER: &str = "data/digest/digest_audio.py";

/// Overrides the digester, for tests.
///
/// It exists because librosa is a five-hundred-megabyte dependency that is not
/// installed on this box, and the alternative to an override is that the
/// **success** path of the queue — subprocess runs, matrix lands, the row flips
/// to digested with the right tick count — is the one path nothing ever
/// exercises. The failure path was already easy to check here and is checked
/// end to end; this is what makes the other half checkable too.
const DIGESTER_ENV: &str = "FW_DIGESTER";

/// Overrides the interpreter the digester runs under, for tests and for a
/// machine whose librosa lives somewhere unusual. Used **verbatim**, even when
/// the path does not exist: it is an explicit instruction, and a silent
/// fallback would hide the typo the override was there to make visible.
const PYTHON_ENV: &str = "FW_PYTHON";

/// The project venv `scripts/setup-digest` creates, relative to `$HOME`.
/// Between the explicit override and the system `python3`: the one the setup
/// script makes is the one that has librosa, so it wins when it is there.
const VENV_PYTHON: &str = "venv/bin/python";

/// What librosa will open. Permissive on purpose: what the corpus contains is
/// whatever got put in the folder (`docs/CORPUS.md`).
/// The **base audio tick rate** (`metal_schema::BASE_HZ`), derived not written:
/// the grid the base projection and `digest_audio.py` are on. The canonical
/// 32 Hz grid is `metal_schema::TickRate::CANONICAL.hz()`.
pub const RATE: f64 = metal_schema::BASE_HZ as f64;

pub const SOUND: [&str; 7] = [".mp3", ".wav", ".flac", ".m4a", ".ogg", ".opus", ".aiff"];

/// Where a song is in the pipeline.
///
/// The three the library shows come from the folder — `docs/CORPUS.md`'s own
/// three — and the two extra ones are the queue's, which the folder cannot
/// know about because a digest in flight has written nothing yet.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Queued,
    Running,
    /// The digester refused, and this is what it said. The traceback's own
    /// last line is kept verbatim — "no module named librosa" is the whole
    /// diagnosis and paraphrasing it costs the reader the fix — and for that
    /// one failure a second line names the command that installs it
    /// (`scripts/setup-digest`).
    Failed(String),
}

impl State {
    fn name(&self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Running => "digesting",
            State::Failed(_) => "failed",
        }
    }
}

/// The ingest: a queue, a worker, and what the worker is doing.
pub struct Ingest {
    audio: PathBuf,
    work: Sender<PathBuf>,
    state: Arc<Mutex<HashMap<String, State>>>,
}

impl Ingest {
    /// Starts the worker. One thread, forever, draining a channel.
    pub fn start(audio: &Path) -> Ingest {
        let (work, jobs) = mpsc::channel::<PathBuf>();
        let state: Arc<Mutex<HashMap<String, State>>> = Arc::default();
        let mine = Arc::clone(&state);
        std::thread::spawn(move || {
            for path in jobs {
                let name = stem(&path);
                mine.lock().unwrap().insert(name.clone(), State::Running);
                match digest(&path) {
                    // Done is the absence of a state: the matrix is on disk
                    // now, so the folder is the manifest again and the queue
                    // has nothing left to say about this song.
                    Ok(()) => {
                        mine.lock().unwrap().remove(&name);
                        println!("  digested {name}");
                    }
                    Err(e) => {
                        eprintln!("  digest failed for {name}: {e}");
                        mine.lock().unwrap().insert(name, State::Failed(e));
                    }
                }
            }
        });
        Ingest {
            audio: audio.to_owned(),
            work,
            state,
        }
    }

    /// Takes one chunk of a stream named `name`.
    ///
    /// The first chunk creates the file; every chunk after it appends. `last`
    /// closes the stream and queues the digest. A file upload passes `last`
    /// on its only chunk; a microphone passes it when recording stops.
    ///
    /// Returns the path it is writing to, so the caller can name it back.
    pub fn chunk(&self, name: &str, bytes: &[u8], last: bool) -> Result<PathBuf, String> {
        let path = self.inside(name)?;
        if let Some(dir) = path.parent() {
            metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // Appending is what makes this a stream. A first chunk truncates,
        // because re-adding a song is a replacement and not a doubling of it.
        let fresh = !self.state.lock().unwrap().contains_key(&stem(&path));
        let mut file = metal_vfs::OpenOptions::new()
            .create(true)
            .append(!fresh)
            .write(true)
            .truncate(fresh)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(bytes).map_err(|e| e.to_string())?;

        let mut state = self.state.lock().unwrap();
        if last {
            state.insert(stem(&path), State::Queued);
            drop(state);
            self.work.send(path.clone()).map_err(|e| e.to_string())?;
        } else {
            // Open streams sit as Queued so the panel can show them filling.
            state.insert(stem(&path), State::Queued);
        }
        Ok(path)
    }

    /// Queues a song already on disk — the "digest what is missing" path, and
    /// what the library's per-row retry uses after a failure is fixed.
    pub fn digest_now(&self, name: &str) -> Result<(), String> {
        let path = self.inside(name)?;
        if !metal_vfs::is_file(&path) {
            return Err(format!("no such song: {}", path.display()));
        }
        self.state
            .lock()
            .unwrap()
            .insert(stem(&path), State::Queued);
        self.work.send(path).map_err(|e| e.to_string())
    }

    /// What the queue is doing, by song stem.
    pub fn states(&self) -> HashMap<String, State> {
        self.state.lock().unwrap().clone()
    }

    /// Resolves a name against the audio directory, and only against it.
    ///
    /// The same rule `choose` follows for the same reason: the browser must not
    /// be able to name a path outside the corpus, and an upload is the one
    /// request that writes a file the server did not choose the name of.
    fn inside(&self, raw: &str) -> Result<PathBuf, String> {
        let name = Path::new(raw)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{raw} is not a file name"))?;
        if name.starts_with('.') {
            return Err(format!("{name} is hidden"));
        }
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();
        if !SOUND.contains(&ext.as_str()) {
            return Err(format!(
                "{name} is not audio librosa opens ({})",
                SOUND.join(" ")
            ));
        }
        Ok(self.audio.join(name))
    }
}

/// Runs the one extractor, at 8 Hz, into `<name>.8hz.npy`.
fn digest(path: &Path) -> Result<(), String> {
    let out = path
        .parent()
        .map(|d| d.join(metal_schema::digest_file_name(&stem(path), metal_schema::TickRate::Base)))
        .unwrap_or_else(|| path.with_file_name(metal_schema::digest_file_name(&stem(path), metal_schema::TickRate::Base)));
    digest_to(path, &out)
}

/// [`digest`], against an explicit output path rather than the co-located
/// one the song ingest queue always wants.
///
/// S.L (`attach_digest`) is the other caller: a clip's own digest lands under
/// `data/digest/<segment id>.8hz.npy`, never beside the footage it was
/// extracted from — a `.8hz.npy` next to a video in `data/reels/**` would be
/// mistaken for a second, unrelated matrix by anything that later walks that
/// folder looking for footage. Same subprocess, same `FW_DIGESTER` override,
/// same "last line of the traceback is the diagnosis" — one digester, called
/// from two places that each pick their own destination.
pub(crate) fn digest_to(path: &Path, out: &Path) -> Result<(), String> {
    if let Some(parent) = out.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // The Rust digester always digests canonical and writes **both** levels
    // beside `out`: `<stem>.32hz.npy` (canonical) and `<stem>.8hz.npy` (its
    // base projection). `out` names the base file the caller asked for; the
    // canonical sibling is derived from its stem. A base machine reads the
    // `.8hz.npy`, a fine one the `.32hz.npy`, and switching rewrites neither.
    if std::env::var_os(DIGESTER_ENV).is_none() && std::env::var_os(PYTHON_ENV).is_none() {
        let dir = out.parent().unwrap_or_else(|| Path::new("."));
        let stem = out
            .file_name()
            .and_then(|n| n.to_str())
            .map(metal_schema::digest_stem)
            .ok_or_else(|| format!("{} has no file name", out.display()))?;
        return metal_listen::file::digest_to_levels_with_tempo(path, dir, stem).map(|_| ());
    }
    // 2026-09-30: the digester is Rust (`metal_listen::file`) — the same columns,
    // decoded by symphonia, no Python and no librosa — handled above. The
    // script is run only when asked for by name (`FW_DIGESTER` / `FW_PYTHON`),
    // which the tests and a machine that wants the reference digester do. It
    // writes the 16-wide base matrix only; there is no canonical file from it.
    let script = std::env::var(DIGESTER_ENV).unwrap_or_else(|_| DIGESTER.to_owned());
    if let Some(parent) = out.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let interpreter = digest_interpreter();
    let run = Command::new(&interpreter)
        .arg(&script)
        .arg(path)
        .arg(out)
        .output()
        .map_err(|e| {
            format!(
                "cannot run {script} with {}: {e}",
                interpreter.to_string_lossy()
            )
        })?;
    if run.status.success() {
        return Ok(());
    }
    // The last line of a Python traceback is the diagnosis, and for the case
    // this box is actually in — librosa not installed — it is the whole fix.
    let err = String::from_utf8_lossy(&run.stderr);
    let last = err
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("digest failed");
    // The verbatim traceback line stays **first**: it is the diagnosis, and
    // paraphrasing "No module named 'librosa'" costs the reader the fix. The
    // one command that fixes it goes on its own line, and only for that
    // failure — another traceback gets no advice it did not ask for.
    let mut message = last.trim().to_owned();
    if message.contains("librosa") {
        message.push_str("\nrun: scripts/setup-digest");
    }
    Err(message)
}

/// The interpreter the digester runs under.
///
/// `FW_PYTHON` → the project venv `scripts/setup-digest` creates
/// (`$HOME/.cache/metalvj/venv/bin/python`, or the pre-rename cache, see
/// `metal_config::app_dir_in`) → the system `python3`. The
/// explicit override is returned verbatim even when the path does not exist;
/// the two fallbacks are returned only when the file is really there. A pure
/// function on purpose — no spawn — so the chain is unit-testable without
/// running librosa, which is the whole bug this exists to fix.
pub(crate) fn digest_interpreter() -> std::ffi::OsString {
    if let Some(explicit) = std::env::var_os(PYTHON_ENV) {
        return explicit;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let venv = metal_config::app_dir_in(&Path::new(&home).join(".cache")).join(VENV_PYTHON);
        if metal_vfs::is_file(&venv) {
            return venv.into_os_string();
        }
    }
    std::ffi::OsString::from("python3")
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_owned()
}

// ---------------------------------------------------------------------------
// the listing
// ---------------------------------------------------------------------------

/// Rows in a digested matrix, read out of the `.npy` header.
///
/// The header is ASCII inside the first 128 bytes and carries the shape, which
/// is all this needs — the same trick `index_corpus.py` uses, and for the same
/// reason: reading the total tick count must not require numpy.
pub fn matrix_ticks(path: &Path) -> Option<usize> {
    memo(&MATRIX_MEMO, path, || {
        let head = metal_vfs::read(path).ok()?;
        let head = head.get(..256.min(head.len()))?;
        let text = String::from_utf8_lossy(head);
        let at = text.find("'shape': (")? + 10;
        text[at..].split(',').next()?.trim().parse().ok()
    })
}

/// One answer per file, kept while the file's length and mtime stay the same.
/// The listing asks for every song on every call; a digest matrix is megabytes,
/// so re-reading it each time was the whole cost of a focus switch.
type Stamp = (PathBuf, u64, Option<std::time::SystemTime>);
type Memo<T> = Mutex<std::collections::BTreeMap<Stamp, T>>;

static MATRIX_MEMO: Memo<Option<usize>> = Mutex::new(std::collections::BTreeMap::new());
static TRACK_MEMO: Memo<Option<(usize, usize)>> = Mutex::new(std::collections::BTreeMap::new());

fn memo<T: Clone>(cache: &Memo<T>, path: &Path, read: impl FnOnce() -> T) -> T {
    let (len, modified) = metal_vfs::metadata(path)
        .map(|m| (m.len(), m.modified().ok()))
        .unwrap_or((0, None));
    let key = (path.to_path_buf(), len, modified);
    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(&key) {
            return hit.clone();
        }
    }
    let value = read();
    if let Ok(mut map) = cache.lock() {
        map.insert(key, value.clone());
    }
    value
}

/// Ticks with anything declared in them, and how many there are in total.
///
/// **Progress is any keyframe of nodes that is present** (the operator): a song is in
/// progress from its first authored tick, and there is no manual "done" flag to
/// go stale. The total is the *song's* length, not the track's, so a
/// half-authored take reads as half-authored rather than as complete.
pub fn authored(track: &Path) -> Option<(usize, usize)> {
    memo(&TRACK_MEMO, track, || {
        let text = metal_vfs::read_to_string(track).ok()?;
        let parsed: metal_rig::frame::Track = serde_json::from_str(&text).ok()?;
        let keyed = parsed.frames.iter().filter(|f| !f.is_empty()).count();
        Some((keyed, parsed.frames.len()))
    })
}

/// Every example the folder knows about, with what is left to do on it.
///
/// **The folder is the manifest.** No separate index file that can drift out of
/// sync with what is actually on disk — this walks `data/audio` and matches
/// each song to its matrix and its track, exactly as `index_corpus.py` scans
/// it, and it is read fresh on every request so a song digested while the app
/// is running shows up in it.
pub fn songs(dir: &Path, tracks: &Path, queue: &HashMap<String, State>) -> Vec<Value> {
    let mut out: Vec<Value> = metal_vfs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| metal_vfs::is_file(&p))
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            SOUND.iter().any(|e| n.to_lowercase().ends_with(e))
        })
        .map(|audio| {
            let name = stem(&audio);
            let matrix = dir.join(metal_schema::digest_file_name(&name, metal_schema::TickRate::Base));
            let track = tracks.join(format!("{name}.track.json"));
            let total = metal_vfs::is_file(&matrix).then(|| matrix_ticks(&matrix)).flatten();
            let progress = authored(&track);
            let queued = queue.get(&name);

            // The queue wins where it has something to say, because a digest
            // in flight has written nothing and the folder cannot see it.
            let status = match (queued, total.is_some(), progress) {
                (Some(s), _, _) => s.name(),
                (None, false, _) => "undigested",
                (None, true, Some((keyed, _))) if keyed > 0 => "in progress",
                (None, true, _) => "digested",
            };
            json!({
                "name": name,
                "audio": audio.display().to_string(),
                "track": track.display().to_string(),
                "status": status,
                "note": match queued { Some(State::Failed(e)) => Some(e.clone()), _ => None },
                "ticks": total,
                "keyed": progress.map(|(k, _)| k),
                // An 8 Hz-only digest (no canonical sibling) is held as steps;
                // the timeline/tick page badges it "base-only".
                "base_only": metal_router::npy::is_base_only(dir, &name),
                "seconds": total.map(|t| t as f64 / RATE),
            })
        })
        .collect();
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("metal-ingest-{name}"));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(dir.join("audio")).unwrap();
        metal_vfs::create_dir_all(dir.join("track")).unwrap();
        dir
    }

    /// The property the whole module is shaped around: a stream is chunks, and
    /// nothing here needs the whole file at once. A microphone is this test
    /// with more iterations.
    #[test]
    fn a_stream_is_chunks_and_the_file_is_whatever_they_add_up_to() {
        let dir = scratch("chunks");
        let ingest = Ingest::start(&dir.join("audio"));
        ingest.chunk("take.wav", b"RIFF", false).unwrap();
        ingest.chunk("take.wav", b"....", false).unwrap();
        let path = ingest.chunk("take.wav", b"WAVE", true).unwrap();
        assert_eq!(metal_vfs::read(&path).unwrap(), b"RIFF....WAVE");
    }

    /// A file upload is the degenerate stream: one chunk, and it closes.
    #[test]
    fn a_whole_file_is_one_chunk_that_closes() {
        let dir = scratch("whole");
        let ingest = Ingest::start(&dir.join("audio"));
        let path = ingest.chunk("song.mp3", b"ID3 whatever", true).unwrap();
        assert!(path.ends_with("song.mp3"));
        assert_eq!(metal_vfs::read(&path).unwrap(), b"ID3 whatever");
    }

    /// The browser names this file, so this is the one request that could write
    /// outside the corpus if it were trusted.
    #[test]
    fn an_upload_cannot_name_a_path_outside_the_corpus() {
        let dir = scratch("escape");
        let ingest = Ingest::start(&dir.join("audio"));
        // A traversal keeps only its file name, so it lands inside anyway.
        let path = ingest.chunk("../../etc/passwd.wav", b"x", true).unwrap();
        assert_eq!(path, dir.join("audio").join("passwd.wav"));
        assert!(ingest.chunk(".hidden.wav", b"x", true).is_err());
        assert!(ingest.chunk("notes.txt", b"x", true).is_err(), "not audio");
        assert!(ingest.chunk("song.exe", b"x", true).is_err());
    }

    /// The folder is the manifest, and the three statuses come out of it: raw
    /// audio is undigested, a matrix with no track is digested, and a track
    /// with any tick keyed is in progress.
    #[test]
    fn the_listing_reads_the_three_statuses_off_the_folder() {
        use metal_rig::frame::{Frame, Track};
        let dir = scratch("listing");
        let (audio, tracks) = (dir.join("audio"), dir.join("track"));
        metal_vfs::write(audio.join("raw.wav"), b"x").unwrap();
        metal_vfs::write(audio.join("ready.wav"), b"x").unwrap();
        metal_vfs::write(audio.join("started.wav"), b"x").unwrap();
        // A minimal .npy header, which is all `matrix_ticks` reads.
        let npy = |n: usize| {
            format!("\u{93}NUMPY\u{1}\u{0}v\u{0}{{'descr': '<f4', 'fortran_order': False, 'shape': ({n}, 16), }}")
        };
        metal_vfs::write(audio.join(metal_schema::digest_file_name("ready", metal_schema::TickRate::Base)), npy(80)).unwrap();
        metal_vfs::write(audio.join(metal_schema::digest_file_name("started", metal_schema::TickRate::Base)), npy(80)).unwrap();

        let track = Track {
            name: "started".into(),
            rig: "koreo.canonical".into(),
            joints: 142,
            rate: 8.0,
            frames: vec![
                Frame::default().turn(5, glam::Vec3::Y),
                Frame::default(),
                Frame::default(),
            ],
        };
        metal_vfs::write(
            tracks.join("started.track.json"),
            serde_json::to_string(&track).unwrap(),
        )
        .unwrap();

        let rows = songs(&audio, &tracks, &HashMap::new());
        let by = |name: &str| rows.iter().find(|r| r["name"] == name).unwrap().clone();
        assert_eq!(rows.len(), 3);
        assert_eq!(by("raw")["status"], "undigested");
        assert_eq!(
            by("raw")["ticks"],
            Value::Null,
            "an undigested song has no length yet"
        );
        assert_eq!(by("ready")["status"], "digested");
        assert_eq!(by("ready")["ticks"], 80);
        assert_eq!(by("started")["status"], "in progress");
        assert_eq!(by("started")["keyed"], 1, "one tick has a node in it");
        assert_eq!(by("started")["seconds"], 10.0);
    }

    /// The success path, which librosa's absence would otherwise leave
    /// permanently unexercised: the worker runs the digester, the matrix lands
    /// beside the audio, the queue goes quiet and the row reads its own length
    /// off the `.npy` header. A stand-in digester, because what is being
    /// checked is the plumbing and not librosa's arithmetic.
    #[test]
    fn a_digest_that_succeeds_lands_a_matrix_and_the_row_reads_it() {
        let dir = scratch("digested");
        let (audio, tracks) = (dir.join("audio"), dir.join("track"));
        let fake = dir.join("fake_digest.py");
        metal_vfs::write(
            &fake,
            "import sys, numpy as np\nnp.save(sys.argv[2], np.zeros((24, 16), dtype='float32'))\n",
        )
        .unwrap();
        metal_vfs::write(audio.join("added.wav"), b"x").unwrap();

        // Set for this process, which the worker thread inherits.
        unsafe { std::env::set_var(DIGESTER_ENV, &fake) };
        let ingest = Ingest::start(&audio);
        ingest.digest_now("added.wav").unwrap();

        let mut rows = Vec::new();
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            rows = songs(&audio, &tracks, &ingest.states());
            if rows[0]["status"] != "queued" && rows[0]["status"] != "digesting" {
                break;
            }
        }
        unsafe { std::env::remove_var(DIGESTER_ENV) };
        assert_eq!(rows[0]["status"], "digested", "note: {}", rows[0]["note"]);
        assert_eq!(rows[0]["ticks"], 24, "the row read the matrix's own length");
        assert_eq!(rows[0]["seconds"], 3.0, "24 ticks at 8 Hz");
        assert!(metal_vfs::is_file(&audio.join(metal_schema::digest_file_name("added", metal_schema::TickRate::Base))));
    }

    /// The interpreter chain, without spawning anything: the explicit override
    /// wins verbatim (even when the path does not exist), then the project
    /// venv, then the system `python3`.
    #[test]
    fn the_interpreter_follows_the_explicit_venv_system_chain() {
        let dir = scratch("interp-chain");
        let prior_python = std::env::var_os(PYTHON_ENV);
        let prior_home = std::env::var_os("HOME");

        // 1. `FW_PYTHON` is an instruction, not a hint: used even if missing.
        let explicit = dir.join("no-such-python");
        unsafe { std::env::set_var(PYTHON_ENV, &explicit) };
        assert_eq!(digest_interpreter(), explicit.into_os_string());

        // 2. With no override, the venv `scripts/setup-digest` makes wins.
        unsafe { std::env::remove_var(PYTHON_ENV) };
        unsafe { std::env::set_var("HOME", &dir) };
        let python = metal_config::app_dir_in(&dir.join(".cache")).join(VENV_PYTHON);
        metal_vfs::create_dir_all(python.parent().unwrap()).unwrap();
        metal_vfs::write(&python, b"#!/bin/sh\n").unwrap();
        assert_eq!(digest_interpreter(), python.clone().into_os_string());

        // 3. Neither present: the system `python3` is the last resort.
        metal_vfs::remove_file(&python).unwrap();
        assert_eq!(digest_interpreter(), std::ffi::OsString::from("python3"));

        match prior_python {
            Some(v) => unsafe { std::env::set_var(PYTHON_ENV, v) },
            None => unsafe { std::env::remove_var(PYTHON_ENV) },
        }
        match prior_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    /// The chain is not only resolved but **used**: the worker runs the
    /// `FW_PYTHON` interpreter, proven by a fake one that writes its own
    /// `$0` (its path) to the output the digester was told to write.
    #[cfg(unix)]
    #[test]
    fn the_worker_runs_the_interpreter_the_chain_resolves() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("interp-runs");
        let audio = dir.join("audio");
        let interp = dir.join("fake-python");
        metal_vfs::write(&interp, "#!/bin/sh\necho \"$0\" > \"$3\"\n").unwrap();
        let mut perms = metal_vfs::metadata(&interp).unwrap().permissions();
        perms.set_mode(0o755);
        metal_vfs::set_permissions(&interp, perms).unwrap();
        metal_vfs::write(audio.join("chain.wav"), b"x").unwrap();

        let prior_python = std::env::var_os(PYTHON_ENV);
        unsafe { std::env::set_var(PYTHON_ENV, &interp) };
        let ingest = Ingest::start(&audio);
        ingest.digest_now("chain.wav").unwrap();

        let out = audio.join(metal_schema::digest_file_name("chain", metal_schema::TickRate::Base));
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if metal_vfs::is_file(&out) {
                break;
            }
        }
        let written = metal_vfs::read_to_string(&out).unwrap_or_default();
        match prior_python {
            Some(v) => unsafe { std::env::set_var(PYTHON_ENV, v) },
            None => unsafe { std::env::remove_var(PYTHON_ENV) },
        }
        assert_eq!(
            written.trim(),
            interp.to_string_lossy(),
            "the worker must run the FW_PYTHON interpreter"
        );
    }

    /// A missing librosa is the one failure the card is expected to name the
    /// fix for. The traceback's own last line stays first and verbatim; the
    /// one command goes on its own line under it.
    #[test]
    fn a_missing_librosa_keeps_the_verbatim_line_and_adds_the_fix() {
        let dir = scratch("librosa-hint");
        let fake = dir.join("fake_digest.py");
        metal_vfs::write(
            &fake,
            "import sys\n\
             sys.stderr.write(\"Traceback (most recent call last):\\n  \
             File \\\"digest_audio.py\\\", line 3, in <module>\\n    \
             import librosa\\nModuleNotFoundError: No module named 'librosa'\\n\")\n\
             sys.exit(1)\n",
        )
        .unwrap();
        let input = dir.join("song.wav");
        metal_vfs::write(&input, b"x").unwrap();
        let out = dir.join(metal_schema::digest_file_name("song", metal_schema::TickRate::Base));

        let prior = std::env::var_os(DIGESTER_ENV);
        unsafe { std::env::set_var(DIGESTER_ENV, &fake) };
        let err = digest_to(&input, &out).unwrap_err();
        match prior {
            Some(v) => unsafe { std::env::set_var(DIGESTER_ENV, v) },
            None => unsafe { std::env::remove_var(DIGESTER_ENV) },
        }
        assert_eq!(
            err,
            "ModuleNotFoundError: No module named 'librosa'\nrun: scripts/setup-digest"
        );
    }

    /// Any other failure is its own diagnosis; it gets no advice it did not
    /// ask for.
    #[test]
    fn a_failure_that_is_not_librosa_gets_no_hint() {
        let dir = scratch("no-hint");
        let fake = dir.join("fake_digest.py");
        metal_vfs::write(
            &fake,
            "import sys\nsys.stderr.write('ValueError: cannot open the file\\n')\nsys.exit(1)\n",
        )
        .unwrap();
        let input = dir.join("song.wav");
        metal_vfs::write(&input, b"x").unwrap();
        let out = dir.join(metal_schema::digest_file_name("song", metal_schema::TickRate::Base));

        let prior = std::env::var_os(DIGESTER_ENV);
        unsafe { std::env::set_var(DIGESTER_ENV, &fake) };
        let err = digest_to(&input, &out).unwrap_err();
        match prior {
            Some(v) => unsafe { std::env::set_var(DIGESTER_ENV, v) },
            None => unsafe { std::env::remove_var(DIGESTER_ENV) },
        }
        assert_eq!(err, "ValueError: cannot open the file");
    }

    /// The corpus is full of names the shell has to round-trip verbatim: a
    /// fullwidth `？` (songs 09, 40), curly `“ ”` (song 20) and a plain `'`
    /// (song 16). `add` writes the file under the name the caller sent and
    /// `open` derives the track from the same `file_stem`; this pins that both
    /// halves agree on the awkward names, through the real code rather than a
    /// re-implementation.
    #[test]
    fn an_add_and_an_open_agree_on_the_stem_of_an_awkward_name() {
        let dir = scratch("awkward-names");
        let (audio, tracks) = (dir.join("data/audio"), dir.join("data/tracks"));
        metal_vfs::create_dir_all(&audio).unwrap();
        metal_vfs::create_dir_all(&tracks).unwrap();
        let names = [
            "09 - Can I Kick It？.mp3",
            "20 - Cuenta Bien - Edwin Bonilla - Jesús “El Niño” Pérez..mp3",
            "16 - Pa' Que Retozen.mp3",
        ];
        let ingest = Ingest::start(&audio);
        for name in names {
            let path = ingest.chunk(name, b"ID3", true).unwrap();
            assert_eq!(path.file_name().unwrap().to_str().unwrap(), name);
        }

        // `choose` resolves `data/audio`/`data/tracks` against the cwd, so
        // point the process at the scratch root for the call. `chdir_lock` is
        // the same serializer `call::door`'s own test uses.
        let prior = std::env::current_dir().unwrap();
        let _chdir = crate::chdir_lock();
        std::env::set_current_dir(&dir).unwrap();
        for name in names {
            let chosen = choose(serde_json::json!({ "audio": name }).to_string().as_bytes())
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let stem = stem(Path::new(name));
            assert_eq!(
                chosen.track,
                Path::new("data/tracks").join(format!("{stem}.track.json")),
                "{name}: the track stem must be the one the add keyed"
            );
        }
        std::env::set_current_dir(prior).unwrap();
    }

    /// A file bigger than the old browser's 50 MB line, uploaded as a chunked
    /// `last=1` stream: every chunk must append and the closed file must be
    /// exactly what the chunks add up to.
    #[test]
    fn a_body_over_fifty_megabytes_arrives_whole_in_chunks() {
        let dir = scratch("big-body");
        let ingest = Ingest::start(&dir.join("audio"));
        let chunk = vec![7u8; 17 * 1024 * 1024];
        let total = 51 * 1024 * 1024;
        let last = vec![9u8; total - 2 * chunk.len()];
        ingest.chunk("big.wav", &chunk, false).unwrap();
        ingest.chunk("big.wav", &chunk, false).unwrap();
        let path = ingest.chunk("big.wav", &last, true).unwrap();
        assert_eq!(metal_vfs::metadata(&path).unwrap().len(), total as u64);
    }

    /// A digest in flight has written nothing, so the folder cannot see it and
    /// the queue has to be the one that says so.
    #[test]
    fn the_queue_speaks_for_what_the_folder_cannot_see() {
        let dir = scratch("queue");
        let (audio, tracks) = (dir.join("audio"), dir.join("track"));
        metal_vfs::write(audio.join("new.wav"), b"x").unwrap();
        let mut queue = HashMap::new();
        queue.insert("new".to_owned(), State::Running);
        assert_eq!(songs(&audio, &tracks, &queue)[0]["status"], "digesting");

        queue.insert(
            "new".to_owned(),
            State::Failed("No module named 'librosa'".into()),
        );
        let row = songs(&audio, &tracks, &queue)[0].clone();
        assert_eq!(row["status"], "failed");
        // Verbatim, because paraphrasing "no module named librosa" costs the
        // reader the fix.
        assert_eq!(row["note"], "No module named 'librosa'");
    }

    // -- G2: trim storage, reused rather than reinvented ---------------------

    /// A song with no entry is "all of it" — `{in: 0, out: null}` — and that
    /// has to hold before anything is ever written, not just after a round
    /// trip.
    #[test]
    fn a_song_with_no_trim_entry_reads_as_the_whole_song() {
        let dir = scratch("trim-absent");
        let path = dir.join("trims.json");
        assert_eq!(
            trim_of_at(&path, "nothing-set"),
            serde_json::json!({ "in": 0, "out": Value::Null })
        );
    }

    /// The round trip Conjunto G2 needs: set a trim, read it back through the
    /// exact same storage — against a scratch file, never `data/trims.json`,
    /// so this cannot race a live show's own trims.
    #[test]
    fn a_trim_round_trips_through_the_shared_storage() {
        let dir = scratch("trim-roundtrip");
        let path = dir.join("trims.json");
        let body = serde_json::json!({ "in": 40, "out": 200 }).to_string();
        set_trim_at(&path, "track-a", body.as_bytes()).expect("a valid trim is accepted");
        assert_eq!(
            trim_of_at(&path, "track-a"),
            serde_json::json!({ "in": 40, "out": 200 })
        );
        // A second song in the same file must not see the first one's marks.
        assert_eq!(
            trim_of_at(&path, "track-b"),
            serde_json::json!({ "in": 0, "out": Value::Null })
        );

        // Setting it back to "all of it" removes the entry rather than
        // writing a row that means nothing — same sparse-file rule `set_trim`
        // always followed, exercised here through the path-parameterised
        // twin `nodes::audio_file` calls.
        let cleared = serde_json::json!({ "in": 0 }).to_string();
        set_trim_at(&path, "track-a", cleared.as_bytes()).expect("clearing is valid");
        assert_eq!(
            trim_of_at(&path, "track-a"),
            serde_json::json!({ "in": 0, "out": Value::Null })
        );
        let stored = trims_at(&path);
        assert!(
            !stored.contains_key("track-a"),
            "a cleared trim must not linger as a no-op row"
        );
    }

    /// The same guard `set_trim`'s HTTP route relies on, exercised directly:
    /// an out mark at or before the in mark is refused rather than accepted
    /// as an empty passage nobody asked for.
    #[test]
    fn an_out_mark_at_or_before_the_in_mark_is_refused() {
        let dir = scratch("trim-bad-range");
        let path = dir.join("trims.json");
        let body = serde_json::json!({ "in": 100, "out": 100 }).to_string();
        let err = set_trim_at(&path, "track-a", body.as_bytes()).unwrap_err();
        assert!(err.contains("must be after"), "{err}");
        // And nothing was written.
        assert_eq!(
            trim_of_at(&path, "track-a"),
            serde_json::json!({ "in": 0, "out": Value::Null })
        );
    }
}

// ---------------------------------------------------------------------------
// the shared routes
// ---------------------------------------------------------------------------
//
// One audio library for both modes. These four routes were duplicated across
// the rig studio and the float studio — same paths, same behaviour, two
// implementations — which meant a song added in one was invisible in the other
// until you uploaded it twice. There is one of them now, and it is here rather
// than in either mode because it belongs to neither.

// the cleanup: the raw-socket `serve` dispatcher that used to live here moved to
// `crate::call::author::audio` as typed handler functions over
// [`trim_of_at`]/[`set_trim_at`]/[`choose`]/[`mime`] below, which are
// unchanged (`pub(crate)` now so that sibling module tree can reach them).

/// Where the in/out marks live. One file for the whole library rather than a
/// sidecar per song: they are two integers, and a hundred two-integer files in
/// the audio folder would be a hundred things that are not audio in the folder
/// whose whole contract is that everything in it is.
pub const TRIMS: &str = "data/trims.json";

/// Against any file — what lets a test (or `nodes::audio_file`,
/// Conjunto G2) read the trim store without going anywhere near the real
/// `data/trims.json` a live show has open.
pub(crate) fn trims_at(path: &Path) -> serde_json::Map<String, Value> {
    metal_vfs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// `{in, out}` in ticks. `out: null` means "to the end", which is different
/// from "0" and is what a song with no out mark has.
/// Against any file — see the const-path note above.
pub(crate) fn trim_of_at(path: &Path, song: &str) -> Value {
    match trims_at(path).get(song) {
        Some(v) => serde_json::json!({
            "in": v.get("in").and_then(Value::as_u64).unwrap_or(0),
            "out": v.get("out").and_then(Value::as_u64),
        }),
        None => serde_json::json!({ "in": 0, "out": Value::Null }),
    }
}

/// Against any file. The validation (a name
/// that cannot escape the folder, an out mark that is not before its in mark)
/// is the storage's own rule and belongs here rather than being re-decided by
/// every caller, `/author/trim` and `nodes::audio_file` alike.
pub(crate) fn set_trim_at(path: &Path, song: &str, body: &[u8]) -> Result<Value, String> {
    if song.is_empty() || song.contains('/') || song.contains('\\') || song.starts_with('.') {
        return Err(format!("{song:?} is not a song name"));
    }
    let posted: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let start = posted.get("in").and_then(Value::as_u64).unwrap_or(0);
    let end = posted.get("out").and_then(Value::as_u64);
    // An out mark at or before the in mark is not a shorter passage, it is an
    // empty one — refused here rather than discovered as a silent export.
    if let Some(e) = end {
        if e <= start {
            return Err(format!("out ({e}) must be after in ({start})"));
        }
    }
    let mut all = trims_at(path);
    if start == 0 && end.is_none() {
        // No marks is the absence of an entry, not an entry saying so. The file
        // is sparse for the same reason the notation is: most songs are played
        // whole, and a row per song saying "all of it" is a row that means
        // nothing and still has to be read, merged and kept in step.
        all.remove(song);
    } else {
        all.insert(
            song.to_string(),
            serde_json::json!({ "in": start, "out": end }),
        );
    }
    if let Some(parent) = path.parent() {
        let _ = metal_vfs::create_dir_all(parent);
    }
    let text = serde_json::to_string_pretty(&Value::Object(all)).map_err(|e| e.to_string())?;
    crate::atomic::write(path, format!("{text}\n")).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "in": start, "out": end }))
}
/// Switches what is open, from a `{ "audio": …, "track": … }` body.
///
/// Both paths are re-resolved against the library's own directories rather
/// than trusted, so the browser cannot ask the server to read `/etc/shadow`
/// or write a track over a source file. A sequence that does not exist yet is
/// allowed — that is how a new one is started — but it still has to land in
/// `data/tracks` under a plain file name.
pub(crate) fn choose(body: &[u8]) -> Result<crate::author::Open, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let inside = |dir: &str, raw: &str| -> Result<PathBuf, String> {
        let name = Path::new(raw)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{raw} is not a file name"))?;
        if name.starts_with('.') {
            return Err(format!("{name} is hidden"));
        }
        Ok(Path::new(dir).join(name))
    };
    let audio = match v.get("audio").and_then(Value::as_str) {
        Some(a) if !a.is_empty() => {
            let p = inside(crate::author::SONGS, a)?;
            if !metal_vfs::is_file(&p) {
                return Err(format!("no such song: {}", p.display()));
            }
            Some(p)
        }
        _ => None,
    };
    let track = match v.get("track").and_then(Value::as_str) {
        Some(t) if !t.is_empty() => inside(crate::author::TRACKS, t)?,
        // No sequence named, so the one beside the song. Same rule the
        // `scripts/studio` wrapper follows when it derives a track path.
        _ => {
            let stem = audio
                .as_deref()
                .and_then(|p| p.file_stem())
                .and_then(|s| s.to_str())
                .unwrap_or("untitled");
            Path::new(crate::author::TRACKS).join(format!("{stem}.track.json"))
        }
    };
    Ok(crate::author::Open { audio, track })
}

pub(crate) fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "m4a" => "audio/mp4",
        _ => "application/octet-stream",
    }
}
