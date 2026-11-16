//! **The Curator** — which footage, out of whatever folder is loaded.
//!
//! # Why this is not a classifier
//!
//! `metal_router` is a multinomial logistic regression over thirty animations,
//! and that is right for it: the classes are known when the binary is built,
//! and an animation nobody authored for scores low everywhere and is never
//! chosen. The whole design leans on the set being closed.
//!
//! A library is not closed. It is a folder somebody dropped on the laptop this
//! afternoon, and a classifier over its clips could not answer a single
//! question until it had been retrained on that folder. A visuals system that
//! needs a training run before the first clip can play is a system that does
//! not work on the night it is needed.
//!
//! So the Curator learns the **relationship** rather than the classes:
//!
//! ```text
//!   music, in metal_router's twelve features  ──►  the image this music wants,
//!                                               in read's twelve columns
//!                                                        │
//!                            the nearest passage in the loaded library  ◄──┘
//! ```
//!
//! Both ends are fixed-width and neither mentions a clip, so a coupling fitted
//! against last month's folder decides for this afternoon's. That is the
//! property that decides the architecture, and it is worth more than the
//! accuracy a classifier would have had.
//!
//! It also means the trained artefact is **one file for the whole app**
//! (`data/reels/curator.json`), not one per library — because it is not about
//! any library. The picks that trained it *are* per library, because a pick
//! names a clip.
//!
//! # It works before it is trained, and says so
//!
//! [`DEFAULT`] is a hand-written coupling: twenty-odd lines saying loud music
//! wants a bright image, machine music wants footage that moves, a wide harsh
//! spectrum wants a cool one. It is not a stand-in for a trained model in the
//! way `digest::Simulated` is a stand-in for a microphone — a simulated digest
//! is a *lie about the input*, and this is a **default taste**, which is a
//! legitimate thing for software to have.
//!
//! [`Curator::trained`] is still false and the app still says so, because the
//! difference between somebody's taste and nobody's taste is the difference the
//! studio's authoring surface exists to close.
//!
//! # Where a passage comes from
//!
//! Not from here. `index::cut` divides a clip at its own cuts, untrained,
//! exactly as `metal_router::segment` divides a song at its novelty peaks: **a cut
//! is a fact about the footage.** What goes in a passage is taste and lives in
//! this module; where one ends does not.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::index::Library;
use crate::read::{COLUMNS, NAMES};

/// How many numbers describe the music. `metal_router`'s twelve, not the digest's
/// sixteen — the question is "what kind of passage is this", which is the
/// question that crate already answers.
pub use metal_router::features::FEATURES;

/// How long a stretch of digest the live Curator describes before deciding.
/// The same twelve seconds `metal-dash::router` uses, so the two agree about what
/// "now" means.
pub const WINDOW_SECONDS: f32 = 12.0;

/// The shortest a cue may stand, in seconds.
///
/// The same 24 s as `metal-dash::router::HOLD_SECONDS`, and the same argument:
/// without a floor the output becomes a clock again. It is stated here rather
/// than imported because that crate depends on this one, and it is checked
/// against by a test in `metal-dash`.
pub const HOLD_SECONDS: f32 = 24.0;

/// How near the music has to be to an authored moment before that pick is
/// played instead of the model's answer. RMS over the twelve features, so it
/// reads in the same units as one feature.
///
/// **Measured over `data/audio` — 74 songs, twelve-second windows, the same
/// windows `samples` trains on.** The distance between two windows is:
///
/// | pair | median | p05 | share within 0.02 |
/// |---|---:|---:|---:|
/// | same song, 3 s apart | 0.020 | 0.007 | 50% |
/// | same song, 30 s apart | 0.058 | 0.021 | 4.5% |
/// | different songs | 0.115 | 0.049 | 0.05% |
///
/// So 0.02 is the median distance from a moment to *itself three seconds
/// later*. Under it, a recall means "this is that moment", not "this resembles
/// it": half a minute further into the same song already misses it 95 times out
/// of a hundred, and another song's music collides five times in ten thousand.
///
/// It is deliberately far tighter than the model's own scale — `Recent`'s
/// cooldown is 0.2 of an image column, "roughly how far apart two genuinely
/// different shots are". A loose radius here would be the failure this project
/// has named twice: an authored pick firing over music it was never about is
/// silent, in range, and the wrong image.
pub const RECALL_RADIUS: f32 = 0.02;

/// How long a passage stays penalised after being cued.
///
/// Long, on purpose. The distance to the second-best passage in a folder of
/// similar rushes is often tiny, so without this the Curator would sit on one
/// clip for an hour and be *right* every time. A set is a night, and a night
/// wants the folder.
pub const COOL_SECONDS: f32 = 240.0;

// ---------------------------------------------------------------------------
// the columns, by name
// ---------------------------------------------------------------------------
//
// Both ends of this model are twelve numbers and neither is readable as an
// index. These are here so [`DEFAULT`] is a table somebody can argue with.

// `metal_router::features::NAMES`.
const ENERGY: usize = 0;
const DYNAMICS: usize = 1;
const BRIGHTNESS: usize = 2;
const WIDTH: usize = 3;
const HIT: usize = 4;
const PEAK: usize = 5;
const DENSITY: usize = 6;
const PULSE: usize = 7;
const TONALITY: usize = 8;
const HARMONY: usize = 9;

// `crate::read::NAMES`.
const EXPOSURE: usize = 0;
const CONTRAST: usize = 1;
const HIGHLIGHTS: usize = 2;
const SHADOWS: usize = 3;
const SATURATION: usize = 4;
const TEMPERATURE: usize = 5;
const TINT: usize = 6;
const R: usize = 7;
const G: usize = 8;
const B: usize = 9;
const DETAIL: usize = 10;
const MOTION: usize = 11;

/// One line of the default coupling: this musical feature pulls that image
/// column this far.
pub struct Pull {
    pub image: usize,
    pub music: usize,
    pub weight: f32,
}

const fn pull(image: usize, music: usize, weight: f32) -> Pull {
    Pull { image, music, weight }
}

/// Where each image column sits when the music says nothing — the bias of the
/// default coupling.
///
/// Not zeros and not 0.5s. These are roughly what ordinary footage reads,
/// measured on the test material and rounded to something a person would write:
/// a normally exposed shot is a bit under half, most footage is neutral in
/// temperature and tint, and nothing is still.
const NEUTRAL: [f32; COLUMNS] = [
    0.45, // exposure
    0.55, // contrast
    0.10, // highlights
    0.20, // shadows
    0.35, // saturation
    0.50, // temperature
    0.50, // tint
    0.45, 0.44, 0.43, // r g b — footage is very slightly warm on average
    0.35, // detail
    0.20, // motion
];

/// The taste the Curator has before anybody gives it one.
///
/// Every line is a sentence somebody would say out loud, which is the test this
/// table has to pass — a default nobody can read is a default nobody can
/// disagree with, and then it never gets replaced.
pub const DEFAULT: &[Pull] = &[
    // Loud wants bright, and quiet wants the shadows.
    pull(EXPOSURE, ENERGY, 0.35),
    pull(EXPOSURE, BRIGHTNESS, 0.15),
    pull(HIGHLIGHTS, PEAK, 0.25),
    pull(SHADOWS, ENERGY, -0.30),
    // A track that hits wants an image with something to hit against.
    pull(CONTRAST, HIT, 0.30),
    pull(CONTRAST, DYNAMICS, 0.20),
    // Loud and tonal wants colour; sparse wants it drained.
    pull(SATURATION, ENERGY, 0.30),
    pull(SATURATION, TONALITY, 0.20),
    // A bright, wide, harsh spectrum reads cool. A tonal one reads warm. This
    // is the one line here that is a real claim rather than an obvious one, and
    // it is the first thing worth overruling by authoring.
    pull(TEMPERATURE, BRIGHTNESS, -0.20),
    pull(TEMPERATURE, TONALITY, 0.15),
    pull(TINT, HARMONY, 0.10),
    pull(R, ENERGY, 0.25),
    pull(G, TONALITY, 0.15),
    pull(B, BRIGHTNESS, 0.25),
    // Busy music wants a busy frame.
    pull(DETAIL, DENSITY, 0.30),
    pull(DETAIL, WIDTH, 0.15),
    // And the one that carries most of it: machine music that hits wants
    // footage that moves and cuts.
    pull(MOTION, DENSITY, 0.30),
    pull(MOTION, PULSE, 0.25),
    pull(MOTION, HIT, 0.20),
];

