//! A folder of videos → a library: a proxy frame ladder, a descriptor per rung,
//! and the passages the Curator chooses between.
//!
//! # The operator's folder is read-only, always
//!
//! Nothing here writes into the directory the operator pointed at. The footage
//! may be on a card, on a share, on somebody else's drive, or simply be the only
//! copy. The index and the ladder live under the app's own data directory
//! (`data/reels/<name>/`), keyed by the source path, and the folder is opened
//! for reading and closed.
//!
//! # Why there is a ladder at all, and not a decoder in the render loop
//!
//! The alternative is `ffmpeg -ss` per jump, in the frame loop, at a venue. A
//! seek plus a process start is 100–300 ms; the first effect built on this
//! ([`metal_render::models::estela`](../../metal_render/models/estela/index.html))
//! *stutters*, which means it jumps several times a second. That trade is not
//! close.
//!
//! What the ladder buys is bigger than avoiding the cost, and it is the thing
//! this whole design rests on: **because every frame that can be shown already
//! exists at a known address, a trail is a lookup rather than a memory.** The
//! one effect in this project that has to remember something is
//! `metal_render::shared::trail`, and it needs two textures, a ping-pong and a
//! `SEEK` threshold to stay deterministic. A transmission needs none of that —
//! it reads the frames behind the one it is on. Sandstorm made the same move
//! against a particle buffer and its header calls it inverting the fall.
//!
//! # What it costs, in numbers rather than in adjectives
//!
//! At the defaults — [`FPS`] rungs a second, [`WIDTH`] pixels across, JPEG
//! quality [`QUALITY`] — a rung costs **whatever is in it**, and the spread is
//! wide enough that one figure would be a lie. Measured across a test library:
//! 1.6 kB for a flat colour field, 3.9 kB for a lit figure on a dark stage,
//! 12.8 kB for a mandelbrot, 18.3 kB for full-frame colour bars in motion.
//!
//! So the arithmetic an operator needs is per library, not per second, and the
//! honest way to give it is to measure the one they built: at 12 rungs a
//! second, ordinary footage in the 10–25 kB band works out to roughly 120–300
//! MB per minute — call it a gigabyte for five minutes of busy material, and
//! much less for anything static. [`Step::Done`] carries the real number and
//! the page prints it, which is the only figure that is actually true.
//!
//! # Why the descriptors are read from the proxies and not from the source
//!
//! A second ffmpeg pass over the original would give a "better" reading of a
//! frame nobody will ever see. The descriptor's whole job is to say what is on
//! screen, and what is on screen is the rung. So the ladder is written first and
//! read back — the descriptor describes the frame it belongs to, by
//! construction, at whatever proxy width the library was built at.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::read::{self, Rgb, COLUMNS};

/// Rungs per second of clip.
///
/// **Not 8, and not the source's own rate.** Eight would put the ladder on the
/// musical digest's grid, which is exactly the coupling this half of the system
/// exists to not have: a clip has its own clock and the descriptor between two
/// rungs is an interpolation, not a step. The source's rate would make the
/// library's size a property of whatever camera shot it.
///
/// Twelve is enough that a stutter has material to hold and that a cut lands
/// within 83 ms of where it is, and few enough that a minute of footage is tens
/// of megabytes rather than hundreds.
pub const FPS: f32 = 12.0;

/// How wide a rung is, in pixels. Height follows the source's aspect.
pub const WIDTH: u32 = 640;

/// The one proxy spec: every HAP proxy is [`FPS`] rungs a second at [`WIDTH`]
/// pixels across, and the cut list is cut from those rows. The app's own
/// proxies (`metal-dash`'s scene playout) and the library build both read this,
/// so a clip has one movie and one cut list. A proxy whose index names another
/// spec (or none, `0`) is stale and is rebuilt through the ingest job.
pub const PROXY_SPEC: u32 = 2;

/// ffmpeg's `-q:v`, where 2 is best and 31 is worst.
pub const QUALITY: u8 = 4;

/// The shortest stretch of a clip that counts as a passage, in seconds.
///
/// Shorter than a musical section on purpose — `metal_router`'s floor is 24 s and
/// a two-second clip is still a perfectly good thing to put under a chorus,
/// because a passage **loops** for as long as it is cued. What this floor stops
/// is a clip with a hard flicker in it shattering into forty passages that all
/// describe the same footage.
pub const MIN_SECONDS: f32 = 2.0;

/// How much of the frame has to change between two rungs to call it a cut.
///
/// `motion` saturates at 1.0 across a real cut and sits well under 0.2 on
/// ordinary movement, so this is a wide gap rather than a tuned edge. A dissolve
/// does not trip it, and that is correct: a dissolve is not a boundary an
/// operator would point at either.
const CUT: f32 = 0.55;

/// Which files are footage. Anything else in the folder is ignored in silence —
/// a library folder in the real world has thumbnails, sidecars and `.DS_Store`
/// in it, and refusing to index because of one would be a worse app.
const EXTENSIONS: [&str; 10] =
    ["mp4", "mov", "mkv", "avi", "webm", "m4v", "mpg", "mpeg", "wmv", "flv"];

/// Where libraries live under the data directory.
pub const DIR: &str = "reels";

/// A clip's HAP proxy and its index: the one folder every proxy lives in,
/// `<data>/projects/hap/<slug>.mov`, whoever made it (the editor's ingest or a
/// library build).
pub fn proxy_paths(data: &Path, slug: &str) -> (PathBuf, PathBuf) {
    crate::hap::Hap::paths(&data.join("projects"), slug)
}

/// How many rungs are decoded and described at once.
///
/// The readback is the expensive half of indexing — ffmpeg writes a
/// seven-minute clip's ladder in nine seconds and reading it back used to take
/// two minutes — so it runs on every core. What sets this number is memory
/// rather than scheduling: a chunk holds decoded frames, at
/// `width × height × 3` bytes each, so sixty-four rungs of a 640 px ladder is
/// about forty megabytes in flight.
///
/// It cannot be one chunk for the whole clip: that is the same arithmetic
/// against five thousand rungs, which is four gigabytes.
const CHUNK: usize = 64;

