//! The app's side of a transmission: the library, the two models, and the
//! standing cue.
//!
//! `metal-reel` is the deciding — the descriptor, the index, the Curator and the
//! Colourist, none of which know there is an app. This module is the part that
//! is about *this* app: where the folder is, what is on right now, and the fact
//! that indexing three hours of rushes cannot happen on the thread serving the
//! page.
//!
//! # The one thing a transmission needs that no other model does
//!
//! > *"every source component of this new type will have a UI component to
//! > require a library, which is the directory."*
//!
//! Thirty models are ready the moment the binary starts. A transmission is not:
//! it needs a folder, and a folder is a thing an operator chooses at a venue.
//! That is a genuinely new state in this app — *selectable, but with nothing to
//! draw* — and everything here exists to make it visible rather than to hide
//! it:
//!
//! - `Model::footage` is in `data/schema.json`, so the page knows which cards
//!   need a library **on the build with no GPU too**;
//! - the model draws its own slate rather than a black frame, so "no folder" and
//!   "broken" do not look alike;
//! - [`Job`] reports indexing as it goes, because pointing this at an hour of
//!   footage is minutes of ffmpeg and a page that just sat there would read as
//!   a hang.
//!
//! # Why the cue is refreshed here and not in the renderer
//!
//! One digest, one Curator, one cue **per library** — and then every screen
//! showing that library is handed the same one. Two screens deciding for
//! themselves would drift apart within a phrase, and the venue would be
//! watching two different shots of the same folder. It is the same argument
//! `OVERLAYS.md` §2 makes for one digest, one level down.
//!
//! # A library is per source, not per app
//!
//! > *"footage selection should be per source and not global, because some
//! > sources may work with one folder and others with another."*
//!
//! So this holds **several open libraries at once**, each with its own standing
//! cue and its own [`Recent`]. A source names one; a source that names none
//! gets [`Reels::default_library`], which is what the metraje sheet sets and
//! what every source used to share.
//!
//! The cue and the cooldown are per library rather than per source on purpose.
//! Two sources pointed at one folder are two screens of one venue and must
//! agree — that is the paragraph above. Two sources pointed at *different*
//! folders have nothing to agree about, and sharing a `Recent` between them
//! would make one source's choices suppress the other's for four minutes.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use metal_reel::curate::{self, Curator, Memory, Recent, FEATURES};
use metal_reel::grade::Colourist;
use metal_reel::index::{self, Library, Options, Step};
use metal_reel::Cue;
use metal_schema::Schema;

/// Where the digested songs are, under the data directory.
///
/// The same folder `author::SONGS` names, said again here because that constant
/// is relative to the working directory and this side of the app addresses
/// everything from `output::data_root()`. A pick is only a moment if its song's
/// digest can be found.
const SONGS: &str = "audio";

/// How an indexing run is going, as the page reads it.
#[derive(Clone, Debug, Default)]
pub struct Job {
    pub library: String,
    pub source: String,
    /// One line, already worded for a human.
    pub step: String,
    /// 0..1 over the files in the folder. Files, not seconds: the app knows how
    /// many files there are before it starts and does not know how long they
    /// are until it has probed them, and a bar that jumps backwards when the
    /// second clip turns out to be an hour long is worse than a coarse one.
    pub progress: f32,
    pub done: bool,
    pub error: Option<String>,
    /// Named files that were left out, and why.
    pub skipped: Vec<String>,
    pub clips: usize,
    pub rungs: usize,
    pub bytes: u64,
    /// Whether [`Reels::collect`] has already opened the library this run
    /// built. Not reported — it exists so a finished run is acted on **once**
    /// and then **stays on the page**.
    ///
    /// The first version took the job out of the slot when it collected it, and
    /// the report went with it: the operator got "8 clips indexed" and the list
    /// of files that were skipped for about one poll, and then a blank line.
    /// Which files did not make it into a library is exactly the thing somebody
    /// needs to read *after* it finishes.
    opened: bool,
}

/// One library that is open, and what is on in it.
struct Open {
    library: Arc<Library>,
    cue: Option<Cue>,
    /// What has been cued lately *out of this folder*. Per library, so one
    /// source's choices do not put another folder's passages on cooldown.
    recent: Recent,
    /// What the operator authored over this folder, resolved once when it
    /// opened. Read on every cue and never off disk: `cue` runs per source per
    /// frame, and this is the surface `INFLIGHT=3` exists to protect.
    memory: Vec<Memory>,
    /// The picks that would not resolve, in the words `curate::recall` used.
    /// Kept and reported rather than dropped — a lane that has silently stopped
    /// working is the thing an operator cannot see from the booth.
    unresolved: Vec<String>,
}