/// How much each image column counts when measuring how close a passage is to
/// what the music asked for.
///
/// **This is where the redundancy `read` admits to is paid for.** Temperature
/// and tint are a rotation of the three channel means, so a library matched on
/// all five would count "how warm is this" roughly three times and "how busy is
/// it" once. The three channels share one column's worth between them.
///
/// `movimiento` is over 1 because it is the column an audience notices being
/// wrong: still footage under a drop reads as the system having crashed, where
/// a shot that is slightly too warm reads as a shot.
const MATCH: [f32; COLUMNS] = [
    1.0,  // exposure
    1.0,  // contrast
    0.6,  // highlights
    0.6,  // shadows
    0.9,  // saturation
    0.8,  // temperature
    0.5,  // tint
    0.33, 0.33, 0.33, // r g b
    1.0,  // detail
    1.3,  // motion
];

// ---------------------------------------------------------------------------
// the model
// ---------------------------------------------------------------------------

/// Music features → an image descriptor. Linear, with a bias.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Coupling {
    /// [`COLUMNS`] rows of `FEATURES + 1`: the weights, then the bias.
    pub w: Vec<Vec<f32>>,
    /// How many songs and how many picks it was fitted on. Printed on the page,
    /// because "the model is trained" and "the model is trained on one song"
    /// have to look different.
    pub songs: usize,
    pub picks: usize,
    /// Held-out mean absolute error and the same for the baseline that always
    /// answers the corpus mean. `None` under three songs — there is no honest
    /// number to give, and `metal_router::learn` refuses in the same place for the
    /// same reason.
    pub error: Option<(f32, f32)>,
}

impl Coupling {
    pub fn trained(&self) -> bool {
        self.w.len() == COLUMNS && self.w.iter().all(|r| r.len() == FEATURES + 1)
    }

    /// The default taste, as a coupling — so [`Curator::want`] has one code
    /// path and the untrained case is not a branch in the middle of the model.
    pub fn authored() -> Coupling {
        let mut w = vec![vec![0.0f32; FEATURES + 1]; COLUMNS];
        for (i, row) in w.iter_mut().enumerate() {
            row[FEATURES] = NEUTRAL[i];
        }
        for p in DEFAULT {
            w[p.image][p.music] += p.weight;
        }
        Coupling { w, songs: 0, picks: 0, error: None }
    }

    fn apply(&self, music: &[f32; FEATURES]) -> [f32; COLUMNS] {
        let mut out = [0.0f32; COLUMNS];
        for i in 0..COLUMNS {
            let row = &self.w[i];
            let mut v = row[FEATURES];
            for k in 0..FEATURES {
                v += row[k] * music[k];
            }
            out[i] = v.clamp(0.0, 1.0);
        }
        out
    }
}

/// The trained artefact, loaded once and asked many times.
#[derive(Clone, Debug)]
pub struct Curator {
    pub coupling: Coupling,
    /// Whether `coupling` came off disk or out of [`DEFAULT`].
    fitted: bool,
}

impl Default for Curator {
    fn default() -> Curator {
        Curator { coupling: Coupling::authored(), fitted: false }
    }
}

/// Where the Curator's own state lives under the data directory. One file for
/// the whole app — see the module header for why it is not per library.
pub fn model_path(data: &Path) -> PathBuf {
    data.join(crate::index::DIR).join("curator.json")
}

impl Curator {
    /// Loads `data/reels/curator.json`, or the authored default if there is
    /// none — which is the normal state of a fresh checkout.
    pub fn load(data: &Path) -> Curator {
        match metal_vfs::read_to_string(model_path(data))
            .ok()
            .and_then(|t| serde_json::from_str::<Coupling>(&t).ok())
            .filter(|c| c.trained())
        {
            Some(coupling) => Curator { coupling, fitted: true },
            None => Curator::default(),
        }
    }

    /// A Curator around a coupling that was just fitted.
    ///
    /// The one way to build a trained one from outside this crate, and it
    /// exists because fitting happens in `fw-studio` while `fitted` has to stay
    /// private — a caller that could set it freely could save a coupling that
    /// came out of [`DEFAULT`] while claiming somebody authored it, and
    /// [`Curator::trained`] is what the app prints to say whether anyone has.
    pub fn from_fit(coupling: Coupling) -> Curator {
        Curator { coupling, fitted: true }
    }

    pub fn save(&self, data: &Path) -> Result<(), String> {
        let path = model_path(data);
        if let Some(parent) = path.parent() {
            metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&self.coupling).map_err(|e| e.to_string())?;
        metal_vfs::write(path, text).map_err(|e| e.to_string())
    }

    /// Whether anybody's curation is in here. False means [`DEFAULT`], and the
    /// app has to say so rather than let a default read as a decision.
    pub fn trained(&self) -> bool {
        self.fitted
    }

    /// The image this music is asking for.
    pub fn want(&self, music: &[f32; FEATURES]) -> [f32; COLUMNS] {
        self.coupling.apply(music)
    }

    /// Which passage of which clip should be on, out of `lib`, **honouring what
    /// the operator authored** before asking the model.
    ///
    /// This is the live entry point. [`Curator::choose`] is still the whole of
    /// the model's answer and is called unchanged when nothing was authored for
    /// this music — a library with no picks, or picks about other moments, gets
    /// exactly the cue it got before this method existed.
    ///
    /// # The rule, in one line
    ///
    /// The nearest [`Memory`] wins outright if it is within [`RECALL_RADIUS`]
    /// of the music playing now; otherwise the model decides.
    ///
    /// "Outright" means what it says: no score is consulted, the cooldown does
    /// not apply, and the pick's own [`Play`] is carried into the cue — which is
    /// the sentence [`Curator::choose`]'s own comment has been promising since
    /// it was written (*"anything else is an authored decision and arrives from
    /// a lane"*). An operator who said *this shot, here* is not making a
    /// suggestion to a regression.
    ///
    /// # Why a radius at all, and why that one
    ///
    /// A pick is authored at a tick of a song. **The live path has no song and
    /// no tick** — `metal-dash`'s clock is app seconds and its input is a
    /// microphone — so the join cannot be by time and has to be by *what the
    /// music sounds like*, which is the pair `samples` already builds for
    /// training. Without a radius the nearest pick would always be "nearest"
    /// and a single authored moment would take over the whole night.
    ///
    /// [`RECALL_RADIUS`] is measured, not chosen: see its own documentation.
    ///
    /// A `Memory` whose `clip_id` does not match the clip sitting at its index
    /// is **skipped**, not followed. That is the case where a table built
    /// against one folder is handed another: every index is in range and every
    /// one is the wrong shot.
    pub fn choose_with(
        &self,
        lib: &Library,
        music: &[f32; FEATURES],
        recent: &Recent,
        now: f32,
        memory: &[Memory],
    ) -> Option<Cue> {
        let mut best: Option<(f32, &Memory)> = None;
        for m in memory {
            // Not `lib.clips[m.clip]`: a stale or foreign table indexes in
            // range and points at another shot.
            let Some(clip) = lib.clips.get(m.clip) else { continue };
            if clip.id != m.clip_id || m.passage >= clip.passages.len() {
                continue;
            }
            let d = music_distance(music, &m.music);
            // Ties broken by the earlier pick, so two picks authored over the
            // same bar answer the same way every time.
            if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, m));
            }
        }

        match best {
            Some((d, m)) if d <= RECALL_RADIUS => {
                let clip = &lib.clips[m.clip];
                let p = clip.passages[m.passage];
                Some(Cue {
                    clip: m.clip,
                    clip_id: clip.id.clone(),
                    passage: m.passage,
                    from: p.from as f32 / lib.rate,
                    span: (p.to - p.from) as f32 / lib.rate,
                    since: now,
                    // Not the model's confidence, and the page must not read it
                    // as one — that is what `authored` is for. Somebody said so
                    // is not a probability.
                    confidence: 1.0,
                    because: None,
                    play: m.play,
                    authored: Some(Authored {
                        song: m.song.clone(),
                        tick: m.tick,
                        near: d,
                    }),
                })
            }
            _ => self.choose(lib, music, recent, now),
        }
    }

    /// Which passage of which clip should be on, out of `lib`.
    ///
    /// `recent` is what has played and when; a passage cued lately is pushed
    /// away rather than forbidden, so a library of one clip still answers.
    /// `now` is wall-clock seconds and is what the returned cue is stamped
    /// with.
    pub fn choose(
        &self,
        lib: &Library,
        music: &[f32; FEATURES],
        recent: &Recent,
        now: f32,
    ) -> Option<Cue> {
        let want = self.want(music);
        let mut scored: Vec<(f32, usize, usize)> = Vec::new();
        for (ci, pi) in lib.passages() {
            let clip = &lib.clips[ci];
            let p = &clip.passages[pi];
            let d = distance(&want, &p.d) + recent.penalty(ci, pi, now);
            scored.push((d, ci, pi));
        }
        if scored.is_empty() {
            return None;
        }
        scored.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.1, a.2).cmp(&(b.1, b.2))));

        let (best, ci, pi) = scored[0];
        let clip = &lib.clips[ci];
        let p = clip.passages[pi];

        // How much better than the runner-up, so a folder of near-identical
        // rushes reports low confidence rather than a decision it cannot
        // defend. One candidate is a certainty by construction.
        let (confidence, because) = match scored.get(1) {
            Some(&(second, oc, op)) if second > 1e-6 => {
                let other = lib.clips[oc].passages[op].d;
                (
                    ((second - best) / second).clamp(0.0, 1.0),
                    winning_column(&want, &p.d, &other),
                )
            }
            _ => (1.0, None),
        };

        Some(Cue {
            clip: ci,
            clip_id: clip.id.clone(),
            passage: pi,
            from: p.from as f32 / lib.rate,
            span: (p.to - p.from) as f32 / lib.rate,
            since: now,
            confidence,
            because,
            // An automatic cue plays the scene as it was shot. Anything else is
            // an authored decision and arrives from a lane, not from the model —
            // which is `Curator::choose_with`, and it does not come through
            // here.
            play: Play::default(),
            authored: None,
        })
    }
}