// ---------------------------------------------------------------------------
// the types
// ---------------------------------------------------------------------------

/// One indexed folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    /// What the operator calls it. Also its directory name under `data/reels/`.
    pub name: String,
    /// The folder that was indexed, as given. Kept so the app can say where the
    /// footage came from and can re-index without being told again.
    pub source: String,
    pub rate: f32,
    pub width: u32,
    pub clips: Vec<Clip>,
    /// Where the ladder is. Not serialised: it is wherever this file was
    /// loaded from, and a path written into the file would be wrong the first
    /// time the data directory moved.
    #[serde(skip)]
    pub root: PathBuf,
}

/// One video file, laddered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    /// Stable, ASCII, unique within the library, and the name of this clip's
    /// folder in the ladder. Derived from the filename but not equal to it —
    /// see [`slug`].
    pub id: String,
    /// The filename inside the source folder. For display and for re-indexing.
    pub file: String,
    /// How long the ladder is, in seconds — `rows.len() / rate`, which is not
    /// quite the source's duration when the last partial rung is dropped.
    pub seconds: f32,
    /// The source file's size and modification time when it was indexed, so a
    /// re-index can keep the clips that did not change.
    pub stamp: Stamp,
    /// One descriptor per rung.
    pub rows: Vec<[f32; COLUMNS]>,
    /// The stretches this clip divides into at its own cuts.
    pub passages: Vec<Passage>,
}

/// A stretch of one clip, and what it looks like as a whole — **a scene**.
///
/// # The three qualities that are not in the mean
///
/// `d` is the average of the descriptors inside the passage, and averaging is
/// exactly what loses the things somebody choosing a shot cares about. A four
/// second scene and a forty second scene average the same. A locked-off shot
/// and a whip pan that ends where it started average the same. A scene with one
/// hit in it averages down to a calm scene.
///
/// So the three that survive averaging are stored beside it. They are facts
/// about the footage, computed at ingest and never trained — the same division
/// [`cut`] draws: *where* a scene is and *what it is like* are measurements,
/// and only *which one to play* is taste.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Passage {
    /// Rung indices, half open: `[from, to)`.
    pub from: usize,
    pub to: usize,
    /// The mean descriptor over the stretch. This is what the Curator matches
    /// against, and it is stored rather than recomputed because it is asked for
    /// once per candidate per decision.
    pub d: [f32; COLUMNS],
    /// How long the scene is, in seconds.
    ///
    /// Derivable from `to - from` and the library's rate, and stored anyway:
    /// every caller that wants it has to be handed the rate as well, and a
    /// passage that travels without its own length is one that gets compared
    /// against another library's rate eventually.
    #[serde(default)]
    pub seconds: f32,
    /// The highest `motion` inside the scene.
    ///
    /// What separates a shot that is busy throughout from one that is still and
    /// then moves once — which the mean cannot tell apart and which are opposite
    /// things to cut to.
    #[serde(default)]
    pub peak: f32,
    /// How far the descriptor travels inside the scene, against how far it would
    /// travel if every rung were the mean.
    ///
    /// Zero is a locked-off shot of a static subject. High is a scene that
    /// becomes something else while it runs — a dissolve, a lighting change, a
    /// pan onto a different subject. It is the column that says whether a
    /// passage is one thing or several.
    #[serde(default)]
    pub variety: f32,
}

impl Passage {
    /// Measures a stretch of rungs. `from`/`to` index into the whole clip;
    /// `rows` is that slice.
    fn over(from: usize, to: usize, rows: &[[f32; COLUMNS]], rate: f32) -> Passage {
        let d = read::average(rows);
        let peak = rows.iter().map(|r| r[11]).fold(0.0f32, f32::max);
        // Mean absolute deviation from the passage's own mean, over every
        // column. Not a variance: this is compared against thresholds an
        // operator reads, and a squared quantity in a panel of Lumetri readings
        // would be the one number there that is not in the units of the rest.
        let variety = if rows.is_empty() {
            0.0
        } else {
            let total: f32 = rows
                .iter()
                .map(|r| {
                    r.iter().zip(d.iter()).map(|(a, b)| (a - b).abs()).sum::<f32>()
                        / COLUMNS as f32
                })
                .sum();
            (total / rows.len() as f32).clamp(0.0, 1.0)
        };
        Passage {
            from,
            to,
            d,
            seconds: (to.saturating_sub(from)) as f32 / rate.max(1e-6),
            peak,
            variety,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub bytes: u64,
    /// Seconds since the epoch. Zero when the filesystem would not say, which
    /// makes the clip always re-index rather than always be kept.
    pub modified: u64,
}

/// How the ladder is built.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub rate: f32,
    pub width: u32,
    pub quality: u8,
}

impl Default for Options {
    fn default() -> Options {
        Options { rate: FPS, width: WIDTH, quality: QUALITY }
    }
}

/// Where a build has got to. Reported often enough that a page can show a bar,
/// and cheap enough that reporting it is not the work.
#[derive(Debug, Clone)]
pub enum Step {
    /// About to look at a file. `of` is how many were found in the folder.
    Probing { file: String, clip: usize, of: usize },
    /// ffmpeg is writing rungs. This is the slow one and it does not report
    /// inside itself — ffmpeg is one process and one call.
    Extracting { file: String, clip: usize, of: usize },
    /// Decoding rungs and describing them.
    Reading { file: String, clip: usize, of: usize, done: usize, total: usize },
    /// A file that could not be used, and why. Named rather than skipped: "I
    /// put that clip in the folder and it is not in the library" has to have an
    /// answer.
    Skipped { file: String, why: String },
    /// 2026-10-01: writing the clip's HAP proxy (see `crate::hap`).
    Encoding { file: String, clip: usize, of: usize },
    Done { clips: usize, rungs: usize, bytes: u64 },
}

impl Clip {
    /// The descriptor at `seconds` into the clip, **interpolated** between the
    /// two rungs it falls between.
    ///
    /// This is the sentence the whole module is built around: a transmission
    /// does not read a staircase, it reads a continuous curve sampled sparsely.
    /// Out of range clamps to the ends rather than wrapping — a caller that
    /// wants a loop is asking for one, and a descriptor that silently jumped
    /// back to the head of the clip would make the last rung of every passage
    /// look like a cut.
    pub fn at(&self, seconds: f32, rate: f32) -> [f32; COLUMNS] {
        if self.rows.is_empty() {
            return [0.0; COLUMNS];
        }
        let x = (seconds * rate).max(0.0);
        let i = x.floor() as usize;
        if i + 1 >= self.rows.len() {
            return self.rows[self.rows.len() - 1];
        }
        read::lerp(&self.rows[i], &self.rows[i + 1], x - i as f32)
    }

