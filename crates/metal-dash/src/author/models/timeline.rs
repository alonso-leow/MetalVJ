//! EDIT MODE: who is alive, and what is stacked on top of them.
//!
//! [`example::Example`] answers *"what is every float of **this** model worth,
//! tick by tick?"* — one model, one song, no notion that any other model
//! exists. That question does not change when a model enters or leaves a show,
//! which is exactly why this is a second document instead of a bigger
//! `Example`: a `Timeline` answers *"who is on screen at tick 900, and what is
//! stacked on top of them?"* and cites `Example` by `(model, song)` rather than
//! containing it. Cost to everything already recorded in `data/animations/**`:
//! zero. Not one byte of an existing example changes shape, and each one
//! becomes an ingredient a `Timeline` can name.
//!
//! # Why this is `Offline`, and not the live `Sequence`
//!
//! `sequence.rs` swaps whole `Collection`s in and out while a show runs, and it
//! is deliberately incapable of putting a *specific* clip or effect at a
//! *specific* tick — the Curator picks by resemblance to the music, on purpose
//! (`the design notes` Part IV §17.5, §18.4). Pre-produced work is the
//! opposite requirement: the operator said tick 900, and tick 900 is what must
//! render, the same way on every run. So this lives in [`metal_graph::Domain::Offline`]
//! and resolves to a graph ending in `export.video`, never `output.device`.
//! `lint_domain_restricted_kinds` already enforces that boundary; this module
//! stays on the right side of it by construction.
//!
//! # The five product questions this file answers, and how
//!
//! Part III listed five decisions as "for the operator". None of them blocks a
//! first cut, and four of the five are cheaper to decide permissively now than
//! to retrofit, so each is resolved here in whichever direction is reversible:
//!
//! 1. **One model at a time, or several?** Several. [`ModelSegment::layer`]
//!    carries a layer index, and segments on *different* layers may overlap
//!    freely. Somebody who wants the strict single-track reading simply never
//!    leaves layer 0, and [`Timeline::validate`] still catches the accidental
//!    overlap for them. The opposite choice — a bare `Vec` with no layer —
//!    would have to grow one later, and every `Timeline` written in the
//!    meantime would need migrating. One `usize` now, versus a migration then.
//! 2. **Does the Curator still pick anything here?** No. A stretch of
//!    `media_track` with no clip authored by hand renders nothing, honestly
//!    empty. Edit mode's whole premise is that the same input yields the same
//!    output — Step 10's gate is literally *two runs, same file hash* — and a
//!    model that chooses by resemblance to the music cannot promise that.
//!    This costs nothing to reverse: filling holes is additive.
//! 3. **What happens when two segments touch or overlap?** A hard error, on
//!    the same layer only, mirroring `Sequence::check`. No automatic
//!    crossfade: a transition nobody asked for is worse than a refusal, and a
//!    `TransitionSegment` can be added later without invalidating a single
//!    file written before it.
//! 4. **Is the export JSON only for re-importing?** It carries **both** ticks
//!    and seconds on every segment. Ticks are the grid this project actually
//!    thinks in; seconds are what any other tool would expect. Emitting both
//!    costs one multiply per segment and closes the question.
//! 5. **Does undo survive a page refresh?** No, and it does not need to: every
//!    mutation is a POST that lands on disk, so the *timeline* survives — only
//!    the undo *stack* is in the page. A server-side command log is real work
//!    for a benefit nobody has asked for yet.
//!
//! Each of these is stated as a decision rather than a fact, so the day one of
//! them turns out wrong the reasoning is here to argue with.
//!
//! # The six lanes, and why the sixth is not like the other five
//!
//! `model_track` says who is generating, `effect_tracks` says what is stacked
//! behind them, and `media_track`/`still_track`/`text_track` put footage, a
//! picture and words on their own branches. Those five all answer the same
//! question — *what is on screen at tick N* — and all five come out of
//! [`resolve`] as nodes in one graph.
//!
//! `signal_track` answers a different one: *what is the room doing at tick N*.
//! Not the screen — the bulbs screwed into the lamps, the DMX fixtures, the
//! instrument listening on MIDI. It reads the eight `sig_*` floats every model
//! now carries (`metal_schema::SIGNAL_FLOATS`) and names one of the six `out.*`
//! destinations to address them to.
//!
//! It is deliberately **absent from [`resolve`]'s graph**, and that is the one
//! asymmetry in this file worth reading twice. This module builds `Offline`
//! graphs, and `metal_graph::lint` refuses an `out.*` node in an `Offline` graph —
//! correctly, because an `.mp4` has no DMX universe in it and a lamp cannot be
//! lit at a tick being rendered faster than real time. Authoring a light cue
//! and sending one are different acts: the cue is a property of the *show* and
//! belongs here, keyframed against the song like every other float; the socket
//! is a property of the *night*, and belongs to a Live graph. So the cue
//! leaves through [`export`] and [`signals_at`], never through the graph.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layers::Blend;

/// A `Timeline` names ticks on the same 8 Hz grid `Example::ticks` does, and
/// the same one the digested `.npy` matrices are on. Nothing here converts
/// between grids; a `rate` that disagrees with the manifest is refused by
/// [`Timeline::validate`] rather than silently resampled.
pub const DEFAULT_RATE: f32 = metal_schema::TickRate::CANONICAL.hz() as f32;

/// The real node catalog, read fresh from `data/schema.json`.
///
/// Read per call rather than cached, the same choice `main.rs`'s own
/// `node_catalog` makes and for the same reason: these are a handful of
/// requests a human editor makes while looking at a lane, not a pump, and a
/// cache would need a story for `data/schema.json` changing under a running
/// app that reading it fresh does not.
///
/// Deliberately `metal_schema::Manifest` and not `author::models::schema::Manifest`.
/// They are two hand-written readers of the same file and only the former
/// carries the `footage` flag that keeps `wake` out of the
/// generators — see [`Timeline::validate`] for why that difference decides
/// whether a segment naming `"wake"` is caught or silently saved.
///
/// Located through [`crate::output::data_root`] rather than the relative
/// [`crate::author::SCHEMA`] its neighbours use, for one concrete reason: the
/// rest of this module only ever touches the real data directory from a
/// running server, whose working directory is the repo root, while `cargo
/// test` runs from the crate directory. `data_root` resolves both. It is the
/// same call `main.rs`'s own `node_catalog` makes.
pub fn catalog() -> Result<Vec<metal_graph::NodeKind>, String> {
    let path = crate::output::data_root().join("schema.json");
    let text = metal_vfs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let manifest = metal_schema::Manifest::parse(&text)
        .map_err(|e| format!("{} did not parse: {e}", path.display()))?;
    Ok(metal_graph::NodeKind::catalog(&manifest))
}

// ---------------------------------------------------------------------------
// the segments
// ---------------------------------------------------------------------------

/// A generator alive over `[from, to)` on one layer.
///
/// `model` is the flat id (`"turing"`), never `"gen.turing"` — the same
/// spelling `Example::model` already uses, so a segment can be handed straight
/// to `example::load` without translating. The `gen.` prefix is a convention of
/// `metal-graph`'s catalog and gets rebuilt in [`resolve`], at the one place that
/// actually talks to the graph.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ModelSegment {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub model: String,
    /// Which layer this generator occupies. Segments on different layers may
    /// overlap; two on the same layer may not — see the module doc, decision 1.
    #[serde(default)]
    pub layer: usize,
    /// How this layer composites against the ones under it. `None` — the
    /// default, and every timeline written before O.3 — is today's behaviour
    /// exactly: no mask, straight alpha, full opacity.
    #[serde(default)]
    pub mix: Option<SegmentMix>,
}

/// How one visual segment composites against the layers below it.
///
/// # Why this is a field and not an `fw.compose` node
///
/// Decided 2026-09-05 (`the design notes` H.10, point 2),
/// and it is the less obvious of the two options on purpose:
///
/// > *"not an explicit graph node but two masks or two outputs from different
/// > source in one same device, also in the edit or lanes may be two models or
/// > two media, or both, in two layers and with fusion mode, also may vary in
/// > forms, being able to create a venn diagram for example."*
///
/// A `fw.compose` node would be a second way to say something the live path
/// already says: a source on a device carries its own `mask`, `blend` and
/// `opacity` and the compositor reads them ([`crate::layers::Layer`],
/// [`crate::compose::Tile`]). Adding a node here would make "two layers over
/// one device" and "two segments over one timeline" two different structures
/// for one idea, and then a change to one would be a bug in the other.
///
/// So these three fields are literally the three the live path uses, and
/// [`SegmentMix::to_node_config`] writes them into the resolved node in the
/// same [`crate::node_layer::NodeLayerConfig`] shape an authored `gen.*` node
/// already carries. One design, read from two places.
///
/// **The mask may be a boolean one.** `mask` is an
/// [`crate::output::Mask`] id, and since O.1 that mask can be the
/// `union`/`intersect`/`subtract` of two others — which is what "venn diagram"
/// asked for. H.5b's nested contours and this are the same requirement seen
/// twice, and they share `Mask` rather than growing two shapes.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SegmentMix {
    /// The contour this layer is cut to, by [`crate::output::Mask::id`].
    /// `None` is uncut.
    #[serde(default)]
    pub mask: Option<String>,
    /// `over`/`add`/`mix` at minimum — [`Blend`] is the existing four, so the
    /// three the ask names are already there and `Screen` comes along.
    #[serde(default)]
    pub blend: Blend,
    /// `1.0` is fully opaque, and is what a segment with no mix reads as.
    #[serde(default = "one")]
    pub opacity: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for SegmentMix {
    fn default() -> SegmentMix {
        SegmentMix {
            mask: None,
            blend: Blend::Normal,
            opacity: 1.0,
        }
    }
}

impl SegmentMix {
    /// This mix as the config an authored `gen.*` node would carry.
    ///
    /// `z` and `sector` are deliberately left unset: a segment's depth is
    /// [`ModelSegment::layer`] and duplicating it here would be two fields
    /// that disagree the first time someone edits one of them.
    pub fn to_node_config(&self) -> crate::node_layer::NodeLayerConfig {
        crate::node_layer::NodeLayerConfig {
            blend: Some(self.blend),
            mask: self.mask.clone(),
            opacity: Some(self.opacity),
            z: None,
            sector: None,
            shader: None,
        }
    }
}

/// An effect living behind one [`ModelSegment`], over a sub-range of it.
///
/// `on` names a `ModelSegment.id` rather than a model name because the same
/// model can appear in several segments and an effect belongs to *one* of
/// them. `state` reuses [`metal_schema::EffectState`] — the three-state shape
/// §3.2 asks for, which existed unused until this file.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct EffectSegment {
    pub id: String,
    /// The `ModelSegment.id` this effect sits behind.
    pub on: String,
    pub from: usize,
    pub to: usize,
    /// One of the sixteen `fx.*` effects, without the prefix (`"vignette"`).
    pub effect: String,
    pub state: metal_schema::EffectState,
    /// Where a `Reactive` effect's number comes from: `true` inherits the
    /// generator's own row (axis 2a, "the AI models"), `false` reads the
    /// raw ingest untranslated (axis 2b, "without any processing"). Ignored
    /// unless `state` is `Reactive` — for `Fixed` there is no cable to have a
    /// source, which is the whole point of `metal_graph::effects`' design.
    #[serde(default)]
    pub from_row: bool,
}

/// A clip from the library, placed by hand at a tick.
///
/// The one track with no equivalent anywhere else in the repo — neither live
/// nor offline has ever placed a specific clip at a specific moment. See the
/// module doc, decision 2, for why an unfilled hole here stays empty.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct MediaSegment {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub library: String,
    pub clip: String,
    /// Seconds into the clip, `(start, end)`. `None` plays it from its head.
    #[serde(default)]
    pub passage: Option<(f32, f32)>,
    #[serde(default)]
    pub speed: f32,
    #[serde(default)]
    pub reverse: bool,
    /// Which layer this clip occupies, the same meaning and the same field
    /// [`ModelSegment::layer`] has.
    ///
    /// H.10 names `media`+`media` and `model`+`media` alongside
    /// `model`+`model`, and none of those three is answerable without a
    /// shared notion of which is on top. Defaulting to `0` puts every clip
    /// written before O.3 on the bottom layer, which — together with the
    /// track order [`resolve`] walks — is exactly where they were.
    #[serde(default)]
    pub layer: usize,
    /// How this clip composites against the layers below it. See
    /// [`SegmentMix`]; `None` is today's behaviour.
    #[serde(default)]
    pub mix: Option<SegmentMix>,
    /// The clip's own digest — decided with the operator 2026-09-05, S.L: "sound
    /// digest" runs the digester over the *clip's own audio*, not the
    /// timeline's song. `None`, and every timeline written before S.L, means
    /// exactly today's behaviour: [`resolve`] keeps reading the song's own
    /// digest for the whole timeline. Set by [`attach_digest`], it overrides
    /// that reading **only across this segment's own span** — see
    /// [`resolve`]'s handling of the `audio` node's config for where that
    /// override actually lands.
    #[serde(default)]
    pub digest: Option<DigestId>,
}

/// The id of a clip-level digest — the stem of its `.8hz.npy` matrix under
/// `data/digest/`, written by [`attach_digest`]. A plain `String` rather than
/// a newtype: it is serialized as one and read back as one, and the wrapper
/// would buy nothing a doc comment does not already say.
pub type DigestId = String;