/// How far a passage is from what was asked for. Weighted RMS over the twelve
/// columns, so the number is in the same units as one column and reads as
/// "about a fifth of a column out".
fn distance(want: &[f32; COLUMNS], have: &[f32; COLUMNS]) -> f32 {
    let mut total = 0.0;
    let mut weight = 0.0;
    for i in 0..COLUMNS {
        let d = want[i] - have[i];
        total += MATCH[i] * d * d;
        weight += MATCH[i];
    }
    (total / weight.max(1e-6)).sqrt()
}

/// How far the music is from the music an operator was hearing when they
/// authored a pick. Unweighted RMS over the twelve features.
///
/// Unweighted on purpose, and it is not the same function as [`distance`]:
/// `MATCH` weights the *image* columns by how much each one matters to how a
/// shot looks, and there is no equivalent claim about the features. Reusing
/// `MATCH` here would have been twelve numbers about pictures deciding which
/// second of a song this is.
pub fn music_distance(a: &[f32; FEATURES], b: &[f32; FEATURES]) -> f32 {
    let mut total = 0.0;
    for i in 0..FEATURES {
        let d = a[i] - b[i];
        total += d * d;
    }
    (total / FEATURES as f32).sqrt()
}

/// Which column most explains the winner beating the runner-up.
///
/// `metal_router::Decision::because` surfaced the feature with the largest weight
/// toward the chosen class; a nearest-match has no weights, so the analogue is
/// the column that contributed most of the *gap*. Same job: making bad curation
/// readable rather than mysterious.
fn winning_column(
    want: &[f32; COLUMNS],
    won: &[f32; COLUMNS],
    lost: &[f32; COLUMNS],
) -> Option<(&'static str, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for i in 0..COLUMNS {
        let a = want[i] - won[i];
        let b = want[i] - lost[i];
        let gain = MATCH[i] * (b * b - a * a);
        if gain > best.map(|(_, g)| g).unwrap_or(0.0) {
            best = Some((i, gain));
        }
    }
    best.map(|(i, g)| (NAMES[i], g))
}

// ---------------------------------------------------------------------------
// a cue
// ---------------------------------------------------------------------------

/// How a scene is played once it has been chosen.
///
/// The Curator answers *which* footage; this is *what is done with it*, and the
/// two are separate because they are authored by different things — the model
/// picks the scene, the operator's lane says it should crawl backwards through
/// the middle second of it.
///
/// # A held frame is a loop of no length
///
/// There is no `hold` field. `from == to` is a window of zero width, which
/// [`Cue::position`] answers with that single frame forever — so "play this
/// scene", "loop this part of it" and "hold this one frame" are one mechanism
/// at three settings rather than three flags that can contradict each other.
/// The flag version has a meaning for *hold and reverse and loop all on*, and
/// nobody would agree what it is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Play {
    /// Multiplies the model's own rate. Below 1 is slow motion.
    ///
    /// Not signed — direction is [`Play::reverse`]. A negative speed and a
    /// reverse flag would be the same thing said twice, and `-1.0` with
    /// `reverse` set is a question with no good answer.
    pub speed: f32,
    pub reverse: bool,
    /// The part of the passage that plays, as fractions of its span. Fractions
    /// rather than seconds so a pick survives the passage being re-cut a rung
    /// wider by a later index.
    pub from: f32,
    pub to: f32,
}

impl Default for Play {
    fn default() -> Play {
        Play { speed: 1.0, reverse: false, from: 0.0, to: 1.0 }
    }
}

impl Play {
    /// The window, ordered and clamped. Authored values arrive from a page and
    /// a dragged handle crosses over its other end all the time.
    fn window(&self) -> (f32, f32) {
        let a = self.from.clamp(0.0, 1.0);
        let b = self.to.clamp(0.0, 1.0);
        (a.min(b), a.max(b))
    }

    /// Whether this holds a single frame rather than playing.
    pub fn held(&self) -> bool {
        let (a, b) = self.window();
        b - a <= f32::EPSILON
    }
}

/// What is on: a passage of a clip, and when it went on.
///
/// Held across a passage of the music, like a `metal_router::Decision` — the same
/// shape asked for on 2026-08-22, one level down. It carries `since` rather
/// than a position so that **everything downstream stays a function of the
/// clock**: the renderer is given a cue and a `time`, and where in the footage
/// that lands is arithmetic. Nothing advances a playhead.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    /// Index into `Library::clips`.
    pub clip: usize,
    /// The clip's stable id, so a cue survives a re-index that reorders the
    /// folder — which adding one file at the top of it does.
    pub clip_id: String,
    pub passage: usize,
    /// Seconds into the clip where the passage starts.
    pub from: f32,
    pub span: f32,
    /// The wall-clock second this cue was taken.
    pub since: f32,
    pub confidence: f32,
    pub because: Option<(&'static str, f32)>,
    /// How it is played. Default plays the whole passage forwards at the
    /// model's own rate, which is what an automatic cue always wants — only the
    /// lane authors anything else.
    pub play: Play,
    /// Which authored pick put this shot on, if one did. `None` is the model's
    /// own answer and is what every cue was before [`Curator::choose_with`]
    /// existed.
    ///
    /// It is carried rather than derived because the page has to be able to say
    /// *why* a shot is on. A recalled cue and a scored cue look identical from
    /// outside — same fields, same range — and a booth that cannot tell them
    /// apart cannot tell a working lane from a broken one.
    pub authored: Option<Authored>,
}

/// The pick a cue came from, for the page to print.
#[derive(Clone, Debug, PartialEq)]
pub struct Authored {
    pub song: String,
    /// The 8 Hz tick of that song the pick was made at.
    pub tick: usize,
    /// How far the music was from that moment when this fired, in the units of
    /// [`RECALL_RADIUS`]. Printed so a recall that keeps firing at the edge of
    /// the radius is visible rather than mysterious.
    pub near: f32,
}