    /// Which rung is on screen at `seconds`. Floor rather than round, so a rung
    /// is shown for the whole of its own interval — rounding would show the
    /// first and last rung for half as long as the rest, which a stutter
    /// holding on the head of a clip makes visible.
    pub fn rung(&self, seconds: f32, rate: f32) -> usize {
        if self.rows.is_empty() {
            return 0;
        }
        let i = (seconds.max(0.0) * rate).floor() as usize;
        i.min(self.rows.len() - 1)
    }

    pub fn rungs(&self) -> usize {
        self.rows.len()
    }
}

impl Library {
    /// Every library indexed under `data`, newest name order irrelevant —
    /// sorted, so a page's list does not reshuffle between reloads.
    pub fn list(data: &Path) -> Vec<String> {
        let mut out: Vec<String> = metal_vfs::read_dir(data.join(DIR))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| metal_vfs::is_file(&e.path().join("index.json")))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        out.sort();
        out
    }

    pub fn dir(data: &Path, name: &str) -> PathBuf {
        data.join(DIR).join(name)
    }

    /// The data root this library lives under (`<data>/reels/<name>`).
    pub fn data(&self) -> PathBuf {
        self.root.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default()
    }

    pub fn load(data: &Path, name: &str) -> Result<Library, String> {
        let root = Library::dir(data, name);
        let text = metal_vfs::read_to_string(root.join("index.json"))
            .map_err(|e| format!("{name}: {e}"))?;
        let mut lib: Library =
            serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
        lib.root = root;
        // An index written before a passage carried its length, its peak
        // movement and its variety reads those as zero, and zero is a
        // *legitimate value* for two of the three — so nothing downstream could
        // tell an unqualified scene from a still, silent one. Re-cutting costs
        // a pass over rows that are already in hand, and `cut` is a pure
        // function of them, so this is idempotent on a library that has them.
        for clip in &mut lib.clips {
            if clip.passages.iter().any(|p| p.seconds <= 0.0 && p.to > p.from) {
                clip.passages = cut(&clip.rows, lib.rate);
            }
        }
        Ok(lib)
    }

    pub fn save(&self) -> Result<(), String> {
        metal_vfs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        metal_vfs::write(self.root.join("index.json"), text).map_err(|e| e.to_string())
    }

    /// Where rung `k` of `clip` is on disk.
    pub fn rung_path(&self, clip: &Clip, k: usize) -> PathBuf {
        // One-based, because that is what ffmpeg's `%06d` writes and renaming
        // a hundred thousand files to make an index zero-based would be a
        // rename per rung to save one subtraction here.
        self.root.join("frames").join(&clip.id).join(format!("{:06}.jpg", k + 1))
    }

    /// 2026-10-01: the clip's HAP proxy, when it was written whole.
    pub fn hap(&self, clip: &Clip) -> Option<crate::hap::Hap> {
        let (movie, index) = proxy_paths(&self.data(), &clip.id);
        crate::hap::Hap::open(&movie, &index).ok().filter(|h| h.frames() == clip.rungs())
    }

    pub fn clip(&self, index: usize) -> Option<&Clip> {
        self.clips.get(index)
    }

    pub fn by_id(&self, id: &str) -> Option<(usize, &Clip)> {
        self.clips.iter().enumerate().find(|(_, c)| c.id == id)
    }

    pub fn rungs(&self) -> usize {
        self.clips.iter().map(|c| c.rungs()).sum()
    }

    pub fn seconds(&self) -> f32 {
        self.clips.iter().map(|c| c.seconds).sum()
    }

    /// Every passage in the library, as `(clip index, passage index)`. The set
    /// the Curator chooses from.
    pub fn passages(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (ci, c) in self.clips.iter().enumerate() {
            for pi in 0..c.passages.len() {
                out.push((ci, pi));
            }
        }
        out
    }

    /// Bytes the ladder occupies on disk.
    pub fn bytes(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            metal_vfs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    _ => e.metadata().map(|m| m.len()).unwrap_or(0),
                })
                .sum()
        }
        walk(&self.root)
    }
}

// ---------------------------------------------------------------------------
// building one
// ---------------------------------------------------------------------------

