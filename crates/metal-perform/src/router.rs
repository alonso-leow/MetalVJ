//! The Router — what is on air, when nobody is choosing.
//!
//! > *"Every so often (5 minutes) a system selects 4 animations, considering
//! > the history of which ones have already been used, in what order and in
//! > which groups. Every 16 beats of the song it would switch to another of the
//! > models enabled in the group. Automix would be one more option for the final
//! > app, letting you choose between the existing models, automix, and a new
//! > option: selecting several so it only switches between those."*
//! > — 2026-08-17
//!
//! # Two clocks, not one
//!
//! A **slow** one picks a group of four every five minutes, and a **fast** one
//! rotates within that group every sixteen beats. They are different clocks in
//! kind and not only in rate: the slow one is wall time, because five minutes
//! is five minutes whatever the tempo, and the fast one is *beats*, because
//! sixteen beats is a phrase and a phrase is what a switch should land on.
//!
//! The beat clock is [`digest::Source::beats`], added for this and deliberately
//! not a seventeenth digest column — the argument is written where it lives.
//! When a source cannot follow a tempo the router falls back to wall time at
//! an assumed 120 BPM and says so, because an operator watching switches that
//! miss the music must be able to tell "the router is broken" from "there is no
//! beat clock".
//!
//! # What a group is
//!
//! `docs/NEXT.md` §1 left this open: hand-authored sets, or derived from some
//! property of the models. It is neither. **A group is what the slow clock just
//! picked** — four animations that will play against each other for the next
//! five minutes — and the ask says so itself, since the history it asks to
//! consider is *"in which groups"*, which only means anything if a group is a
//! thing the router made. Nothing has to be authored and nothing has to be
//! taxonomised, and the operator's own selection is a different feature: that
//! is [`Mode::Picked`].
//!
//! # The history, and what it is for
//!
//! Three things, exactly the three the ask names:
//!
//! | asked | kept as | used for |
//! |---|---|---|
//! | *which ones have already been used* | `plays` per model | a model that has played less is favoured |
//! | *in what order* | `last_group` per model | one that played recently is penalised |
//! | *in which groups* | `together` counts per pair | a pairing seen recently is penalised |
//!
//! Scored, not shuffled. A shuffle satisfies the first sentence of the ask and
//! none of the rest of it, and over a four-hour set a shuffle visibly repeats
//! while claiming not to.
//!
//! **In memory, for one run.** A set is a night; the history that matters is
//! the one inside it. Persisting across restarts would mean an app restarted
//! at the venue opens with the tail of last week's history deciding what plays
//! first, which is not what "considering the history" was asking for.

use std::collections::{HashMap, VecDeque};

use metal_router::features::{self, FEATURES};
use metal_router::segment;
use metal_router::take::WINDOW;
use metal_router::{Brain, Decision};

use crate::digest::Frame;

/// How the operator has decided what goes on air.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// One animation, chosen by hand. What the app did before the router
    /// existed, and still the default: a router that switched on by itself
    /// would take the venue's picture away from whoever was watching it.
    Single,
    /// Everything built, in groups of four.
    Automix,
    /// Only what the operator ticked. Under four ticked, the pick *is* the
    /// group and the slow clock has nothing to do; over four, the slow clock
    /// still picks four out of them.
    Picked,
    /// **The trained one.** `metal-router` reads the music itself and names one
    /// animation, held until the music changes — see [`Content`], and
    /// `metal_router`'s crate docs for why this is a different thing from the
    /// three above rather than a better version of them.
    ///
    /// Falls back to [`Mode::Automix`]'s clock and history when there is no
    /// trained model, and the page says which one is deciding.
    Model,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Single => "single",
            Mode::Automix => "automix",
            Mode::Picked => "picked",
            Mode::Model => "model",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "single" => Some(Mode::Single),
            "automix" => Some(Mode::Automix),
            "picked" => Some(Mode::Picked),
            "model" => Some(Mode::Model),
            _ => None,
        }
    }
}

/// How many animations are in a group.
pub const GROUP: usize = 4;

/// Seconds between groups. The ask says five minutes.
pub const REGROUP_SECONDS: f32 = 300.0;

/// Beats between switches inside a group. The ask says sixteen — four bars of
/// four, which is a phrase in nearly everything a DJ plays.
pub const BEATS_PER_SWITCH: f32 = 16.0;

/// The tempo assumed when there is no beat clock. Only reached when the source
/// cannot follow one; the page says when this is what is happening.
const FALLBACK_BPM: f32 = 120.0;

/// What the router has seen. One entry per model plus the pairings.
#[derive(Default)]
struct History {
    /// How many groups each model has been in.
    plays: Vec<u32>,
    /// The index of the last group each model appeared in. `None` is "never".
    last_group: Vec<Option<u32>>,
    /// The last group index in which each unordered pair appeared together.
    together: HashMap<(usize, usize), u32>,
    /// How many groups have been formed. Also the id of the current one.
    groups: u32,
}