/// A still image held over `[from, to)`.
///
/// Separate from [`MediaSegment`] and not a variant of it, because the two
/// share only their span. A clip has a passage, a speed and a direction —
/// three fields that mean nothing for a picture that does not move — and a
/// still has a `fit`, which means nothing for footage that already arrives at
/// a known size. One struct with five ignored fields would make every reader
/// ask which half applies before it could do anything.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct StillSegment {
    pub id: String,
    pub from: usize,
    pub to: usize,
    /// Where the file lives, the same two-level shape Library indexes in.
    pub library: String,
    pub image: String,
    /// `"cover"`, `"contain"` or `"stretch"`. Free-form on purpose: the
    /// renderer that will honour it does not exist yet, and pinning an enum
    /// now would be this crate deciding a question the renderer has not been
    /// asked. `validate` checks it is one of the three so a typo is caught at
    /// the point it is typed.
    #[serde(default)]
    pub fit: String,
}

/// A flat colour held over `[from, to)`: a solid overlay as a clip.
///
/// `rgb` covers the frame; `masks` (a venue's `<mask id> -> rgb`) paint those
/// contours over it, as `POST /overlay {op:"solid", masks}` does. Resolved
/// into a `solid` source node (flat `rgb`; the contours need a device, so they
/// play through `POST /overlay {op:"color-clip"}` on a live one).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ColorSegment {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub rgb: [u8; 3],
    #[serde(default)]
    pub masks: Vec<(String, [u8; 3])>,
}

/// Words on screen over `[from, to)`.
///
/// The one segment whose content is *in the document* rather than a pointer
/// into a library — a title is three words, and making the operator save them
/// as a file first would be ceremony for nothing.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TextSegment {
    pub id: String,
    pub from: usize,
    pub to: usize,
    pub text: String,
    /// Height as a fraction of the frame, so the same show reads the same on
    /// a phone and on a wall. Points would be a lie at both sizes.
    #[serde(default)]
    pub size: f32,
    /// `"left"`, `"center"` or `"right"`. Same reasoning as `StillSegment::fit`.
    #[serde(default)]
    pub align: String,
    /// Where the words sit in the frame, `0..1` from the top-left.
    #[serde(default)]
    pub at: Option<(f32, f32)>,
}

/// What the room outside the screen does over `[from, to)`.
///
/// The lane that is not pixels. It names one of the six `out.*` destinations
/// and says, in exactly the three states an effect has, where its numbers come
/// from: `Off` is dark, `Reactive` follows the eight `sig_*` floats of the
/// generator named by `on`, `Fixed` holds `held` regardless of what the model
/// is doing. Reusing [`metal_schema::EffectState`] rather than inventing a fourth
/// vocabulary is deliberate — an operator who has understood off/reactive/fixed
/// for an effect has already understood it for a lamp.
///
/// **Why this track exists in an `Offline` document at all**, when
/// `lint_domain_restricted_kinds` refuses an `out.*` node in an `Offline`
/// graph: authoring a light cue and sending one are different acts. The cue is
/// a property of the show, keyframed against the song like everything else
/// here. The socket is a property of the night it is played on. So [`resolve`]
/// deliberately emits no `out.*` node — see its own note — and [`export`]
/// carries the cues out, for a Live rig to play back.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SignalSegment {
    pub id: String,
    pub from: usize,
    pub to: usize,
    /// The destination without its `out.` prefix: `"ddp"`, `"sacn"`, `"midi"`.
    /// Same spelling convention `ModelSegment::model` and `EffectSegment::effect`
    /// already use, and rebuilt into a kind at the one place that talks to the
    /// catalog.
    pub out: String,
    /// The `ModelSegment.id` whose row drives this when `state` is `Reactive`.
    /// Empty for a `Fixed` or `Off` cue, which has no model to follow.
    #[serde(default)]
    pub on: String,
    pub state: metal_schema::EffectState,
    /// The eight `sig_*` values held when `state` is `Fixed`. `None` on a
    /// reactive cue, where the model's row is the answer.
    #[serde(default)]
    pub held: Option<metal_schema::Signal>,
}

// ---------------------------------------------------------------------------
// the document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Timeline {
    pub id: String,
    pub name: String,
    /// Stem of the song, the same spelling `Example::song` uses.
    pub song: String,
    pub rate: f32,
    pub length_ticks: usize,
    #[serde(default)]
    pub model_track: Vec<ModelSegment>,
    #[serde(default)]
    pub effect_tracks: Vec<EffectSegment>,
    #[serde(default)]
    pub media_track: Vec<MediaSegment>,
    #[serde(default)]
    pub still_track: Vec<StillSegment>,
    #[serde(default)]
    pub text_track: Vec<TextSegment>,
    #[serde(default)]
    pub signal_track: Vec<SignalSegment>,
    /// Flat colour clips (solid overlays on the timeline).
    #[serde(default)]
    pub color_track: Vec<ColorSegment>,
}

impl Timeline {
    pub fn new(id: &str, name: &str, song: &str, length_ticks: usize) -> Timeline {
        Timeline {
            id: id.to_string(),
            name: name.to_string(),
            song: song.to_string(),
            rate: DEFAULT_RATE,
            length_ticks,
            model_track: Vec::new(),
            effect_tracks: Vec::new(),
            media_track: Vec::new(),
            still_track: Vec::new(),
            color_track: Vec::new(),
            text_track: Vec::new(),
            signal_track: Vec::new(),
        }
    }

    /// Move a legacy 8 Hz timeline onto the canonical 32 Hz grid, **exactly**:
    /// every segment span and `length_ticks` ×4 and `rate` set to canonical.
    /// Idempotent and detectable via `rate` (the marker), so an already
    /// canonical file is untouched — the keyframe/segment form of the
    /// `Example` migration.
    pub fn upgrade_to_canonical(&mut self) -> bool {
        if (self.rate - metal_schema::TickRate::Base.hz() as f32).abs() > f32::EPSILON {
            return false;
        }
        let x4 = |v: &mut usize| *v = metal_schema::TickRate::Base.to_canonical(*v as i64) as usize;
        x4(&mut self.length_ticks);
        for s in &mut self.model_track {
            x4(&mut s.from);
            x4(&mut s.to);
        }
        for s in &mut self.effect_tracks {
            x4(&mut s.from);
            x4(&mut s.to);
        }
        for s in &mut self.media_track {
            x4(&mut s.from);
            x4(&mut s.to);
        }
        for s in &mut self.still_track {
            x4(&mut s.from);
            x4(&mut s.to);
        }
        for s in &mut self.text_track {
            x4(&mut s.from);
            x4(&mut s.to);
        }
        for s in &mut self.signal_track {
            x4(&mut s.from);
            x4(&mut s.to);
        }
        for s in &mut self.color_track {
            x4(&mut s.from);
            x4(&mut s.to);
        }
        self.rate = metal_schema::TickRate::CANONICAL.hz() as f32;
        true
    }

    /// Everything that must be true before this reaches disk.
    ///
    /// Validated against the **real node catalog**, not against the flat
    /// authoring manifest. The two disagree on purpose: `metal_schema::ModelSchema`
    /// carries a `footage` flag and `author::models::schema::ModelSchema` does
    /// not, and it is that flag which keeps `wake` out of the generators.
    /// Checking the flat manifest here would happily accept
    /// `"wake"` as a placeable generator when architecturally it is an
    /// effect — a file that saves clean and resolves to a graph naming a kind
    /// the catalog has never heard of.
    pub fn validate(&self, catalog: &[metal_graph::NodeKind]) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("a timeline needs an id".into());
        }
        if !self.rate.is_finite() || self.rate <= 0.0 {
            return Err(format!(
                "rate {} is not a positive number of ticks per second",
                self.rate
            ));
        }

        let kinds: std::collections::HashSet<&str> =
            catalog.iter().map(|k| k.kind.as_str()).collect();

        let mut seg_ids = std::collections::HashSet::new();
        for s in &self.model_track {
            if !seg_ids.insert(s.id.as_str()) {
                return Err(format!("segment id {:?} is used twice", s.id));
            }
            check_range(s.from, s.to, self.length_ticks, &s.id)?;
            // The check the plan calls out by name, with the reason in the
            // message: somebody who typed a footage model is not making a typo,
            // they are making a category error, and "no such model" would send
            // them looking for a misspelling that is not there.
            if !kinds.contains(format!("gen.{}", s.model).as_str()) {
                return Err(if kinds.contains(format!("fx.{}", s.model).as_str()) {
                    format!(
                        "{:?} is an effect, not a generator — put it on an effect track",
                        s.model
                    )
                } else {
                    format!("no such generator: {:?}", s.model)
                });
            }
            check_mix(s.mix.as_ref(), &s.id)?;
        }

        // Overlap is per layer. Sorting a copy rather than requiring the
        // caller to keep the track sorted: a track arrives from a page that
        // just dragged something, and demanding sorted input would make the
        // client responsible for an invariant the server can check in one pass.
        let mut by_layer: HashMap<usize, Vec<&ModelSegment>> = HashMap::new();
        for s in &self.model_track {
            by_layer.entry(s.layer).or_default().push(s);
        }
        for (layer, mut segs) in by_layer {
            segs.sort_by_key(|s| s.from);
            for pair in segs.windows(2) {
                if pair[1].from < pair[0].to {
                    return Err(format!(
                        "{:?} and {:?} overlap on layer {layer} (ticks {}..{} and {}..{}) — \
                         move one to another layer, or trim it",
                        pair[0].id, pair[1].id, pair[0].from, pair[0].to, pair[1].from, pair[1].to
                    ));
                }
            }
        }

        for e in &self.effect_tracks {
            if !seg_ids.contains(e.on.as_str()) {
                return Err(format!(
                    "effect {:?} sits on {:?}, which is not a model segment",
                    e.id, e.on
                ));
            }
            check_range(e.from, e.to, self.length_ticks, &e.id)?;
            if !kinds.contains(format!("fx.{}", e.effect).as_str()) {
                return Err(format!("no such effect: {:?}", e.effect));
            }
            // An effect outliving the generator it is stacked on has nothing
            // to apply to for part of its span. Caught here rather than
            // clamped silently, because a clamp turns a mistake into a picture
            // nobody can explain.
            let host = self
                .model_track
                .iter()
                .find(|m| m.id == e.on)
                .expect("checked present just above");
            if e.from < host.from || e.to > host.to {
                return Err(format!(
                    "effect {:?} runs {}..{} but {:?} only lives {}..{}",
                    e.id, e.from, e.to, host.id, host.from, host.to
                ));
            }
            if let metal_schema::EffectState::Fixed(v) = e.state {
                if !v.is_finite() {
                    return Err(format!("effect {:?} is fixed at a NaN", e.id));
                }
            }
        }

        for m in &self.media_track {
            check_range(m.from, m.to, self.length_ticks, &m.id)?;
            if m.clip.trim().is_empty() {
                return Err(format!("media segment {:?} names no clip", m.id));
            }
            if let Some((a, b)) = m.passage {
                if !a.is_finite() || !b.is_finite() || b <= a {
                    return Err(format!(
                        "media segment {:?} has an empty or invalid passage",
                        m.id
                    ));
                }
            }
            check_mix(m.mix.as_ref(), &m.id)?;
        }

        for s in &self.still_track {
            check_range(s.from, s.to, self.length_ticks, &s.id)?;
            if s.image.trim().is_empty() {
                return Err(format!("still segment {:?} names no image", s.id));
            }
            if !s.fit.is_empty() && !matches!(s.fit.as_str(), "cover" | "contain" | "stretch") {
                return Err(format!(
                    "still segment {:?} has fit {:?} — expected cover, contain or stretch",
                    s.id, s.fit
                ));
            }
        }

        for c in &self.color_track {
            check_range(c.from, c.to, self.length_ticks, &c.id)?;
        }

        for t in &self.text_track {
            check_range(t.from, t.to, self.length_ticks, &t.id)?;
            // Empty text is not a picture of nothing, it is a segment the
            // operator started and did not finish. Saving it would put a
            // block on the lane that draws as blank and reads as a bug.
            if t.text.is_empty() {
                return Err(format!("text segment {:?} has no words in it", t.id));
            }
            if !t.size.is_finite() || t.size < 0.0 || t.size > 1.0 {
                return Err(format!(
                    "text segment {:?} is {} of the frame tall — size is a fraction, 0..1",
                    t.id, t.size
                ));
            }
            if !t.align.is_empty() && !matches!(t.align.as_str(), "left" | "center" | "right") {
                return Err(format!(
                    "text segment {:?} has align {:?} — expected left, center or right",
                    t.id, t.align
                ));
            }
            if let Some((x, y)) = t.at {
                if !x.is_finite() || !y.is_finite() {
                    return Err(format!("text segment {:?} sits at a NaN", t.id));
                }
            }
        }

        for s in &self.signal_track {
            check_range(s.from, s.to, self.length_ticks, &s.id)?;
            // The same category-error message the generator check gives, for
            // the same reason: somebody who typed `sacn` when they meant a
            // model is not making a typo.
            if !kinds.contains(format!("out.{}", s.out).as_str()) {
                return Err(format!("no such signal output: {:?}", s.out));
            }
            match s.state {
                // A reactive cue with no model behind it has nothing to
                // follow. Refused rather than treated as dark, because "the
                // lamp did nothing" is the one symptom that never points at
                // its own cause.
                metal_schema::EffectState::Reactive => {
                    if s.on.trim().is_empty() {
                        return Err(format!(
                            "signal {:?} is reactive but names no model segment to follow",
                            s.id
                        ));
                    }
                    if !seg_ids.contains(s.on.as_str()) {
                        return Err(format!(
                            "signal {:?} follows {:?}, which is not a model segment",
                            s.id, s.on
                        ));
                    }
                    let host = self
                        .model_track
                        .iter()
                        .find(|m| m.id == s.on)
                        .expect("checked present just above");
                    if s.from < host.from || s.to > host.to {
                        return Err(format!(
                            "signal {:?} runs {}..{} but {:?} only lives {}..{}",
                            s.id, s.from, s.to, host.id, host.from, host.to
                        ));
                    }
                }
                metal_schema::EffectState::Fixed(_) => {
                    let Some(h) = s.held else {
                        return Err(format!(
                            "signal {:?} is fixed but carries no values to hold",
                            s.id
                        ));
                    };
                    let all = [h.level, h.hue, h.sat, h.strobe, h.pan, h.tilt, h.a, h.b];
                    if all.iter().any(|v| !v.is_finite()) {
                        return Err(format!("signal {:?} holds a NaN", s.id));
                    }
                }
                metal_schema::EffectState::Off => {}
            }
        }

        Ok(())
    }

    /// Ascending by `from`, then by layer — the order that makes a diff of the
    /// file read as a timeline, the same reason `Example::sorted` exists.
    fn sorted(mut self) -> Timeline {
        self.model_track
            .sort_by_key(|s| (s.from, s.layer, s.id.clone()));
        self.effect_tracks.sort_by_key(|s| (s.from, s.id.clone()));
        self.media_track.sort_by_key(|s| (s.from, s.id.clone()));
        self.still_track.sort_by_key(|s| (s.from, s.id.clone()));
        self.text_track.sort_by_key(|s| (s.from, s.id.clone()));
        self.signal_track.sort_by_key(|s| (s.from, s.id.clone()));
        self.color_track.sort_by_key(|s| (s.from, s.id.clone()));
        self
    }

    /// Seconds for a tick on this timeline's own grid.
    pub fn seconds(&self, tick: usize) -> f32 {
        tick as f32 / self.rate
    }
}