/// Indexes `source` into `data/reels/<name>/`.
///
/// **Incremental.** A clip whose file has the same size and modification time as
/// the last build keeps its rungs and its descriptors untouched; only new and
/// changed files are extracted. Changing `rate` or `width` invalidates
/// everything, because those are what a rung *is*. Adding three clips to a
/// folder of two hundred should cost three clips.
///
/// A file that will not probe or will not extract is reported through `on` and
/// left out. One bad file must not cost the other hundred and ninety-nine.
pub fn build(
    source: &Path,
    data: &Path,
    name: &str,
    opts: &Options,
    on: &mut dyn FnMut(Step),
) -> Result<Library, String> {
    let root = Library::dir(data, name);
    let files = footage_in(source)?;
    let previous = Library::load(data, name).ok().filter(|p| {
        // Same source, same ladder shape. Anything else and "unchanged" is a
        // claim about a different library.
        p.source == source.display().to_string()
            && (p.rate - opts.rate).abs() < 1e-6
            && p.width == opts.width
    });

    metal_vfs::create_dir_all(root.join("frames")).map_err(|e| e.to_string())?;

    let mut clips: Vec<Clip> = Vec::new();
    let total = files.len();

    for (i, path) in files.iter().enumerate() {
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        // One slug for a clip everywhere: its path's, as the editor's proxy and
        // the scene book name it (`scenes::slug`), not a filename-only one.
        let id = crate::scenes::slug(&path.to_string_lossy());

        let stamp = match stamp_of(path) {
            Some(s) => s,
            None => {
                on(Step::Skipped { file, why: "cannot read the file".into() });
                continue;
            }
        };

        // Unchanged since last time, and its rungs are still on disk.
        if let Some(kept) = previous.as_ref().and_then(|p| {
            p.clips.iter().find(|c| c.file == file && c.stamp == stamp && c.id == id)
        }) {
            let dir = root.join("frames").join(&id);
            if count_rungs(&dir) == kept.rungs() {
                let mut kept = kept.clone();
                // The rungs are unchanged; the scenes are cut again anyway.
                // `cut` reads `rows`, which is already in the index file, so
                // this costs no ffmpeg and no decode — and it means a library
                // built by an older version picks up new scene qualities on the
                // next re-index instead of carrying zeros the Curator would
                // read as fact.
                kept.passages = cut(&kept.rows, opts.rate);
                clips.push(kept);
                continue;
            }
        }

        on(Step::Probing { file: file.clone(), clip: i, of: total });
        let dir = root.join("frames").join(&id);
        let movie = proxy_paths(data, &id).1;
        on(Step::Extracting { file: file.clone(), clip: i, of: total });
        let written = match write_rungs(path, &dir, &movie, opts) {
            Ok(n) => n,
            Err(why) => {
                on(Step::Skipped { file, why });
                continue;
            }
        };
        if written == 0 {
            on(Step::Skipped { file, why: "ffmpeg wrote no frames".into() });
            continue;
        }

        // Read them back and describe each against the one before it.
        let mut rows: Vec<[f32; COLUMNS]> = Vec::with_capacity(written);
        let mut carry: Option<Rgb> = None;
        for base in (0..written).step_by(CHUNK) {
            let end = (base + CHUNK).min(written);
            on(Step::Reading {
                file: file.clone(),
                clip: i,
                of: total,
                done: base,
                total: written,
            });

            let mut frames: Vec<Option<Rgb>> =
                across(&(base..end).collect::<Vec<_>>(), |k| {
                    decode(&dir.join(format!("{:06}.jpg", k + 1))).ok()
                });

            {
                // References, so describing costs no copy of a frame. Rung `k`
                // reads `k - 1`, and the first of a chunk reads the last of the
                // one before — which is the whole reason a chunk carries a tail.
                let pairs: Vec<(Option<&Rgb>, Option<&Rgb>)> = (0..frames.len())
                    .map(|k| {
                        let previous =
                            if k == 0 { carry.as_ref() } else { frames[k - 1].as_ref() };
                        (frames[k].as_ref(), previous)
                    })
                    .collect();
                for row in across(&pairs, |(frame, previous)| {
                    frame.map(|f| read::describe(f, *previous))
                }) {
                    // A single unreadable rung repeats its neighbour rather than
                    // ending the clip. The ladder is a sampling of something
                    // continuous and one missing sample is not a reason to lose
                    // four minutes of footage.
                    rows.push(row.unwrap_or_else(|| *rows.last().unwrap_or(&[0.0; COLUMNS])));
                }
            }

            carry = frames.pop().flatten();
        }

        let passages = cut(&rows, opts.rate);
        clips.push(Clip {
            id,
            file,
            seconds: rows.len() as f32 / opts.rate,
            stamp,
            rows,
            passages,
        });
    }

    // Ladders belonging to clips that are no longer in the folder. Left behind,
    // these are the whole size of the library and nothing would ever remove
    // them.
    sweep(&root.join("frames"), &clips);

    // 2026-10-01: the HAP proxy of every clip that lacks a whole one. A
    // missing ffmpeg `hap` encoder skips this in silence — the JPEG rungs
    // still draw, only slower.
    let total = clips.len();
    for (i, clip) in clips.iter().enumerate() {
        let (movie, index) = proxy_paths(data, &clip.id);
        if let Some(dir) = movie.parent() {
            let _ = metal_vfs::create_dir_all(dir);
        }
        let whole = crate::hap::Hap::open(&movie, &index).is_ok_and(|h| h.frames() == clip.rungs());
        if whole {
            continue;
        }
        on(Step::Encoding { file: clip.file.clone(), clip: i, of: total });
        // Encode from the source rather than the JPEG ladder: the ladder has
        // no alpha, so a Resolume DXV alpha export would lose its transparency
        // on the way through JPEG. The probe is best-effort — a clip whose
        // probe fails still gets an ordinary opaque proxy, the same tolerant
        // answer the rest of the build gives a hostile file.
        let src = source.join(&clip.file);
        let (prefix, alpha) = match metal_ingest::video::probe(&src) {
            Ok(info) => (
                metal_ingest::video::normalize_prefix(&info.pix_fmt, &info.color_space),
                metal_ingest::video::has_alpha(&info.pix_fmt),
            ),
            Err(_) => ("", false),
        };
        match metal_ingest::video::encode_hap_video(
            &src,
            opts.rate as f64,
            opts.width,
            clip.rungs() as u32,
            &movie,
            prefix,
            alpha,
            &mut |_| {},
        ) {
            Ok((width, height, packets)) => {
                let text = serde_json::to_string(&crate::hap::Index { width, height, packets, spec: PROXY_SPEC }).map_err(|e| e.to_string())?;
                metal_vfs::write(&index, text).map_err(|e| e.to_string())?;
            }
            Err(e) => {
                let _ = metal_vfs::remove_file(&movie);
                on(Step::Skipped { file: clip.file.clone(), why: format!("HAP: {e}") });
            }
        }
    }

    let lib = Library {
        name: name.to_string(),
        source: source.display().to_string(),
        rate: opts.rate,
        width: opts.width,
        clips,
        root,
    };
    lib.save()?;
    on(Step::Done { clips: lib.clips.len(), rungs: lib.rungs(), bytes: lib.bytes() });
    Ok(lib)
}