impl History {
    fn sized(n: usize) -> History {
        History {
            plays: vec![0; n],
            last_group: vec![None; n],
            together: HashMap::new(),
            groups: 0,
        }
    }

    fn pair(a: usize, b: usize) -> (usize, usize) {
        if a < b {
            (a, b)
        } else {
            (b, a)
        }
    }

    fn record(&mut self, group: &[usize]) {
        let id = self.groups;
        for &m in group {
            self.plays[m] += 1;
            self.last_group[m] = Some(id);
        }
        for (i, &a) in group.iter().enumerate() {
            for &b in &group[i + 1..] {
                self.together.insert(History::pair(a, b), id);
            }
        }
        self.groups += 1;
    }
}

// ---------------------------------------------------------------------------
// the trained side
// ---------------------------------------------------------------------------

/// The shortest a decision is allowed to stand, in seconds.
///
/// *"a single output that varies little."* The segmenter has the same floor offline
/// (`metal_router::segment::sections`); this is the live half of it, and without
/// it a busy arrangement would have the model re-deciding every few bars, which
/// is the clock again with extra steps.
pub const HOLD_SECONDS: f32 = 24.0;

/// How much of the recent past the live decision looks at, in ticks. Two
/// windows: the one being described, and the one before it to compare against.
const MEMORY: usize = WINDOW * 2;

/// The live half of `metal-router`: a rolling window of the music, and the one
/// decision in force.
///
/// # Why it keeps a buffer at all
///
/// Offline, the router sees a whole song and cuts it at its novelty peaks. At a
/// venue there is no song — there is a microphone and the last few seconds. So
/// the same question is asked incrementally: describe the window just gone,
/// compare it to the window before the current decision was made, and re-decide
/// when they have diverged enough. `metal_router::segment::changed` is that test
/// and it is shared with the offline side, so the studio and the booth agree
/// about what counts as a change.
pub struct Content {
    pub brain: Brain,
    /// Newest last. Fed at the digest's own 8 Hz rather than at whatever rate
    /// the browser asks for frames — see [`Content::observe`].
    rows: VecDeque<[f32; features::COLUMNS]>,
    /// The digest tick of the newest row.
    last_tick: Option<i64>,
    /// What the music was like when the decision in force was made. Compared
    /// against, rather than against the previous *window*, so a slow drift
    /// eventually triggers a change instead of never doing.
    anchor: Option<[f32; FEATURES]>,
    pub decision: Option<Decision>,
    decided_at: f32,
    /// 0..1. How little of a change it takes to decide again.
    pub sensitivity: f32,
    /// True when the model was asked and had nothing to say, so the group
    /// rotation is what is actually on air. The page has to print this.
    pub falling_back: bool,
}

impl Content {
    fn new(dir: &std::path::Path) -> Content {
        Content {
            brain: Brain::load(dir),
            rows: VecDeque::with_capacity(MEMORY),
            last_tick: None,
            anchor: None,
            decision: None,
            decided_at: f32::NEG_INFINITY,
            sensitivity: 0.5,
            falling_back: false,
        }
    }

    /// Takes whatever the digest has, at the digest's rate.
    ///
    /// Called once per served request, which is *not* a fixed rate — so the
    /// buffer is filled by tick rather than by call, and a burst of requests in
    /// one tick adds one row rather than twenty. The backfill is capped because
    /// a paused app resumed an hour later would otherwise ask the source for
    /// thirty thousand rows it does not have.
    ///
    /// **The backfill means different things to the two sources, and both are
    /// fine.** `Simulated::poll` is a pure function of `time`, so filling a gap
    /// gives a genuinely correct history of what the stand-in was playing. A
    /// real input ignores `time` and hands back the row it has now, so a gap
    /// fills with copies of the present — which makes the window look unusually
    /// still for up to twelve seconds after a resume, and then corrects itself.
    /// Neither is worth a second code path: the alternative is a router that
    /// refuses to decide for twelve seconds every time the clock is touched.
    pub fn observe(&mut self, time: f32, mut poll: impl FnMut(f32) -> Frame) {
        let tick = (time.max(0.0) * features::RATE) as i64;
        let first = match self.last_tick {
            None => tick,
            Some(last) if tick <= last => return,
            Some(last) => (last + 1).max(tick - MEMORY as i64),
        };
        for k in first..=tick {
            let frame = poll(k as f32 / features::RATE);
            if self.rows.len() == MEMORY {
                self.rows.pop_front();
            }
            self.rows.push_back(frame.f);
        }
        self.last_tick = Some(tick);
    }

    /// How much audio it has heard, 0..1 — the page shows it warming up rather
    /// than looking broken for the first twelve seconds.
    pub fn warmth(&self) -> f32 {
        (self.rows.len() as f32 / WINDOW as f32).clamp(0.0, 1.0)
    }