fn check_range(from: usize, to: usize, length: usize, id: &str) -> Result<(), String> {
    if to <= from {
        return Err(format!(
            "segment {id:?} runs {from}..{to}, which is empty or backwards"
        ));
    }
    if to > length {
        return Err(format!(
            "segment {id:?} ends at {to}, past the timeline's {length} ticks"
        ));
    }
    Ok(())
}

/// O.3: a [`SegmentMix`] is checked where it is typed, not where it is
/// rendered.
///
/// The mask id is deliberately **not** resolved here. A contour belongs to a
/// device (`crate::output::Output::masks`) and a timeline does not name one —
/// binding a segment to a mask that exists on the device it ends up played
/// on is the same late binding `crate::layers::Layer::mask` already has, and
/// `main.rs`'s `mask_of` already treats a name that no longer matches as
/// "broken binding, shown as such" rather than as a blacked-out source.
/// Refusing to save the document for it would be stricter here than on the
/// live path for no reason.
fn check_mix(mix: Option<&SegmentMix>, id: &str) -> Result<(), String> {
    let Some(mix) = mix else { return Ok(()) };
    if !mix.opacity.is_finite() || !(0.0..=1.0).contains(&mix.opacity) {
        return Err(format!(
            "segment {id:?} composites at opacity {}, which is not a fraction between 0 and 1",
            mix.opacity
        ));
    }
    if mix.mask.as_ref().is_some_and(|m| m.trim().is_empty()) {
        return Err(format!(
            "segment {id:?} names an empty contour — use no mask instead"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// resolving a tick to a graph
// ---------------------------------------------------------------------------

/// The graph this timeline describes at `tick`.
///
/// A pure function over `metal-graph`'s existing public API — it builds exactly
/// the kind of dumb, freshly-synthesized graph `main.rs`'s live device path
/// already builds per frame, just from segments instead of from `Vec<Layer>`.
/// It never reads a node's config back, and nothing here is cached: the whole
/// discipline of `metal_graph::effects` is that the graph *is* the state, and a
/// resolver that started consulting a second store would be the same drift bug
/// §3.2 exists to prevent.
///
/// A tick nothing covers resolves to a graph with an audio node and an export
/// node and no picture between them — honestly empty, per decision 2. That is
/// a valid graph, and `lint` says so.
pub fn resolve(catalog: &[metal_graph::NodeKind], tl: &Timeline, tick: usize) -> metal_graph::Graph {
    use metal_graph::{Domain, Endpoint};

    let mut g = metal_graph::Graph::new(&tl.id, &tl.name, Domain::Offline);
    // `audio.file`, not `audio.live` — the domain is inferred from this, and
    // inferring `Live` here would make every graph this produces fail the
    // `export.video`-only-in-Offline lint.
    let audio = g.add_node("audio", "audio.file", [0.0, 0.0]);
    let export = g.add_node("export", "export.video", [10.0, 0.0]);

    // S.L: a clip with its own digest overrides the song's digest, but only
    // across that clip's own span. `None` on every clip — every timeline
    // written before S.L — leaves this node's config untouched, which is
    // `Config::default()`, exactly what `resolve` has always emitted here.
    // On overlap (two clips with a digest on different layers, both live at
    // `tick`), the topmost layer wins — the same "who is on top decides"
    // rule [`stack_order`] already applies to what is on screen.
    if let Some(m) = tl
        .media_track
        .iter()
        .filter(|m| tick >= m.from && tick < m.to && m.digest.is_some())
        .max_by_key(|m| m.layer)
    {
        if let Some(n) = g.node_mut(&audio) {
            n.config = metal_graph::Config::from_json_str(
                &serde_json::json!({
                    "digest": m.digest,
                    "offset": tick - m.from,
                })
                .to_string(),
            )
            .unwrap_or_default();
        }
    }

    // Live segments, in layer order so the graph reads bottom-up the way the
    // lane draws it.
    let mut live: Vec<&ModelSegment> = tl
        .model_track
        .iter()
        .filter(|s| tick >= s.from && tick < s.to)
        .collect();
    live.sort_by_key(|s| (s.layer, s.id.clone()));

    // O.3: the composition rule.
    //
    // More than one live visual source used to mean N edges into
    // `export.image` and nothing anywhere saying which of them is on top —
    // the order was whatever order this function happened to walk the tracks
    // in, which is an accident and not a decision. [`stack_order`] makes it a
    // decision, and each source below writes how it composites into its own
    // node config, in the same `NodeLayerConfig` shape an authored `gen.*`
    // node carries.
    //
    // **A single live source is unaffected, byte for byte**: with nothing
    // under it there is nothing to compose against, so no config is written
    // and the graph is the one this function has always produced.
    let mut media_live: Vec<&MediaSegment> = tl
        .media_track
        .iter()
        .filter(|m| tick >= m.from && tick < m.to)
        .collect();
    // Stable, so clips on one layer keep the order the track holds them in —
    // which, with every `layer` at its default of 0, is exactly the order
    // this loop walked before O.3.
    media_live.sort_by_key(|m| m.layer);
    let composing = live.len() + media_live.len() > 1;

    let kinds: std::collections::HashSet<&str> = catalog.iter().map(|k| k.kind.as_str()).collect();

    for (row, seg) in live.iter().enumerate() {
        let kind = format!("gen.{}", seg.model);
        // A timeline written against a schema this build no longer has. Skip
        // it rather than emit a node naming a kind the catalog has never heard
        // of: `lint` would flag that as an error and the whole tick would read
        // as broken, when the honest answer is that one segment is stale.
        // Same "drop rather than crash" posture `Post::resolved_chain` takes
        // with an index no schema declares. `validate` refuses this on save;
        // this is the guard for a file that predates the schema change.
        if !kinds.contains(kind.as_str()) {
            continue;
        }
        let y = row as f32 * 2.0;
        let gen = g.add_node("gen", &kind, [2.0, y]);
        // How this layer meets the ones below it, written where the live path
        // already reads it from. Only when there *is* something below —
        // otherwise the config stays empty and the node is the one this
        // function has always emitted.
        if composing {
            if let Some(mix) = &seg.mix {
                if let Some(n) = g.node_mut(&gen) {
                    n.config = mix.to_node_config().to_config();
                }
            }
        }
        g.add_edge(
            Endpoint::new(&audio, "control"),
            Endpoint::new(&gen, "control"),
        );

        // The effects on this segment, live at this tick, in the order the
        // author put them in. Chain order is graph order — the same rule the
        // live path now follows — so the sequence they are wired in here is
        // the sequence the shader will compose them in.
        let mut tail = gen.clone();
        let mut x = 4.0;
        for fx in tl
            .effect_tracks
            .iter()
            .filter(|e| e.on == seg.id && tick >= e.from && tick < e.to)
            .filter(|e| !matches!(e.state, metal_schema::EffectState::Off))
        {
            let node = g.add_node("fx", &format!("fx.{}", fx.effect), [x, y]);
            g.add_edge(Endpoint::new(&tail, "image"), Endpoint::new(&node, "image"));
            match fx.state {
                // Reactive: the cable *is* the state, and which node it comes
                // from is the second axis. From the generator's own `row` port
                // it inherits the translated row; from `audio.file` it reads
                // the raw digest untouched.
                metal_schema::EffectState::Reactive => {
                    let src = if fx.from_row {
                        Endpoint::new(&gen, "row")
                    } else {
                        Endpoint::new(&audio, "control")
                    };
                    g.add_edge(src, Endpoint::new(&node, "control"));
                }
                // Fixed: no control edge at all, and the value rides in the
                // node's own config — `metal_graph::effects::FxConfig`, the one
                // thing about an effect that cannot be read back off the graph.
                metal_schema::EffectState::Fixed(v) => {
                    if let Some(n) = g.node_mut(&node) {
                        n.config = metal_graph::effects::FxConfig { fixed: v }.to_config();
                    }
                }
                metal_schema::EffectState::Off => unreachable!("filtered above"),
            }
            tail = node;
            x += 2.0;
        }

        g.add_edge(
            Endpoint::new(&tail, "image"),
            Endpoint::new(&export, "image"),
        );
    }

    // Media is a source, not a modifier: a clip enters the graph the same way
    // a generator does, on its own branch into the export.
    //
    // Which clip rides in the node's own `config`, the free-form per-kind slot
    // `metal-graph` documents as belonging to whoever reads `kind` — the graph
    // crate itself never looks inside it. Without this the resolved graph
    // would say "some clip plays here" and lose which one, which is the whole
    // content of a media segment.
    for (row, m) in media_live.iter().enumerate() {
        let clip = g.add_node("reel", "reel.clip", [2.0, -2.0 - row as f32 * 2.0]);
        let mut cfg = serde_json::json!({
            "library": m.library,
            "clip": m.clip,
            "passage": m.passage.map(|(a, b)| [a, b]),
            "speed": if m.speed > 0.0 { m.speed } else { 1.0 },
            "reverse": m.reverse,
        });
        // The mix rides in the same config object as the clip's own fields
        // rather than replacing it: a `reel.clip` node has to say *which
        // clip* as well as *how it composites*, and those are the same
        // free-form slot. Merged key by key so a future field on either side
        // does not silently drop the other.
        if composing {
            if let Some(mix) = &m.mix {
                if let (Some(dst), Ok(serde_json::Value::Object(src))) = (
                    cfg.as_object_mut(),
                    serde_json::to_value(mix.to_node_config()),
                ) {
                    dst.extend(src);
                }
            }
        }
        if let (Some(n), Ok(cfg)) = (
            g.node_mut(&clip),
            metal_graph::Config::from_json_str(&cfg.to_string()),
        ) {
            n.config = cfg;
        }
        g.add_edge(
            Endpoint::new(&audio, "control"),
            Endpoint::new(&clip, "control"),
        );
        g.add_edge(
            Endpoint::new(&clip, "image"),
            Endpoint::new(&export, "image"),
        );
    }

    // Stills, on the same footing as clips: a source with its own branch into
    // the export, carrying which file it is in its config. No `control` edge —
    // `still` has no control port, because a picture that does not move has
    // nothing for the music to move.
    for (row, s) in tl
        .still_track
        .iter()
        .filter(|s| tick >= s.from && tick < s.to)
        .enumerate()
    {
        let node = g.add_node("still", "still", [2.0, -20.0 - row as f32 * 2.0]);
        let cfg = serde_json::json!({
            "library": s.library,
            "image": s.image,
            "fit": if s.fit.is_empty() { "cover" } else { s.fit.as_str() },
        });
        if let (Some(n), Ok(cfg)) = (
            g.node_mut(&node),
            metal_graph::Config::from_json_str(&cfg.to_string()),
        ) {
            n.config = cfg;
        }
        g.add_edge(
            Endpoint::new(&node, "image"),
            Endpoint::new(&export, "image"),
        );
    }

    // A colour clip is a `solid` source on its own branch, like a still.
    for (row, c) in tl
        .color_track
        .iter()
        .filter(|c| tick >= c.from && tick < c.to)
        .enumerate()
    {
        let node = g.add_node("solid", metal_graph::SOLID_KIND, [2.0, -60.0 - row as f32 * 2.0]);
        let cfg = serde_json::json!({ "rgb": c.rgb });
        if let (Some(n), Ok(cfg)) = (
            g.node_mut(&node),
            metal_graph::Config::from_json_str(&cfg.to_string()),
        ) {
            n.config = cfg;
        }
        g.add_edge(
            Endpoint::new(&node, "image"),
            Endpoint::new(&export, "image"),
        );
    }

    // Text, likewise a source — and wired to `audio.file` unlike a still,
    // because `text` does have a control port: a title that pulses with the
    // music is the ordinary ask, and the cable is what makes it possible
    // without routing the letters through an effect chain.
    for (row, t) in tl
        .text_track
        .iter()
        .filter(|t| tick >= t.from && tick < t.to)
        .enumerate()
    {
        let node = g.add_node(
            "text",
            metal_graph::TEXT_SOURCE_KIND,
            [2.0, -40.0 - row as f32 * 2.0],
        );
        let cfg = serde_json::json!({
            "text": t.text,
            "size": if t.size > 0.0 { t.size } else { 0.1 },
            "align": if t.align.is_empty() { "center" } else { t.align.as_str() },
            "at": t.at.map(|(x, y)| [x, y]),
        });
        if let (Some(n), Ok(cfg)) = (
            g.node_mut(&node),
            metal_graph::Config::from_json_str(&cfg.to_string()),
        ) {
            n.config = cfg;
        }
        g.add_edge(
            Endpoint::new(&audio, "control"),
            Endpoint::new(&node, "control"),
        );
        g.add_edge(
            Endpoint::new(&node, "image"),
            Endpoint::new(&export, "image"),
        );
    }

    // `signal_track` is deliberately absent from this graph, and its absence
    // is the design rather than an omission.
    //
    // This function builds an `Offline` graph — the one that ends in
    // `export.video` and renders to a file with no wall clock in it. An
    // `out.*` node in an `Offline` graph is an error that `metal_graph::lint`
    // raises by name, and it is right to: a `.mp4` has no DMX universe, and a
    // lamp cannot be lit at a tick that is being rendered faster or slower
    // than real time. Emitting one here would make every resolved tick of a
    // show that has a single light cue lint as broken.
    //
    // So the cue lives in the document and leaves through [`export`], which
    // writes what the room does tick by tick beside what the screen does. A
    // Live graph — the one with the socket in it — is what plays that back.
    // See `SignalSegment`'s own doc for the full split.

    g
}

#[cfg(test)]
/// One visual source alive at a tick — what [`stack_order`] returns.
///
/// Stills and text are deliberately not here. H.10 names three pairings —
/// `model`+`model`, `media`+`media`, `model`+`media` — and those are the two
/// kinds that got a [`SegmentMix`]. A still or a title still enters the graph
/// on its own branch exactly as before; giving them a fusion knob nobody
/// asked for would be a wider surface to keep coherent for no case that
/// exists.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VisualSource<'a> {
    Model(&'a ModelSegment),
    Media(&'a MediaSegment),
}

#[cfg(test)]
impl VisualSource<'_> {
    pub fn id(&self) -> &str {
        match self {
            VisualSource::Model(s) => &s.id,
            VisualSource::Media(m) => &m.id,
        }
    }

    pub fn layer(&self) -> usize {
        match self {
            VisualSource::Model(s) => s.layer,
            VisualSource::Media(m) => m.layer,
        }
    }

    pub fn mix(&self) -> Option<&SegmentMix> {
        match self {
            VisualSource::Model(s) => s.mix.as_ref(),
            VisualSource::Media(m) => m.mix.as_ref(),
        }
    }
}

#[cfg(test)]
/// The live visual sources at `tick`, **back to front** — the answer to
/// "which layers are below this one", which is the question
/// [`SegmentMix`] is applied against.
///
/// This exists as its own function, and is public, because the ordering is
/// the load-bearing half of O.3's rule and it has exactly one definition.
/// [`resolve`] walks it to write each node's config; whoever renders an
/// `export.video` (which does not exist yet) has to composite in this order or
/// the mask and the blend mode of a layer would be applied against a
/// different set of pixels than the author was looking at.
///
/// The order is `(layer, kind, position in its track)`: layer first because
/// that is what the operator drags, then models before clips on the same
/// layer, then track order. That last tiebreak is why this is a *stable*
/// sort — with every `layer` left at its default of `0`, this reproduces the
/// exact walk `resolve` did before O.3, which is what keeps a timeline with
/// no mixes byte-identical.
pub fn stack_order(tl: &Timeline, tick: usize) -> Vec<VisualSource<'_>> {
    let mut out: Vec<VisualSource<'_>> = Vec::new();
    let mut models: Vec<&ModelSegment> = tl
        .model_track
        .iter()
        .filter(|s| tick >= s.from && tick < s.to)
        .collect();
    models.sort_by_key(|s| (s.layer, s.id.clone()));
    out.extend(models.into_iter().map(VisualSource::Model));
    out.extend(
        tl.media_track
            .iter()
            .filter(|m| tick >= m.from && tick < m.to)
            .map(VisualSource::Media),
    );
    out.sort_by_key(|s| s.layer());
    out
}

/// What the room is being asked for at `tick`, per destination.
///
/// The signal counterpart of [`resolve`]: that answers "what is on screen",
/// this answers "what are the lamps doing", and they are separate functions
/// for the same reason `signal_track` is not in the graph — one produces
/// pixels for a file, the other produces cues for a night.
///
/// A `Reactive` cue needs the row of the model it follows, which this function
/// does not have and does not go looking for: `rows` is passed in, keyed by
/// `ModelSegment::id`, by whoever already loaded the examples. A cue whose
/// model has no row at this tick contributes nothing rather than guessing —
/// the same "honestly empty" posture the module doc's decision 2 takes.
pub fn signals_at(
    tl: &Timeline,
    tick: usize,
    rows: &HashMap<String, Vec<f32>>,
) -> Vec<(String, metal_schema::Signal)> {
    let mut out = Vec::new();
    for s in tl
        .signal_track
        .iter()
        .filter(|s| tick >= s.from && tick < s.to)
    {
        let sig = match s.state {
            metal_schema::EffectState::Off => continue,
            metal_schema::EffectState::Fixed(_) => match s.held {
                Some(h) => h,
                None => continue,
            },
            metal_schema::EffectState::Reactive => match rows.get(&s.on) {
                // Short rows are skipped rather than padded: a row that does
                // not carry the signal block is a row from a schema that
                // predates it, and inventing zeros for it would put a dark
                // fixture on stage and call it authored.
                Some(row) if row.len() >= metal_schema::EFFECTS_WIDTH + metal_schema::SIGNAL_WIDTH => {
                    metal_schema::read_signal(row)
                }
                _ => continue,
            },
        };
        out.push((s.out.clone(), sig));
    }
    out
}

// ---------------------------------------------------------------------------
// disk
// ---------------------------------------------------------------------------

fn path_for(root: &Path, id: &str) -> PathBuf {
    root.join(format!("{id}.json"))
}

/// Where `POST /timeline/export` writes a show's dense rows. Public because
/// the route builds the same path to write it, and a second `format!` at the
/// call site is how a file ends up written under one name and deleted under
/// another — the exact way [`remove`] used to orphan one.
///
/// The `.export.json` suffix still ends in `.json`, so [`list`] globs it as a
/// candidate timeline. That is survivable only because `list` drops whatever
/// fails to deserialise, which an export document always does — a fact worth
/// naming rather than relying on quietly.
pub fn export_path_for(root: &Path, id: &str) -> PathBuf {
    root.join(format!("{id}.export.json"))
}

/// Same guard `example::safe_component` applies, for the same reason: an id
/// out of a request body becomes a path component here.
pub fn safe_id(raw: &str) -> Result<&str, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("empty timeline id".into());
    }
    if name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        return Err(format!("{name:?} is not a valid timeline id"));
    }
    Ok(name)
}