impl Cue {
    /// Where in the clip this cue is at `time`, in seconds, at playback rate
    /// `speed`.
    ///
    /// **Loops inside the window.** A cue outlives its own footage all the
    /// time — a four-second insert under a ninety-second section — and the
    /// alternatives are freezing on the last rung or falling out of the passage
    /// into unrelated footage. Neither is what "this shot is on" means.
    pub fn position(&self, time: f32, speed: f32) -> f32 {
        if self.span <= 0.0 {
            return self.from;
        }
        let (lo, hi) = self.play.window();
        let head = self.from + lo * self.span;
        let window = (hi - lo) * self.span;
        if window <= 0.0 {
            // A held frame. It is not a degenerate case to guard against, it is
            // what `from == to` means.
            return head;
        }
        let elapsed = ((time - self.since) * speed * self.play.speed.max(0.0)).max(0.0);
        // `inf % span` is NaN, and a NaN playhead does not crash — it casts to
        // rung 0 and shows the head of the passage forever, with nothing in any
        // log. `NaN.max(0.0)` is already 0.0 in Rust, so only the infinities
        // need saying. Found by `models::estela`'s runaway-clock gate.
        if !elapsed.is_finite() {
            return head;
        }
        let x = elapsed % window;
        // The second modulo is what keeps a reversed loop inside its own
        // window: `window - 0` is `window`, which is the *next* passage's first
        // rung, and one frame of the wrong shot at every loop point is exactly
        // the class of bug the forward direction never had.
        let offset = if self.play.reverse { (window - x) % window } else { x };
        head + offset
    }

    /// Whether this cue has stood long enough to be replaced.
    pub fn ripe(&self, time: f32) -> bool {
        time - self.since >= HOLD_SECONDS
    }
}

/// What has been cued lately, so the Curator works its way through a folder
/// instead of finding one right answer and sitting on it.
#[derive(Clone, Debug, Default)]
pub struct Recent {
    seen: Vec<(usize, usize, f32)>,
}

impl Recent {
    /// How much worse a passage looks for having played recently. Decays
    /// linearly to nothing over [`COOL_SECONDS`].
    ///
    /// The scale is a fifth of a column, which is roughly how far apart two
    /// genuinely different shots are. So a *clearly* better passage still wins
    /// immediately after playing, and two near-identical ones alternate.
    pub fn penalty(&self, clip: usize, passage: usize, now: f32) -> f32 {
        const SCALE: f32 = 0.2;
        self.seen
            .iter()
            .filter(|(c, p, _)| *c == clip && *p == passage)
            .map(|(_, _, at)| {
                let age = (now - at).max(0.0);
                SCALE * (1.0 - age / COOL_SECONDS).max(0.0)
            })
            .fold(0.0f32, f32::max)
    }

    pub fn note(&mut self, cue: &Cue) {
        self.seen.retain(|(c, p, _)| !(*c == cue.clip && *p == cue.passage));
        self.seen.push((cue.clip, cue.passage, cue.since));
        // A night is a night; anything older than the cooldown has no effect
        // and is only taking up room.
        let cutoff = cue.since - COOL_SECONDS;
        self.seen.retain(|(_, _, at)| *at >= cutoff);
    }

    pub fn clear(&mut self) {
        self.seen.clear();
    }
}

// ---------------------------------------------------------------------------
// training
// ---------------------------------------------------------------------------

/// One thing an operator said: at this point in this song, that footage,
/// played like this.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pick {
    pub song: String,
    /// The digest tick it was authored at, on the 8 Hz grid.
    pub tick: usize,
    /// The clip's id inside the library this pick belongs to.
    pub clip: String,
    pub passage: usize,
    /// How the operator played it. **Not a training input** — [`Sample`] does
    /// not carry it, and that is deliberate: the Curator learns which footage a
    /// piece of music wants, and how fast somebody chose to run it is a
    /// separate decision that would otherwise leak into the image descriptor as
    /// noise. It is here because the lane has to save it somewhere, and the
    /// pick is where the rest of that authoring already lives.
    #[serde(default)]
    pub play: Play,
}

/// One authored decision, resolved: the music that was playing, and the shot
/// somebody put on over it.
///
/// This is a [`Pick`] with the song and the folder already looked up, which is
/// the form the live path can use — it holds no path, opens no file and is
/// asked a question a hundred times a minute.
///
/// It keeps both `clip` and `clip_id` because the index is what the renderer
/// needs and the id is what proves the index still means the same shot. See
/// [`Curator::choose_with`].
#[derive(Clone, Debug, PartialEq)]
pub struct Memory {
    pub song: String,
    pub tick: usize,
    /// The twelve features of the twelve seconds *ending* at `tick` — the same
    /// window [`Sample`] is built from, because a pick has to mean the same
    /// thing to the model that learns from it and to the booth that replays it.
    pub music: [f32; FEATURES],
    /// Index into `Library::clips`, resolved against the library given to
    /// [`recall`].
    pub clip: usize,
    pub clip_id: String,
    pub passage: usize,
    pub play: Play,
}

/// One resolved training example. The library is gone by this point, which is
/// the property the whole design rests on.
#[derive(Clone, Debug)]
pub struct Sample {
    pub song: String,
    pub music: [f32; FEATURES],
    pub image: [f32; COLUMNS],
}

/// Where a library's picks live.
pub fn picks_path(data: &Path, library: &str, song: &str) -> PathBuf {
    Library::dir(data, library).join("picks").join(format!("{song}.json"))
}

pub fn load_picks(data: &Path, library: &str) -> Vec<Pick> {
    let dir = Library::dir(data, library).join("picks");
    let mut out = Vec::new();
    for entry in metal_vfs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.path().extension().is_some_and(|x| x == "json") {
            if let Ok(text) = metal_vfs::read_to_string(entry.path()) {
                if let Ok(picks) = serde_json::from_str::<Vec<Pick>>(&text) {
                    out.extend(picks);
                }
            }
        }
    }
    out
}