    /// The passage as it stands, or `None` before there is enough of it.
    ///
    /// Public because the **Curator** asks the same question of the same
    /// window: which footage suits this passage is a different decision from
    /// which animation suits it, but it is a decision about the same twelve
    /// seconds, and two windows that drifted apart would let the two models
    /// disagree about what is playing right now.
    pub fn passage(&self) -> Option<[f32; FEATURES]> {
        self.now()
    }

    fn now(&self) -> Option<[f32; FEATURES]> {
        if self.rows.len() < WINDOW {
            return None;
        }
        let recent: Vec<[f32; features::COLUMNS]> =
            self.rows.iter().rev().take(WINDOW).rev().copied().collect();
        Some(features::describe(&recent))
    }

    /// Re-decides if the music has moved on, and returns what is in force.
    fn decide(&mut self, time: f32, allowed: &[String]) -> Option<&Decision> {
        let now = self.now()?;
        let stale = match (&self.decision, self.anchor) {
            (None, _) => true,
            (Some(_), None) => true,
            (Some(_), Some(then)) => {
                time - self.decided_at >= HOLD_SECONDS
                    && segment::changed(&then, &now, self.sensitivity)
            }
        };
        if stale {
            if let Some(d) = self.brain.decide(&now, allowed) {
                self.decision = Some(d);
                self.anchor = Some(now);
                self.decided_at = time;
            }
        }
        self.decision.as_ref()
    }

    /// Seconds until the decision may change. `None` when it is already free to.
    pub fn holding_for(&self, time: f32) -> Option<f32> {
        if self.decision.is_none() {
            return None;
        }
        let left = HOLD_SECONDS - (time - self.decided_at);
        (left > 0.0).then_some(left)
    }
}

pub struct Router {
    pub mode: Mode,
    /// What `Mode::Single` puts on air, and where the other modes return to
    /// when they are switched off. Always a valid model index.
    pub chosen: usize,
    /// The operator's ticks, for `Mode::Picked`. Indices into the built models.
    pub picked: Vec<usize>,
    /// The four on rotation now, in the order they will play.
    group: Vec<usize>,
    /// When the current group was formed, in the app's own seconds.
    group_started: f32,
    /// The beat count at which the current group started, if there was a beat
    /// clock at the time.
    group_beat: Option<f32>,
    /// Where in the group the last call landed. Kept so the page can show it
    /// and so a switch can be detected without re-deriving it.
    position: usize,
    /// The fast clock's raw count since the group formed, before it is folded
    /// into `position` by the group's length. A source rotating a set of its
    /// own has a different length to fold by and needs the count itself — see
    /// [`Router::step`].
    steps: usize,
    history: History,
    models: usize,
    /// The suite's ids, in index order. The trained model names animations by
    /// id and this is what turns one back into the index everything downstream
    /// takes — see `metal_router::learn::Weights::classes` for why it is ids on
    /// that side of the seam.
    ids: Vec<String>,
    /// True while the fast clock is running off wall time because the source
    /// had no beat to give.
    pub guessing_tempo: bool,
    pub content: Content,
}

impl Router {
    pub fn new(ids: Vec<String>, data: &std::path::Path) -> Router {
        let models = ids.len();
        Router {
            // **Automix, not Single.** Every source chooses its own content
            // now (`layers::Feed`), and the ones that subscribe read `group`
            // and `on_air` off this router — which in `Mode::Single` are an
            // empty vector and a frozen index. The dashboard no longer sets a
            // mode at all; `/router` still does, for the harness and for
            // anything driving this from outside.
            mode: Mode::Automix,
            chosen: 0,
            picked: Vec::new(),
            group: Vec::new(),
            group_started: 0.0,
            group_beat: None,
            position: 0,
            steps: 0,
            history: History::sized(models),
            models,
            ids,
            guessing_tempo: false,
            content: Content::new(&data.join(metal_router::DIR)),
        }
    }

    /// Which animations the trained model may name: the operator's ticks, or
    /// the whole suite if they have not ticked any.
    ///
    /// *"automatic selection must be curated by the user"* — this is
    /// where that lives, and it is deliberately the same list `Mode::Picked`
    /// rotates through, so ticking a set means one thing in the app.
    pub fn curated(&self) -> Vec<String> {
        if self.picked.is_empty() {
            self.ids.clone()
        } else {
            self.picked
                .iter()
                .filter_map(|i| self.ids.get(*i).cloned())
                .collect()
        }
    }

    /// Feeds the live buffer. Called once per served request, before anything
    /// asks what is on air.
    pub fn observe(&mut self, time: f32, poll: impl FnMut(f32) -> Frame) {
        self.content.observe(time, poll);
    }