/// Everything about footage that this app owns.
pub struct Reels {
    data: PathBuf,
    /// Every library some source has asked for, loaded once and kept.
    ///
    /// A `Vec` rather than a map: it holds one entry per *distinct folder in
    /// use*, which is a handful, and the lookup is a string compare against
    /// that handful once per source per frame.
    open: Vec<Open>,
    /// What a source that names no library of its own gets.
    default: Option<String>,
    pub curator: Curator,
    /// One per model id that is a transmission, loaded on demand.
    colourists: Vec<(String, Colourist)>,
    /// The indexing run, if one is going. Shared with the thread doing it.
    job: Arc<Mutex<Option<Job>>>,
    /// A library that should become the default the moment the run that
    /// builds it finishes — `open_lazy`'s own memory. `collect` consumes it,
    /// so `POST /reel {op:"open", path}` on a folder nobody has indexed yet
    /// opens without the operator ever seeing an "index" step.
    pending_open: Option<String>,
}

impl Reels {
    pub fn new(data: &std::path::Path) -> Reels {
        Reels {
            data: data.to_path_buf(),
            open: Vec::new(),
            default: None,
            curator: Curator::load(data),
            colourists: Vec::new(),
            job: Arc::new(Mutex::new(None)),
            pending_open: None,
        }
    }

    /// Opens the library `name`, indexing `path` as `name` first if no such
    /// library exists yet — `POST /reel {op:"open", path}`'s lazy half.
    ///
    /// A folder that is already indexed opens straight away; one that is not
    /// starts an index job and remembers `name` in `pending_open`, so the
    /// `collect` that already runs every frame (`GET /state`) opens it the
    /// moment the run finishes. The operator sees a job's progress and never
    /// an "index" of their own.
    ///
    /// Returns whether a job was started (`true`) or the library opened at
    /// once (`false`).
    pub fn open_lazy(&mut self, path: PathBuf, name: String) -> Result<bool, String> {
        // Already open: nothing to load, only to make it the default.
        if self.slot(Some(&name)).is_some() {
            self.default = Some(name);
            return Ok(false);
        }
        // Indexed on disk already: open it rather than rebuild it.
        if self.available().iter().any(|n| n == &name) {
            self.open(Some(&name))?;
            return Ok(false);
        }
        // A fresh folder: index it, and let `collect` open it on finish.
        self.index(path, name.clone(), Options::default())?;
        self.pending_open = Some(name);
        Ok(true)
    }

    /// The open library whose source folder is `path`, if one is open — a
    /// read-only lookup for `/fs`, which reads a clip's length off it rather
    /// than probing the file (rule 5: the number comes from the index).
    pub fn library_for_source(&self, path: &std::path::Path) -> Option<&Arc<Library>> {
        let want = metal_vfs::canonicalize(&path).unwrap_or_else(|_| path.to_path_buf());
        self.open.iter().map(|o| &o.library).find(|l| {
            let src = std::path::Path::new(&l.source);
            metal_vfs::canonicalize(&src).unwrap_or_else(|_| src.to_path_buf()) == want
        })
    }

    /// Everything the operator authored over `lib`, resolved, and everything
    /// they authored that no longer resolves.
    ///
    /// Reading `data/audio/<song>.8hz.npy` is what makes a pick mean a moment
    /// rather than a tick number — see `curate::choose_with`. It happens **once
    /// per library, when it opens**, because the alternative is a digest load
    /// per source per frame.
    ///
    /// A library with no `picks/` folder is the normal state and costs one
    /// `read_dir` that fails. Nothing here can make a cue worse: with no
    /// memories `choose_with` is `choose`.
    ///
    /// What it does cost, said out loud: one digest read per *song* the folder
    /// has picks for, on the frame a source first names it. A night's worth of
    /// authoring is a few megabytes read once. It is on the same thread that
    /// serves the page, which is why it is here — at the open — and not in
    /// `cue`, where it would be that read on every frame forever.
    fn remember(&self, lib: &Library) -> (Vec<Memory>, Vec<String>) {
        let picks = curate::load_picks(&self.data, &lib.name);
        if picks.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let songs = self.data.join(SONGS);
        curate::recall(&picks, lib, &mut |song| {
            // These strings come off disk. `POST /curator/picks` refuses a song
            // name that is not a single path segment, but a hand-edited file has
            // not been through that route, and this is a path being built out of
            // one.
            if song.is_empty() || song.starts_with('.') || song.contains('/') || song.contains('\\')
            {
                return None;
            }
            metal_router::npy::load(&metal_router::npy::base_path(&songs, song)).ok()
        })
    }

    /// Re-reads the picks of every open library.
    ///
    /// The studio saves picks through `POST /curator/picks`, which writes the
    /// file and knows nothing about this struct — the booth is holding a
    /// resolved copy from when the folder opened. So authoring a lane and
    /// hearing it takes the same `{"action":"reload"}` that re-reads a retrained
    /// coupling, and for the same reason.
    pub fn relearn(&mut self) {
        for i in 0..self.open.len() {
            let lib = Arc::clone(&self.open[i].library);
            let (memory, unresolved) = self.remember(&lib);
            self.open[i].memory = memory;
            self.open[i].unresolved = unresolved;
        }
    }

    /// The library a source called `name` should draw, or the default if it
    /// named none. `None` for both means there is nothing to draw.
    pub fn library(&self, name: Option<&str>) -> Option<&Arc<Library>> {
        self.slot(name).map(|i| &self.open[i].library)
    }