/// `None` covers both "no such timeline" and "the file is not valid JSON" —
/// same contract as `example::load`, and the caller's response is the same
/// either way: there is nothing to open.
pub fn load(root: &Path, id: &str) -> Option<Timeline> {
    let id = safe_id(id).ok()?;
    let path = path_for(root, id);
    let text = metal_vfs::read_to_string(&path).ok()?;
    let mut tl = serde_json::from_str::<Timeline>(&text).ok()?;
    // The one-time 8→32 Hz segment migration, persisted once (rate is the
    // marker); an already-canonical file is not rewritten.
    if tl.upgrade_to_canonical() {
        if let Ok(json) = serde_json::to_string_pretty(&tl) {
            let _ = crate::atomic::write(&path, json);
        }
    }
    Some(tl.sorted())
}

pub fn save(root: &Path, tl: Timeline, catalog: &[metal_graph::NodeKind]) -> Result<usize, String> {
    let mut tl = tl.sorted();
    tl.upgrade_to_canonical();
    tl.validate(catalog)?;
    let id = safe_id(&tl.id)?;
    let path = path_for(root, id);
    if let Some(dir) = path.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&tl).map_err(|e| e.to_string())?;
    crate::atomic::write(&path, json).map_err(|e| e.to_string())?;
    Ok(tl.model_track.len())
}