    /// Which models the router is allowed to pick from, in this mode.
    fn pool(&self) -> Vec<usize> {
        match self.mode {
            Mode::Single => vec![self.chosen],
            Mode::Automix => (0..self.models).collect(),
            // The fallback pool when the trained model has nothing to say is
            // the **curated** set — an operator who ticked six animations for
            // the router to choose between does not want the other sixteen back
            // because it is still warming up. It must be exactly what
            // [`Router::curated`] returns, including for an empty tick list:
            // there, an empty list is a positive statement that the router may
            // use everything, where in `Mode::Picked` it means "nothing is
            // ticked yet, keep the hand-chosen one on air". Two readings of one
            // empty vector, and the modes want opposite ones.
            Mode::Model if self.picked.is_empty() => (0..self.models).collect(),
            Mode::Model | Mode::Picked => {
                let mut p: Vec<usize> = self
                    .picked
                    .iter()
                    .copied()
                    .filter(|i| *i < self.models)
                    .collect();
                p.sort_unstable();
                p.dedup();
                // An empty or single pick is not a rotation. Falling back to
                // the hand-chosen model keeps the picture on air rather than
                // blanking it while the operator is still ticking boxes.
                if p.is_empty() {
                    p.push(self.chosen.min(self.models.saturating_sub(1)));
                }
                p
            }
        }
    }

    /// Forms a group of up to [`GROUP`] out of `pool`, by score.
    ///
    /// Greedy and deterministic: the best-scoring model starts the group and
    /// each further pick is scored again against what is already in it, which
    /// is what makes *"in which groups"* mean something — the third pick knows
    /// what the first two were and avoids a trio that played together an hour
    /// ago.
    ///
    /// Deterministic on purpose. A random tie-break would make the same history
    /// produce different groups and there would be no way to tell a policy bug
    /// from luck.
    fn form(&self, pool: &[usize]) -> Vec<usize> {
        let now = self.history.groups;
        let mut picked: Vec<usize> = Vec::new();
        let mut left: Vec<usize> = pool.to_vec();

        while picked.len() < GROUP.min(pool.len()) {
            let mut best = (f32::NEG_INFINITY, usize::MAX);
            for &m in &left {
                // Never played beats everything: the first pass over the suite
                // shows every animation before any of them comes round twice.
                let unplayed = if self.history.plays[m] == 0 {
                    1000.0
                } else {
                    0.0
                };
                // How long ago, in groups. Capped so a model parked for an
                // hour does not outweigh everything else for ever.
                let ago = match self.history.last_group[m] {
                    None => 64.0,
                    Some(g) => (now.saturating_sub(g) as f32).min(64.0),
                };
                let fairness = -(self.history.plays[m] as f32) * 4.0;
                // The pairing term. Each model already in the group penalises
                // this one by how recently they last shared a group.
                let mut pairing = 0.0;
                for &other in &picked {
                    if let Some(g) = self.history.together.get(&History::pair(m, other)) {
                        let since = now.saturating_sub(*g) as f32;
                        pairing -= (16.0 - since).max(0.0);
                    }
                }
                let score = unplayed + ago + fairness + pairing;
                // Ties go to the lower index, so the whole thing is stable.
                if score > best.0 {
                    best = (score, m);
                }
            }
            if best.1 == usize::MAX {
                break;
            }
            picked.push(best.1);
            left.retain(|&m| m != best.1);
        }
        picked
    }