    /// Which entry of `open` serves `name`, loading nothing.
    fn slot(&self, name: Option<&str>) -> Option<usize> {
        let want = name.filter(|s| !s.is_empty()).or(self.default.as_deref())?;
        self.open.iter().position(|o| o.library.name == want)
    }

    /// The name every source without one of its own falls back to.
    pub fn default_library(&self) -> Option<&str> {
        self.default.as_deref()
    }

    /// Opens `name` if it is not open already, and says whether it is now.
    ///
    /// Called once per source per frame, so the hit path is a string compare —
    /// the load only happens the first time a source names a folder, which is
    /// what makes a per-source library cost nothing to *have*.
    pub fn ensure(&mut self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        if self.open.iter().any(|o| o.library.name == name) {
            return true;
        }
        match Library::load(&self.data, name) {
            Ok(lib) if !lib.clips.is_empty() => {
                let (memory, unresolved) = self.remember(&lib);
                self.open.push(Open {
                    library: Arc::new(lib),
                    cue: None,
                    recent: Recent::default(),
                    memory,
                    unresolved,
                });
                true
            }
            // A source naming a folder that is gone draws its slate, which says
            // on its face that there is no footage. Loading it every frame
            // afterwards would be a file-system hit per source per frame, so
            // the miss is simply reported and nothing is remembered.
            _ => false,
        }
    }

    /// Every library that has been indexed under the data directory.
    pub fn available(&self) -> Vec<String> {
        Library::list(&self.data)
    }

    /// The files inside `name` — id, source filename and length, and nothing
    /// else: no rungs, no passages. `Library::load` parses the whole ladder
    /// back off disk to get at that, which for a folder of any size is
    /// indexing's own expensive half all over again just to list what is in
    /// it. A folder some source has already `ensure`d answers from that copy
    /// instead — the same `Arc<Library>` `cue` already reads every frame,
    /// borrowed rather than reloaded — so browsing a folder someone put on
    /// air a minute ago costs nothing extra.
    pub fn files(&self, name: &str) -> Result<Vec<(String, String, f32)>, String> {
        fn describe(lib: &Library) -> Vec<(String, String, f32)> {
            lib.clips
                .iter()
                .map(|c| (c.id.clone(), c.file.clone(), c.seconds))
                .collect()
        }
        if let Some(o) = self.open.iter().find(|o| o.library.name == name) {
            return Ok(describe(&o.library));
        }
        Library::load(&self.data, name).map(|lib| describe(&lib))
    }

    /// Loads one and makes it the default — what a source with no library of
    /// its own draws. `None` closes everything.
    pub fn open(&mut self, name: Option<&str>) -> Result<(), String> {
        match name {
            None => {
                // Everything, not just the default: this is the sheet's
                // "close", and a source still holding an `Arc` of a folder the
                // operator just closed would keep drawing it.
                self.open.clear();
                self.default = None;
                Ok(())
            }
            Some(name) => {
                let lib = Library::load(&self.data, name)?;
                if lib.clips.is_empty() {
                    return Err(format!("{name} has no clips in it"));
                }
                // Re-opening replaces: this is also the path a re-index takes,
                // and a cue names a clip by *index* into the library that was
                // open when it was taken. Keeping one across a rebuild is a cue
                // into whatever clip now sits at that number.
                self.open.retain(|o| o.library.name != name);
                // Resolved against the folder as it is *now*, for the same
                // reason the cue is thrown away: a memory names a clip by index
                // into the library that was open when it was built.
                let (memory, unresolved) = self.remember(&lib);
                self.open.push(Open {
                    library: Arc::new(lib),
                    cue: None,
                    recent: Recent::default(),
                    memory,
                    unresolved,
                });
                self.default = Some(name.to_string());
                Ok(())
            }
        }
    }

    /// Every library currently open, by name.
    pub fn in_use(&self) -> Vec<String> {
        self.open.iter().map(|o| o.library.name.clone()).collect()
    }

    pub fn job(&self) -> Option<Job> {
        self.job.lock().ok().and_then(|j| j.clone())
    }

    /// Acts on a finished run, once.
    ///
    /// The indexing thread never touches `self` — it only writes progress — so
    /// this is where the two meet: the run is marked as acted on and its library
    /// is opened. The job itself **stays** in the slot until the next run
    /// replaces it, so the page can go on reporting what it built and what it
    /// skipped.
    pub fn collect(&mut self) -> Option<Job> {
        let finished = {
            let mut slot = self.job.lock().ok()?;
            match slot.as_mut() {
                Some(j) if j.done && !j.opened => {
                    j.opened = true;
                    Some(j.clone())
                }
                _ => None,
            }
        }?;
        if finished.error.is_none() {
            // A lazy open (`open_lazy`) asked for `finished.library` by name,
            // so the two agree; the fallback keeps a plain index's own open
            // exactly as it was.
            let want = match self.pending_open.as_deref() {
                Some(p) if p == finished.library => self.pending_open.take().unwrap(),
                _ => finished.library.clone(),
            };
            let _ = self.open(Some(&want));
        } else if self.pending_open.as_deref() == Some(finished.library.as_str()) {
            // The run that was going to open it failed: drop the memory so a
            // later `collect` does not open a library that is not there.
            self.pending_open = None;
        }
        Some(finished)
    }