pub fn save_picks(data: &Path, library: &str, song: &str, picks: &[Pick]) -> Result<(), String> {
    let path = picks_path(data, library, song);
    if let Some(parent) = path.parent() {
        metal_vfs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(picks).map_err(|e| e.to_string())?;
    metal_vfs::write(path, text).map_err(|e| e.to_string())
}

/// Resolves picks against the library they were authored in and the song
/// matrices they name.
///
/// `matrix_for` hands back a song's `(T, 29)` digest — the caller owns where
/// that comes from, because `fw-studio` has it in memory and `metal-dash` has it
/// on disk. A pick naming a song that will not load, or a clip that is no
/// longer in the folder, is dropped and named in the returned notes: *"I
/// authored that and it is not in the model"* has to have an answer.
///
/// **Both the corpus and the booth come through here**, [`samples`] for the
/// first and [`Curator::choose_with`] for the second, so a pick cannot mean one
/// window to the model that learns it and a different window to the night that
/// plays it.
pub fn recall(
    picks: &[Pick],
    lib: &Library,
    matrix_for: &mut dyn FnMut(&str) -> Option<Vec<[f32; metal_router::features::COLUMNS]>>,
) -> (Vec<Memory>, Vec<String>) {
    let window = (WINDOW_SECONDS * metal_router::features::RATE) as usize;
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let mut cache: Vec<(String, Option<Vec<[f32; metal_router::features::COLUMNS]>>)> = Vec::new();

    for p in picks {
        if !cache.iter().any(|(s, _)| *s == p.song) {
            cache.push((p.song.clone(), matrix_for(&p.song)));
        }
        let Some((_, Some(matrix))) = cache.iter().find(|(s, _)| *s == p.song) else {
            notes.push(format!("{}: no digest for this song", p.song));
            continue;
        };
        let Some((ci, clip)) = lib.by_id(&p.clip) else {
            notes.push(format!("{}: {} is not in the library any more", p.song, p.clip));
            continue;
        };
        if p.passage >= clip.passages.len() {
            notes.push(format!("{}: {} has no passage {}", p.song, p.clip, p.passage));
            continue;
        }
        // The window *ending* at the pick, because that is what the operator
        // had heard when they chose. A window centred on it would train the
        // model on music that had not played yet, and the live Curator only
        // ever has the past.
        let end = p.tick.min(matrix.len());
        let start = end.saturating_sub(window);
        if end - start < 4 {
            notes.push(format!("{}: picked too early in the song to describe", p.song));
            continue;
        }
        out.push(Memory {
            song: p.song.clone(),
            tick: p.tick,
            music: metal_router::features::describe(&matrix[start..end]),
            clip: ci,
            clip_id: clip.id.clone(),
            passage: p.passage,
            play: p.play,
        });
    }
    (out, notes)
}

/// Turns picks into training samples.
///
/// The resolving is [`recall`]'s, and what this adds is the one thing training
/// wants and the booth does not: the library is dropped here, and a `Sample`
/// carries the *descriptor* of the scene rather than which scene it was. That
/// is the property the whole design rests on — see the module header.
pub fn samples(
    picks: &[Pick],
    lib: &Library,
    matrix_for: &mut dyn FnMut(&str) -> Option<Vec<[f32; metal_router::features::COLUMNS]>>,
) -> (Vec<Sample>, Vec<String>) {
    let (memories, notes) = recall(picks, lib, matrix_for);
    let out = memories
        .into_iter()
        .map(|m| Sample {
            song: m.song,
            music: m.music,
            image: lib.clips[m.clip].passages[m.passage].d,
        })
        .collect();
    (out, notes)
}

/// Ridge regression, one solve for all twelve outputs.
///
/// # Why ridge and why linear
///
/// The same argument `the router notes` §4 makes, and it applies harder here.
/// Convex, so it converges or the data is degenerate. Readable, so *"why did it
/// pick that shot"* is answered by reading twelve numbers with the features'
/// own names on them. And it extrapolates gently, which matters because the
/// input is deliberately allowed to be a genre nobody authored for.
///
/// The **penalty** is doing real work rather than being a formality:
/// `read`'s own header admits that `temperatura`, `tinte` and the three channel
/// means are a rotation of each other. Correlated inputs are exactly what an
/// unpenalised least squares handles worst — it puts enormous opposing weights
/// on them — and exactly what ridge is for.
pub const LAMBDA: f32 = 0.05;

/// Fits a coupling. Fewer than [`MIN_SONGS`] songs and it reports no error
/// figure, because there is no honest one to give.
pub const MIN_SONGS: usize = 3;

pub fn train(samples: &[Sample]) -> Coupling {
    if samples.is_empty() {
        return Coupling::authored();
    }
    let mut songs: Vec<&str> = samples.iter().map(|s| s.song.as_str()).collect();
    songs.sort_unstable();
    songs.dedup();

    let w = fit(samples);
    let error = if songs.len() >= MIN_SONGS {
        Some(hold_out_by_song(samples, &songs))
    } else {
        None
    };

    Coupling { w, songs: songs.len(), picks: samples.len(), error }
}

/// Solves `(X'X + λI) W = X'Y` once, with the bias column appended to X.
fn fit(samples: &[Sample]) -> Vec<Vec<f32>> {
    const N: usize = FEATURES + 1;
    let mut xtx = vec![vec![0.0f64; N]; N];
    let mut xty = vec![vec![0.0f64; COLUMNS]; N];

    for s in samples {
        let mut x = [0.0f64; N];
        for k in 0..FEATURES {
            x[k] = s.music[k] as f64;
        }
        x[FEATURES] = 1.0;
        for i in 0..N {
            for j in 0..N {
                xtx[i][j] += x[i] * x[j];
            }
            for c in 0..COLUMNS {
                xty[i][c] += x[i] * s.image[c] as f64;
            }
        }
    }
    // The bias is not penalised. Shrinking it toward zero would pull every
    // prediction toward black rather than toward the mean, which is the one
    // thing ridge must not do to a column that means "how bright is this".
    for i in 0..FEATURES {
        xtx[i][i] += LAMBDA as f64 * samples.len() as f64;
    }

    match solve(xtx, xty) {
        Some(beta) => (0..COLUMNS)
            .map(|c| (0..N).map(|i| beta[i][c] as f32).collect())
            .collect(),
        // Degenerate — every pick identical, or one pick. Falling back to the
        // authored taste is better than a matrix of NaNs that renders as a
        // black frame with nothing in any log.
        None => Coupling::authored().w,
    }
}

/// Gauss–Jordan with partial pivoting, multiple right-hand sides.
///
/// Thirteen by thirteen. A decomposition would be the same answer with more
/// code, and this is the whole numerical content of the crate.
pub(crate) fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<Vec<f64>>) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    for col in 0..n {
        let (mut pivot, mut best) = (col, a[col][col].abs());
        for r in col + 1..n {
            if a[r][col].abs() > best {
                best = a[r][col].abs();
                pivot = r;
            }
        }
        if best < 1e-9 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let d = a[col][col];
        for j in 0..n {
            a[col][j] /= d;
        }
        for j in 0..b[col].len() {
            b[col][j] /= d;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col];
            if f == 0.0 {
                continue;
            }
            for j in 0..n {
                a[r][j] -= f * a[col][j];
            }
            for j in 0..b[col].len() {
                b[r][j] -= f * b[col][j];
            }
        }
    }
    Some(b)
}