    /// Which model should be on air at `time`, given the beat clock.
    ///
    /// Called once per served frame. Everything it changes — the group, the
    /// position, the history — is a function of `time` and what came before,
    /// and calling it twice at the same instant changes nothing, which is what
    /// keeps the preview and the monitor agreeing about what they are looking
    /// at.
    pub fn on_air(&mut self, time: f32, beats: Option<f32>) -> usize {
        if self.models == 0 {
            return 0;
        }
        if self.mode == Mode::Single {
            // Not just "return chosen": the group is dropped so that switching
            // into automix later starts a fresh one rather than resuming a
            // rotation from before the operator took over.
            self.group.clear();
            self.guessing_tempo = false;
            self.content.falling_back = false;
            return self.chosen.min(self.models - 1);
        }

        // The trained model gets first refusal. One animation, held — no group
        // and no fast clock, because the ask is explicit that the output should
        // change only when the music does, and a sixteen-beat rotation on top
        // of that would be the old router wearing the new one's answer.
        if self.mode == Mode::Model {
            let allowed = self.curated();
            let named = self.content.decide(time, &allowed).map(|d| d.model.clone());
            if let Some(id) = named {
                if let Some(index) = self.ids.iter().position(|c| *c == id) {
                    self.group.clear();
                    self.guessing_tempo = false;
                    self.content.falling_back = false;
                    return index.min(self.models - 1);
                }
            }
            // Nothing trained, not enough audio yet, or the model named an
            // animation this build does not have. Rotate the curated set the
            // old way and say so — a router that silently stopped switching is
            // indistinguishable from one that is working on a quiet passage.
            self.content.falling_back = true;
        } else {
            self.content.falling_back = false;
        }

        let pool = self.pool();

        // The slow clock. A group also has to be formed when there is not one
        // yet, and when the pool has changed underneath it — an operator
        // un-ticking the animation that is currently on air must not leave it
        // on air.
        let stale_pool = self.group.iter().any(|m| !pool.contains(m));
        let elapsed = time - self.group_started;
        if self.group.is_empty() || stale_pool || !(0.0..REGROUP_SECONDS).contains(&elapsed) {
            let group = self.form(&pool);
            if !group.is_empty() {
                self.history.record(&group);
                self.group = group;
                self.group_started = time;
                self.group_beat = beats;
                self.position = 0;
                self.steps = 0;
            }
        }
        if self.group.is_empty() {
            return self.chosen.min(self.models - 1);
        }

        // The fast clock. Beats since this group started, or wall time at the
        // assumed tempo when there is no beat to count.
        self.guessing_tempo = beats.is_none();
        let since = match (beats, self.group_beat) {
            (Some(now), Some(then)) => now - then,
            // A beat clock that appeared or vanished mid-group: re-anchor
            // rather than jump, so the switch after it is still sixteen beats
            // from the last one and not from an arbitrary origin.
            (Some(now), None) => {
                self.group_beat = Some(now);
                0.0
            }
            _ => (time - self.group_started) * (FALLBACK_BPM / 60.0),
        };
        self.steps = (since / BEATS_PER_SWITCH).max(0.0) as usize;
        self.position = self.steps % self.group.len();
        self.group[self.position]
    }

    /// The group as it stands, for the page.
    pub fn group(&self) -> &[usize] {
        &self.group
    }

    pub fn position(&self) -> usize {
        self.position
    }

    /// Seconds until the next group. `None` outside the rotating modes — and
    /// in model mode, unless it has fallen back to being one.
    pub fn regroup_in(&self, time: f32) -> Option<f32> {
        if self.mode == Mode::Single || self.group.is_empty() {
            return None;
        }
        if self.mode == Mode::Model && !self.content.falling_back {
            return None;
        }
        Some((REGROUP_SECONDS - (time - self.group_started)).max(0.0))
    }

    /// How many times each model has been put in a group this run.
    pub fn plays(&self) -> &[u32] {
        &self.history.plays
    }

    /// How many sixteen-beat switches have happened since the group formed.
    ///
    /// Exposed because a source can rotate a set of its own now
    /// ([`crate::layers::Feed::Group`]), and it has to step on *this* clock —
    /// otherwise every source in the room drifts on its own phase and the
    /// changes stop landing together on the beat.
    pub fn step(&self) -> usize {
        self.steps
    }