    /// Starts indexing `source` as `name`, on a thread.
    ///
    /// Refuses while one is already running. Two ffmpeg runs writing the same
    /// ladder is a library that is half of each.
    pub fn index(&mut self, source: PathBuf, name: String, opts: Options) -> Result<(), String> {
        {
            let mut slot = self.job.lock().map_err(|_| "the indexer is wedged")?;
            if matches!(slot.as_ref(), Some(j) if !j.done) {
                return Err("already indexing".into());
            }
            // Fail here rather than three seconds into a thread: a mistyped
            // path is the common case and it should answer the click.
            let files = index::footage_in(&source)?;
            if files.is_empty() {
                return Err(format!("no video files in {}", source.display()));
            }
            *slot = Some(Job {
                library: name.clone(),
                source: source.display().to_string(),
                step: format!("{} files", files.len()),
                ..Job::default()
            });
        }

        let job = Arc::clone(&self.job);
        let data = self.data.clone();
        std::thread::spawn(move || {
            let note = |f: &dyn Fn(&mut Job)| {
                if let Ok(mut slot) = job.lock() {
                    if let Some(j) = slot.as_mut() {
                        f(j);
                    }
                }
            };
            let result = index::build(&source, &data, &name, &opts, &mut |step| match step {
                Step::Probing { file, clip, of } => note(&|j| {
                    j.step = format!("reading {file}");
                    j.progress = clip as f32 / of.max(1) as f32;
                }),
                Step::Extracting { file, clip, of } => note(&|j| {
                    j.step = format!("extracting {file}");
                    j.progress = clip as f32 / of.max(1) as f32;
                }),
                Step::Reading {
                    file,
                    clip,
                    of,
                    done,
                    total,
                } => note(&|j| {
                    j.step = format!("describing {file} — {done}/{total}");
                    // Within a file, between its own slot and the next one, so
                    // the bar moves during the long part instead of once per
                    // clip.
                    let per = 1.0 / of.max(1) as f32;
                    j.progress = clip as f32 * per + per * done as f32 / total.max(1) as f32;
                }),
                Step::Skipped { file, why } => note(&|j| j.skipped.push(format!("{file}: {why}"))),
                Step::Encoding { file, clip, of } => note(&|j| {
                    j.step = format!("HAP {file}");
                    j.progress = clip as f32 / of.max(1) as f32;
                }),
                Step::Done {
                    clips,
                    rungs,
                    bytes,
                } => note(&|j| {
                    j.clips = clips;
                    j.rungs = rungs;
                    j.bytes = bytes;
                    j.progress = 1.0;
                }),
            });
            note(&|j| {
                j.done = true;
                j.step = match &result {
                    Ok(_) => format!("{} clips indexed", j.clips),
                    Err(e) => e.clone(),
                };
                if let Err(e) = &result {
                    j.error = Some(e.clone());
                }
            });
        });
        Ok(())
    }

    /// What is on, refreshed if the standing cue has stood long enough and the
    /// music has moved on.
    ///
    /// `passage` is the twelve features of the last twelve seconds —
    /// `router::Content::passage`, the *same window* the Router decides on.
    /// `None` means the input has not warmed up yet, and a neutral description
    /// is used instead: a venue that has just started should be showing
    /// footage, not a slate that says there is none.
    pub fn cue(
        &mut self,
        name: Option<&str>,
        time: f32,
        passage: Option<[f32; FEATURES]>,
    ) -> Option<Cue> {
        let at = self.slot(name)?;
        let ripe = match &self.open[at].cue {
            None => true,
            Some(c) => c.ripe(time),
        };
        if ripe {
            let f = passage.unwrap_or([0.5; FEATURES]);
            let next = {
                let o = &self.open[at];
                // `choose_with` and not `choose`: what the operator authored
                // over this folder is consulted first, and the model answers
                // when they authored nothing about this music. With an empty
                // memory the two are the same call.
                //
                // **A pick cannot interrupt a standing cue**, because this only
                // runs when the cue is ripe. That floor is `HOLD_SECONDS` and it
                // stays: it is what keeps the output from becoming a clock, and
                // an authored pick is not a reason to cut on the beat somebody
                // happened to save on.
                self.curator
                    .choose_with(&o.library, &f, &o.recent, time, &o.memory)
            };
            if let Some(next) = next {
                self.open[at].recent.note(&next);
                self.open[at].cue = Some(next);
            }
        }
        self.open[at].cue.clone()
    }

    /// The grade for a model, loaded once and kept.
    pub fn colourist(&mut self, model: &str, schema: &Schema) -> &Colourist {
        if !self.colourists.iter().any(|(id, _)| id == model) {
            let c = Colourist::load(&self.data, model, schema);
            self.colourists.push((model.to_string(), c));
        }
        self.colourists
            .iter()
            .find(|(id, _)| id == model)
            .map(|(_, c)| c)
            .expect("just inserted")
    }

    /// Throws away a cached grade, so retraining takes effect without a
    /// restart.
    pub fn reload(&mut self, model: &str) {
        self.colourists.retain(|(id, _)| id != model);
    }