/// Writes one clip's rungs into `dir` and returns how many were written.
///
/// Desktop: `ffprobe` for the length and rate, then `ffmpeg` writes the rungs.
/// Android has neither (`spawn` is unsupported there), so the clip is decoded
/// in-process with `AMediaExtractor` + `AMediaCodec` and each sampled picture
/// is JPEG-encoded at the rung's size. Either way the rungs are the same
/// `000001.jpg…` files, and the read-back below does not know which made them.
#[cfg(not(target_os = "android"))]
fn write_rungs(path: &Path, dir: &Path, movie: &Path, opts: &Options) -> Result<usize, String> {
    let info = metal_ingest::video::probe(path).map_err(|e| e.to_string())?;
    if info.duration <= 0.0 || info.fps <= 0.0 {
        return Err("no usable video stream".into());
    }
    let count = (info.duration * opts.rate as f64).floor() as u32;
    if count == 0 {
        return Err("shorter than one rung".into());
    }
    let _ = metal_vfs::remove_dir_all(dir);
    let _ = metal_vfs::remove_file(movie);
    let prefix = metal_ingest::video::normalize_prefix(&info.pix_fmt, &info.color_space);
    metal_ingest::video::extract(
        path,
        dir,
        0.0,
        opts.rate as f64,
        count,
        info.fps,
        opts.quality,
        Some(opts.width),
        prefix,
    )
    .map_err(|e| e.to_string())?;
    Ok(count_rungs(dir))
}

#[cfg(target_os = "android")]
fn write_rungs(path: &Path, dir: &Path, movie: &Path, opts: &Options) -> Result<usize, String> {
    let _ = metal_vfs::remove_dir_all(dir);
    let _ = metal_vfs::remove_file(movie);
    let kept = match native_rungs(path, dir, opts, false) {
        Err(e) if e.contains(crate::clip::media::STALLED) => {
            // The default decoder took samples and never answered; the same
            // fallback the playout path takes.
            let _ = metal_vfs::remove_dir_all(dir);
            native_rungs(path, dir, opts, true)
        }
        other => other,
    }?;
    if kept == 0 {
        return Err("no frames decoded".into());
    }
    Ok(count_rungs(dir))
}

/// Android: samples `path` at the rung rate into `dir` as JPEGs. The clip's
/// length comes from the container when it has one (the probe), otherwise the
/// decode runs to end of stream.
#[cfg(target_os = "android")]
fn native_rungs(path: &Path, dir: &Path, opts: &Options, software: bool) -> Result<usize, String> {
    use crate::clip::ClipDecoder;
    let mut clip = crate::clip::media::MediaClip::open_with(path, opts.width, software)?;
    let duration = clip.duration();
    if duration > 0.0 && (duration * opts.rate as f64).floor() < 1.0 {
        return Err("shorter than one rung".into());
    }
    let slots = (duration > 0.0).then(|| (duration * opts.rate as f64).ceil() as usize);
    metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    clip.sample_all(opts.rate as f64, slots, |k, frame| {
        let mut jpeg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg, 90)
            .encode(&frame.pixels, frame.width as u16, frame.height as u16, jpeg_encoder::ColorType::Rgba)
            .map_err(|e| e.to_string())?;
        metal_vfs::write(dir.join(format!("{:06}.jpg", k + 1)), jpeg).map_err(|e| e.to_string())
    })
}

/// Every video file in `source`, sorted, one level deep.
///
/// One level rather than a walk, on purpose: a folder of footage is a folder of
/// footage, and recursing turns "point at your Movies folder" into an hour of
/// indexing that nobody asked for.
pub fn footage_in(source: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = metal_vfs::read_dir(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| metal_vfs::is_file(&p))
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| EXTENSIONS.contains(&x.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
        })
        .collect();
    out.sort();
    Ok(out)
}

/// Where a clip divides, at its own cuts.
///
/// The parallel with `metal_router::segment::sections` is exact and deliberate:
/// **a cut is a fact about the footage**, the way a novelty peak is a fact about
/// the audio, so nothing here is trained. What goes in a passage is taste and
/// lives in the Curator; where a passage ends does not.
pub fn cut(rows: &[[f32; COLUMNS]], rate: f32) -> Vec<Passage> {
    let min = (MIN_SECONDS * rate).ceil() as usize;
    if rows.is_empty() {
        return Vec::new();
    }
    if rows.len() < min * 2 {
        // Too short to divide. One passage over the whole thing, which is the
        // honest answer for a two-second clip and is what makes short loops
        // usable at all.
        return vec![Passage::over(0, rows.len(), rows, rate)];
    }

    let mut bounds = vec![0usize];
    for (i, r) in rows.iter().enumerate() {
        // `motion` is column 11 and it saturates across a cut.
        if r[11] >= CUT && i - bounds[bounds.len() - 1] >= min && rows.len() - i >= min {
            bounds.push(i);
        }
    }
    bounds.push(rows.len());

    bounds
        .windows(2)
        .map(|w| Passage::over(w[0], w[1], &rows[w[0]..w[1]], rate))
        .collect()
}

// ---------------------------------------------------------------------------
// the plumbing
// ---------------------------------------------------------------------------

/// A still on disk → RGB8: JPEG (the ladder's own format, decoded by zune),
/// PNG or WebP (decoded by the `image` crate). The format is read from the
/// bytes, not the extension, so a renamed file still decodes.
///
/// Greyscale sources come back one byte per pixel and are expanded here rather
/// than handled everywhere downstream. Black-and-white footage is a real thing
/// to point a library at, and the alternative is `read::describe` growing a
/// component count.
pub fn decode(path: &Path) -> Result<Rgb, String> {
    let bytes = metal_vfs::read(path).map_err(|e| e.to_string())?;
    if is_jpeg(&bytes) {
        return decode_jpeg(&bytes);
    }
    decode_image(&bytes)
}

/// JPEG starts `FF D8 FF`.
fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