    /// The trained model's ranking over the whole suite, best first, re-decided
    /// as the music moves on.
    ///
    /// Run every frame regardless of [`Router::mode`], because sources
    /// subscribe to it one at a time now and there is no global switch that
    /// turns it on. A source restricts the ranking to its own ticks rather than
    /// asking for a decision of its own — one buffer, one window, one hold, and
    /// N sources reading the order it produced. Asking per source would give
    /// each of them a private hold clock over the same audio, which is several
    /// answers to a question that has one.
    ///
    /// Empty when there is nothing trained or the window is not full yet, which
    /// is the state a fresh checkout is in and must not look like a decision.
    pub fn ranking(&mut self, time: f32) -> Vec<usize> {
        let all = self.ids.clone();
        let Some(d) = self.content.decide(time, &all) else {
            return Vec::new();
        };
        d.ranked
            .iter()
            .filter_map(|(id, _)| all.iter().position(|c| c == id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A router over `n` animations with no trained model behind it — which is
    /// the state every one of the tests below is about except the last few.
    fn router(n: usize) -> Router {
        Router::new(
            (0..n).map(|i| format!("m{i}")).collect(),
            std::path::Path::new("/nonexistent-on-purpose"),
        )
    }

    fn beats_at(time: f32, bpm: f32) -> Option<f32> {
        Some(time * (bpm / 60.0))
    }

    #[test]
    fn single_is_exactly_what_the_operator_chose_and_forms_no_group() {
        let mut r = router(17);
        // Not the default any more — see `Router::new`. Still a mode, and still
        // has to be exactly one animation with nothing rotating behind it.
        r.mode = Mode::Single;
        r.chosen = 5;
        assert_eq!(r.on_air(0.0, beats_at(0.0, 120.0)), 5);
        assert_eq!(r.on_air(9999.0, beats_at(9999.0, 120.0)), 5);
        assert!(
            r.group().is_empty(),
            "single mode should not be rotating anything"
        );
    }

    #[test]
    fn automix_switches_every_sixteen_beats_and_not_before() {
        // The one behaviour the ask names a number for.
        let mut r = router(17);
        r.mode = Mode::Automix;
        let bpm = 120.0; // two beats a second: 16 beats is 8 seconds
        let at = |r: &mut Router, t: f32| r.on_air(t, beats_at(t, bpm));

        let first = at(&mut r, 0.0);
        assert_eq!(at(&mut r, 7.9), first, "switched before the sixteenth beat");
        let second = at(&mut r, 8.1);
        assert_ne!(second, first, "did not switch on the sixteenth beat");
        let third = at(&mut r, 16.1);
        assert_ne!(third, second);
        // Four in the group, so it comes back round on the fifth switch.
        assert_eq!(at(&mut r, 32.1), first, "the group did not come back round");
    }

    #[test]
    fn a_group_is_four_animations_and_they_are_all_different() {
        let mut r = router(17);
        r.mode = Mode::Automix;
        r.on_air(0.0, beats_at(0.0, 124.0));
        let g = r.group().to_vec();
        assert_eq!(g.len(), GROUP);
        let mut sorted = g.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), GROUP, "a group repeated an animation: {g:?}");
    }

    #[test]
    fn the_slow_clock_regroups_after_five_minutes_and_not_before() {
        let mut r = router(17);
        r.mode = Mode::Automix;
        r.on_air(0.0, beats_at(0.0, 120.0));
        let first = r.group().to_vec();
        r.on_air(299.0, beats_at(299.0, 120.0));
        assert_eq!(r.group(), first.as_slice(), "regrouped early");
        r.on_air(301.0, beats_at(301.0, 120.0));
        assert_ne!(r.group(), first.as_slice(), "never regrouped");
    }

    /// The whole point of scoring rather than shuffling: every animation gets
    /// on air before any of them plays twice.
    #[test]
    fn every_animation_plays_before_any_of_them_plays_a_second_time() {
        let models: usize = 17;
        let mut r = router(models);
        r.mode = Mode::Automix;
        // Enough groups to cover the suite: 17 models, 4 a group.
        let groups = models.div_ceil(GROUP);
        for g in 0..groups {
            r.on_air(g as f32 * REGROUP_SECONDS * 1.01, None);
        }
        let plays = r.plays();
        assert!(
            plays.iter().all(|&p| p >= 1),
            "some animations never played in the first pass: {plays:?}"
        );
        assert!(
            plays.iter().all(|&p| p <= 2),
            "an animation played three times before the suite was through: {plays:?}"
        );
    }

    /// *"in which groups"* — a pairing that just happened should not happen
    /// again immediately while there are untried pairings available.
    #[test]
    fn the_same_two_animations_are_not_grouped_twice_in_a_row() {
        let mut r = router(17);
        r.mode = Mode::Automix;
        let mut seen: Vec<Vec<usize>> = Vec::new();
        for g in 0..6 {
            r.on_air(g as f32 * REGROUP_SECONDS * 1.01, None);
            seen.push(r.group().to_vec());
        }
        for pair in seen.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            let shared = a.iter().filter(|m| b.contains(m)).count();
            assert!(
                shared <= 1,
                "consecutive groups shared {shared} animations: {a:?} then {b:?}"
            );
        }
    }

    #[test]
    fn picked_rotates_only_through_what_was_ticked() {
        let mut r = router(17);
        r.mode = Mode::Picked;
        r.picked = vec![2, 9, 14];
        let bpm = 120.0;
        for step in 0..12 {
            let t = step as f32 * 8.5;
            let on = r.on_air(t, beats_at(t, bpm));
            assert!(r.picked.contains(&on), "router left the picked set: {on}");
        }
        assert_eq!(r.group().len(), 3, "a pick of three is a group of three");
    }

    #[test]
    fn un_ticking_what_is_on_air_takes_it_off_air_immediately() {
        // Otherwise an operator removing an animation watches it keep playing
        // until the next switch, and reasonably concludes the tick did nothing.
        let mut r = router(17);
        r.mode = Mode::Picked;
        r.picked = vec![1, 3, 5, 7];
        let on = r.on_air(0.0, beats_at(0.0, 120.0));
        r.picked.retain(|m| *m != on);
        let after = r.on_air(0.1, beats_at(0.1, 120.0));
        assert_ne!(after, on);
        assert!(r.picked.contains(&after));
    }

    #[test]
    fn no_beat_clock_falls_back_to_wall_time_and_says_so() {
        // A router that silently stopped switching would be indistinguishable
        // from a router that was working and a song that had stopped.
        let mut r = router(17);
        r.mode = Mode::Automix;
        let first = r.on_air(0.0, None);
        assert!(r.guessing_tempo, "should be flagged as guessing");
        // 120 BPM assumed: 16 beats is 8 seconds.
        assert_eq!(r.on_air(7.5, None), first);
        assert_ne!(r.on_air(8.5, None), first);
    }