    /// What the page prints about all of this.
    pub fn report(&self) -> serde_json::Value {
        let describe = |l: &Library| {
            serde_json::json!({
                "name": l.name,
                "source": l.source,
                "clips": l.clips.len(),
                "seconds": l.seconds(),
                "rate": l.rate,
                "width": l.width,
            })
        };
        serde_json::json!({
            "libraries": self.available(),
            // The default — what a source that names no folder of its own
            // draws. Still called `library` because that is what it was when
            // there was only one, and the page's badge means the same thing.
            "library": self.library(None).map(|l| describe(l)),
            // Every folder some source has asked for, so the sheet can say
            // which are actually in use rather than only which are indexed.
            "inUse": self.open.iter().map(|o| describe(&o.library)).collect::<Vec<_>>(),
            // Both models say whether anybody's curation is in them, in the
            // same words the router's does. A default taste reading as a
            // decision is the thing this line exists to stop.
            "curator": {
                "trained": self.curator.trained(),
                "songs": self.curator.coupling.songs,
                "picks": self.curator.coupling.picks,
            },
            // One cue per open folder, keyed by folder. The page used to read
            // a single `cue`; that one is still here and is the default's, so
            // a card that shows "what is on" keeps meaning it.
            "cue": self.slot(None).and_then(|i| self.open[i].cue.as_ref()).map(cue_json),
            "cues": self.open.iter().map(|o| serde_json::json!({
                "library": o.library.name,
                "cue": o.cue.as_ref().map(cue_json),
                // How many authored picks this folder can actually play, and
                // the ones it cannot. A lane the operator authored and the
                // booth silently dropped is exactly what this line is for.
                "picks": o.memory.len(),
                "unresolved": o.unresolved,
            })).collect::<Vec<_>>(),
            "job": self.job().map(|j| serde_json::json!({
                "library": j.library,
                "source": j.source,
                "step": j.step,
                "progress": j.progress,
                "done": j.done,
                "error": j.error,
                "skipped": j.skipped,
                "clips": j.clips,
                "rungs": j.rungs,
                "bytes": j.bytes,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_schema::{spec, Block, FloatSpec};

    static FLOATS: &[FloatSpec] = &[
        spec("stutter", 0.0, 1.0, 0.3),
        spec("advance", 0.0, 3.0, 1.0),
    ];
    static SCHEMA: Schema = Schema {
        blocks: &[Block::Floats(FLOATS), metal_schema::color("fantasma")],
    };

    #[test]
    fn with_no_library_there_is_no_cue_and_nothing_claims_otherwise() {
        let dir = std::env::temp_dir().join("metal-dash-reels-empty");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let mut r = Reels::new(&dir);
        assert!(r.library(None).is_none());
        assert!(r.cue(None, 10.0, None).is_none());
        assert!(r.available().is_empty());
        // A source naming a folder that was never indexed gets the same
        // nothing, and does not fall back to some other source's footage.
        assert!(!r.ensure("no-such-folder"));
        assert!(r.library(Some("no-such-folder")).is_none());
        assert!(r.cue(Some("no-such-folder"), 10.0, None).is_none());
        assert!(r.in_use().is_empty());
        // And the untrained Curator says so rather than claiming a decision.
        assert!(!r.curator.trained());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn a_model_with_no_grade_on_disk_is_graded_by_the_authored_table() {
        // The seam `row_for` runs through, at the one width that matters: the
        // Colourist reads **the music alone** since 2026-08-25, so what is
        // handed to it here is a digest row and nothing else. Which column moves
        // which float is `metal_reel::grade`'s to say and is said there; what is
        // checked here is that the app's side of the call still lines up.
        let dir = std::env::temp_dir().join("metal-dash-reels-grade");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let mut r = Reels::new(&dir);

        let c = r.colourist("wake", &SCHEMA);
        assert!(!c.trained(), "an empty data directory has no grade in it");

        let mut music = [0.5f32; metal_reel::MUSIC];
        music[metal_reel::grade::ONSET] = 0.9;
        let offsets = c.offsets(&SCHEMA, &music);
        assert_eq!(
            offsets.len(),
            2,
            "one per own float, and none for the colour slot"
        );
        assert!(
            offsets[0] > 0.5,
            "the hit did not reach tartamudeo: {}",
            offsets[0]
        );
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_a_library_that_is_not_there_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join("metal-dash-reels-missing");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let mut r = Reels::new(&dir);
        assert!(r.open(Some("nope")).is_err());
        assert!(r.open(None).is_ok());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // the folder index -> files split (the library's two columns)
    // -----------------------------------------------------------------------

    #[test]
    fn a_folder_added_to_the_index_lists_its_files_and_nothing_else() {
        // The whole of the folder/files split: level one is `available()`,
        // covered above; level two is this — one clip's id, filename and
        // length, and nothing that would drag a rung matrix into a JSON
        // reply meant for a search box.
        let data = std::env::temp_dir().join("metal-dash-reels-files");
        let _ = metal_vfs::remove_dir_all(&data);
        metal_vfs::create_dir_all(&data).unwrap();
        two_clip_library(&data, "folder");

        let r = Reels::new(&data);
        let mut files = r
            .files("folder")
            .expect("an indexed folder lists its files");
        files.sort_by(|a, b| a.0.cmp(&b.0)); // f32 isn't Ord; the id alone already orders the two
        assert_eq!(
            files,
            vec![
                ("calm".to_string(), "calm.mp4".to_string(), 10.0),
                ("wild".to_string(), "wild.mp4".to_string(), 10.0),
            ]
        );
        let _ = metal_vfs::remove_dir_all(&data);
    }

    #[test]
    fn a_source_that_already_opened_a_folder_lists_its_files_without_reading_disk_again() {
        // `files` on a folder no source has named yet has to load it fresh —
        // there is nothing else it could read. Once a source has `ensure`d
        // it, the cached copy is right there, and re-parsing rungs off disk a
        // second time is the exact cost this method exists to avoid. Proven
        // by removing the file a second load would need: if this reached
        // disk it would error, not answer.
        let data = std::env::temp_dir().join("metal-dash-reels-files-cached");
        let _ = metal_vfs::remove_dir_all(&data);
        metal_vfs::create_dir_all(&data).unwrap();
        two_clip_library(&data, "folder");

        let mut r = Reels::new(&data);
        assert!(r.ensure("folder"));
        metal_vfs::remove_file(Library::dir(&data, "folder").join("index.json")).unwrap();

        let files = r
            .files("folder")
            .expect("the cached copy answers, not disk");
        assert_eq!(
            files.len(),
            2,
            "the folder ensure() cached did not come back whole"
        );
        let _ = metal_vfs::remove_dir_all(&data);
    }

    #[test]
    fn listing_files_for_a_folder_that_was_never_indexed_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join("metal-dash-reels-files-missing");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let r = Reels::new(&dir);
        assert!(r.files("nope").is_err());
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn default_library_is_none_until_something_is_opened_and_then_names_it() {
        // What `GET /reel/files` falls back to when the operator has not
        // clicked a folder yet — see `main.rs`. A getter this small has no
        // logic of its own to get wrong, but it reads `self.default` and not
        // `self.open`, and a test is what stops a future edit from quietly
        // pointing it at the wrong field.
        let data = std::env::temp_dir().join("metal-dash-reels-default");
        let _ = metal_vfs::remove_dir_all(&data);
        metal_vfs::create_dir_all(&data).unwrap();
        two_clip_library(&data, "folder");

        let mut r = Reels::new(&data);
        assert_eq!(r.default_library(), None);
        assert!(r.open(Some("folder")).is_ok());
        assert_eq!(r.default_library(), Some("folder"));
        let _ = metal_vfs::remove_dir_all(&data);
    }

    // -----------------------------------------------------------------------
    // authored picks, through the live path
    // -----------------------------------------------------------------------

    /// A library on disk, with two scenes a choice between them is readable
    /// from: a still dark one and a moving bright one, the same pair
    /// `metal_reel::curate`'s tests use.
    fn two_clip_library(data: &std::path::Path, name: &str) -> Library {
        use metal_reel::index::{Clip, Passage, Stamp};
        let scene = |d: [f32; metal_reel::read::COLUMNS]| Clip {
            id: String::new(),
            file: String::new(),
            seconds: 10.0,
            stamp: Stamp {
                bytes: 0,
                modified: 0,
            },
            rows: vec![[0.0; metal_reel::read::COLUMNS]; 120],
            passages: vec![Passage {
                from: 0,
                to: 120,
                d,
                seconds: 10.0,
                peak: d[11],
                variety: 0.0,
            }],
        };
        let mut calm = [0.2f32; metal_reel::read::COLUMNS];
        calm[11] = 0.05;
        calm[0] = 0.15;
        let mut wild = [0.7f32; metal_reel::read::COLUMNS];
        wild[11] = 0.9;
        wild[0] = 0.8;
        let mut a = scene(calm);
        a.id = "calm".into();
        a.file = "calm.mp4".into();
        let mut b = scene(wild);
        b.id = "wild".into();
        b.file = "wild.mp4".into();
        let lib = Library {
            name: name.into(),
            source: "/tmp/src".into(),
            rate: 12.0,
            width: 640,
            clips: vec![a, b],
            root: Library::dir(data, name),
        };
        lib.save().unwrap();
        lib
    }

    /// The eight-column-per-second matrix `digest_audio.py` writes, as the
    /// smallest `.npy` `metal_router::npy` will read. Written rather than mocked
    /// because the point of this test is the file being found on disk.
    fn write_song(data: &std::path::Path, song: &str, rows: &[[f32; metal_reel::MUSIC]]) {
        let dir = data.join(SONGS);
        metal_vfs::create_dir_all(&dir).unwrap();
        let mut head = format!(
            "{{'descr': '<f4', 'fortran_order': False, 'shape': ({}, {}), }}",
            rows.len(),
            metal_reel::MUSIC
        );
        while (10 + head.len() + 1) % 64 != 0 {
            head.push(' ');
        }
        head.push('\n');
        let mut out = b"\x93NUMPY\x01\x00".to_vec();
        out.extend((head.len() as u16).to_le_bytes());
        out.extend(head.as_bytes());
        for r in rows {
            for v in r {
                out.extend(v.to_le_bytes());
            }
        }
        metal_vfs::write(dir.join(metal_schema::digest_file_name(song, metal_schema::TickRate::Base)), out).unwrap();
    }

    /// The twelve features of the last twelve seconds of `rows` — what the live
    /// path hands `cue`, and what `recall` computes for a pick at that tick.
    fn heard(rows: &[[f32; metal_reel::MUSIC]]) -> [f32; FEATURES] {
        let w = (metal_reel::curate::WINDOW_SECONDS * metal_router::features::RATE) as usize;
        // `npy::load` pads a 16-wide script matrix to the current width with
        // the voice block zero, so the test describes the same rows the load
        // path would have handed `recall`.
        let wide: Vec<[f32; metal_router::features::COLUMNS]> = rows
            .iter()
            .map(|r| {
                let mut p = [0.0f32; metal_router::features::COLUMNS];
                p[..metal_reel::MUSIC].copy_from_slice(r);
                p
            })
            .collect();
        metal_router::features::describe(&wide[wide.len() - w..])
    }

    #[test]
    fn a_pick_authored_in_the_studio_is_what_the_booth_plays() {
        // The whole of task #20 in one assertion. Until this, picks were a
        // training corpus and nothing else: an operator could author a lane and
        // the live path would never look at it.
        let data = std::env::temp_dir().join("metal-dash-reels-picks");
        let _ = metal_vfs::remove_dir_all(&data);
        metal_vfs::create_dir_all(&data).unwrap();
        two_clip_library(&data, "folder");

        // Loud music for two hundred ticks, and a pick at the end of it.
        let rows = vec![[0.9f32; metal_reel::MUSIC]; 200];
        write_song(&data, "song", &rows);
        let music = heard(&rows);
        curate::save_picks(
            &data,
            "folder",
            "song",
            &[metal_reel::curate::Pick {
                song: "song".into(),
                tick: 200,
                clip: "calm".into(),
                passage: 0,
                play: metal_reel::curate::Play {
                    speed: 0.5,
                    reverse: true,
                    from: 0.0,
                    to: 0.5,
                },
            }],
        )
        .unwrap();

        let mut r = Reels::new(&data);
        assert!(r.ensure("folder"));
        let cue = r
            .cue(Some("folder"), 10.0, Some(music))
            .expect("a folder with clips cues");
        assert_eq!(
            cue.clip_id, "calm",
            "the authored pick did not reach the live path"
        );
        assert_eq!(
            cue.play.speed, 0.5,
            "and it is played the way it was authored"
        );
        assert!(cue.play.reverse);
        let a = cue.authored.expect("a recalled cue has to say so");
        assert_eq!((a.song.as_str(), a.tick), ("song", 200));

        // Music that pick was not authored over is the model's again — and the
        // model wants the moving shot for loud music, which is what makes the
        // line above mean something.
        let mut other = Reels::new(&data);
        assert!(other.ensure("folder"));
        let quiet = [0.05f32; FEATURES];
        let cue = other.cue(Some("folder"), 10.0, Some(quiet)).unwrap();
        assert!(
            cue.authored.is_none(),
            "a pick fired over music it was never about"
        );

        let _ = metal_vfs::remove_dir_all(&data);
    }

    #[test]
    fn a_pick_the_booth_cannot_resolve_is_reported_and_never_played() {
        // Stale picks are the shape this project has named twice: in range,
        // silent, wrong image. They have to be dropped where somebody can read
        // that they were.
        let data = std::env::temp_dir().join("metal-dash-reels-stale");
        let _ = metal_vfs::remove_dir_all(&data);
        metal_vfs::create_dir_all(&data).unwrap();
        two_clip_library(&data, "folder");
        let rows = vec![[0.9f32; metal_reel::MUSIC]; 200];
        write_song(&data, "song", &rows);

        let pick = |song: &str, clip: &str, passage: usize| metal_reel::curate::Pick {
            song: song.into(),
            tick: 200,
            clip: clip.into(),
            passage,
            play: metal_reel::curate::Play::default(),
        };
        curate::save_picks(
            &data,
            "folder",
            "song",
            &[
                // A clip that has left the folder since it was authored.
                pick("song", "deleted", 0),
                // A passage a re-cut no longer has.
                pick("song", "calm", 40),
            ],
        )
        .unwrap();
        // And a song whose digest is not on this machine.
        curate::save_picks(
            &data,
            "folder",
            "undigested",
            &[pick("undigested", "calm", 0)],
        )
        .unwrap();
        // A song name that is a path rather than a name. It never came through
        // `POST /curator/picks`, which refuses it — this file was edited by
        // hand, and the booth is about to build a path out of it.
        curate::save_picks(
            &data,
            "folder",
            "escape",
            &[pick("../../etc/passwd", "calm", 0)],
        )
        .unwrap();

        let mut r = Reels::new(&data);
        assert!(r.ensure("folder"));
        let cue = r.cue(Some("folder"), 10.0, Some(heard(&rows))).unwrap();
        assert!(cue.authored.is_none(), "not one of those was playable");
        assert_eq!(
            cue.clip_id, "wild",
            "and the model answered as it always did"
        );

        let report = r.report();
        let one = &report["cues"][0];
        assert_eq!(one["picks"], 0);
        let said = one["unresolved"]
            .as_array()
            .expect("the notes are reported")
            .len();
        assert_eq!(
            said, 4,
            "every dropped pick is named: {}",
            one["unresolved"]
        );

        let _ = metal_vfs::remove_dir_all(&data);
    }

    #[test]
    fn a_folder_with_no_picks_is_the_cue_it_always_was() {
        // The migration rule of §7, as an assertion. Nothing under `data/` has
        // a `picks/` folder today, so this is the path every library on disk
        // takes.
        let data = std::env::temp_dir().join("metal-dash-reels-nopicks");
        let _ = metal_vfs::remove_dir_all(&data);
        metal_vfs::create_dir_all(&data).unwrap();
        let lib = two_clip_library(&data, "folder");

        let mut r = Reels::new(&data);
        assert!(r.ensure("folder"));
        let music = [0.6f32; FEATURES];
        let got = r.cue(Some("folder"), 10.0, Some(music)).unwrap();
        let want = Curator::default()
            .choose(&lib, &music, &metal_reel::curate::Recent::default(), 10.0)
            .unwrap();
        assert_eq!(got, want, "an empty memory changed what the model answered");
        let _ = metal_vfs::remove_dir_all(&data);
    }

    #[test]
    fn indexing_a_folder_with_no_video_in_it_answers_the_click() {
        // Rather than starting a thread that finds nothing three seconds later
        // and leaves the page saying "indexing" until somebody reloads.
        let dir = std::env::temp_dir().join("metal-dash-reels-novideo");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        metal_vfs::write(dir.join("notes.txt"), "not a video").unwrap();
        let mut r = Reels::new(&dir);
        let e = r
            .index(dir.clone(), "x".into(), Options::default())
            .unwrap_err();
        assert!(e.contains("no video files"), "{e}");
        assert!(
            r.job().is_none(),
            "a failed start must not leave a job standing"
        );
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// `open_lazy` on a name that is already indexed opens it and starts
    /// nothing — the path argument is never even read.
    #[test]
    fn reel_open_with_an_indexed_name_opens_at_once() {
        let data = std::env::temp_dir().join("metal-dash-reels-lazy-indexed");
        let _ = metal_vfs::remove_dir_all(&data);
        metal_vfs::create_dir_all(&data).unwrap();
        two_clip_library(&data, "folder");

        let mut r = Reels::new(&data);
        let indexing = r
            .open_lazy(data.join("elsewhere"), "folder".into())
            .expect("an indexed folder opens");
        assert!(
            !indexing,
            "an already-indexed folder opens, it does not re-index"
        );
        assert_eq!(r.default_library(), Some("folder"));
        assert!(
            r.job().is_none(),
            "no index job for a folder that is already there"
        );
        let _ = metal_vfs::remove_dir_all(&data);
    }

    /// `open_lazy` on a fresh folder starts an index job and remembers the
    /// name to open when it finishes; the library is not open until then.
    #[test]
    fn reel_open_with_a_fresh_path_starts_an_index_job() {
        let data = std::env::temp_dir().join("metal-dash-reels-lazy-fresh-data");
        let src = std::env::temp_dir().join("metal-dash-reels-lazy-fresh-src");
        let _ = metal_vfs::remove_dir_all(&data);
        let _ = metal_vfs::remove_dir_all(&src);
        metal_vfs::create_dir_all(&data).unwrap();
        metal_vfs::create_dir_all(&src).unwrap();
        metal_vfs::write(src.join("clip.mp4"), b"not really a video").unwrap();

        let mut r = Reels::new(&data);
        let indexing = r
            .open_lazy(src.clone(), "fresh".into())
            .expect("a folder with footage starts a job");
        assert!(indexing, "a folder nobody indexed starts a job");
        assert!(r.job().is_some(), "and the job is reported");
        assert_eq!(
            r.default_library(),
            None,
            "it is not open until the job finishes"
        );
        let _ = metal_vfs::remove_dir_all(&data);
        let _ = metal_vfs::remove_dir_all(&src);
    }
}

/// A cue as the page reads it. A free function rather than a method so the
/// report can map it over the open libraries and over the default alike.
fn cue_json(c: &Cue) -> serde_json::Value {
    serde_json::json!({
        "clip": c.clip_id,
        "passage": c.passage,
        "from": c.from,
        "span": c.span,
        "since": c.since,
        "confidence": c.confidence,
        "because": c.because.map(|(n, v)| serde_json::json!([n, v])),
        // Null is the model's own answer, which is what every cue was before
        // authored picks reached the live path. When it is not null the page
        // must say so: a recalled cue and a scored one are the same shape, and
        // `confidence` on a recalled one is not a probability.
        "authored": c.authored.as_ref().map(|a| serde_json::json!({
            "song": a.song, "tick": a.tick, "near": a.near,
        })),
    })
}