fn decode_jpeg(bytes: &[u8]) -> Result<Rgb, String> {
    // `ZCursor` rather than the slice: zune's reader trait is implemented for
    // `BufRead + Seek`, which a `&[u8]` is not, and for its own cursor, which
    // wraps one without copying.
    let mut decoder = zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(bytes));
    let px = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("no jpeg header")?;
    let (w, h) = (info.width as u32, info.height as u32);
    let n = (w as usize) * (h as usize);
    if n == 0 {
        return Err("zero-sized frame".into());
    }
    let px = match px.len() / n.max(1) {
        3 => px,
        1 => px.iter().flat_map(|v| [*v, *v, *v]).collect(),
        4 => px.chunks(4).flat_map(|c| [c[0], c[1], c[2]]).collect(),
        other => return Err(format!("{other} components per pixel")),
    };
    Ok(Rgb { width: w, height: h, px })
}

/// PNG or WebP (anything else `image` reads is refused by name). An alpha
/// channel is dropped: a still is a picture, and the descriptor and the
/// proxies are RGB.
fn decode_image(bytes: &[u8]) -> Result<Rgb, String> {
    let format = image::guess_format(bytes).map_err(|_| "not a JPEG, PNG or WebP".to_string())?;
    if !matches!(format, image::ImageFormat::Png | image::ImageFormat::WebP) {
        return Err(format!("unsupported still format {format:?}"));
    }
    let rgb = image::load_from_memory_with_format(bytes, format).map_err(|e| e.to_string())?.to_rgb8();
    let (w, h) = rgb.dimensions();
    if w == 0 || h == 0 {
        return Err("zero-sized frame".into());
    }
    Ok(Rgb { width: w, height: h, px: rgb.into_raw() })
}