    #[test]
    fn asking_twice_at_the_same_instant_changes_nothing() {
        let mut r = router(17);
        r.mode = Mode::Automix;
        let a = r.on_air(120.0, beats_at(120.0, 128.0));
        let group = r.group().to_vec();
        let b = r.on_air(120.0, beats_at(120.0, 128.0));
        assert_eq!(a, b);
        assert_eq!(
            r.group(),
            group.as_slice(),
            "a second call reformed the group"
        );
    }

    // ---------------------------------------------------------------- model

    /// A row that describes loud, regular, tonal music — and one that does not.
    /// The two the toy brain below is fitted to tell apart.
    fn loud(i: usize) -> Frame {
        let mut f = [0.0f32; features::COLUMNS];
        f[0] = 0.8;
        f[1] = 0.4;
        f[3] = 0.35;
        f[2] = if i % 4 == 0 { 0.9 } else { 0.02 };
        f[4] = 0.9;
        for k in 1..12 {
            f[4 + k] = 0.02;
        }
        Frame::step(f)
    }
    fn quiet(i: usize) -> Frame {
        let mut f = [0.0f32; features::COLUMNS];
        f[0] = 0.2 + 0.03 * (i as f32 * 0.02).sin();
        f[1] = 0.75;
        f[2] = 0.02;
        f[3] = 0.62;
        for k in 0..12 {
            f[4 + k] = 0.5;
        }
        Frame::step(f)
    }