/// Leave one song out, and report the mean absolute error beside the baseline
/// that always answers the training mean.
///
/// **Held out by song, never by pick.** `docs/CORPUS.md` recorded this trap for
/// the dance model and `the router notes` recorded it again for the Router: picks
/// out of one song are not independent examples, they are the same four minutes
/// described a dozen times. An error figure computed over shuffled picks would
/// look excellent and mean nothing.
fn hold_out_by_song(samples: &[Sample], songs: &[&str]) -> (f32, f32) {
    let mut ours = 0.0f64;
    let mut base = 0.0f64;
    let mut n = 0usize;

    for held in songs {
        let train_set: Vec<Sample> =
            samples.iter().filter(|s| s.song != *held).cloned().collect();
        let test: Vec<&Sample> = samples.iter().filter(|s| s.song == *held).collect();
        if train_set.is_empty() || test.is_empty() {
            continue;
        }
        let w = fit(&train_set);
        let coupling = Coupling { w, songs: 0, picks: 0, error: None };

        let mut mean = [0.0f32; COLUMNS];
        for s in &train_set {
            for c in 0..COLUMNS {
                mean[c] += s.image[c];
            }
        }
        for v in mean.iter_mut() {
            *v /= train_set.len() as f32;
        }

        for s in test {
            let got = coupling.apply(&s.music);
            for c in 0..COLUMNS {
                ours += (got[c] - s.image[c]).abs() as f64;
                base += (mean[c] - s.image[c]).abs() as f64;
                n += 1;
            }
        }
    }
    if n == 0 {
        return (0.0, 0.0);
    }
    ((ours / n as f64) as f32, (base / n as f64) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Clip, Passage, Stamp};

    fn clip(id: &str, passages: &[[f32; COLUMNS]]) -> Clip {
        Clip {
            id: id.into(),
            file: format!("{id}.mp4"),
            seconds: 10.0 * passages.len() as f32,
            stamp: Stamp { bytes: 0, modified: 0 },
            rows: vec![[0.0; COLUMNS]; 120 * passages.len()],
            passages: passages
                .iter()
                .enumerate()
                .map(|(i, d)| Passage {
                    from: i * 120,
                    to: (i + 1) * 120,
                    d: *d,
                    seconds: 10.0,
                    peak: d[11],
                    variety: 0.0,
                })
                .collect(),
        }
    }

    fn library(clips: Vec<Clip>) -> Library {
        Library {
            name: "t".into(),
            source: "/tmp/t".into(),
            rate: 12.0,
            width: 640,
            clips,
            root: std::path::PathBuf::from("/tmp/t"),
        }
    }

    /// A still dark shot and a moving bright one, so a choice between them is
    /// unambiguous and readable.
    fn two() -> Library {
        let mut calm = [0.2f32; COLUMNS];
        calm[MOTION] = 0.05;
        calm[EXPOSURE] = 0.15;
        let mut wild = [0.7f32; COLUMNS];
        wild[MOTION] = 0.9;
        wild[EXPOSURE] = 0.8;
        library(vec![clip("calm", &[calm]), clip("wild", &[wild])])
    }

    fn loud() -> [f32; FEATURES] {
        let mut m = [0.5f32; FEATURES];
        m[ENERGY] = 0.95;
        m[DENSITY] = 0.9;
        m[PULSE] = 0.95;
        m[HIT] = 0.8;
        m
    }

    fn quiet() -> [f32; FEATURES] {
        let mut m = [0.2f32; FEATURES];
        m[ENERGY] = 0.08;
        m[DENSITY] = 0.05;
        m[PULSE] = 0.1;
        m[HIT] = 0.05;
        m
    }

    #[test]
    fn the_default_taste_picks_the_moving_shot_for_the_loud_passage() {
        // The one thing the untrained Curator has to get right, because it is
        // what an operator sees in the first thirty seconds.
        let c = Curator::default();
        let lib = two();
        assert!(!c.trained(), "the default must not claim to be trained");
        assert_eq!(c.choose(&lib, &loud(), &Recent::default(), 0.0).unwrap().clip_id, "wild");
        assert_eq!(c.choose(&lib, &quiet(), &Recent::default(), 0.0).unwrap().clip_id, "calm");
    }

    #[test]
    fn the_default_coupling_never_asks_for_an_image_outside_0_1() {
        // It is a hand-written table of weights that sum, so it is the one
        // place in this crate where a column can leave the range by arithmetic
        // rather than by a bug.
        let c = Curator::default();
        for step in 0..=10 {
            for other in [0.0f32, 0.5, 1.0] {
                let mut m = [other; FEATURES];
                for k in 0..FEATURES {
                    m[k] = step as f32 / 10.0;
                }
                for (i, v) in c.want(&m).iter().enumerate() {
                    assert!((0.0..=1.0).contains(v), "{} = {v}", NAMES[i]);
                }
            }
        }
    }

    #[test]
    fn an_empty_library_is_no_cue_rather_than_a_panic() {
        assert!(Curator::default()
            .choose(&library(vec![]), &loud(), &Recent::default(), 0.0)
            .is_none());
    }

    #[test]
    fn two_comparable_shots_alternate_rather_than_one_playing_all_night() {
        // The common case in a real folder: rushes of the same shoot, all
        // plausible under the same music, differing by a little. Without the
        // cooldown the Curator finds one right answer and is right about it for
        // an hour.
        let mut a = [0.7f32; COLUMNS];
        a[MOTION] = 0.85;
        let mut b = a;
        b[MOTION] = 0.80;
        b[SATURATION] = 0.65;
        let lib = library(vec![clip("take-a", &[a]), clip("take-b", &[b])]);

        let c = Curator::default();
        let mut recent = Recent::default();
        let mut seen: Vec<String> = Vec::new();
        for k in 0..4 {
            let cue = c.choose(&lib, &loud(), &recent, k as f32 * HOLD_SECONDS).unwrap();
            recent.note(&cue);
            seen.push(cue.clip_id.clone());
        }
        assert!(seen.contains(&"take-a".to_string()), "{seen:?}");
        assert!(seen.contains(&"take-b".to_string()), "it sat on one clip: {seen:?}");
    }

    #[test]
    fn a_clearly_better_shot_still_wins_immediately_after_playing() {
        // The other half, and the reason the penalty is a nudge rather than a
        // filter. A still dark shot under a drop because the right one played
        // twenty seconds ago is worse than repeating the right one.
        let c = Curator::default();
        let lib = two();
        let mut recent = Recent::default();
        let first = c.choose(&lib, &loud(), &recent, 0.0).unwrap();
        assert_eq!(first.clip_id, "wild");
        recent.note(&first);
        assert_eq!(c.choose(&lib, &loud(), &recent, 1.0).unwrap().clip_id, "wild");
    }

    #[test]
    fn the_cooldown_expires() {
        let c = Curator::default();
        let lib = two();
        let mut recent = Recent::default();
        let first = c.choose(&lib, &loud(), &recent, 0.0).unwrap();
        recent.note(&first);
        assert!(recent.penalty(first.clip, first.passage, 0.0) > 0.0);
        assert_eq!(recent.penalty(first.clip, first.passage, COOL_SECONDS + 1.0), 0.0);
    }

    #[test]
    fn a_library_of_one_passage_still_answers_however_recently_it_played() {
        // The penalty is additive, not a filter. A folder with one clip in it
        // is a real case and it must not go black.
        let mut d = [0.5f32; COLUMNS];
        d[MOTION] = 0.5;
        let lib = library(vec![clip("only", &[d])]);
        let c = Curator::default();
        let mut recent = Recent::default();
        let first = c.choose(&lib, &loud(), &recent, 0.0).unwrap();
        recent.note(&first);
        assert!(c.choose(&lib, &loud(), &recent, 1.0).is_some());
    }

    #[test]
    fn one_candidate_is_certain_and_two_similar_ones_are_not() {
        let mut d = [0.5f32; COLUMNS];
        d[MOTION] = 0.5;
        let c = Curator::default();
        let one = library(vec![clip("only", &[d])]);
        assert_eq!(c.choose(&one, &loud(), &Recent::default(), 0.0).unwrap().confidence, 1.0);

        let mut nearly = d;
        nearly[EXPOSURE] += 0.01;
        let similar = library(vec![clip("a", &[d]), clip("b", &[nearly])]);
        let got = c.choose(&similar, &loud(), &Recent::default(), 0.0).unwrap();
        assert!(got.confidence < 0.2, "two near-identical shots reported {}", got.confidence);
    }

    #[test]
    fn it_says_which_column_decided() {
        let c = Curator::default();
        let got = c.choose(&two(), &loud(), &Recent::default(), 0.0).unwrap();
        let (name, _) = got.because.expect("a reason");
        assert!(
            name == "motion" || name == "exposure" || name == "contrast",
            "the reason was {name}"
        );
    }

    #[test]
    fn a_cue_loops_inside_its_own_passage_and_never_leaves_it() {
        // The property that makes a four-second insert usable under a
        // ninety-second section, and the one that stops a cue drifting into
        // unrelated footage.
        let cue = Cue {
            clip: 0,
            clip_id: "c".into(),
            passage: 0,
            from: 10.0,
            span: 4.0,
            since: 100.0,
            confidence: 1.0,
            because: None,
            play: Play::default(),
            authored: None,
        };
        for step in 0..400 {
            let t = 100.0 + step as f32 * 0.25;
            let at = cue.position(t, 1.0);
            assert!((10.0..14.0).contains(&at), "left the passage at {t}: {at}");
        }
        assert_eq!(cue.position(100.0, 1.0), 10.0);
        assert_eq!(cue.position(105.0, 1.0), 11.0);
        // Before its own start, and at speed zero, it is at the head.
        assert_eq!(cue.position(0.0, 1.0), 10.0);
        assert_eq!(cue.position(200.0, 0.0), 10.0);
    }

    #[test]
    fn a_clock_that_is_not_a_number_is_the_head_of_the_passage() {
        // Not defensiveness: `inf % span` is NaN, a NaN playhead casts to rung
        // 0, and the symptom is a transmission frozen on the first frame of a
        // shot with every float in range and nothing logged.
        let cue = Cue {
            clip: 0,
            clip_id: "c".into(),
            passage: 0,
            from: 5.0,
            span: 3.0,
            since: 0.0,
            confidence: 1.0,
            because: None,
            play: Play::default(),
            authored: None,
        };
        for t in [f32::NAN, f32::INFINITY, -f32::INFINITY, 1e38, -1e38] {
            for speed in [0.0f32, 1.0, f32::INFINITY] {
                let at = cue.position(t, speed);
                assert!(at.is_finite() && (5.0..8.0).contains(&at), "t={t} speed={speed}: {at}");
            }
        }
    }

    #[test]
    fn a_cue_is_a_function_of_the_clock_and_nothing_else() {
        let cue = Cue {
            clip: 0,
            clip_id: "c".into(),
            passage: 0,
            from: 0.0,
            span: 7.0,
            since: 3.0,
            confidence: 1.0,
            because: None,
            play: Play::default(),
            authored: None,
        };
        // Asked twice, the same answer — the property the render gates rest on.
        assert_eq!(cue.position(41.5, 1.0), cue.position(41.5, 1.0));
        assert!(!cue.ripe(3.0 + HOLD_SECONDS - 0.1));
        assert!(cue.ripe(3.0 + HOLD_SECONDS));
    }

    // ---- how a scene is played ------------------------------------------

    fn played(play: Play) -> Cue {
        Cue {
            clip: 0,
            clip_id: "c".into(),
            passage: 0,
            from: 10.0,
            span: 8.0,
            since: 0.0,
            confidence: 1.0,
            because: None,
            play,
            authored: None,
        }
    }

    #[test]
    fn a_played_window_never_leaves_the_passage_whatever_it_is_set_to() {
        // The one property everything else here rests on: a lane can author any
        // combination, including nonsense, and the playhead stays inside the
        // scene. Outside it is another shot's footage with nothing reporting it.
        for from in [0.0f32, 0.3, 0.9, 1.0, -0.5, 2.0] {
            for to in [0.0f32, 0.25, 0.7, 1.0, -1.0, 5.0] {
                for reverse in [false, true] {
                    for speed in [0.0f32, 0.25, 1.0, 4.0] {
                        let cue = played(Play { speed, reverse, from, to });
                        for k in 0..400 {
                            let at = cue.position(k as f32 * 0.13, 1.0);
                            assert!(
                                (10.0..=18.0).contains(&at),
                                "{from}/{to}/{reverse}/{speed} gave {at}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_window_of_no_width_holds_one_frame_forever() {
        // "An independent frame" — and it must be *exactly* still, because a
        // held frame that creeps is slow motion, which is a different control.
        let cue = played(Play { from: 0.5, to: 0.5, ..Play::default() });
        assert!(cue.play.held());
        let at = cue.position(0.0, 1.0);
        assert_eq!(at, 14.0);
        for k in 0..200 {
            assert_eq!(cue.position(k as f32 * 0.37, 1.0), at, "the held frame moved");
        }
    }

    #[test]
    fn a_loop_stays_in_its_part_of_the_scene() {
        // The middle quarter of an eight-second passage that starts at ten.
        let cue = played(Play { from: 0.5, to: 0.75, ..Play::default() });
        let mut lowest = f32::MAX;
        let mut highest = f32::MIN;
        for k in 0..500 {
            let at = cue.position(k as f32 * 0.031, 1.0);
            lowest = lowest.min(at);
            highest = highest.max(at);
        }
        assert!(lowest >= 14.0, "{lowest}");
        assert!(highest < 16.0, "{highest}");
        // And it actually uses the window rather than sitting at one end.
        assert!(highest - lowest > 1.5, "the loop covered only {}s", highest - lowest);
    }

    #[test]
    fn reverse_runs_backwards_and_never_touches_the_next_passage() {
        // `window - 0` is `window`, which is the first rung of the *next*
        // scene. One frame of the wrong shot at every loop point is invisible
        // in a still and obvious on a wall.
        let cue = played(Play { reverse: true, ..Play::default() });
        for k in 0..500 {
            let at = cue.position(k as f32 * 0.017, 1.0);
            assert!((10.0..18.0).contains(&at), "{at}");
        }
        // Time moving forward walks the footage backwards, inside one lap.
        let a = cue.position(1.0, 1.0);
        let b = cue.position(2.0, 1.0);
        assert!(b < a, "reverse went forwards: {a} then {b}");
    }

    #[test]
    fn slow_motion_is_the_same_footage_taking_longer() {
        // Half speed at twice the time is the same frame — which is what makes
        // this slow motion rather than a different edit.
        let fast = played(Play::default());
        let slow = played(Play { speed: 0.5, ..Play::default() });
        for k in 1..40 {
            let t = k as f32 * 0.1;
            let d = (slow.position(t * 2.0, 1.0) - fast.position(t, 1.0)).abs();
            assert!(d < 1e-4, "at {t}: {d}");
        }
    }

    // ---- training -------------------------------------------------------

    /// Loud music was paired with moving footage, quiet with still, across
    /// several songs.
    fn taught() -> Vec<Sample> {
        let mut out = Vec::new();
        for s in 0..4 {
            for k in 0..6 {
                let j = k as f32 * 0.01;
                let mut m = loud();
                m[ENERGY] = 0.9 + j;
                let mut img = [0.5f32; COLUMNS];
                img[MOTION] = 0.9;
                out.push(Sample { song: format!("s{s}"), music: m, image: img });

                let mut m = quiet();
                m[ENERGY] = 0.1 + j;
                let mut img = [0.5f32; COLUMNS];
                img[MOTION] = 0.05;
                out.push(Sample { song: format!("s{s}"), music: m, image: img });
            }
        }
        out
    }

    #[test]
    fn training_learns_the_pairing_it_was_shown() {
        let c = Curator { coupling: train(&taught()), fitted: true };
        assert!(c.coupling.trained());
        assert!(c.want(&loud())[MOTION] > 0.7);
        assert!(c.want(&quiet())[MOTION] < 0.3);
    }

    #[test]
    fn training_beats_the_baseline_and_reports_both() {
        let c = train(&taught());
        let (ours, baseline) = c.error.expect("four songs is enough to report");
        assert!(ours < baseline, "held out at {ours} against a baseline of {baseline}");
        assert_eq!(c.songs, 4);
    }

    #[test]
    fn under_three_songs_it_reports_no_error_at_all() {
        // One song's picks are the same four minutes described a dozen times.
        // A number here would look excellent and mean nothing.
        let one: Vec<Sample> = taught().into_iter().filter(|s| s.song == "s0").collect();
        let c = train(&one);
        assert!(c.trained(), "it still fits");
        assert!(c.error.is_none(), "but it must not claim an accuracy");
    }

    #[test]
    fn a_degenerate_corpus_falls_back_to_the_authored_taste_rather_than_to_nans() {
        // Every pick identical: the normal matrix is singular. The failure to
        // avoid is a coupling of NaNs, which renders as a frozen black frame
        // with nothing in any log.
        let same = vec![
            Sample { song: "a".into(), music: [0.5; FEATURES], image: [0.5; COLUMNS] };
            8
        ];
        let c = train(&same);
        assert!(c.trained());
        for row in &c.w {
            assert!(row.iter().all(|v| v.is_finite()), "{row:?}");
        }
    }

    #[test]
    fn training_on_nothing_is_the_authored_taste() {
        let c = train(&[]);
        assert!(c.trained(), "it is a usable coupling");
        let curator = Curator { coupling: c, fitted: false };
        assert!(!curator.trained(), "and it does not claim to be fitted");
    }

    #[test]
    fn a_curator_survives_being_written_and_read_back() {
        let dir = std::env::temp_dir().join("metal-reel-curator");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let c = Curator { coupling: train(&taught()), fitted: true };
        c.save(&dir).expect("saves");
        let back = Curator::load(&dir);
        assert!(back.trained());
        assert_eq!(back.want(&loud()), c.want(&loud()));
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    #[test]
    fn loading_from_a_directory_with_nothing_in_it_is_the_default_taste() {
        let dir = std::env::temp_dir().join("metal-reel-curator-empty");
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let c = Curator::load(&dir);
        assert!(!c.trained());
        assert_eq!(c.want(&loud()), Curator::default().want(&loud()));
    }

    #[test]
    fn a_pick_is_described_by_the_music_before_it_not_after() {
        // The live Curator only ever has the past. Training on a window centred
        // on the pick would fit it to music that had not played yet, and the
        // symptom is a model that works in the studio and not in the booth.
        let lib = two();
        let mut matrix: Vec<[f32; metal_router::features::COLUMNS]> = vec![[0.1; metal_router::features::COLUMNS]; 200];
        for row in matrix.iter_mut().skip(100) {
            *row = [0.9; metal_router::features::COLUMNS];
        }
        let picks = vec![Pick { song: "s".into(), tick: 100, clip: "wild".into(), passage: 0, play: Play::default() }];
        let (got, notes) = samples(&picks, &lib, &mut |_| Some(matrix.clone()));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(got.len(), 1);
        // Everything before tick 100 was quiet, so the description must be.
        assert!(got[0].music[ENERGY] < 0.3, "{}", got[0].music[ENERGY]);
    }

    #[test]
    fn a_pick_naming_a_clip_that_is_gone_is_named_rather_than_dropped_in_silence() {
        let lib = two();
        let picks = vec![
            Pick { song: "s".into(), tick: 200, clip: "deleted".into(), passage: 0, play: Play::default() },
            Pick { song: "s".into(), tick: 200, clip: "wild".into(), passage: 9, play: Play::default() },
        ];
        let (got, notes) = samples(&picks, &lib, &mut |_| Some(vec![[0.5; metal_router::features::COLUMNS]; 400]));
        assert!(got.is_empty());
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("deleted"));
        assert!(notes[1].contains("passage"));
    }

    // -----------------------------------------------------------------------
    // authored picks on the live path
    // -----------------------------------------------------------------------

    /// A memory of `music`, naming a clip by id, resolved against `lib` the way
    /// `recall` would have.
    fn memory(lib: &Library, music: [f32; FEATURES], clip: &str, play: Play) -> Memory {
        let (ci, c) = lib.by_id(clip).expect("a clip that is in the library");
        Memory {
            song: "s".into(),
            tick: 400,
            music,
            clip: ci,
            clip_id: c.id.clone(),
            passage: 0,
            play,
        }
    }

    #[test]
    fn an_authored_pick_beats_the_model_over_the_music_it_was_authored_over() {
        // The whole point. The default taste answers `wild` for loud music —
        // the test above pins that — so a pick of `calm` over loud music is a
        // decision the model would never make, and it has to win anyway.
        let c = Curator::default();
        let lib = two();
        let slow = Play { speed: 0.25, reverse: true, from: 0.2, to: 0.6 };
        let mem = vec![memory(&lib, loud(), "calm", slow)];

        let scored = c.choose(&lib, &loud(), &Recent::default(), 0.0).unwrap();
        assert_eq!(scored.clip_id, "wild", "the model's own answer, unchanged");

        let cue = c.choose_with(&lib, &loud(), &Recent::default(), 0.0, &mem).unwrap();
        assert_eq!(cue.clip_id, "calm", "the operator said calm and was overruled");
        // And it plays it the way they said, which is the sentence `choose`'s
        // own comment has been promising since it was written.
        assert_eq!(cue.play, slow);
        let a = cue.authored.expect("a recalled cue has to say it is one");
        assert_eq!((a.song.as_str(), a.tick), ("s", 400));
        assert!(a.near < RECALL_RADIUS);
    }

    #[test]
    fn music_nobody_authored_over_gets_exactly_the_cue_it_got_before() {
        // The migration rule, as an assertion: with a memory that does not
        // apply, `choose_with` must be `choose` — not nearly, not usually.
        let c = Curator::default();
        let lib = two();
        let mem = vec![memory(&lib, quiet(), "calm", Play::default())];
        for music in [loud(), [0.5; FEATURES]] {
            let want = c.choose(&lib, &music, &Recent::default(), 7.0);
            let got = c.choose_with(&lib, &music, &Recent::default(), 7.0, &mem);
            assert_eq!(got, want, "an unrelated pick changed the model's answer");
            assert!(got.unwrap().authored.is_none());
        }
        // And with no picks at all, which is every library on disk today.
        assert_eq!(
            c.choose_with(&lib, &loud(), &Recent::default(), 7.0, &[]),
            c.choose(&lib, &loud(), &Recent::default(), 7.0),
        );
    }

    #[test]
    fn a_pick_stops_applying_before_the_music_is_merely_similar() {
        // The radius is what keeps one authored moment from taking over the
        // night. Walked out from the authored music one step at a time: inside
        // it recalls, outside it is the model's again, and nothing in between
        // is left to chance.
        let c = Curator::default();
        let lib = two();
        let mem = vec![memory(&lib, loud(), "calm", Play::default())];
        for step in 0..12 {
            let mut music = loud();
            // One feature moved, so the distance is `d / sqrt(12)`.
            music[BRIGHTNESS] += step as f32 * 0.02;
            let d = music_distance(&music, &loud());
            let cue = c.choose_with(&lib, &music, &Recent::default(), 0.0, &mem).unwrap();
            if d <= RECALL_RADIUS {
                assert_eq!(cue.clip_id, "calm", "d={d} is inside the radius");
            } else {
                assert_eq!(cue.clip_id, "wild", "d={d} is outside it and is the model's");
                assert!(cue.authored.is_none());
            }
        }
    }

    #[test]
    fn a_memory_built_against_another_folder_is_refused_rather_than_followed() {
        // Every index in range and every one the wrong shot: this project's
        // named worst failure (§3.3, §11.7). A re-index that reorders the
        // folder produces exactly this, and the id is what catches it.
        let c = Curator::default();
        let lib = two();
        let mut stale = memory(&lib, loud(), "calm", Play::default());
        stale.clip_id = "some-other-folders-clip".into();
        let cue = c.choose_with(&lib, &loud(), &Recent::default(), 0.0, &[stale]).unwrap();
        assert_eq!(cue.clip_id, "wild", "a mismatched id must not play clip 0");
        assert!(cue.authored.is_none(), "and must not claim somebody authored it");

        // Out of the folder's range entirely, which a shrunken library gives.
        let mut gone = memory(&lib, loud(), "calm", Play::default());
        gone.clip = 99;
        assert!(c.choose_with(&lib, &loud(), &Recent::default(), 0.0, &[gone]).is_some());

        // And a passage that no longer exists in a clip that still does.
        let mut recut = memory(&lib, loud(), "calm", Play::default());
        recut.passage = 40;
        let cue = c.choose_with(&lib, &loud(), &Recent::default(), 0.0, &[recut]).unwrap();
        assert_eq!(cue.clip_id, "wild");
    }

    #[test]
    fn the_nearest_authored_moment_is_the_one_that_plays() {
        let c = Curator::default();
        let lib = two();
        let mut near = loud();
        near[BRIGHTNESS] += 0.01;
        let mem = vec![
            memory(&lib, near, "wild", Play::default()),
            memory(&lib, loud(), "calm", Play::default()),
        ];
        // `calm` is the exact moment; `wild` is a hair off. Order in the table
        // must not decide it.
        assert_eq!(
            c.choose_with(&lib, &loud(), &Recent::default(), 0.0, &mem).unwrap().clip_id,
            "calm",
        );
    }

    #[test]
    fn recall_and_samples_resolve_a_pick_to_the_same_moment() {
        // They are one function with two ends. If they ever drift, the model is
        // trained on one window and the booth replays another, and the symptom
        // is a lane that works in the studio and not in the room.
        let lib = two();
        let mut matrix: Vec<[f32; metal_router::features::COLUMNS]> = vec![[0.1; metal_router::features::COLUMNS]; 200];
        for row in matrix.iter_mut().skip(100) {
            *row = [0.9; metal_router::features::COLUMNS];
        }
        let picks = vec![Pick {
            song: "s".into(),
            tick: 100,
            clip: "wild".into(),
            passage: 0,
            play: Play { speed: 2.0, reverse: false, from: 0.0, to: 0.5 },
        }];
        let (mem, notes) = recall(&picks, &lib, &mut |_| Some(matrix.clone()));
        let (smp, snotes) = samples(&picks, &lib, &mut |_| Some(matrix.clone()));
        assert!(notes.is_empty() && snotes.is_empty());
        assert_eq!(mem.len(), 1);
        assert_eq!(mem[0].music, smp[0].music, "the same twelve seconds, or nothing works");
        assert_eq!(mem[0].clip_id, "wild");
        assert_eq!(mem[0].clip, lib.by_id("wild").unwrap().0);
        assert_eq!(mem[0].play.speed, 2.0, "the pick's own playback survives resolving");
    }

    #[test]
    fn a_pick_that_cannot_be_resolved_never_becomes_a_memory() {
        // The stale ones are dropped *here*, where there is somebody to tell,
        // rather than reaching the live path as an index into a folder that has
        // moved on.
        let lib = two();
        let picks = vec![
            Pick { song: "s".into(), tick: 200, clip: "deleted".into(), passage: 0, play: Play::default() },
            Pick { song: "s".into(), tick: 200, clip: "wild".into(), passage: 9, play: Play::default() },
            Pick { song: "nodigest".into(), tick: 200, clip: "wild".into(), passage: 0, play: Play::default() },
        ];
        let (mem, notes) = recall(&picks, &lib, &mut |s| {
            (s != "nodigest").then(|| vec![[0.5; metal_router::features::COLUMNS]; 400])
        });
        assert!(mem.is_empty(), "not one of those can be played");
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(notes[2].contains("no digest"), "{}", notes[2]);
    }
}