/// Deletes a show, and the dense export beside it if one was ever written.
///
/// The export goes too because it is derived, not authored: it is one
/// `POST /timeline/export` away from existing again as long as the timeline
/// does, and worth nothing once the timeline does not. Leaving it behind
/// would put a file in `data/timelines/` that no timeline explains, that
/// [`list`] globs on its way past, and that the next show to reuse the id
/// would silently inherit as its own already-exported rows.
///
/// A missing export is not an error — most timelines are never exported.
pub fn remove(root: &Path, id: &str) -> Result<(), String> {
    let id = safe_id(id)?;
    let path = path_for(root, id);
    if !metal_vfs::exists(&path) {
        return Err(format!("no such timeline: {id}"));
    }
    metal_vfs::remove_file(&path).map_err(|e| e.to_string())?;
    let export = export_path_for(root, id);
    if metal_vfs::exists(&export) {
        metal_vfs::remove_file(&export).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Every timeline on disk, by id, sorted — what the picker asks for.
pub fn list(root: &Path) -> Vec<Timeline> {
    let Ok(dir) = metal_vfs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<Timeline> = dir
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .filter_map(|e| metal_vfs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<Timeline>(&t).ok())
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

/// One segment's dense rows over the **whole** timeline (`tl.length_ticks`
/// of them, `width` floats each) — the matrix [`export`] builds per segment,
/// and since `S.O.5` also what `POST /overlay {op:"rasterize"}` windows to
/// `[seg.from, seg.to)` and saves as a library movement. One function so the
/// exported `.export.json` and the rasterised overlay can never disagree
/// about what a segment's floats are at a tick.
///
/// An unauthored model comes back as its own defaults held flat, rather than
/// an error. Placing a generator on the track and not having keyframed it yet
/// is an ordinary intermediate state during a session, and an export that
/// failed on it would be unusable exactly when it is most wanted.
pub fn segment_rows(
    root_examples: &Path,
    tl: &Timeline,
    seg: &ModelSegment,
    manifest: &super::schema::Manifest,
) -> Result<Vec<Vec<f32>>, String> {
    let schema = manifest
        .model(&seg.model)
        .ok_or_else(|| format!("no such model in the authoring manifest: {}", seg.model))?;
    let width = schema.width();
    let has_own_override =
        super::example::has_override(root_examples, &seg.model, &tl.song, &tl.id, &seg.id);
    Ok(
        match super::example::load(
            root_examples,
            &seg.model,
            &tl.song,
            Some((tl.id.as_str(), seg.id.as_str())),
        ) {
            Some(mut ex) if has_own_override => {
                // Confined to this segment's own span — see the comment inside
                // `export` for why the override and the shared corpus are not
                // windowed the same way.
                ex.ticks.retain(|t| t.tick >= seg.from && t.tick < seg.to);
                super::dense::interpolate(&ex, width, tl.length_ticks)
            }
            Some(ex) => super::dense::interpolate(&ex, width, tl.length_ticks),
            None => vec![schema.defaults(); tl.length_ticks],
        },
    )
}

/// The simple export: what is on screen at every tick, already resolved, as
/// plain JSON.
///
/// Deliberately **not** the graph per tick — that would be thousands of
/// near-identical node lists, and nothing downstream wants to diff those. It
/// is one row per tick naming which models are live, each with its dense float
/// row already interpolated by [`super::dense::interpolate`] (no new
/// interpolation is written here; Part III Step 8 says reuse it and there is
/// no reason to disagree), plus the effects standing behind each one.
///
/// Every time carries both `tick` and `seconds` — decision 4 in the module
/// doc.
pub fn export(
    root_examples: &Path,
    tl: &Timeline,
    manifest: &super::schema::Manifest,
) -> Result<serde_json::Value, String> {
    use serde_json::json;

    // One dense matrix per *segment*, not per model — this is Phase E3's whole
    // point. A segment can carry its own `(tl.id, seg.id)` override of the
    // shared `(model, song)` corpus (`example::load`'s `target` argument, the
    // same axis `SUITE`'s `device`/`layer` already addresses — see
    // `override_path_for`'s own doc comment: the two strings are opaque path
    // components, so a second caller naming a timeline/segment pair instead
    // of a device/layer pair costs nothing). So two segments naming the same
    // generator are no longer guaranteed to share one authored automation.
    //
    // `example::load` already does "try the override, fall back to the
    // shared corpus" internally — passing `Some((&tl.id, &seg.id))` instead
    // of `None` is most of the fix. A segment with no override of its own
    // still reads exactly the shared file it always has, so a show where
    // nobody has ever authored a segment override exports the same bytes as
    // before this axis existed (see the regression test below, named for
    // exactly this).
    //
    // The rest of the fix: an override's own ticks are windowed to the
    // segment's `[from, to)` before interpolating; the shared corpus's are
    // not. This is a real asymmetry, not an oversight — `dense::interpolate`
    // eases towards whichever authored tick comes *after* the one it is
    // resolving, with no notion of where a segment ends, so a keyframe placed
    // anywhere past a segment's own end already leaks backward into that
    // segment's tail today, for the shared corpus, and always has: two
    // segments on one model, far apart in the document, already bend each
    // other's edges through exactly this mechanism, and changing that would
    // be changing what "shared" has always meant. An override is different
    // in kind, not just in file: its entire reason to exist is to be *this
    // segment's own* automation and nothing else's, so a stray keyframe an
    // operator leaves at some other tick while the embedded koreo lane
    // happens to still be pointed at this override — the lane edits on the
    // whole song's axis, not a clamped one, see `modoToggleKeyEdit` — must
    // not quietly bend this segment's own values. Confining the anchors an
    // override contributes to its own span is what keeps that reachable
    // mistake from becoming the "silent, in-range, and wrong" shape this
    // project's own docs name twice (§3.3/§11.7), by construction rather
    // than by hoping nobody drags the playhead past the end.
    //
    // Cost: the old cache shared one dense matrix across every segment naming
    // the same model, on the reasoning that "a show that cuts back to the
    // same generator eight times should read its example once, not eight
    // times." That sharing cannot survive per-segment overrides — two
    // segments on the same model can now legitimately resolve to different
    // rows — so this reads once *per segment* instead. A show is a few
    // hundred segments at the outside (`edit_segment`'s own reasoning,
    // `mod.rs`), not the hot path, so trading that cache for correctness is
    // the easy call.
    let mut dense_by_segment: HashMap<String, Vec<Vec<f32>>> = HashMap::new();
    for seg in &tl.model_track {
        dense_by_segment.insert(
            seg.id.clone(),
            segment_rows(root_examples, tl, seg, manifest)?,
        );
    }

    let mut frames = Vec::with_capacity(tl.length_ticks);
    for tick in 0..tl.length_ticks {
        let mut live = Vec::new();
        for seg in tl
            .model_track
            .iter()
            .filter(|s| tick >= s.from && tick < s.to)
        {
            let floats: Vec<f32> = dense_by_segment[&seg.id]
                .get(tick)
                .cloned()
                .unwrap_or_default();

            let effects: Vec<serde_json::Value> = tl
                .effect_tracks
                .iter()
                .filter(|e| e.on == seg.id && tick >= e.from && tick < e.to)
                .filter(|e| !matches!(e.state, metal_schema::EffectState::Off))
                .map(|e| {
                    let mut o = json!({ "effect": e.effect });
                    match e.state {
                        metal_schema::EffectState::Reactive => {
                            o["state"] = json!("reactive");
                            o["source"] = json!(if e.from_row { "row" } else { "raw" });
                        }
                        metal_schema::EffectState::Fixed(v) => {
                            o["state"] = json!("fixed");
                            o["value"] = json!(v);
                        }
                        metal_schema::EffectState::Off => unreachable!("filtered above"),
                    }
                    o
                })
                .collect();

            live.push(json!({
                "segment": seg.id,
                "layer": seg.layer,
                "model": seg.model,
                "floats": floats,
                "effects": effects,
            }));
        }

        let media: Vec<serde_json::Value> = tl
            .media_track
            .iter()
            .filter(|m| tick >= m.from && tick < m.to)
            .map(|m| {
                json!({
                    "library": m.library,
                    "clip": m.clip,
                    "passage": m.passage.map(|(a, b)| json!([a, b])),
                    "speed": if m.speed > 0.0 { m.speed } else { 1.0 },
                    "reverse": m.reverse,
                })
            })
            .collect();

        let stills: Vec<serde_json::Value> = tl
            .still_track
            .iter()
            .filter(|s| tick >= s.from && tick < s.to)
            .map(|s| {
                json!({
                    "library": s.library,
                    "image": s.image,
                    "fit": if s.fit.is_empty() { "cover" } else { s.fit.as_str() },
                })
            })
            .collect();

        let texts: Vec<serde_json::Value> = tl
            .text_track
            .iter()
            .filter(|t| tick >= t.from && tick < t.to)
            .map(|t| {
                json!({
                    "text": t.text,
                    "size": if t.size > 0.0 { t.size } else { 0.1 },
                    "align": if t.align.is_empty() { "center" } else { t.align.as_str() },
                    "at": t.at.map(|(x, y)| json!([x, y])),
                })
            })
            .collect();

        // What the room does, beside what the screen does. This is the only
        // way a light cue leaves the document at all — `resolve` deliberately
        // keeps `out.*` nodes out of the Offline graph, so an exported show is
        // where a Live rig picks the cues up.
        //
        // `rgb` rides along with the eight floats even though it is derived
        // from three of them. Every one of the six transports needs it first
        // and none of them agrees on anything after, so computing it once here
        // — with `Signal::rgb`, the one implementation — is what keeps six
        // playback paths from each writing their own HSV conversion and
        // drifting.
        let rows: HashMap<String, Vec<f32>> = tl
            .model_track
            .iter()
            .filter(|s| tick >= s.from && tick < s.to)
            .filter_map(|s| {
                dense_by_segment
                    .get(&s.id)?
                    .get(tick)
                    .map(|r| (s.id.clone(), r.clone()))
            })
            .collect();
        let signals: Vec<serde_json::Value> = signals_at(tl, tick, &rows)
            .into_iter()
            .map(|(out, s)| {
                json!({
                    "out": out,
                    "level": s.level,
                    "hue": s.hue,
                    "sat": s.sat,
                    "strobe": s.strobe,
                    "pan": s.pan,
                    "tilt": s.tilt,
                    "a": s.a,
                    "b": s.b,
                    "rgb": s.rgb(),
                })
            })
            .collect();

        frames.push(json!({
            "tick": tick,
            "seconds": tl.seconds(tick),
            "models": live,
            "media": media,
            "stills": stills,
            "texts": texts,
            "signals": signals,
        }));
    }

    Ok(json!({
        "timeline": tl.id,
        "name": tl.name,
        "song": tl.song,
        "rate": tl.rate,
        "length_ticks": tl.length_ticks,
        "length_seconds": tl.seconds(tl.length_ticks),
        "frames": frames,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Vec<metal_graph::NodeKind> {
        super::catalog().expect("data/schema.json is checked in")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fw-timeline-{name}"));
        let _ = metal_vfs::remove_dir_all(&dir);
        dir
    }

    fn seg(id: &str, from: usize, to: usize, model: &str) -> ModelSegment {
        ModelSegment {
            id: id.into(),
            from,
            to,
            model: model.into(),
            layer: 0,
            mix: None,
        }
    }

    fn two_cuts() -> Timeline {
        let mut tl = Timeline::new("show", "A show", "click-100", 200);
        tl.model_track = vec![seg("a", 0, 100, "turing"), seg("b", 100, 200, "room")];
        tl
    }

    fn sig(
        id: &str,
        from: usize,
        to: usize,
        out: &str,
        state: metal_schema::EffectState,
    ) -> SignalSegment {
        SignalSegment {
            id: id.into(),
            from,
            to,
            out: out.into(),
            on: String::new(),
            state,
            held: None,
        }
    }

    // `assert_eq!` on a JSON value is exact, and a `Vec<f32>` serialized
    // through `json!` widens each value to `f64` *as that f32 bit pattern*,
    // not as the decimal literal a test writes — `0.9f32` widened prints as
    // `0.8999999761581421`, while `json!([0.9])` parses `0.9` straight into
    // an `f64`. The two are a different type's rounding of the same number,
    // not a real disagreement — this project's own hue-wrap test hit exactly
    // this once already. Compares the one float actually at stake instead of
    // the whole JSON array, so a real shape mismatch (wrong length, wrong
    // field) still fails loudly rather than being swallowed by the tolerance.
    fn assert_float_eq(actual: &serde_json::Value, expected: f32, msg: &str) {
        let got = actual[0]
            .as_f64()
            .unwrap_or_else(|| panic!("{msg}: {actual:?} is not a float array"));
        assert!(
            (got - expected as f64).abs() < 1e-5,
            "{msg}: got {got}, expected {expected}"
        );
    }

    fn lit() -> metal_schema::Signal {
        metal_schema::Signal {
            level: 0.8,
            hue: 0.1,
            sat: 1.0,
            strobe: 4.0,
            pan: 0.0,
            tilt: -0.5,
            a: 0.0,
            b: 0.0,
            cct: 5600.0,
            zoom: 0.0,
            focus: 0.0,
            iris: 0.0,
            frost: 0.0,
        }
    }

    // -- the three lanes that are not the model track ------------------------

    #[test]
    fn a_still_a_text_and_a_cue_all_survive_the_round_trip() {
        let root = scratch("new-tracks");
        let mut tl = two_cuts();
        tl.still_track = vec![StillSegment {
            id: "s1".into(),
            from: 0,
            to: 50,
            library: "posters".into(),
            image: "poster.png".into(),
            fit: "contain".into(),
        }];
        tl.text_track = vec![TextSegment {
            id: "t1".into(),
            from: 10,
            to: 40,
            text: "METALVJ".into(),
            size: 0.2,
            align: "center".into(),
            at: Some((0.5, 0.5)),
        }];
        let mut cue = sig("c1", 0, 100, "sacn", metal_schema::EffectState::Fixed(1.0));
        cue.held = Some(lit());
        tl.signal_track = vec![cue];

        save(&root, tl.clone(), &catalog()).expect("save");
        let back = load(&root, "show").expect("load");
        assert_eq!(back.still_track, tl.still_track);
        assert_eq!(back.text_track, tl.text_track);
        assert_eq!(back.signal_track, tl.signal_track);
    }

    /// A file written before these three tracks existed. `#[serde(default)]`
    /// is what makes that work, and this is the test that would catch anyone
    /// dropping it — a show authored last week must still open.
    #[test]
    fn a_timeline_written_before_the_new_tracks_existed_still_loads() {
        let old = r#"{"id":"old","name":"Old","song":"click-100","rate":8.0,
                      "length_ticks":100,"model_track":[],"effect_tracks":[]}"#;
        let tl: Timeline = serde_json::from_str(old).expect("an older show still parses");
        assert!(tl.still_track.is_empty());
        assert!(tl.text_track.is_empty());
        assert!(tl.signal_track.is_empty());
    }

    #[test]
    fn a_text_segment_with_no_words_in_it_is_refused() {
        // Not "a picture of nothing" — a segment the operator started and did
        // not finish, which would draw as a blank block on the lane.
        let mut tl = two_cuts();
        tl.text_track = vec![TextSegment {
            id: "t1".into(),
            from: 0,
            to: 10,
            text: String::new(),
            size: 0.2,
            align: String::new(),
            at: None,
        }];
        let err = tl
            .validate(&catalog())
            .expect_err("empty text must not save");
        assert!(err.contains("no words"), "{err}");
    }

    #[test]
    fn a_text_taller_than_the_frame_names_the_unit_it_got_wrong() {
        let mut tl = two_cuts();
        tl.text_track = vec![TextSegment {
            id: "t1".into(),
            from: 0,
            to: 10,
            text: "hello".into(),
            size: 48.0, // points, from somebody who expected points
            align: String::new(),
            at: None,
        }];
        let err = tl
            .validate(&catalog())
            .expect_err("48 frames tall is not a size");
        assert!(
            err.contains("fraction"),
            "the message has to say what the unit is: {err}"
        );
    }

    #[test]
    fn a_cue_naming_a_destination_that_is_not_a_node_kind_is_refused() {
        let mut tl = two_cuts();
        tl.signal_track = vec![sig("c1", 0, 10, "telnet", metal_schema::EffectState::Off)];
        let err = tl
            .validate(&catalog())
            .expect_err("telnet is not a signal output");
        assert!(err.contains("no such signal output"), "{err}");
    }

    #[test]
    fn a_reactive_cue_with_no_model_behind_it_is_refused_rather_than_left_dark() {
        // "The lamp did nothing" is the one symptom that never points at its
        // own cause, so it is caught at the moment it is authored instead.
        let mut tl = two_cuts();
        tl.signal_track = vec![sig("c1", 0, 10, "sacn", metal_schema::EffectState::Reactive)];
        let err = tl.validate(&catalog()).expect_err("reactive to what?");
        assert!(err.contains("names no model segment"), "{err}");
    }

    #[test]
    fn a_reactive_cue_may_not_outlive_the_generator_it_follows() {
        // Same rule an effect already has, for the same reason: for part of
        // its span there would be no row to read.
        let mut tl = two_cuts();
        let mut c = sig("c1", 50, 150, "sacn", metal_schema::EffectState::Reactive);
        c.on = "a".into(); // "a" only lives 0..100
        tl.signal_track = vec![c];
        let err = tl.validate(&catalog()).expect_err("outlives its host");
        assert!(err.contains("only lives"), "{err}");
    }

    #[test]
    fn a_fixed_cue_with_nothing_to_hold_is_refused() {
        let mut tl = two_cuts();
        tl.signal_track = vec![sig("c1", 0, 10, "midi", metal_schema::EffectState::Fixed(1.0))];
        let err = tl.validate(&catalog()).expect_err("fixed at what?");
        assert!(err.contains("no values to hold"), "{err}");
    }

    /// The split this whole design rests on: a light cue is authored in the
    /// Offline document and does **not** become a node in the Offline graph,
    /// because `lint_domain_restricted_kinds` refuses `out.*` there — and it
    /// is right to, since a `.mp4` has no DMX universe.
    #[test]
    fn a_cue_never_becomes_a_node_in_the_offline_graph_it_is_authored_in() {
        let mut tl = two_cuts();
        let mut c = sig("c1", 0, 100, "sacn", metal_schema::EffectState::Fixed(1.0));
        c.held = Some(lit());
        tl.signal_track = vec![c];

        let cat = catalog();
        let g = resolve(&cat, &tl, 10);
        assert!(
            !g.nodes.iter().any(|n| n.kind.starts_with("out.")),
            "an out.* node here would make every tick of this show lint as broken"
        );
        assert!(
            !g.lint(&cat)
                .iter()
                .any(|i| i.severity == metal_graph::Severity::Error),
            "{:?}",
            g.lint(&cat)
        );
    }

    /// And the other half: the cue leaves through the reader, not the graph.
    #[test]
    fn a_fixed_cue_reads_back_exactly_what_it_holds_over_exactly_its_span() {
        let mut tl = two_cuts();
        let mut c = sig("c1", 20, 40, "ddp", metal_schema::EffectState::Fixed(1.0));
        c.held = Some(lit());
        tl.signal_track = vec![c];
        let none = HashMap::new();

        assert!(signals_at(&tl, 19, &none).is_empty(), "before it starts");
        let at = signals_at(&tl, 20, &none);
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].0, "ddp");
        assert_eq!(at[0].1, lit());
        assert!(
            signals_at(&tl, 40, &none).is_empty(),
            "the end is exclusive, like every other track"
        );
    }

    #[test]
    fn a_reactive_cue_reads_the_signal_block_off_the_model_row_it_follows() {
        let mut tl = two_cuts();
        let mut c = sig("c1", 0, 100, "osc", metal_schema::EffectState::Reactive);
        c.on = "a".into();
        tl.signal_track = vec![c];

        // A row shaped the way a real one is: anything at all, then the
        // `SIGNAL_WIDTH` `sig_*` floats, then the effects block and its
        // colour slot. The signal block grew with BLE Phase B, so build it
        // from the schema's own defaults and set the eight original floats.
        let mut row = vec![0.0f32; 6];
        let mut sig: Vec<f32> = metal_schema::SIGNAL_FLOATS.iter().map(|s| s.default).collect();
        sig[0] = 0.8;
        sig[1] = 0.1;
        sig[2] = 1.0;
        sig[3] = 4.0;
        sig[4] = 0.0;
        sig[5] = -0.5;
        sig[6] = 0.0;
        sig[7] = 0.0;
        row.extend(sig);
        row.extend(vec![0.0f32; metal_schema::EFFECTS_WIDTH]);
        let rows = HashMap::from([("a".to_string(), row)]);

        let at = signals_at(&tl, 10, &rows);
        assert_eq!(at.len(), 1);
        assert_eq!(
            at[0].1,
            lit(),
            "read straight out of the row's signal block"
        );
    }

    #[test]
    fn a_reactive_cue_whose_model_has_no_row_here_contributes_nothing() {
        // Honestly empty rather than a guessed blackout — the same posture
        // decision 2 takes for an unfilled media hole.
        let mut tl = two_cuts();
        let mut c = sig("c1", 0, 100, "http", metal_schema::EffectState::Reactive);
        c.on = "a".into();
        tl.signal_track = vec![c];
        assert!(signals_at(&tl, 10, &HashMap::new()).is_empty());
        // And a row too short to carry the signal block at all — a row from a
        // schema that predates it. Padding it with zeros would put a dark
        // fixture on stage and call it authored.
        let short = HashMap::from([("a".to_string(), vec![0.0f32; 4])]);
        assert!(signals_at(&tl, 10, &short).is_empty());
    }

    #[test]
    fn a_still_and_a_text_do_become_nodes_in_the_graph() {
        let mut tl = two_cuts();
        tl.still_track = vec![StillSegment {
            id: "s1".into(),
            from: 0,
            to: 50,
            library: "posters".into(),
            image: "poster.png".into(),
            fit: String::new(),
        }];
        tl.text_track = vec![TextSegment {
            id: "t1".into(),
            from: 0,
            to: 50,
            text: "HELLO".into(),
            size: 0.2,
            align: String::new(),
            at: None,
        }];
        let cat = catalog();
        let g = resolve(&cat, &tl, 10);

        let still = g
            .nodes
            .iter()
            .find(|n| n.kind == "still")
            .expect("a still node");
        assert!(still.config.to_json_str().contains("poster.png"));
        assert!(
            still.config.to_json_str().contains("cover"),
            "an unset fit defaults rather than saving empty"
        );

        let text = g
            .nodes
            .iter()
            .find(|n| n.kind == metal_graph::TEXT_SOURCE_KIND)
            .expect("a text node");
        assert!(text.config.to_json_str().contains("HELLO"));
        // The port `still` does not have, and the reason `text` is its own
        // family: a title that pulses with the music is the ordinary ask.
        assert!(
            g.edges
                .iter()
                .any(|e| e.to.node == text.id && e.to.port == "control"),
            "text is wired to the audio; a still is not"
        );
        assert!(
            !g.edges.iter().any(|e| e.to.node == still.id),
            "a picture that does not move has nothing for the music to move"
        );

        assert!(
            !g.lint(&cat)
                .iter()
                .any(|i| i.severity == metal_graph::Severity::Error),
            "{:?}",
            g.lint(&cat)
        );
    }

    /// Deleting a show takes its dense export with it. The export is derived,
    /// so it is worth nothing once the timeline is gone — and leaving it
    /// behind is worse than nothing: `list` globs `*.json`, and the next show
    /// to claim this id would find rows it never exported sitting beside it.
    #[test]
    fn deleting_a_timeline_takes_its_export_with_it() {
        let root = scratch("delete-export");
        save(&root, two_cuts(), &catalog()).expect("save");
        let export = export_path_for(&root, "show");
        metal_vfs::write(&export, "{}").expect("stand in for a real export");

        remove(&root, "show").expect("remove");

        assert!(
            !metal_vfs::exists(&path_for(&root, "show")),
            "the timeline itself is gone"
        );
        assert!(
            !metal_vfs::exists(&export),
            "and so is the export that only described it"
        );
    }

    /// The common case: most shows are never exported, and deleting one must
    /// not fail looking for a file that was never written.
    #[test]
    fn deleting_a_never_exported_timeline_is_not_an_error() {
        let root = scratch("delete-no-export");
        save(&root, two_cuts(), &catalog()).expect("save");
        remove(&root, "show").expect("a missing export is not an error");
        assert!(!metal_vfs::exists(&path_for(&root, "show")));
    }

    #[test]
    fn a_saved_timeline_round_trips_sorted_and_intact() {
        let root = scratch("roundtrip");
        let mut tl = two_cuts();
        tl.model_track.reverse(); // saved out of order on purpose
        save(&root, tl, &catalog()).expect("save");
        let back = load(&root, "show").expect("load");
        assert_eq!(back.model_track[0].id, "a", "sorted ascending on disk");
        assert_eq!(back.model_track[1].id, "b");
    }

    /// The `Example` discipline, applied here: saving twice must produce the
    /// same bytes, or a timeline in version control stops being diffable.
    #[test]
    fn saving_the_same_timeline_twice_produces_the_same_bytes() {
        let root = scratch("bytes");
        save(&root, two_cuts(), &catalog()).expect("first");
        let first = metal_vfs::read(root.join("show.json")).expect("read");
        save(&root, load(&root, "show").expect("load"), &catalog()).expect("second");
        let second = metal_vfs::read(root.join("show.json")).expect("read");
        assert_eq!(first, second);
    }

    /// The acceptance test Part III Step 4 names word for word.
    #[test]
    fn a_segment_naming_a_footage_model_is_refused_as_an_effect_not_as_a_typo() {
        let mut tl = Timeline::new("t", "t", "s", 100);
        tl.model_track = vec![seg("a", 0, 50, "wake")];
        let err = tl
            .validate(&catalog())
            .expect_err("wake is not a generator");
        assert!(
            err.contains("no such generator"),
            "the message must say why, got: {err}"
        );
    }

    #[test]
    fn a_segment_naming_nothing_at_all_is_refused_as_a_missing_generator() {
        let mut tl = Timeline::new("t", "t", "s", 100);
        tl.model_track = vec![seg("a", 0, 50, "no-such-model")];
        let err = tl.validate(&catalog()).expect_err("unknown model");
        assert!(err.contains("no such generator"), "got: {err}");
    }

    #[test]
    fn two_segments_overlapping_on_one_layer_are_refused() {
        let mut tl = Timeline::new("t", "t", "s", 200);
        tl.model_track = vec![seg("a", 0, 100, "turing"), seg("b", 50, 150, "room")];
        let err = tl.validate(&catalog()).expect_err("overlap on layer 0");
        assert!(err.contains("overlap"), "got: {err}");
    }

    /// Decision 1: the same overlap is legal once they are on different
    /// layers, because that is what a layer means.
    #[test]
    fn the_same_two_segments_on_different_layers_are_fine() {
        let mut tl = Timeline::new("t", "t", "s", 200);
        let mut b = seg("b", 50, 150, "room");
        b.layer = 1;
        tl.model_track = vec![seg("a", 0, 100, "turing"), b];
        tl.validate(&catalog())
            .expect("different layers may overlap");
    }

    #[test]
    fn segments_that_merely_touch_end_to_end_are_fine() {
        two_cuts()
            .validate(&catalog())
            .expect("0..100 and 100..200 do not overlap");
    }

    #[test]
    fn an_effect_outliving_its_generator_is_refused() {
        let mut tl = two_cuts();
        tl.effect_tracks = vec![EffectSegment {
            id: "e".into(),
            on: "a".into(),
            from: 50,
            to: 150, // "a" ends at 100
            effect: "vignette".into(),
            state: metal_schema::EffectState::Reactive,
            from_row: false,
        }];
        let err = tl
            .validate(&catalog())
            .expect_err("effect runs past its host");
        assert!(err.contains("only lives"), "got: {err}");
    }

    #[test]
    fn an_effect_on_a_segment_that_does_not_exist_is_refused() {
        let mut tl = two_cuts();
        tl.effect_tracks = vec![EffectSegment {
            id: "e".into(),
            on: "ghost-segment".into(),
            from: 0,
            to: 10,
            effect: "vignette".into(),
            state: metal_schema::EffectState::Reactive,
            from_row: false,
        }];
        assert!(tl.validate(&catalog()).is_err());
    }

    #[test]
    fn a_backwards_or_empty_range_is_refused() {
        let mut tl = Timeline::new("t", "t", "s", 100);
        tl.model_track = vec![seg("a", 50, 50, "turing")];
        assert!(
            tl.validate(&catalog()).is_err(),
            "an empty span is not a segment"
        );
        tl.model_track = vec![seg("a", 60, 50, "turing")];
        assert!(
            tl.validate(&catalog()).is_err(),
            "a backwards span is not a segment"
        );
    }

    #[test]
    fn a_segment_past_the_end_of_the_timeline_is_refused() {
        let mut tl = Timeline::new("t", "t", "s", 100);
        tl.model_track = vec![seg("a", 0, 500, "turing")];
        assert!(tl.validate(&catalog()).is_err());
    }

    // -- resolve ------------------------------------------------------------

    /// Part III Step 2's own acceptance test: a two-segment timeline resolves
    /// to different graphs before and after the cut, and both lint clean.
    #[test]
    fn a_two_segment_timeline_resolves_to_different_graphs_across_the_cut() {
        let cat = catalog();
        let tl = two_cuts();
        let before = resolve(&cat, &tl, 50);
        let after = resolve(&cat, &tl, 150);

        let kinds = |g: &metal_graph::Graph| -> Vec<String> {
            let mut k: Vec<String> = g.nodes.iter().map(|n| n.kind.clone()).collect();
            k.sort();
            k
        };
        assert!(kinds(&before).contains(&"gen.turing".to_string()));
        assert!(kinds(&after).contains(&"gen.room".to_string()));
        assert_ne!(
            kinds(&before),
            kinds(&after),
            "the cut must change the graph"
        );

        for (name, g) in [("before", &before), ("after", &after)] {
            let errors: Vec<_> = g
                .lint(&cat)
                .into_iter()
                .filter(|i| matches!(i.severity, metal_graph::lint::Severity::Error))
                .collect();
            assert!(errors.is_empty(), "{name} must lint clean, got {errors:?}");
        }
    }

    #[test]
    fn resolving_the_same_tick_twice_gives_the_same_graph() {
        let cat = catalog();
        let tl = two_cuts();
        assert_eq!(
            resolve(&cat, &tl, 50).to_json(),
            resolve(&cat, &tl, 50).to_json()
        );
    }

    /// A tick nobody authored is an empty picture, not a panic and not a
    /// leftover from the neighbouring segment — decision 2.
    #[test]
    fn a_tick_no_segment_covers_resolves_to_an_honestly_empty_graph() {
        let cat = catalog();
        let mut tl = Timeline::new("t", "t", "s", 200);
        tl.model_track = vec![seg("a", 0, 50, "turing")];
        let g = resolve(&cat, &tl, 120);
        assert!(
            !g.nodes.iter().any(|n| n.kind.starts_with("gen.")),
            "no generator is alive at 120"
        );
        assert!(
            g.nodes.iter().any(|n| n.kind == "export.video"),
            "the export node is always there"
        );
    }

    /// Part III Step 7's acceptance test, both halves: `fixed` becomes a node
    /// carrying that value and no cable, `reactive` becomes the same node with
    /// one.
    #[test]
    fn a_fixed_effect_resolves_to_a_node_holding_the_value_with_no_control_cable() {
        let cat = catalog();
        let mut tl = two_cuts();
        tl.effect_tracks = vec![EffectSegment {
            id: "e".into(),
            on: "a".into(),
            from: 0,
            to: 100,
            effect: "vignette".into(),
            state: metal_schema::EffectState::Fixed(0.7),
            from_row: false,
        }];
        let g = resolve(&cat, &tl, 50);
        let fx = g
            .nodes
            .iter()
            .find(|n| n.kind == "fx.vignette")
            .expect("the effect node exists");
        assert!(
            !g.edges
                .iter()
                .any(|e| e.to.node == fx.id && e.to.port == "control"),
            "fixed means no control cable at all — that is how the graph stores it"
        );
        let cfg: metal_graph::effects::FxConfig =
            serde_json::from_str(&fx.config.to_json_str()).expect("fx config parses");
        assert_eq!(cfg.fixed, 0.7);
    }

    #[test]
    fn a_reactive_effect_reads_the_row_or_the_raw_ingest_depending_on_the_axis() {
        let cat = catalog();
        let mut tl = two_cuts();
        tl.effect_tracks = vec![
            EffectSegment {
                id: "row".into(),
                on: "a".into(),
                from: 0,
                to: 100,
                effect: "vignette".into(),
                state: metal_schema::EffectState::Reactive,
                from_row: true,
            },
            EffectSegment {
                id: "raw".into(),
                on: "a".into(),
                from: 0,
                to: 100,
                effect: "grain".into(),
                state: metal_schema::EffectState::Reactive,
                from_row: false,
            },
        ];
        let g = resolve(&cat, &tl, 50);
        let vig = g
            .nodes
            .iter()
            .find(|n| n.kind == "fx.vignette")
            .expect("vignette");
        let grain = g
            .nodes
            .iter()
            .find(|n| n.kind == "fx.grain")
            .expect("grain");
        assert_eq!(
            metal_graph::effects::control_source(&g, &vig.id),
            Some(metal_graph::effects::ControlSource::Row),
            "from_row must wire to the generator's own row port"
        );
        assert_eq!(
            metal_graph::effects::control_source(&g, &grain.id),
            Some(metal_graph::effects::ControlSource::RawIngest),
            "otherwise straight off the audio node, untranslated"
        );
    }

    /// The point of the whole effects half: the author's order is the render
    /// order, read back off the resolved graph by the same walker the live
    /// path uses.
    #[test]
    fn the_order_effects_are_authored_in_is_the_order_the_chain_reads_back() {
        let cat = catalog();
        let mut tl = two_cuts();
        let mk = |id: &str, effect: &str| EffectSegment {
            id: id.into(),
            on: "a".into(),
            from: 0,
            to: 100,
            effect: effect.into(),
            state: metal_schema::EffectState::Fixed(0.5),
            from_row: false,
        };
        tl.effect_tracks = vec![mk("one", "zoom"), mk("two", "vignette")];
        let g = resolve(&cat, &tl, 50);
        let gen = g
            .nodes
            .iter()
            .find(|n| n.kind == "gen.turing")
            .expect("turing");
        let chain: Vec<String> = metal_graph::effects::chain(&g, &gen.id)
            .iter()
            .filter_map(|id| g.node(id).map(|n| n.kind.clone()))
            .collect();
        assert_eq!(
            chain,
            vec!["fx.zoom".to_string(), "fx.vignette".to_string()]
        );

        // And the reverse authoring order reverses the chain — the property
        // that makes "vignette then zoom" a different picture from "zoom then
        // vignette".
        tl.effect_tracks.reverse();
        let g = resolve(&cat, &tl, 50);
        let gen = g
            .nodes
            .iter()
            .find(|n| n.kind == "gen.turing")
            .expect("turing");
        let chain: Vec<String> = metal_graph::effects::chain(&g, &gen.id)
            .iter()
            .filter_map(|id| g.node(id).map(|n| n.kind.clone()))
            .collect();
        assert_eq!(
            chain,
            vec!["fx.vignette".to_string(), "fx.zoom".to_string()]
        );
    }

    /// Two generators alive at once — the shape decision 1 keeps possible.
    #[test]
    fn two_layers_alive_at_the_same_tick_both_reach_the_export() {
        let cat = catalog();
        let mut tl = Timeline::new("t", "t", "s", 200);
        let mut b = seg("b", 0, 100, "room");
        b.layer = 1;
        tl.model_track = vec![seg("a", 0, 100, "turing"), b];
        let g = resolve(&cat, &tl, 50);
        let export = g
            .nodes
            .iter()
            .find(|n| n.kind == "export.video")
            .expect("export");
        let into_export = g.edges.iter().filter(|e| e.to.node == export.id).count();
        assert_eq!(into_export, 2, "both layers feed the export");
    }

    #[test]
    fn a_deleted_timeline_is_gone_and_deleting_it_twice_is_an_error_not_a_panic() {
        let root = scratch("delete");
        save(&root, two_cuts(), &catalog()).expect("save");
        remove(&root, "show").expect("first delete");
        assert!(load(&root, "show").is_none());
        assert!(
            remove(&root, "show").is_err(),
            "the second delete has nothing to delete"
        );
    }

    #[test]
    fn an_id_that_would_escape_the_directory_is_refused() {
        assert!(safe_id("../../etc/passwd").is_err());
        assert!(safe_id("").is_err());
        assert!(safe_id(".hidden").is_err());
        assert!(safe_id("show-1").is_ok());
    }

    #[test]
    fn listing_a_directory_that_does_not_exist_is_empty_not_an_error() {
        assert!(list(&scratch("nothing")).is_empty());
    }

    // -- export: the per-segment override axis ------------------------------
    // Phase E3: `Timeline::export` now tries `example::load(..., Some((tl.id,
    // seg.id)))` before falling back to the shared corpus — the same
    // `(device, layer)` axis `SUITE` already addresses, just spelled with a
    // timeline/segment pair instead of a device/layer one. See the module doc
    // above `export` for the trade this makes against the old per-model
    // cache.

    // A tiny authoring manifest, just wide enough for these tests — real
    // widths and blocks do not matter here, only that `turing`/`matrix` (the
    // model names `seg`/`two_cuts` already use) exist and agree on a width
    // with the `Example`s saved against them.
    fn export_manifest() -> super::super::schema::Manifest {
        let text = r#"{"rate":8.0,"models":[
            {"id":"turing","label":"Turing","blocks":[
                {"type":"floats","fields":[{"name":"a","min":0.0,"max":1.0,"default":0.0}]}
            ]},
            {"id":"room","label":"Room","blocks":[
                {"type":"floats","fields":[{"name":"a","min":0.0,"max":1.0,"default":0.0}]}
            ]}
        ]}"#;
        serde_json::from_str(text).expect("export_manifest fixture parses")
    }

    // One authored tick (`tick 0`, held flat) is all these tests need: the
    // point is *which file* `export` reads from, not the interpolation
    // itself — `dense::interpolate`'s own tests already cover easing.
    fn save_one_tick(
        root: &Path,
        model: &str,
        song: &str,
        target: Option<(&str, &str)>,
        value: f32,
    ) {
        let manifest = export_manifest();
        let schema = manifest
            .model(model)
            .expect("model in export_manifest fixture");
        let ex = super::super::example::Example {
            model: model.into(),
            song: song.into(),
            rate: DEFAULT_RATE,
            ticks: vec![super::super::example::Tick {
                tick: 0,
                floats: vec![value],
            }],
        };
        super::super::example::save(root, ex, schema, target).expect("save fixture example");
    }

    /// The regression test the task brief asks for by name: a `Timeline`
    /// whose segments have never had a `(tl.id, seg.id)` override authored
    /// must export byte-identical dense rows to before this axis existed —
    /// the override existing is not supposed to change anything for a show
    /// that never opted into it. With no override file on disk,
    /// `example::load`'s own fallback means every segment reads exactly the
    /// shared `(model, song)` corpus it always has.
    #[test]
    fn export_with_no_segment_overrides_reads_the_shared_corpus_exactly_as_before() {
        let root = scratch("export-no-override");
        save_one_tick(&root, "turing", "click-100", None, 0.25);
        save_one_tick(&root, "room", "click-100", None, 0.75);

        let mut tl = Timeline::new("show", "A show", "click-100", 20);
        tl.model_track = vec![seg("a", 0, 10, "turing"), seg("b", 10, 20, "room")];

        let manifest = export_manifest();
        let doc = export(&root, &tl, &manifest).expect("export");
        let frames = doc["frames"].as_array().expect("frames");
        assert_eq!(frames.len(), 20);

        for tick in 0..10 {
            assert_eq!(
                frames[tick]["models"][0]["floats"],
                serde_json::json!([0.25]),
                "tick {tick}: turing's shared corpus, held flat from its one authored tick"
            );
        }
        for tick in 10..20 {
            assert_eq!(
                frames[tick]["models"][0]["floats"],
                serde_json::json!([0.75]),
                "tick {tick}: matrix's shared corpus"
            );
        }
    }

    /// The other half: a segment with its own override exports that
    /// override's values, not the shared corpus's — and a sibling segment on
    /// the same model with no override of its own still gets the shared
    /// corpus, untouched by its neighbour's override. Two segments on one
    /// model used to be guaranteed the same row (one cache entry keyed by
    /// model name); this is the test that would fail if that sharing ever
    /// crept back in.
    #[test]
    fn a_segment_with_its_own_override_exports_that_not_the_shared_corpus() {
        let root = scratch("export-override");
        save_one_tick(&root, "turing", "click-100", None, 0.1); // the shared corpus

        let mut tl = Timeline::new("show", "A show", "click-100", 20);
        tl.model_track = vec![seg("a", 0, 10, "turing"), seg("b", 10, 20, "turing")];
        // Only "a" gets an override, addressed by (timeline id, segment id) —
        // exactly the shape `example::load`'s `target` already takes.
        save_one_tick(&root, "turing", "click-100", Some(("show", "a")), 0.9);

        let manifest = export_manifest();
        let doc = export(&root, &tl, &manifest).expect("export");
        let frames = doc["frames"].as_array().expect("frames");

        assert_float_eq(
            &frames[5]["models"][0]["floats"],
            0.9,
            "segment \"a\" reads its own override",
        );
        assert_float_eq(
            &frames[15]["models"][0]["floats"],
            0.1,
            "segment \"b\" has no override of its own — falls back to the shared corpus",
        );
    }

    /// The windowing question the brief raised, and what answering it
    /// honestly actually found: a keyframe placed outside a segment's own
    /// `[from, to)`, *in that segment's own override file*, is not harmless
    /// by default — `dense::interpolate` eases towards whichever authored
    /// tick comes after the one it is resolving, with no notion of where a
    /// segment ends, so an override keyframe at tick 15 already bends the
    /// curve at tick 9 before this test ever runs. Confirmed the hard way:
    /// this test originally asserted the opposite and failed against the
    /// real interpolator, at `0.459…` instead of the `0.2` it expected. The
    /// fix is `export`'s own `ex.ticks.retain(...)` — see the module-level
    /// comment above the loop that builds `dense_by_segment` for why that
    /// applies to an override and not to the shared corpus.
    #[test]
    fn a_keyframe_outside_a_segments_own_override_span_is_never_read_back() {
        let root = scratch("export-outside-span");
        let manifest = export_manifest();
        let schema = manifest.model("turing").expect("turing in fixture");
        // In this segment's own override — ticks 10..20 are outside where
        // segment "a" (0..10) is ever live.
        let ex = super::super::example::Example {
            model: "turing".into(),
            song: "click-100".into(),
            rate: DEFAULT_RATE,
            ticks: vec![
                super::super::example::Tick {
                    tick: 0,
                    floats: vec![0.2],
                },
                super::super::example::Tick {
                    tick: 15,
                    floats: vec![0.6],
                },
            ],
        };
        super::super::example::save(&root, ex, schema, Some(("show", "a"))).expect("save override");

        let mut tl = Timeline::new("show", "A show", "click-100", 10); // ends before tick 15
        tl.model_track = vec![seg("a", 0, 10, "turing")];

        let doc = export(&root, &tl, &manifest).expect("export");
        let frames = doc["frames"].as_array().expect("frames");
        assert_eq!(
            frames.len(),
            10,
            "the timeline's own length, not the example's furthest tick"
        );
        // Held flat from the one authored tick actually inside [0, 10) — not
        // eased towards the tick-15 anchor, because that anchor was outside
        // this override's own span and `export` windows an override to it.
        assert_float_eq(
            &frames[9]["models"][0]["floats"],
            0.2,
            "held flat from the one in-span tick",
        );
    }

    /// The other half of the asymmetry the sibling test above states but does
    /// not itself exercise: the *shared* corpus is not windowed to any one
    /// segment's span, on purpose, because "shared" already meant "one
    /// continuous automation across the document, sampled by whichever
    /// segment is live" before overrides existed — narrowing that now would
    /// be changing long-standing behaviour for every show that never opted
    /// into a per-segment override at all. Two segments on the same model,
    /// each shorter than the gap between two authored shared ticks, already
    /// bend each other's edges through exactly this mechanism, and always
    /// have. This test is what would fail if a future change tried to make
    /// the shared corpus symmetric with the override case "for consistency".
    #[test]
    fn the_shared_corpus_is_not_windowed_to_a_segments_span_unlike_an_override() {
        let root = scratch("export-shared-not-windowed");
        save_one_tick(&root, "turing", "click-100", None, 0.2); // tick 0, shared
        let manifest = export_manifest();
        let schema = manifest.model("turing").expect("turing in fixture");
        // A second authored tick in the *shared* file, at 15 — outside
        // segment "a" below, same shape as the override test above.
        let mut shared = super::super::example::load(&root, "turing", "click-100", None)
            .expect("just saved the first tick");
        shared.ticks.push(super::super::example::Tick {
            tick: 15,
            floats: vec![0.6],
        });
        super::super::example::save(&root, shared, schema, None).expect("extend shared corpus");

        let mut tl = Timeline::new("show", "A show", "click-100", 10);
        tl.model_track = vec![seg("a", 0, 10, "turing")]; // no override of its own

        let doc = export(&root, &tl, &manifest).expect("export");
        let frames = doc["frames"].as_array().expect("frames");
        // Eased towards the tick-15 shared anchor, unlike the windowed
        // override case — this is the documented asymmetry, not a bug.
        assert_float_eq(
            &frames[9]["models"][0]["floats"],
            0.459_200_02,
            "the shared corpus eases toward an anchor outside this segment's own span",
        );
    }

    // -----------------------------------------------------------------
    // O.3: composition in `resolve`
    // -----------------------------------------------------------------

    fn clip(id: &str, from: usize, to: usize, name: &str) -> MediaSegment {
        MediaSegment {
            id: id.into(),
            from,
            to,
            library: "reel".into(),
            clip: name.into(),
            passage: None,
            speed: 0.0,
            reverse: false,
            layer: 0,
            mix: None,
            digest: None,
        }
    }

    fn mix(mask: Option<&str>, blend: Blend, opacity: f32) -> SegmentMix {
        SegmentMix {
            mask: mask.map(str::to_string),
            blend,
            opacity,
        }
    }

    // -----------------------------------------------------------------
    // S.L: a clip's own digest, and only within its own span
    // -----------------------------------------------------------------

    #[test]
    fn no_clip_level_digest_resolves_byte_for_byte_the_graph_it_always_did() {
        // Every timeline written before S.L has `digest: None` on every
        // clip. The whole point of the field being optional is that such a
        // timeline must not change at all — this is that promise, checked
        // against the exact JSON `resolve` produces.
        let cat = catalog();
        let mut tl = two_cuts();
        tl.media_track = vec![clip("m1", 0, 100, "loop.mp4")];
        let before = resolve(&cat, &tl, 50).to_json();

        // Touching the field and putting it back to `None` must be a no-op —
        // this is not testing "never call attach_digest", it is testing that
        // the absence of a digest is what decides, not the field merely
        // existing.
        tl.media_track[0].digest = None;
        assert_eq!(resolve(&cat, &tl, 50).to_json(), before);
    }

    #[test]
    fn a_clips_own_digest_overrides_the_audio_node_only_inside_its_own_span() {
        let cat = catalog();
        let mut tl = two_cuts();
        tl.media_track = vec![
            MediaSegment {
                digest: Some("take-one".into()),
                ..clip("m1", 0, 50, "loop.mp4")
            },
            MediaSegment {
                digest: None,
                ..clip("m2", 50, 100, "other.mp4")
            },
        ];

        // Inside the digest-bearing clip's own span: the audio node carries
        // the override, offset from the clip's own `from`.
        let inside = resolve(&cat, &tl, 20);
        let audio = inside
            .nodes
            .iter()
            .find(|n| n.kind == "audio.file")
            .expect("the audio node");
        let cfg: serde_json::Value = serde_json::from_str(&audio.config.to_json_str()).unwrap();
        assert_eq!(cfg["digest"], "take-one");
        assert_eq!(cfg["offset"], 20);

        // The neighbouring segment's tick did not move: no digest of its own,
        // so the audio node is exactly what it always was — empty config.
        let neighbour = resolve(&cat, &tl, 70);
        let audio = neighbour
            .nodes
            .iter()
            .find(|n| n.kind == "audio.file")
            .expect("the audio node");
        assert!(
            audio.config.is_empty(),
            "the neighbour's tick must not see the other clip's digest"
        );

        // And a tick with no clip at all is unaffected too — the override is
        // confined to the span of the segment that carries it, not the whole
        // document from the moment one segment gets a digest.
        let mut tl2 = tl.clone();
        tl2.media_track.truncate(1);
        let outside = resolve(&cat, &tl2, 70);
        let audio = outside
            .nodes
            .iter()
            .find(|n| n.kind == "audio.file")
            .expect("the audio node");
        assert!(audio.config.is_empty());
    }

    #[test]
    fn one_live_layer_resolves_byte_for_byte_the_graph_it_always_did() {
        // The identity O.3 turns on: with nothing underneath, there is nothing
        // to compose against, so a mix must not reach the graph even when the
        // author left one on the segment.
        let cat = catalog();
        let mut tl = two_cuts();
        let plain = resolve(&cat, &tl, 50).to_json();

        tl.model_track[0].mix = Some(mix(Some("pillar"), Blend::Add, 0.5));
        assert_eq!(
            resolve(&cat, &tl, 50).to_json(),
            plain,
            "a lone layer has nothing below it — the mix is not information yet"
        );
    }

    #[test]
    fn two_live_layers_carry_their_mask_and_blend_into_the_graph() {
        let cat = catalog();
        let mut tl = two_cuts();
        // Both alive at tick 50, on two layers.
        tl.model_track = vec![
            ModelSegment {
                layer: 0,
                ..seg("under", 0, 100, "turing")
            },
            ModelSegment {
                layer: 1,
                mix: Some(mix(Some("door"), Blend::Screen, 0.75)),
                ..seg("over", 0, 100, "room")
            },
        ];

        let g = resolve(&cat, &tl, 50);
        let over = g
            .nodes
            .iter()
            .find(|n| n.kind == "gen.room")
            .expect("the top layer");
        let cfg = crate::node_layer::NodeLayerConfig::from_node(over);
        assert_eq!(cfg.blend, Some(Blend::Screen));
        assert_eq!(cfg.mask.as_deref(), Some("door"));
        assert_eq!(cfg.opacity, Some(0.75));

        // The layer with no mix of its own stays empty rather than being
        // filled in with defaults — "nothing said" and "said normal at 1.0"
        // have to stay distinguishable, or a later default change would
        // silently rewrite every old document.
        let under = g
            .nodes
            .iter()
            .find(|n| n.kind == "gen.turing")
            .expect("the bottom layer");
        assert!(crate::node_layer::NodeLayerConfig::from_node(under).is_empty());
    }

    #[test]
    fn a_model_and_a_clip_compose_by_the_same_rule_as_two_models() {
        // H.10 names three pairings and this is the third. The point of the
        // assertion is that `reel.clip` goes through the *same* config shape,
        // not a second mechanism for footage.
        let cat = catalog();
        let mut tl = two_cuts();
        tl.model_track = vec![seg("a", 0, 100, "turing")];
        tl.media_track = vec![MediaSegment {
            layer: 1,
            mix: Some(mix(None, Blend::Add, 0.5)),
            ..clip("m1", 0, 100, "loop.mp4")
        }];

        let g = resolve(&cat, &tl, 50);
        let reel = g
            .nodes
            .iter()
            .find(|n| n.kind == "reel.clip")
            .expect("the clip");
        let cfg = crate::node_layer::NodeLayerConfig::from_node(reel);
        assert_eq!(cfg.blend, Some(Blend::Add));
        assert_eq!(cfg.opacity, Some(0.5));
        // And it did not lose which clip it is — the mix merges into the
        // node's own config, it does not replace it.
        assert!(reel.config.to_json_str().contains("loop.mp4"));
    }

    #[test]
    fn the_timeline_and_the_live_path_produce_the_same_layer_from_the_same_mix() {
        // The test that says O.3 is *one* rule and not two implementations.
        // A `SegmentMix` written by the timeline and a `NodeLayerConfig`
        // written by the canvas have to land on identical `Layer` fields,
        // because a device with two sources and a timeline with two layers
        // are the same composition seen from two places in the graph.
        let m = mix(Some("pillar"), Blend::Multiply, 0.25);

        let mut from_timeline = crate::layers::Layer::new("l", "L", 0);
        m.to_node_config().apply_to(&mut from_timeline);

        let mut from_canvas = crate::layers::Layer::new("l", "L", 0);
        crate::node_layer::NodeLayerConfig {
            blend: Some(Blend::Multiply),
            mask: Some("pillar".into()),
            opacity: Some(0.25),
            z: None,
            sector: None,
            shader: None,
        }
        .apply_to(&mut from_canvas);

        assert_eq!(from_timeline.blend, from_canvas.blend);
        assert_eq!(from_timeline.mask, from_canvas.mask);
        assert_eq!(from_timeline.opacity, from_canvas.opacity);
    }

    #[test]
    fn stack_order_is_back_to_front_and_layer_beats_track() {
        let mut tl = two_cuts();
        tl.model_track = vec![
            ModelSegment {
                layer: 2,
                ..seg("top", 0, 100, "turing")
            },
            ModelSegment {
                layer: 0,
                ..seg("bottom", 0, 100, "room")
            },
        ];
        tl.media_track = vec![MediaSegment {
            layer: 1,
            ..clip("mid", 0, 100, "loop.mp4")
        }];

        let stack = stack_order(&tl, 50);
        let order: Vec<&str> = stack.iter().map(|s| s.id()).collect();
        assert_eq!(
            order,
            vec!["bottom", "mid", "top"],
            "a clip can sit between two models"
        );
    }

    #[test]
    fn stack_order_with_no_layers_set_is_the_order_resolve_already_walked() {
        // Every `layer` at its default. Models first, then clips in track
        // order — which is what `resolve` did before O.3 and what keeps an
        // untouched timeline byte-identical.
        let mut tl = two_cuts();
        tl.model_track = vec![seg("b", 0, 100, "room"), seg("a", 0, 100, "turing")];
        tl.media_track = vec![clip("m2", 0, 100, "two.mp4"), clip("m1", 0, 100, "one.mp4")];

        let stack = stack_order(&tl, 50);
        let order: Vec<&str> = stack.iter().map(|s| s.id()).collect();
        // Models sorted by id (as `resolve` sorts them); clips left in track
        // order, not sorted — that difference is deliberate and is what makes
        // the resolved node positions stable.
        assert_eq!(order, vec!["a", "b", "m2", "m1"]);
    }

    #[test]
    fn a_mix_at_an_impossible_opacity_is_refused_where_it_is_typed() {
        let cat = catalog();
        let mut tl = two_cuts();
        tl.model_track[0].mix = Some(mix(None, Blend::Normal, 1.5));
        let err = tl.validate(&cat).unwrap_err();
        assert!(err.contains("opacity"), "{err}");

        tl.model_track[0].mix = Some(mix(Some("  "), Blend::Normal, 1.0));
        let err = tl.validate(&cat).unwrap_err();
        assert!(err.contains("empty contour"), "{err}");
    }

    #[test]
    fn a_timeline_written_before_o3_still_deserialises_with_no_mix_and_layer_zero() {
        let json = r#"{
            "id": "old", "name": "Old", "song": "click-100", "rate": 8.0,
            "length_ticks": 200,
            "model_track": [{"id":"a","from":0,"to":100,"model":"turing"}],
            "effect_tracks": [],
            "media_track": [{"id":"m","from":0,"to":100,"library":"reel","clip":"x.mp4"}],
            "still_track": [], "text_track": [], "signal_track": []
        }"#;
        let tl: Timeline = serde_json::from_str(json).expect("an old document still parses");
        assert_eq!(tl.model_track[0].mix, None);
        assert_eq!(tl.media_track[0].mix, None);
        assert_eq!(tl.media_track[0].layer, 0);
    }

    // -- colour clips -----------------------------------------------------------

    #[test]
    fn a_colour_clip_round_trips_and_resolves_to_a_flat_solid_source() {
        let root = scratch("color-clip");
        let mut tl = two_cuts();
        tl.color_track = vec![ColorSegment {
            id: "c1".into(),
            from: 20,
            to: 60,
            rgb: [250, 10, 10],
            masks: vec![("bar.panel-left".into(), [0, 0, 255])],
        }];
        let cat = catalog();
        save(&root, tl.clone(), &cat).expect("save");
        let back = load(&root, "show").expect("load");
        assert_eq!(back.color_track, tl.color_track, "rgb and per-mask colours survive");

        let solids = |tick: usize| {
            resolve(&cat, &back, tick)
                .nodes
                .iter()
                .filter(|n| n.kind == metal_graph::SOLID_KIND)
                .map(|n| n.config.to_json_str())
                .collect::<Vec<_>>()
        };
        assert!(solids(10).is_empty(), "before the clip: nothing");
        let during = solids(30);
        assert_eq!(during.len(), 1);
        assert!(during[0].contains("250"), "{during:?}");
        assert!(solids(60).is_empty(), "the clip ends at `to`");

        let mut bad = tl.clone();
        bad.color_track[0].to = 9999;
        assert!(bad.validate(&cat).is_err(), "a clip past the timeline is refused");
        let _ = metal_vfs::remove_dir_all(&root);
    }
}