    /// A brain that has learnt one thing: loud regular music gets `m2`, and
    /// everything else gets `m5`.
    fn taught(ids: &[String]) -> Brain {
        use metal_router::learn::{train, Options, Sample};
        let mut samples = Vec::new();
        for s in 0..5 {
            let song = format!("s{s}");
            for k in 0..6 {
                let j = k as f32 * 0.01;
                let mut a = [0.3f32; FEATURES];
                a[0] = 0.8 + j;
                a[7] = 0.9;
                let mut b = [0.3f32; FEATURES];
                b[0] = 0.2 + j;
                b[7] = 0.05;
                samples.push(Sample {
                    song: song.clone(),
                    f: a,
                    class: 2,
                });
                samples.push(Sample {
                    song: song.clone(),
                    f: b,
                    class: 5,
                });
            }
        }
        Brain {
            weights: train(&samples, ids.to_vec(), &Options::default()),
        }
    }

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("m{i}")).collect()
    }

    /// Feeds `seconds` of one kind of music, at the digest's own rate.
    fn feed(r: &mut Router, from: f32, seconds: f32, mut row: impl FnMut(usize) -> Frame) -> f32 {
        let mut t = from;
        let mut i = 0usize;
        while t < from + seconds {
            r.observe(t, |_| row(i));
            i += 1;
            t += 1.0 / features::RATE;
        }
        t
    }

    #[test]
    fn the_mode_survives_a_round_trip_through_its_own_name() {
        // The page sends a string and `/router` parses it. A mode that did not
        // round-trip would be a button that silently does nothing.
        for m in [Mode::Single, Mode::Automix, Mode::Picked, Mode::Model] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(Mode::parse("nonsense"), None);
    }

    /// The state of every fresh checkout and every venue laptop nobody has
    /// authored on. It must keep drawing, must not sit on animation 0, and must
    /// say that the clock is what is deciding.
    #[test]
    fn model_mode_with_nothing_trained_rotates_and_admits_it() {
        let mut r = router(17);
        r.mode = Mode::Model;
        assert!(!r.content.brain.trained());
        let bpm = 120.0;
        let first = r.on_air(0.0, beats_at(0.0, bpm));
        assert!(
            r.content.falling_back,
            "it should say the clock is deciding"
        );
        assert_ne!(
            r.on_air(8.5, beats_at(8.5, bpm)),
            first,
            "it stopped switching entirely"
        );
    }

    #[test]
    fn a_trained_model_names_one_animation_and_holds_it() {
        let list = ids(8);
        let mut r = Router::new(
            list.clone(),
            std::path::Path::new("/nonexistent-on-purpose"),
        );
        r.content.brain = taught(&list);
        r.mode = Mode::Model;

        // Not enough audio yet: it falls back rather than guessing off two
        // seconds of a window that wants twelve.
        r.observe(0.0, |_| loud(0));
        r.on_air(0.1, None);
        assert!(
            r.content.falling_back,
            "it decided before it had heard anything"
        );
        assert!(r.content.warmth() < 1.0);

        let t = feed(&mut r, 0.0, 20.0, loud);
        assert_eq!(
            r.content.warmth(),
            1.0,
            "twenty seconds should be a full window"
        );
        let on = r.on_air(t, None);
        assert!(
            !r.content.falling_back,
            "a trained model on a full window must decide"
        );
        assert_eq!(list[on], "m2", "it did not learn which one loud music gets");

        // And it holds: the whole of *"a single output that varies little"*.
        let t = feed(&mut r, t, HOLD_SECONDS * 0.5, loud);
        assert_eq!(r.on_air(t, None), on, "it changed its mind mid-passage");
        assert!(r.content.holding_for(t).is_some());
    }

    #[test]
    fn it_changes_its_mind_when_the_music_changes_and_not_before() {
        let list = ids(8);
        let mut r = Router::new(
            list.clone(),
            std::path::Path::new("/nonexistent-on-purpose"),
        );
        r.content.brain = taught(&list);
        r.mode = Mode::Model;

        let mut t = feed(&mut r, 0.0, 20.0, loud);
        let first = r.on_air(t, None);
        assert_eq!(list[first], "m2");

        // The music changes, but not enough time has passed. The floor is what
        // keeps a fill or a breakdown from being a section.
        t = feed(&mut r, t, HOLD_SECONDS * 0.4, quiet);
        assert_eq!(r.on_air(t, None), first, "it switched inside the hold");

        // Past the floor, with the music genuinely different.
        t = feed(&mut r, t, HOLD_SECONDS, quiet);
        let second = r.on_air(t, None);
        assert_eq!(list[second], "m5", "it never noticed the music had changed");

        // And it does not keep flipping while nothing changes.
        let t2 = feed(&mut r, t, HOLD_SECONDS * 2.0, quiet);
        assert_eq!(
            r.on_air(t2, None),
            second,
            "it changed its mind for no reason"
        );
    }

    /// *"automatic selection must be curated by the user"*.
    #[test]
    fn the_model_never_reaches_past_what_the_operator_ticked() {
        let list = ids(8);
        let mut r = Router::new(
            list.clone(),
            std::path::Path::new("/nonexistent-on-purpose"),
        );
        r.content.brain = taught(&list);
        r.mode = Mode::Model;
        // Everything except the one it would have chosen.
        r.picked = vec![0, 1, 3, 4, 5];
        let t = feed(&mut r, 0.0, 20.0, loud);
        let on = r.on_air(t, None);
        assert!(
            r.picked.contains(&on),
            "it played {on}, which was not ticked"
        );
    }

    #[test]
    fn ticking_nothing_means_the_whole_suite_and_ticking_something_means_that() {
        let list = ids(6);
        let mut r = Router::new(
            list.clone(),
            std::path::Path::new("/nonexistent-on-purpose"),
        );
        assert_eq!(r.curated(), list);
        r.picked = vec![1, 4];
        assert_eq!(r.curated(), vec!["m1".to_string(), "m4".to_string()]);
        // And a tick past the end of the suite is dropped rather than panicking.
        r.picked = vec![1, 99];
        assert_eq!(r.curated(), vec!["m1".to_string()]);
    }

    /// The buffer is filled by tick, not by call. The dash asks what is on air
    /// several times per served frame and the browser asks for frames as fast
    /// as it can; a buffer that grew per call would hold a fraction of a second
    /// of audio and call it twelve.
    #[test]
    fn the_buffer_fills_at_the_digests_rate_and_not_at_the_request_rate() {
        let mut r = router(4);
        let mut polls = 0;
        for _ in 0..500 {
            r.observe(1.0, |_| {
                polls += 1;
                loud(0)
            });
        }
        assert_eq!(
            polls, 1,
            "five hundred asks at one instant polled {polls} times"
        );

        // And a long pause does not ask for an hour of rows it does not have.
        let mut polls = 0;
        r.observe(4000.0, |_| {
            polls += 1;
            loud(0)
        });
        assert!(
            polls <= MEMORY + 1,
            "a resumed clock asked for {polls} rows"
        );
    }

    #[test]
    fn leaving_model_mode_stops_it_claiming_to_be_falling_back() {
        // `falling_back` drives a warning on the page. Left set after a mode
        // change it would be a warning about a mode nobody is in.
        let mut r = router(9);
        r.mode = Mode::Model;
        r.on_air(0.0, None);
        assert!(r.content.falling_back);
        r.mode = Mode::Single;
        r.on_air(0.1, None);
        assert!(!r.content.falling_back);
        r.mode = Mode::Automix;
        r.on_air(0.2, None);
        assert!(!r.content.falling_back);
    }

    #[test]
    fn a_suite_smaller_than_a_group_still_rotates() {
        // Not hypothetical: `Mode::Picked` with two ticks is exactly this.
        let mut r = router(2);
        r.mode = Mode::Automix;
        let bpm = 120.0;
        let first = r.on_air(0.0, beats_at(0.0, bpm));
        assert_eq!(r.group().len(), 2);
        assert_ne!(r.on_air(8.5, beats_at(8.5, bpm)), first);
        assert_eq!(r.on_air(16.5, beats_at(16.5, bpm)), first);
    }
}