/// `f` over every item, across every core, results in the original order.
///
/// # Why this is fifteen lines rather than a dependency
///
/// The crate header promises serde, a JPEG decoder and nothing else, because
/// this is what a venue laptop runs and what `fw-studio` trains. `std`'s scoped
/// threads borrow the slice for the length of the scope, which is the one thing
/// a work-stealing pool would buy here — there is no nesting, no recursion and
/// no uneven work, since every item is one frame of one ladder.
///
/// A panic in `f` propagates on `join`, as it would in the sequential loop this
/// replaced. Nothing here catches it: a JPEG that panics a decoder is a bug in
/// the decoder, and swallowing it would write a library with silent holes.
pub(crate) fn across<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    if cores <= 1 || items.len() <= 1 {
        return items.iter().map(f).collect();
    }
    let per = items.len().div_ceil(cores);
    let f = &f;
    std::thread::scope(|s| {
        let running: Vec<_> = items
            .chunks(per)
            .map(|c| s.spawn(move || c.iter().map(f).collect::<Vec<R>>()))
            .collect();
        running.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

fn count_rungs(dir: &Path) -> usize {
    metal_vfs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
        .count()
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let m = metal_vfs::metadata(path).ok()?;
    let modified = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(web_time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(Stamp { bytes: m.len(), modified })
}

fn sweep(frames: &Path, keep: &[Clip]) {
    for entry in metal_vfs::read_dir(frames).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !keep.iter().any(|c| c.id == name) {
            let _ = metal_vfs::remove_dir_all(entry.path());
        }
    }
}

/// Moves every library onto the one proxy folder and the one slug (a clip's
/// id is `scenes::slug` of its path): the ladder folder is renamed, the proxy
/// moves to `projects/hap/`, the clip id in `index.json` and in the picks
/// changes. Idempotent (a library already on the new slug is untouched), and
/// before it changes a file it keeps a `.bak-slug` copy beside it (once).
/// Returns how many clips it moved.
pub fn migrate_to_one_slug(data: &Path) -> usize {
    fn backup(path: &Path) {
        let bak = PathBuf::from(format!("{}.bak-slug", path.display()));
        if path.is_file() && !bak.exists() {
            let _ = std::fs::copy(path, bak);
        }
    }
    let mut moved = 0;
    for name in Library::list(data) {
        let Ok(mut lib) = Library::load(data, &name) else { continue };
        let source = PathBuf::from(&lib.source);
        let renames: Vec<(String, String)> = lib
            .clips
            .iter()
            .map(|c| (c.id.clone(), crate::scenes::slug(&source.join(&c.file).to_string_lossy())))
            .filter(|(old, new)| old != new)
            .collect();
        if renames.is_empty() {
            continue;
        }
        backup(&lib.root.join("index.json"));
        let picks_dir = lib.root.join("picks");
        let pick_files: Vec<PathBuf> = metal_vfs::read_dir(&picks_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        for f in &pick_files {
            backup(f);
        }
        for (old, new) in &renames {
            let (frames_old, frames_new) = (lib.root.join("frames").join(old), lib.root.join("frames").join(new));
            if frames_old.is_dir() && !frames_new.exists() {
                let _ = std::fs::rename(&frames_old, &frames_new);
            }
            let (old_movie, old_index) = crate::hap::Hap::paths(&lib.root, old);
            let (new_movie, new_index) = proxy_paths(data, new);
            if let Some(dir) = new_movie.parent() {
                let _ = metal_vfs::create_dir_all(dir);
            }
            for (from, to) in [(old_movie, new_movie), (old_index, new_index)] {
                if from.is_file() && !to.exists() {
                    let _ = std::fs::rename(&from, &to);
                }
            }
            for c in lib.clips.iter_mut().filter(|c| &c.id == old) {
                c.id = new.clone();
            }
            moved += 1;
        }
        for f in pick_files {
            let Ok(text) = metal_vfs::read_to_string(&f) else { continue };
            let Ok(mut picks) = serde_json::from_str::<Vec<crate::curate::Pick>>(&text) else { continue };
            for p in &mut picks {
                if let Some((_, new)) = renames.iter().find(|(old, _)| *old == p.clip) {
                    p.clip = new.clone();
                }
            }
            if let Ok(out) = serde_json::to_string_pretty(&picks) {
                let _ = metal_vfs::write(&f, out);
            }
        }
        let _ = lib.save();
    }
    moved
}

/// A filename → a directory name that is safe on every filesystem and stable
/// across them.
///
/// Not the filename. Footage is named by cameras and by people: spaces,
/// accents, colons, emoji, and two files that differ only in case. A ladder
/// directory is a path this app builds a hundred thousand times, and it has to
/// be one that means the same thing on the venue laptop and on the machine the
/// library was built on.
pub fn slug(file: &str) -> String {
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    let mut out = String::with_capacity(stem.len());
    for c in stem.chars() {
        match c {
            'a'..='z' | '0'..='9' => out.push(c),
            'A'..='Z' => out.push(c.to_ascii_lowercase()),
            _ if out.ends_with('-') => {}
            _ => out.push('-'),
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "clip".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 picture, one colour per corner, written in `format` to a tempdir.
    fn still_file(name: &str, format: image::ImageFormat) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-reel-still-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = image::RgbImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        img.put_pixel(1, 0, image::Rgb([0, 255, 0]));
        img.put_pixel(0, 1, image::Rgb([0, 0, 255]));
        img.put_pixel(1, 1, image::Rgb([255, 255, 255]));
        let path = dir.join(name);
        img.save_with_format(&path, format).unwrap();
        path
    }

    /// PNG and WebP stills decode to the same RGB8 pixels the picture holds
    /// (both are lossless here), beside the JPEG path that was already there.
    #[test]
    fn a_png_and_a_webp_still_decode_like_a_jpeg_does_not_have_to() {
        for (name, format) in [("a.png", image::ImageFormat::Png), ("a.webp", image::ImageFormat::WebP)] {
            let path = still_file(name, format);
            let rgb = decode(&path).expect("decodes");
            assert_eq!((rgb.width, rgb.height), (2, 2), "{name}");
            assert_eq!(&rgb.px[0..3], &[255, 0, 0], "{name}: top-left is red");
            assert_eq!(&rgb.px[9..12], &[255, 255, 255], "{name}: bottom-right is white");
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }

    /// The format is read from the bytes: a PNG renamed `.jpg` still decodes,
    /// and bytes that are none of the three are refused by name.
    #[test]
    fn a_still_is_read_by_its_bytes_and_a_stranger_is_refused() {
        let path = still_file("renamed.jpg", image::ImageFormat::Png);
        assert!(decode(&path).is_ok(), "a PNG under a .jpg name decodes");
        std::fs::write(&path, b"GIF89a not a still we take").unwrap();
        let Err(err) = decode(&path) else { panic!("a GIF is refused") };
        assert!(err.contains("not a JPEG, PNG or WebP") || err.contains("unsupported"), "{err}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    fn rows(n: usize, motion_at: &[usize]) -> Vec<[f32; COLUMNS]> {
        (0..n)
            .map(|i| {
                let mut r = [0.4f32; COLUMNS];
                r[11] = if motion_at.contains(&i) { 0.9 } else { 0.05 };
                r
            })
            .collect()
    }

    #[test]
    fn a_clip_with_no_cuts_is_one_passage() {
        let p = cut(&rows(240, &[]), FPS);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].from, p[0].to), (0, 240));
    }

    #[test]
    fn a_clip_is_cut_at_its_cuts() {
        let p = cut(&rows(360, &[120, 240]), FPS);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].to, 120);
        assert_eq!(p[1].from, 120);
        assert_eq!(p[2].from, 240);
    }

    #[test]
    fn passages_tile_the_clip_with_no_gaps_and_no_overlap() {
        // The property the Curator relies on: every rung belongs to exactly one
        // passage, so "play this passage" can never land on footage that is not
        // in it.
        for cuts in [vec![], vec![50], vec![40, 90, 200], vec![25, 26, 27]] {
            let n = 300;
            let p = cut(&rows(n, &cuts), FPS);
            assert_eq!(p[0].from, 0);
            assert_eq!(p[p.len() - 1].to, n);
            for w in p.windows(2) {
                assert_eq!(w[0].to, w[1].from, "gap or overlap with cuts {cuts:?}");
            }
        }
    }

    #[test]
    fn no_passage_is_shorter_than_the_floor() {
        // Cuts one rung apart must not shatter the clip. This is the case a
        // flickering strobe shot produces, and without the floor it makes a
        // hundred passages that all describe the same footage.
        let every = (0..300).collect::<Vec<_>>();
        let p = cut(&rows(300, &every), FPS);
        let min = (MIN_SECONDS * FPS).ceil() as usize;
        for s in &p {
            assert!(s.to - s.from >= min, "a passage of {} rungs", s.to - s.from);
        }
    }

    #[test]
    fn a_clip_too_short_to_divide_is_still_a_passage() {
        // A one-second sting is a legitimate thing to cue. Returning nothing
        // would make it invisible to the Curator with no error anywhere.
        let p = cut(&rows(8, &[4]), FPS);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].from, p[0].to), (0, 8));
    }

    #[test]
    fn a_descriptor_between_two_rungs_is_between_them() {
        let clip = Clip {
            id: "c".into(),
            file: "c.mp4".into(),
            seconds: 2.0,
            stamp: Stamp { bytes: 0, modified: 0 },
            rows: vec![[0.0; COLUMNS], [1.0; COLUMNS]],
            passages: vec![],
        };
        // Halfway between rung 0 and rung 1, at 12 rungs a second, is 1/24 s.
        let mid = clip.at(0.5 / FPS, FPS);
        assert!((mid[0] - 0.5).abs() < 1e-5, "{}", mid[0]);
        assert_eq!(clip.at(0.0, FPS)[0], 0.0);
        // Past the end holds the last rung rather than wrapping.
        assert_eq!(clip.at(99.0, FPS)[0], 1.0);
    }

    #[test]
    fn the_rung_on_screen_is_the_one_whose_interval_contains_the_time() {
        let clip = Clip {
            id: "c".into(),
            file: "c.mp4".into(),
            seconds: 1.0,
            stamp: Stamp { bytes: 0, modified: 0 },
            rows: vec![[0.0; COLUMNS]; 12],
            passages: vec![],
        };
        assert_eq!(clip.rung(0.0, FPS), 0);
        // Just before the second rung is still the first.
        assert_eq!(clip.rung(1.0 / FPS - 1e-4, FPS), 0);
        assert_eq!(clip.rung(1.0 / FPS, FPS), 1);
        assert_eq!(clip.rung(1000.0, FPS), 11);
    }

    #[test]
    fn a_filename_becomes_a_path_that_means_the_same_thing_everywhere() {
        assert_eq!(slug("Get Down On It.MOV"), "get-down-on-it");
        assert_eq!(slug("C0012.MP4"), "c0012");
        assert_eq!(slug("naïve — b (2).mkv"), "na-ve-b-2");
        assert_eq!(slug("....mp4"), "clip");
        assert_eq!(slug("no-extension"), "no-extension");
    }

    #[test]
    fn two_files_that_slug_the_same_do_not_share_a_ladder() {
        let used = vec!["a".to_string()];
        assert_eq!(unique("a", &used), "a-2");
        assert_eq!(unique("a", &["a".into(), "a-2".into()]), "a-3");
        assert_eq!(unique("b", &used), "b");
    }

    #[test]
    fn a_scene_carries_the_three_qualities_the_mean_loses() {
        // The whole reason these are stored: each pair below averages to the
        // same descriptor and is a different thing to cut to.
        let flat = |n: usize, m: f32| {
            (0..n)
                .map(|_| {
                    let mut r = [0.4f32; COLUMNS];
                    r[11] = m;
                    r
                })
                .collect::<Vec<_>>()
        };

        // Length. Two scenes of identical footage, one short and one long.
        let short = Passage::over(0, 24, &flat(24, 0.2), FPS);
        let long = Passage::over(0, 240, &flat(240, 0.2), FPS);
        for (a, b) in short.d.iter().zip(long.d.iter()) {
            assert!((a - b).abs() < 1e-4, "the means must match for this test");
        }
        assert!((short.seconds - 2.0).abs() < 1e-4, "{}", short.seconds);
        assert!((long.seconds - 20.0).abs() < 1e-4, "{}", long.seconds);

        // Peak. Steady movement against still-then-one-hit, same mean.
        let steady = flat(100, 0.2);
        let mut hit = flat(100, 0.0);
        for r in hit.iter_mut().take(20) {
            r[11] = 1.0;
        }
        let (steady, hit) =
            (Passage::over(0, 100, &steady, FPS), Passage::over(0, 100, &hit, FPS));
        assert!((steady.d[11] - hit.d[11]).abs() < 1e-5, "the means must match for this test");
        assert!(steady.peak < 0.3, "{}", steady.peak);
        assert!(hit.peak > 0.9, "{}", hit.peak);

        // Variety. A locked-off shot against one that becomes something else,
        // both averaging to the middle.
        let locked = vec![[0.5f32; COLUMNS]; 100];
        let mut moving = vec![[0.0f32; COLUMNS]; 100];
        for r in moving.iter_mut().take(50) {
            *r = [1.0; COLUMNS];
        }
        let (locked, moving) =
            (Passage::over(0, 100, &locked, FPS), Passage::over(0, 100, &moving, FPS));
        assert_eq!(locked.variety, 0.0);
        assert!(moving.variety > 0.4, "{}", moving.variety);
    }

    #[test]
    fn work_spread_over_the_cores_comes_back_in_order() {
        // The rung order *is* the clip. A pool that returned frames as they
        // finished would describe a shuffled clip, and every descriptor would
        // still be in range and every passage still tile it — so nothing
        // downstream could catch it.
        for n in [0, 1, 2, 7, 8, 9, 63, CHUNK, CHUNK + 1, 500] {
            let items: Vec<usize> = (0..n).collect();
            assert_eq!(across(&items, |k| k * 2), (0..n).map(|k| k * 2).collect::<Vec<_>>());
        }
    }

    /// 2b: a library built under the filename slug moves onto the path slug and
    /// the one proxy folder; its picks follow; the originals are kept as
    /// `.bak-slug`; a second run moves nothing.
    #[test]
    fn a_library_migrates_onto_the_one_slug_and_proxy_folder_once() {
        let data = std::env::temp_dir().join(format!("fw-one-slug-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let root = Library::dir(&data, "lib");
        let clip = Clip {
            id: "intro".into(),
            file: "intro.mp4".into(),
            seconds: 1.0,
            stamp: Stamp { bytes: 1, modified: 1 },
            rows: Vec::new(),
            passages: Vec::new(),
        };
        let lib = Library { name: "lib".into(), source: "/footage".into(), rate: FPS, width: WIDTH, clips: vec![clip], root: root.clone() };
        lib.save().unwrap();
        std::fs::create_dir_all(root.join("frames/intro")).unwrap();
        std::fs::write(root.join("frames/intro/000001.jpg"), b"j").unwrap();
        std::fs::create_dir_all(root.join("hap")).unwrap();
        std::fs::write(root.join("hap/intro.mov"), b"mov").unwrap();
        std::fs::write(root.join("hap/intro.json"), b"{}").unwrap();
        let pick = crate::curate::Pick { song: "s".into(), tick: 3, clip: "intro".into(), passage: 0, play: Default::default() };
        crate::curate::save_picks(&data, "lib", "s", &[pick]).unwrap();

        let new = crate::scenes::slug("/footage/intro.mp4");
        assert_eq!(migrate_to_one_slug(&data), 1);
        let after = Library::load(&data, "lib").unwrap();
        assert_eq!(after.clips[0].id, new);
        assert!(root.join("frames").join(&new).join("000001.jpg").is_file());
        assert!(proxy_paths(&data, &new).0.is_file() && proxy_paths(&data, &new).1.is_file());
        assert!(!root.join("hap/intro.mov").exists());
        assert_eq!(crate::curate::load_picks(&data, "lib")[0].clip, new);
        assert!(root.join("index.json.bak-slug").is_file(), "the index is backed up");
        assert!(root.join("picks/s.json.bak-slug").is_file(), "the picks are backed up");
        let bak = std::fs::read_to_string(root.join("picks/s.json.bak-slug")).unwrap();
        assert!(bak.contains("\"intro\""), "the backup is the old file");

        assert_eq!(migrate_to_one_slug(&data), 0, "idempotent");
        assert_eq!(std::fs::read_to_string(root.join("picks/s.json.bak-slug")).unwrap(), bak, "the backup is kept from the first run");
        let _ = std::fs::remove_dir_all(&data);
    }
}
