//! Digest row → model float row.
//!
//! This is the mapping the handoff calls the contract between the input and
//! the outputs: *"adding a new model is writing a new mapping, not
//! re-analysing the music."* One function does it for every model, driven by
//! the schema each model already publishes, so a model added tomorrow is
//! driven the day it is added without anything being written here.
//!
//! ## The three block kinds are treated differently, on purpose
//!
//! - **Floats** are driven. Each is assigned a digest column and is modulated
//!   *around the value the operator set*, by a fraction of its declared range
//!   ([`SWING`], scaled by the float's own `drive` weight) — and clamped to
//!   that range, so a driven model can never reach a state a slider could not.
//! - **Colour slots** are driven from chroma, because harmony changing colour
//!   is in the handoff and chroma is the twelve columns that carry harmony.
//! - **Cameras are never driven.** They stay at the model's declared default.
//!   This is not caution, it is a recorded bug: one shared camera default had
//!   three of the five 3D models opening edge-on or off-frame
//!   (`docs/CORRECTIONS.md` §43). A camera moved by a random-ish signal would
//!   reproduce that failure continuously, and every model would be blamed for
//!   it except the mapping.

use metal_notation::color::{to_srgb, Oklch};
use metal_notation::tone::{self, tone_gradient};
use metal_schema::{Block, Schema};

use crate::clock::Position;
use crate::digest::{Frame, FEATURES};
use crate::pattern;

/// Which colours a driven model is allowed to reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    /// Greys and reds, keyed to the MetalVJ accent. The default, because
    /// it is what the surface around it is.
    MetalVJ,
    /// The full hue wheel, pitch class to hue. Louder and less coherent with
    /// the app around it, but it is what "harmony drives colour" means
    /// literally, and some rooms want it.
    Spectrum,
    /// The model's own hue. Lightness and chroma follow the music exactly as
    /// they do for [`Palette::MetalVJ`], but each slot keeps the hue of the
    /// colour the operator set — so every model keeps its own look, where
    /// `MetalVJ` repaints them all in the accent red. A slot still at white
    /// (no tint) has no hue to keep and falls back to the `MetalVJ` colour.
    Hold,
}

/// How the digest is allowed to move a model.
#[derive(Clone, Copy, Debug)]
pub struct Drive {
    /// 0..1 — how far the music moves a float from where the operator set it.
    /// At 0 the row is exactly what was set and the digest has no effect,
    /// which is the honest way to see what a model does at rest. At 1 each
    /// float swings by [`SWING`] of its declared range, less if the model
    /// marked it structural.
    pub depth: f32,
    pub palette: Palette,
    /// Whether colour slots follow harmony at all. Off leaves them white,
    /// which is what the renderers treat as "no tint".
    pub colour: bool,
    /// Which analysis drives the colour slots — tone, chroma, or the `Auto`
    /// crossfade between them (the tone design notes D8). Only consulted by the
    /// `ColorSlot` branch when [`Drive::colour`] is on.
    pub colour_source: ColourSource,
    /// 0..1 — a global scale on every float's own smoothing, so the suite can
    /// be seen stepped against interpolated without editing models.
    ///
    /// At 0 every float lands on the digest row exactly as it steps, which is
    /// what the app did before smoothing existed and is still the honest way
    /// to see what the 8 Hz grid is actually doing. At 1 each float smooths as
    /// much as it declared. It only ever scales *down*: a float that declared
    /// itself instant stays instant at any setting, because the reason it is
    /// instant is that lag would break it.
    pub smooth: f32,
}

impl Default for Drive {
    fn default() -> Drive {
        Drive {
            depth: 0.65,
            palette: Palette::MetalVJ,
            colour: true,
            colour_source: ColourSource::Auto,
            smooth: 1.0,
        }
    }
}

/// The config key a source node (`gen.*`, `text.source`) keeps its dials
/// under — the `reaction` section of its properties (plan "Audio input is a
/// node", Design 4). Written by `POST /graph/node {op:"config"}` and the AI's
/// `set_params`; there is no global copy (rule 4).
pub const CONFIG_KEY: &str = "drive";

impl Drive {
    /// The dials a source node's config holds, each missing one at its
    /// default — so a node saved before the dials moved reacts as before.
    pub fn from_config(cfg: &metal_graph::Config) -> Drive {
        let mut d = Drive::default();
        let Some(v) = cfg.get(CONFIG_KEY) else { return d };
        if let Some(x) = v.get("depth").and_then(serde_json::Value::as_f64) {
            d.depth = (x as f32).clamp(0.0, 1.0);
        }
        if let Some(x) = v.get("smooth").and_then(serde_json::Value::as_f64) {
            d.smooth = (x as f32).clamp(0.0, 1.0);
        }
        if let Some(x) = v.get("colour").and_then(serde_json::Value::as_bool) {
            d.colour = x;
        }
        if let Some(x) = v.get("palette").and_then(serde_json::Value::as_str) {
            d.palette = Palette::parse(x);
        }
        // Absent means the default `Auto`, so a node saved before tone existed
        // keeps the material-dependent behaviour it had (D8).
        if let Some(x) = v.get("colour_source").and_then(serde_json::Value::as_str) {
            d.colour_source = ColourSource::parse(x);
        }
        d
    }

    /// [`Drive::from_config`] of an optional node: a source with no node
    /// (a pool layer) reacts with the defaults.
    pub fn of_node(node: Option<&metal_graph::Node>) -> Drive {
        node.map(|n| Drive::from_config(&n.config)).unwrap_or_default()
    }

    /// The dials as the JSON object [`Drive::from_config`] reads.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "depth": self.depth,
            "smooth": self.smooth,
            "colour": self.colour,
            "palette": self.palette.as_str(),
            "colour_source": self.colour_source.as_str(),
        })
    }

    /// The dials of a source with nothing audio wired to it: depth 0 and no
    /// harmony, so the row is the operator's base row ("no audio").
    pub fn at_rest(self) -> Drive {
        Drive { depth: 0.0, colour: false, ..self }
    }
}

impl Palette {
    /// `"spectrum"` and `"hold"` name themselves, anything else the house palette.
    pub fn parse(s: &str) -> Palette {
        match s {
            "spectrum" => Palette::Spectrum,
            "hold" => Palette::Hold,
            _ => Palette::MetalVJ,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Palette::MetalVJ => "metalvj",
            Palette::Spectrum => "spectrum",
            Palette::Hold => "hold",
        }
    }
}

/// Which analysis drives a model's colour slots — the tone design notes D8.
///
/// The tone block is Basic Pitch's harmony summary; chroma is the FFT
/// pitch-class vector that has always driven colour. Tone is the better signal
/// (polyphonic, overtone-separated, instrument-agnostic) but it only exists
/// when a model ran and pitched content was heard, so the *default* is a
/// crossfade rather than a switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColourSource {
    /// Follow tone when `tone_conf` is high, chroma when it is not — the
    /// material-dependent blend of D8. This is the default so a room with the
    /// model loaded gets the gradient and an old song or a drum break still
    /// behaves exactly as it did before tone existed.
    #[default]
    Auto,
    /// Always the tone gradient, whatever `tone_conf` says. For an operator who
    /// has a model and wants the harmony mapping unconditionally.
    Tone,
    /// Always chroma, whatever the tone block says. The pre-tone behaviour,
    /// pinned.
    Chroma,
}

impl ColourSource {
    /// `"tone"` and `"chroma"` name themselves; anything else is [`Auto`] —
    /// the same forgiving parse `Palette::parse` uses, so a config saved with a
    /// spelling this build does not know falls back to the default rather than
    /// to a missing branch.
    ///
    /// [`Auto`]: ColourSource::Auto
    pub fn parse(s: &str) -> ColourSource {
        match s {
            "tone" => ColourSource::Tone,
            "chroma" => ColourSource::Chroma,
            _ => ColourSource::Auto,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ColourSource::Auto => "auto",
            ColourSource::Tone => "tone",
            ColourSource::Chroma => "chroma",
        }
    }
}

/// How much of a float's declared range the digest may swing it across at
/// `depth` 1.0, as a fraction.
///
/// **Not 1.0, and this is the whole lesson of the module.** The first version
/// mapped a feature straight onto `min..max`, so a fully driven float swept
/// its entire declared range. That is defensible on paper and wrong in
/// practice: an author picks a model's defaults because that is where the
/// model *works*, and the declared range is how far the parameter can legally
/// go, not how far it should go while music is playing.
///
/// Turing is the proof. Its Gray-Scott feed and kill rates produce a labyrinth
/// only in a narrow band around their defaults; swept across the declared
/// range the reaction leaves the pattern-forming regime altogether and the
/// mesh renders as a flat, blank sheet. Nothing errors. The model simply stops
/// being the model. Several others degrade the same way less obviously.
///
/// So driving is a modulation *around* the operator's row, not a sweep of the
/// parameter space, and `depth` means "how far the music moves this from where
/// you set it".
const SWING: f32 = 0.6;

/// The value a digest column sits at when nothing is happening, so an
/// excursion can be measured from rest rather than from the middle.
///
/// Onset rests at zero — it is a transient, silent between hits — so it must
/// push a float *away* from its resting value and let it fall back. The
/// continuous columns hover around the middle of their range, so they swing
/// both ways. Treating both alike would park every onset-driven float below
/// its default and make the hit look like a return to normal rather than an
/// event.
fn rest_value(column: usize) -> f32 {
    if column == 2 {
        0.0
    } else {
        0.5
    }
}

/// How much a float rides the interpolated row rather than the stepped one,
/// when its author has not said — see `FloatSpec::smooth`.
///
/// **Derived from the driving column, because the column already is the
/// distinction.** Onset is a transient: it rests at zero, spikes at the hit
/// and falls back, and it is pooled *by max* upstream specifically so the
/// spike survives. Interpolating it would move the peak to the end of the
/// 125 ms window — a shake or a light leak landing an eighth-note behind the
/// hit, which reads as a mistake rather than as a softer hit. So onset lands
/// raw.
///
/// The other three — rms, bandwidth, centroid — are continuous envelopes that
/// hover around the middle of their range. They step at 8 Hz like everything
/// else, and that step is exactly the staircase an operator sees on a slow
/// colour drift. 125 ms of lag on a colour is invisible; the staircase is not.
/// So they interpolate.
///
/// This is the default and not a rule. A float whose meaning disagrees with
/// the column it happened to be assigned says so in its own spec.
pub fn smooth_default(column: usize) -> f32 {
    // Onset and `hiss` are transients — onset by its window max, hiss by the
    // same pooling rule. Interpolating either throws the spike away.
    if column == 2 || column == crate::digest::HISS {
        0.0
    } else {
        1.0
    }
}

/// Which digest column drives the float at position `i` of a model's own
/// floats.
///
/// Round-robin over the first four columns — the ones that carry loudness,
/// brightness and transients — rather than over all sixteen, because twelve
/// of the sixteen are chroma and a geometry parameter driven by "how much D#
/// is in this chord" is noise wearing a feature's name. Chroma reaches the
/// visuals through colour, which is where harmony belongs.
fn column_for(i: usize) -> usize {
    // 0 rms, 1 bandwidth, 2 onset, 3 centroid — and onset twice as often as
    // the rest, because a transient is the one thing an audience actually
    // sees a visual respond to.
    const ORDER: [usize; 6] = [0, 2, 1, 2, 3, 2];
    ORDER[i % ORDER.len()]
}

/// The digest row as anything reading all of it at once should see: each
/// column blended between the stepped and the interpolated row by its own
/// `smooth_default`, scaled by the operator's global `smooth`.
///
/// **The Colourist is why this exists** (`metal_reel::grade`). A float driven by
/// exactly one column can say for itself which of the two rows it rides —
/// `FloatSpec::smooth`. A model whose floats come from a mapping over *all
/// sixteen music columns* cannot: there is no single column whose staircase it
/// could opt out of. So the policy stays here, in one place, and the whole row
/// (voice included) is blended once by the same rule.
///
/// It is not a loss in practice: `smooth_default` already lands onset and hiss
/// raw and the continuous columns interpolated, which is the distinction the
/// per-float override exists to correct *exceptions* to.
pub fn blended(frame: &Frame, smooth: f32) -> [f32; FEATURES] {
    let mut out = [0.0f32; FEATURES];
    for c in 0..FEATURES {
        let s = smooth_default(c).clamp(0.0, 1.0) * smooth.clamp(0.0, 1.0);
        // The fraction is a per-tick decay: at base it is unchanged, at fine it
        // converts so the same wall-clock smoothing survives the level switch.
        let s = metal_schema::per_tick_decay(s, metal_schema::TickRate::Base, frame.rate);
        out[c] = frame.raw[c] + (frame.f[c] - frame.raw[c]) * s;
    }
    out
}

/// The floats a model should be rendered with for this digest row.
///
/// `base` is the row the operator has set by hand — a preset, or the model's
/// defaults. Driving moves floats away from `base` toward what the digest
/// asks for, so turning `depth` down always returns to something the operator
/// chose rather than to an arbitrary middle.
pub fn row(schema: &Schema, base: &[f32], frame: &Frame, drive: &Drive) -> Vec<f32> {
    row_with(schema, base, frame, drive, None)
}

/// [`row`], with the round-robin column assignment replaced.
///
/// # The one seam a transmission needed, and everything it did not touch
///
/// `graded` is one signed value per **own float**, in −1..1, from
/// `metal_reel::grade::Colourist` — a mapping over the sixteen-column music
/// prefix, learnt or authored, rather than [`column_for`]'s round robin, which
/// assigns
/// each float one column by counting.
///
/// It replaces exactly one term: `feature - rest_value(column)`. Every other
/// rule in this function is untouched, and each of them was a bug once —
/// modulating around the operator's row rather than sweeping the declared
/// range, damping structural floats by their own `drive` weight, clamping to
/// the range, never touching a camera. The offsets are clamped to −1..1 at
/// source precisely so that a saturated one moves a float exactly as far as a
/// saturated digest column could and no further; `SWING`'s own gate did not
/// have to change, and it covers this path too.
///
/// **So a Colourist that has learnt something absurd can still only produce a
/// row a slider could have produced.** That is what lets an untrained default
/// ship at all.
pub fn row_with(
    schema: &Schema,
    base: &[f32],
    frame: &Frame,
    drive: &Drive,
    graded: Option<&[f32]>,
) -> Vec<f32> {
    let mut out = base.to_vec();
    // A `base` row saved before the BLE Phase B signal append is five floats
    // short, and because the signal block sits *before* the effects, simply
    // resizing would leave the effects block misplaced rather than merely
    // short. Pad it into the current shape first; a row already current, or
    // one of a width this cannot account for, is left as it was and the
    // resize below keeps today's behaviour.
    metal_schema::migrate_row(&mut out, schema.width());
    out.resize(schema.width(), 0.0);

    let depth = drive.depth.clamp(0.0, 1.0);
    let mut at = 0usize; // where this block starts in the row
    let mut float_index = 0usize; // how many own-floats we have passed

    for block in schema.blocks {
        match block {
            Block::Floats(specs) => {
                // Use explicit effect column map if this is the effects block,
                // otherwise use the round-robin column_for assignment.
                let is_effects = metal_schema::is_effects_block(specs);
                let effect_columns = if is_effects {
                    metal_schema::effect_column_map()
                } else {
                    &[]
                };

                for (k, spec) in specs.iter().enumerate() {
                    let column = if is_effects && k < effect_columns.len() {
                        effect_columns[k]
                    } else {
                        column_for(float_index + k)
                    };
                    let c = column % FEATURES;
                    // Which of the two rows this float rides. The author's
                    // statement if there is one, otherwise the column's own
                    // — scaled by the operator's global `smooth`, so the whole
                    // suite can be compared stepped against interpolated
                    // without editing a single model.
                    let smooth = spec
                        .smooth
                        .unwrap_or_else(|| smooth_default(column))
                        .clamp(0.0, 1.0)
                        * drive.smooth.clamp(0.0, 1.0);
                    let smooth =
                        metal_schema::per_tick_decay(smooth, metal_schema::TickRate::Base, frame.rate);
                    let feature = frame.raw[c] + (frame.f[c] - frame.raw[c]) * smooth;
                    let held = out[at + k];
                    // The Colourist's answer if there is one, otherwise the
                    // round robin's. This is the whole of the seam.
                    let signal = match graded.and_then(|g| g.get(float_index + k)) {
                        Some(v) => v.clamp(-1.0, 1.0),
                        None => feature - rest_value(column),
                    };
                    // `spec.drive` is the model author's own statement about
                    // whether this float is expressive or structural — see
                    // `FloatSpec::drive`. A structural float is damped here
                    // and nowhere else, so a slider is still free.
                    let excursion =
                        signal * (spec.max - spec.min) * SWING * spec.drive.clamp(0.0, 1.0) * depth;
                    // The clamp is what makes the range declaration mean
                    // something: a base row near a rail plus an excursion
                    // toward it must not push past it.
                    out[at + k] = (held + excursion).clamp(spec.min, spec.max);
                }
                float_index += specs.len();
            }
            Block::ColorSlot(..) => {
                if drive.colour {
                    // Which analysis drives the slot is `colour_slots`' job
                    // (the tone design notes D8): tone when confidence is high, chroma
                    // otherwise, crossfaded between.
                    let mut colours = colour_slots(frame, drive);
                    if drive.palette == Palette::Hold {
                        let own: [f32; 12] = out[at..at + 12].try_into().expect("a slot is 12 floats");
                        hold_slot_hues(&mut colours, &own);
                    }
                    out[at..at + 12].copy_from_slice(&colours);
                }
            }
            // Never driven — see the module note.
            Block::Camera(_) => {}
        }
        at += block.width();
    }
    out
}

/// [`Palette::Hold`]: move each of the four slot colours onto the hue of the
/// operator's own colour in that slot, keeping the driven lightness and chroma.
/// A base colour with no chroma (white = "no tint", greys) has no hue and
/// leaves the driven colour as it is.
fn hold_slot_hues(colours: &mut [f32; 12], base: &[f32]) {
    for i in 0..4 {
        let at = i * 3;
        let driven = [colours[at], colours[at + 1], colours[at + 2]];
        let own = [base[at], base[at + 1], base[at + 2]];
        colours[at..at + 3].copy_from_slice(&hold_hue(driven, own));
    }
}

/// The `hold` hue rule, for one colour: the driven colour's lightness and
/// chroma with the operator's own colour's hue — the OkLCh swap the
/// model-level `Palette::Hold` slot mode uses (`hold_slot_hues`),
/// applied here to a single colour so a `venue.fixture` node with
/// `hue_lock = "hold"` keeps the source model's own hue (F4, decision 6). A
/// colour with no chroma (white = "no tint", greys) has no hue and leaves the
/// driven colour as it is.
pub(crate) fn hold_hue(colour: [f32; 3], own: [f32; 3]) -> [f32; 3] {
    use metal_notation::color::{from_srgb01, to_srgb, Oklch};
    let own = from_srgb01(own);
    if own.c < 0.02 {
        return colour;
    }
    let driven = from_srgb01(colour);
    let rgb = to_srgb(Oklch { l: driven.l, c: driven.c, h: own.h });
    [rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0]
}

/// One effect float embedded in a `gen.*` node's config (`config.effects`),
/// with the base value the operator set and its declared range.
///
/// §4 "ai_fx": the single generator carries its effects' parameters itself, so
/// the engine drives them without a separate `fx.*` node. The shape is the one
/// the properties panel writes — an object of `name -> f32`, or an array of
/// `{name, value, min, max}`. An object value takes the effect's own declared
/// range when the name is a common effect ([`metal_schema::EFFECTS_FLOATS`]),
/// otherwise `0..1`.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectFloat {
    pub name: String,
    pub value: f32,
    pub min: f32,
    pub max: f32,
}

/// The effect floats a `gen.*` node's config embeds under `effects`, in a
/// stable order (an object's keys sorted, an array's order kept). Absent,
/// null or malformed is empty.
pub fn embedded_effect_floats(node: Option<&metal_graph::Node>) -> Vec<EffectFloat> {
    let Some(v) = node.and_then(|n| n.config.get("effects")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(map) = v.as_object() {
        for (name, x) in map {
            let Some(value) = x.as_f64() else { continue };
            let (min, max) = match metal_schema::effect_index(name) {
                Some(i) => (metal_schema::EFFECTS_FLOATS[i].min, metal_schema::EFFECTS_FLOATS[i].max),
                None => (0.0, 1.0),
            };
            out.push(EffectFloat { name: name.clone(), value: value as f32, min, max });
        }
    } else if let Some(arr) = v.as_array() {
        for e in arr {
            let Some(name) = e.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let value = e.get("value").and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
            let min = e.get("min").and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
            let max = e.get("max").and_then(serde_json::Value::as_f64).unwrap_or(1.0) as f32;
            out.push(EffectFloat { name: name.to_string(), value, min, max });
        }
    }
    out
}

/// Which digest column drives the embedded effect float at `i`.
///
/// A name that is a common effect takes its own declared column
/// ([`metal_schema::effect_column_map`]); every other name round-robins like a
/// model float ([`column_for`]). Defined for any name, so an effect the schema
/// does not know still moves predictably rather than not at all.
pub fn effect_float_column(name: &str, i: usize) -> usize {
    metal_schema::effect_index(name)
        .and_then(|k| metal_schema::effect_column_map().get(k).copied())
        .unwrap_or_else(|| column_for(i))
}

/// The digest-driven value of one embedded effect float — [`row_with`]'s
/// modulation around the operator's value, by `SWING` of the declared range,
/// damped by `drive.depth`, clamped to the range. `column` is
/// [`effect_float_column`]'s answer.
pub fn drive_effect_float(f: &EffectFloat, column: usize, frame: &Frame, drive: &Drive) -> f32 {
    let c = column % FEATURES;
    let smooth = smooth_default(column).clamp(0.0, 1.0) * drive.smooth.clamp(0.0, 1.0);
    let smooth = metal_schema::per_tick_decay(smooth, metal_schema::TickRate::Base, frame.rate);
    let feature = frame.raw[c] + (frame.f[c] - frame.raw[c]) * smooth;
    let signal = feature - rest_value(column);
    let excursion = signal * (f.max - f.min) * SWING * drive.depth.clamp(0.0, 1.0);
    (f.value + excursion).clamp(f.min, f.max)
}

/// Writes what `l` says at `pos` over an already-driven `row` (the loops design notes
/// T7, decision 6): a [`pattern::TrackTarget::Float`] track sets its float to
/// [`pattern::to_range`] of the step value; a palette track sets its colour
/// slot to the picked colour of its group, followed by the group's other
/// three in order (so the slot stays a gradient of a tested set). Everything
/// the loop does not name keeps the audio-driven value it already had.
/// **Cameras are never touched**, whatever a template names (§43): only
/// `Floats` and `ColorSlot` blocks are searched for targets.
pub fn apply_loop(
    schema: &Schema,
    row: &mut [f32],
    l: &pattern::Loop,
    pos: Position,
    lib: &pattern::Library,
) {
    let values = l.eval(pos);
    if values.is_empty() {
        return;
    }
    let mut at = 0usize;
    for block in schema.blocks {
        match block {
            Block::Floats(specs) => {
                for (k, spec) in specs.iter().enumerate() {
                    let hit = values.iter().find_map(|(t, v)| match (t, v) {
                        (pattern::TrackTarget::Float { param }, pattern::Value::Unit(u))
                            if param == spec.name =>
                        {
                            Some(*u)
                        }
                        _ => None,
                    });
                    if let (Some(u), Some(slot)) = (hit, row.get_mut(at + k)) {
                        *slot = pattern::to_range(u, spec);
                    }
                }
            }
            Block::ColorSlot(name, _) => {
                for (t, v) in &values {
                    let (pattern::TrackTarget::Palette { slot, palette, .. }, pattern::Value::Pick { group, colour }) =
                        (t, *v)
                    else {
                        continue;
                    };
                    if slot != name || lib.colour(t, *v).is_none() {
                        continue;
                    }
                    let Some(set) = lib.palette(palette) else { continue };
                    for i in 0..4u8 {
                        if let Some(rgb) = set.colour(group, (colour + i) % 4) {
                            let o = at + i as usize * 3;
                            if let Some(dst) = row.get_mut(o..o + 3) {
                                dst.copy_from_slice(&rgb);
                            }
                        }
                    }
                }
            }
            Block::Camera(_) => {}
        }
        at += block.width();
    }
}

/// Four colours from the chroma columns: the three loudest pitch classes plus
/// one derived from overall brightness, ordered so the slot reads as a
/// gradient rather than as four unrelated swatches.
///
/// Returned as the 12 floats a `ColorSlot` block wants — 4 colours x RGB.
fn slot(frame: &Frame, palette: Palette, smooth: f32) -> [f32; 12] {
    // Every column this reads, blended the same way a float's is — so the
    // operator's `smooth` at 0 shows the 8 Hz staircase on colour too. A
    // control that quietly exempted the one thing smoothing helps most would
    // be worse than no control.
    let smooth =
        metal_schema::per_tick_decay(smooth.clamp(0.0, 1.0), metal_schema::TickRate::Base, frame.rate);
    let at = |i: usize| frame.raw[i] + (frame.f[i] - frame.raw[i]) * smooth;
    let chroma: Vec<f32> = (4..16).map(at).collect();

    // The three strongest pitch classes, loudest first.
    let mut ranked: Vec<(usize, f32)> = chroma.iter().copied().enumerate().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

    let brightness = at(3);
    let energy = at(0);

    let mut out = [0.0f32; 12];
    for i in 0..4 {
        let (class, weight) = if i < 3 { ranked[i] } else { ranked[0] };
        let colour = match palette {
            Palette::Spectrum => Oklch {
                // Twelve pitch classes around the wheel.
                h: class as f32 * 30.0,
                // Louder pitch class, more saturated.
                c: 0.06 + 0.15 * weight,
                l: 0.35 + 0.45 * (0.35 + 0.65 * energy) * (0.6 + 0.4 * brightness),
            },
            Palette::MetalVJ | Palette::Hold => {
                // One hue family. The pitch class moves lightness rather than
                // hue, so harmony is still legible in the image while the
                // image stays the colour the rest of the app is. 27° in OkLCh
                // is the accent red.
                let step = i as f32 / 3.0;
                Oklch {
                    h: 27.0,
                    // The fourth stop is grey: a slot of four reds is a wash,
                    // and the gradient needs somewhere to go.
                    c: (0.19 * weight.max(0.15)) * (1.0 - step * 0.85),
                    l: 0.22 + 0.5 * step + 0.18 * energy * (1.0 - step),
                }
            }
        };
        // `to_srgb` returns 8-bit channels, already gamut-clipped — the
        // renderers want 0..1, so this is the one conversion.
        let rgb = to_srgb(colour);
        out[i * 3] = rgb.r as f32 / 255.0;
        out[i * 3 + 1] = rgb.g as f32 / 255.0;
        out[i * 3 + 2] = rgb.b as f32 / 255.0;
    }
    out
}

/// One frame column blended between the stepped and the interpolated row by
/// `smooth` — the `raw + (f - raw) * smooth` every colour read uses, in one
/// place so a caller blending several columns at once cannot blend them
/// differently. `slot` predates it and keeps its own closure, but the tone
/// path reads all its columns through this.
fn at_col(frame: &Frame, smooth: f32, i: usize) -> f32 {
    frame.raw[i] + (frame.f[i] - frame.raw[i]) * smooth
}

/// The twelve floats a `ColorSlot` block should hold, choosing between the
/// chroma slot and the tone gradient by [`Drive::colour_source`] (the tone design notes D8).
///
/// # Why the default is a crossfade, not a switch
///
/// The tone block is only meaningful where a model ran and pitched content was
/// heard; `tone_conf` measures exactly that. `Auto` blends over 0.05..0.15: at
/// `conf` 0 the output is *exactly* [`slot`]'s chroma output — byte for byte,
/// not "close" — so a 29-wide digest (an old song), a silent row and a build
/// with no model all render exactly as they did before tone existed. Only
/// above 0.05 does the tone gradient bleed in, and by 0.15 it is wholly tone.
/// A drum break with no pitched content sits at zero confidence and never
/// flickers into a hue it cannot hear (the tone design notes §7).
///
/// `Tone` and `Chroma` pin one source unconditionally, so an operator can
/// compare them or lock either.
fn colour_slots(frame: &Frame, drive: &Drive) -> [f32; 12] {
    let smooth = drive.smooth.clamp(0.0, 1.0);
    match drive.colour_source {
        ColourSource::Chroma => slot(frame, drive.palette, smooth),
        // A pinned tone source still has nothing to show at conf 0 (tone mode
        // `off`, no model, silence): the all-zero block would paint the brand
        // accent over whatever was authored, so it falls back to chroma.
        ColourSource::Tone => {
            let conf = at_col(frame, 1.0, crate::digest::TONE_CONF);
            if conf.is_finite() && conf > 0.0 {
                tone_slot(frame, drive.palette)
            } else {
                slot(frame, drive.palette, smooth)
            }
        }
        ColourSource::Auto => {
            // `tone_conf` smooths like any continuous column, by the same
            // per-tick decay `slot` applies, so the crossfade follows the same
            // wall-clock clock the colour does.
            let conf_smooth =
                metal_schema::per_tick_decay(smooth, metal_schema::TickRate::Base, frame.rate);
            let conf = at_col(frame, conf_smooth, crate::digest::TONE_CONF);
            // A non-finite confidence is "no signal": fall back to chroma
            // rather than let a NaN reach a lerp and then `to_srgb`.
            let t = if conf.is_finite() {
                ((conf - 0.05) / (0.15 - 0.05)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let chroma = slot(frame, drive.palette, smooth);
            if t <= 0.0 {
                return chroma;
            }
            let tone = tone_slot(frame, drive.palette);
            let mut out = [0.0f32; 12];
            for i in 0..12 {
                out[i] = chroma[i] + (tone[i] - chroma[i]) * t;
            }
            out
        }
    }
}

/// Four colours from the tone block, the tone design notes §5's gradient.
///
/// The hue is the angle of the circle-of-fifths vector (`tone_x`, `tone_y`),
/// which is the harmony centre; the vector's length says how focused that
/// harmony is, and inverts into a *spread*: focused gives 2 close stops,
/// diffuse gives up to 4 further-apart ones, cycled to fill the 4 slots.
/// Register sets lightness, density sets chroma and the alternate-stop
/// lightness variety, mode warms or cools the whole gradient by ±15°.
///
/// **The formula itself lives in `metal_notation::tone::tone_gradient`**, because
/// the proof sheet that renders a digest's tone block needs the same arithmetic
/// and `metal-render` cannot reach `metal-perform` without a cycle. This function's
/// only job is to turn a digest [`Frame`] into the five (§5) inputs: the frame
/// carries the hue as a vector, so the angle and inverted length are recovered
/// here. The non-finite guard stays here too, because this path can fall back
/// to the chroma [`slot`] and the pure function cannot.
///
/// The MetalVJ palette keeps the single hue family the rest of the app is
/// — the accent nudged ±10° by where the centre sits, stops varying only
/// lightness and chroma. Spectrum uses the angle directly.
fn tone_slot(frame: &Frame, palette: Palette) -> [f32; 12] {
    // The tone columns are continuous, so they smooth at the default weight
    // (1.0) converted to the frame's tick level — no per-float override exists
    // for a block, the same reasoning the chroma slot uses.
    let smooth = metal_schema::per_tick_decay(1.0, metal_schema::TickRate::Base, frame.rate);
    let x = at_col(frame, smooth, crate::digest::TONE_X);
    let y = at_col(frame, smooth, crate::digest::TONE_Y);
    let register = at_col(frame, smooth, crate::digest::TONE_REGISTER).clamp(0.0, 1.0);
    let density = at_col(frame, smooth, crate::digest::TONE_DENSITY).clamp(0.0, 1.0);
    let mode = at_col(frame, smooth, crate::digest::TONE_MODE).clamp(0.0, 1.0);

    // A NaN in any of the five would reach `to_srgb`; fall back to the chroma
    // path rather than paint a colour from garbage (the tone design notes §5 guard).
    if !(x.is_finite()
        && y.is_finite()
        && register.is_finite()
        && density.is_finite()
        && mode.is_finite())
    {
        return slot(frame, palette, 1.0);
    }

    // Centre and focus, from the stored vector (D3). Silence has no angle; the
    // MetalVJ accent (27°) is the stable neutral centre, so an all-zero
    // tone block paints the brand, not a random hue.
    let cx = (x - 0.5) * 2.0;
    let cy = (y - 0.5) * 2.0;
    let length = (cx * cx + cy * cy).sqrt();
    let spread = (1.0 - length).clamp(0.0, 1.0);
    let centre_deg = if length < 1e-4 { 27.0 } else { cy.atan2(cx).to_degrees() };

    let palette = match palette {
        Palette::MetalVJ | Palette::Hold => tone::Palette::MetalVJ,
        Palette::Spectrum => tone::Palette::Spectrum,
    };
    let stops = tone_gradient(centre_deg, spread, register, density, mode, palette);
    let mut out = [0.0f32; 12];
    for (i, rgb) in stops.iter().enumerate() {
        out[i * 3] = rgb[0];
        out[i * 3 + 1] = rgb[1];
        out[i * 3 + 2] = rgb[2];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::{Simulated, Source};
    use crate::paint::painter;

    /// Every model in the suite, through whichever painter this build has.
    ///
    /// Asked of the painter rather than of `metal_render::registry` so these gates
    /// run on both builds. They are gates on the *mapping*, which is identical
    /// either way — and the GPU-free build is exactly where a lost `drive`
    /// weight would show up, so it is the build that most needs them.
    fn schemas() -> Vec<(&'static str, &'static Schema)> {
        let p = painter();
        p.ids().into_iter().zip(p.schemas()).collect()
    }

    #[test]
    fn no_driven_float_ever_leaves_its_declared_range() {
        // The gate this module exists for. A float past its declared max is a
        // renderer being asked for a state its author never allowed, and the
        // symptom is a wrong-looking image with nothing in any log.
        let mut sim = Simulated::new(11);
        let drive = Drive {
            depth: 1.0,
            ..Drive::default()
        };
        for (id, schema) in schemas() {
            let base = schema.defaults();
            let mut t = 0.0;
            while t < 40.0 {
                let out = row(schema, &base, &sim.poll(t), &drive);
                assert_eq!(out.len(), schema.width(), "{id}: wrong row width");
                let mut at = 0;
                for block in schema.blocks {
                    if let Block::Floats(specs) = block {
                        for (k, spec) in specs.iter().enumerate() {
                            let v = out[at + k];
                            assert!(v.is_finite(), "{id}.{} went non-finite", spec.name);
                            assert!(
                                v >= spec.min - 1e-4 && v <= spec.max + 1e-4,
                                "{id}.{} = {v} left [{}, {}] at t={t}",
                                spec.name,
                                spec.min,
                                spec.max
                            );
                        }
                    }
                    at += block.width();
                }
                t += 1.0 / crate::digest::RATE;
            }
        }
    }

    /// The regression gate for a bug that produced no error and no warning:
    /// driven at full depth, Turing rendered as a flat blank sheet, because
    /// sweeping its feed and kill rates across their declared range took the
    /// reaction out of the regime where a pattern exists at all.
    ///
    /// The general shape of that failure is "a driven model wanders far enough
    /// from its authored defaults to stop being itself", and it is invisible
    /// to every other test here — the floats were all inside their declared
    /// ranges the whole time. So the gate is on distance travelled, not on
    /// legality.
    #[test]
    fn no_driven_float_wanders_far_from_the_row_the_operator_set() {
        let mut sim = Simulated::new(13);
        let drive = Drive {
            depth: 1.0,
            ..Drive::default()
        };
        for (id, schema) in schemas() {
            let base = schema.defaults();
            let mut t = 0.0;
            while t < 60.0 {
                let out = row(schema, &base, &sim.poll(t), &drive);
                let mut at = 0;
                for block in schema.blocks {
                    if let Block::Floats(specs) = block {
                        for (k, spec) in specs.iter().enumerate() {
                            let span = (spec.max - spec.min).abs().max(1e-6);
                            let moved = (out[at + k] - base[at + k]).abs() / span;
                            // A float the model marks structural is held
                            // tighter still, so the cap is its own.
                            let cap = SWING * spec.drive.clamp(0.0, 1.0);
                            assert!(
                                moved <= cap + 1e-3,
                                "{id}.{} moved {:.0}% of its range from the authored value \
                                 (cap {:.0}%) at t={t} — a model driven this far stops being \
                                 the model, see the SWING note",
                                spec.name,
                                moved * 100.0,
                                cap * 100.0
                            );
                        }
                    }
                    at += block.width();
                }
                t += 1.0 / crate::digest::RATE;
            }
        }
    }

    /// Onset rests at zero, so a float it drives must sit at the operator's
    /// value between hits and rise on one — not sit below it and return.
    #[test]
    fn an_onset_driven_float_rests_where_it_was_set_and_rises_on_the_hit() {
        let (_, schema) = schemas()[0];
        let base = schema.defaults();
        let mut sim = Simulated::new(1);
        sim.bpm = 120.0;
        let drive = Drive {
            depth: 1.0,
            ..Drive::default()
        };

        // Float 1 of the first model is onset-driven (`column_for(1) == 2`).
        assert_eq!(
            column_for(1),
            2,
            "this test is pinned to an onset-driven slot"
        );
        let between = row(schema, &base, &sim.poll(4.25), &drive)[1];
        let on_hit = row(schema, &base, &sim.poll(4.0), &drive)[1];
        assert!(
            (between - base[1]).abs() < (on_hit - base[1]).abs(),
            "the float is further from rest between hits than on one"
        );
    }

    #[test]
    fn the_camera_is_never_touched_however_hard_it_is_driven() {
        // CORRECTIONS.md §43: a wrong camera is the difference between a
        // model that opens framed and one that opens as a black frame. If
        // this regressed, every 3D model would intermittently look broken.
        let mut sim = Simulated::new(2);
        let drive = Drive {
            depth: 1.0,
            ..Drive::default()
        };
        for (id, schema) in schemas() {
            let base = schema.defaults();
            let out = row(schema, &base, &sim.poll(9.75), &drive);
            let mut at = 0;
            for block in schema.blocks {
                if matches!(block, Block::Camera(_)) {
                    assert_eq!(
                        &out[at..at + 7],
                        &base[at..at + 7],
                        "{id}: the digest moved the camera"
                    );
                }
                at += block.width();
            }
        }
    }

    #[test]
    fn depth_zero_returns_the_operators_row_untouched() {
        // Turning the drive off must give back exactly what was set by hand,
        // not "close to it" — otherwise there is no way to see a preset.
        let mut sim = Simulated::new(4);
        let drive = Drive {
            depth: 0.0,
            colour: false,
            ..Drive::default()
        };
        for (id, schema) in schemas() {
            let base = schema.defaults();
            assert_eq!(
                row(schema, &base, &sim.poll(5.0), &drive),
                base,
                "{id} drifted at depth 0"
            );
        }
    }

    #[test]
    fn a_short_base_row_is_still_rendered_at_full_width() {
        // A preset saved before a model gained a float would otherwise hand
        // the renderer a short slice and panic it.
        let (_, schema) = schemas()[0];
        let mut sim = Simulated::new(1);
        let out = row(schema, &[0.5], &sim.poll(1.0), &Drive::default());
        assert_eq!(out.len(), schema.width());
    }

    /// `Hold` keeps every model's own hue under a loud tone block, in all models
    /// (those with a tinted slot), while `Spectrum` moves it.
    #[test]
    fn hold_keeps_each_models_own_hue() {
        use metal_notation::color::from_srgb01;
        let hue_gap = |a: f32, b: f32| ((a - b + 540.0) % 360.0 - 180.0).abs();
        let mut moved = 0;
        for (id, schema) in schemas() {
            let mut base = schema.defaults();
            let mut at = 0;
            let mut tinted = Vec::new();
            for block in schema.blocks {
                if let Block::ColorSlot(..) = block {
                    // Give each slot colour a definite hue: red, green, blue, yellow.
                    base[at..at + 12].copy_from_slice(&[
                        0.9, 0.2, 0.2, 0.2, 0.8, 0.3, 0.2, 0.3, 0.9, 0.9, 0.8, 0.2,
                    ]);
                    tinted.push(at);
                }
                at += block.width();
            }
            for tone_deg in [0.0f32, 0.25, 0.5, 0.75] {
                let mut f = tone_frame(0.5 + 0.5 * tone_deg.cos(), 0.5 + 0.5 * tone_deg.sin());
                f.raw[crate::digest::TONE_DENSITY] = 0.8;
                f.f[crate::digest::TONE_DENSITY] = 0.8;
                let hold = Drive { palette: Palette::Hold, depth: 0.0, ..Drive::default() };
                let spec = Drive { palette: Palette::Spectrum, ..hold };
                let (h, s) = (row(schema, &base, &f, &hold), row(schema, &base, &f, &spec));
                for &a in &tinted {
                    for i in 0..4 {
                        let want = from_srgb01([base[a + i * 3], base[a + i * 3 + 1], base[a + i * 3 + 2]]).h;
                        let got = from_srgb01([h[a + i * 3], h[a + i * 3 + 1], h[a + i * 3 + 2]]).h;
                        assert!(hue_gap(want, got) < 6.0, "{id}: hold moved slot {i} hue {want} -> {got}");
                        let sp = from_srgb01([s[a + i * 3], s[a + i * 3 + 1], s[a + i * 3 + 2]]).h;
                        if hue_gap(want, sp) > 30.0 {
                            moved += 1;
                        }
                    }
                }
            }
        }
        assert!(moved > 0, "spectrum never moved a hue, so the comparison proves nothing");
    }

    #[test]
    fn colour_slots_stay_in_gamut_in_both_palettes() {
        let mut sim = Simulated::new(6);
        for palette in [Palette::MetalVJ, Palette::Spectrum] {
            let mut t = 0.0;
            while t < 30.0 {
                for v in slot(&sim.poll(t), palette, 1.0) {
                    assert!((0.0..=1.0).contains(&v), "{palette:?} left gamut: {v}");
                }
                t += 0.25;
            }
        }
    }

    #[test]
    fn the_metalvj_palette_stays_grey_and_red() {
        // The whole point of the default palette. Blue or green leading a
        // channel means the brand has leaked.
        let mut sim = Simulated::new(8);
        let mut t = 0.0;
        while t < 30.0 {
            let colours = slot(&sim.poll(t), Palette::MetalVJ, 1.0);
            for c in colours.chunks(3) {
                let (r, g, b) = (c[0], c[1], c[2]);
                assert!(
                    r >= g - 1e-3 && r >= b - 1e-3,
                    "not red-led at t={t}: {r},{g},{b}"
                );
            }
            t += 0.25;
        }
    }

    /// A frame with a chosen tone block and nothing else, so a colour test is
    /// about the tone mapping and not about chroma happening to agree.
    fn tone_frame(x: f32, y: f32) -> Frame {
        let mut f = [0.0f32; FEATURES];
        f[crate::digest::TONE_X] = x;
        f[crate::digest::TONE_Y] = y;
        f[crate::digest::TONE_REGISTER] = 0.5;
        f[crate::digest::TONE_DENSITY] = 0.5;
        f[crate::digest::TONE_MODE] = 0.5;
        f[crate::digest::TONE_CONF] = 1.0;
        Frame::step(f)
    }

    /// How many of the four slots are visually different colours, by 8-bit
    /// triple — the observable a "n stops" claim actually has when slots are
    /// always four.
    fn distinct_slots(slots: [f32; 12]) -> usize {
        let mut seen: Vec<[i32; 3]> = Vec::new();
        for c in slots.chunks(3) {
            let t = [
                (c[0] * 255.0).round() as i32,
                (c[1] * 255.0).round() as i32,
                (c[2] * 255.0).round() as i32,
            ];
            if !seen.contains(&t) {
                seen.push(t);
            }
        }
        seen.len()
    }

    /// The back-compat gate (the tone design notes step 10, D8): a row with no tone
    /// analysis — an old song, silence, a build with no model — must render
    /// byte-for-byte the colour it rendered before tone existed. Not
    /// approximately: exactly, so `assert_eq!` is the right assertion.
    #[test]
    fn tone_zero_is_exactly_todays_colours() {
        let mut sim = Simulated::new(23);
        let mut frame = sim.poll(3.25);
        for i in crate::digest::TONE_X..crate::digest::TONE_X + crate::digest::TONE {
            frame.raw[i] = 0.0;
            frame.f[i] = 0.0;
        }
        for palette in [Palette::MetalVJ, Palette::Spectrum] {
            let drive = Drive {
                palette,
                colour_source: ColourSource::Auto,
                ..Drive::default()
            };
            assert_eq!(
                colour_slots(&frame, &drive),
                slot(&frame, palette, drive.smooth),
                "{palette:?}: tone at zero changed the colour"
            );
        }
    }

    /// Focused harmony gives few close stops, diffuse gives more and further
    /// apart (the tone design notes §5, step 10). Slots are always four, so the observable
    /// is the number of *distinct* colours; this checks the Spectrum palette,
    /// where the stops differ in hue and the count is legible. MetalVJ
    /// varies lightness only across its stops by design, so it is pinned
    /// elsewhere.
    #[test]
    fn n_stops_grows_with_spread() {
        // x = 1, y = 0.5 -> |v| = 1 -> spread 0 -> two stops.
        let focused = distinct_slots(tone_slot(&tone_frame(1.0, 0.5), Palette::Spectrum));
        // x = 0.55, y = 0.45 -> |v| ~ 0.14 -> spread ~ 0.86 -> four stops.
        let diffuse = distinct_slots(tone_slot(&tone_frame(0.55, 0.45), Palette::Spectrum));
        assert_eq!(focused, 2, "a focused harmony must be two stops");
        assert_eq!(diffuse, 4, "a diffuse harmony must be four stops");
        assert!(focused < diffuse, "spread did not add stops");
    }

    /// `Chroma` is the pre-tone behaviour pinned: a loud tone block must not
    /// move the colour at all (the tone design notes step 10, D8).
    #[test]
    fn chroma_source_ignores_tone() {
        let mut frame = tone_frame(1.0, 0.0);
        frame.raw[crate::digest::TONE_REGISTER] = 0.95;
        frame.f[crate::digest::TONE_REGISTER] = 0.95;
        frame.raw[crate::digest::TONE_DENSITY] = 0.95;
        frame.f[crate::digest::TONE_DENSITY] = 0.95;
        frame.raw[crate::digest::TONE_MODE] = 0.95;
        frame.f[crate::digest::TONE_MODE] = 0.95;
        for palette in [Palette::MetalVJ, Palette::Spectrum] {
            let drive = Drive {
                palette,
                colour_source: ColourSource::Chroma,
                ..Drive::default()
            };
            assert_eq!(
                colour_slots(&frame, &drive),
                slot(&frame, palette, drive.smooth),
                "{palette:?}: chroma source let tone through"
            );
        }
    }

    /// A `NaN` anywhere in the tone block must not reach `to_srgb` — it falls
    /// back to the chroma path instead of painting a colour from garbage
    /// (the tone design notes §5 guard).
    #[test]
    fn a_non_finite_tone_block_falls_back_to_chroma() {
        let mut frame = tone_frame(0.66, 0.33);
        frame.raw[crate::digest::TONE_X] = f32::NAN;
        frame.f[crate::digest::TONE_X] = f32::NAN;
        let drive = Drive {
            colour_source: ColourSource::Tone,
            ..Drive::default()
        };
        assert_eq!(
            colour_slots(&frame, &drive),
            slot(&frame, drive.palette, drive.smooth)
        );
    }

    /// Three numbers this crate and `metal-reel` each state for themselves,
    /// because the dependency only points one way.
    ///
    /// Every one of them is a constant whose drift would be invisible until a
    /// venue: a `SWING` that disagreed would make a grade authored in the studio
    /// land somewhere else in the booth; a `HOLD_SECONDS` that disagreed would
    /// have the Curator changing shot on a clock the Router never switches on;
    /// and a digest width that disagreed would feed the Colourist a row of the
    /// wrong shape. `AUDIO_FEATURES.md` fixes the third and three crates now
    /// restate it — `metal-dash::digest`, `metal-router::features` and `metal-reel` — each
    /// saying so beside itself.
    #[test]
    fn the_constants_restated_across_the_seam_are_the_same_numbers() {
        assert_eq!(SWING, metal_reel::grade::SWING, "SWING");
        assert_eq!(
            crate::router::HOLD_SECONDS,
            metal_reel::curate::HOLD_SECONDS,
            "HOLD_SECONDS"
        );
        assert_eq!(crate::digest::MUSIC, metal_reel::MUSIC, "the digest's music prefix");
        assert_eq!(FEATURES, metal_router::features::COLUMNS, "the digest width");
        assert_eq!(
            FEATURES,
            crate::digest::MUSIC + crate::digest::VOICE + crate::digest::TONE
        );
    }

    /// The seam itself: a graded row must obey every rule the round-robin row
    /// obeys.
    ///
    /// The offsets here are all saturated in the worst direction, which is what
    /// a Colourist fitted on nonsense would produce. Nothing may leave its
    /// declared range, and nothing may travel further than the round robin
    /// could — otherwise a transmission is a hole in the one guarantee every
    /// range declaration in this suite makes.
    #[test]
    fn a_graded_row_cannot_reach_a_state_a_slider_could_not() {
        let mut sim = Simulated::new(17);
        let drive = Drive {
            depth: 1.0,
            ..Drive::default()
        };
        for (id, schema) in schemas() {
            let base = schema.defaults();
            let n: usize = schema
                .blocks
                .iter()
                .map(|b| match b {
                    Block::Floats(f) => f.len(),
                    _ => 0,
                })
                .sum();
            // Every combination of the two rails, and out of range on purpose:
            // `row_with` clamps the offsets itself, and this is what proves it.
            for fill in [-4.0f32, -1.0, 0.0, 1.0, 4.0] {
                let offsets = vec![fill; n];
                let out = row_with(schema, &base, &sim.poll(3.0), &drive, Some(&offsets));
                let mut at = 0;
                for block in schema.blocks {
                    if let Block::Floats(specs) = block {
                        for (k, spec) in specs.iter().enumerate() {
                            let v = out[at + k];
                            assert!(v.is_finite(), "{id}.{} went non-finite", spec.name);
                            assert!(
                                v >= spec.min - 1e-4 && v <= spec.max + 1e-4,
                                "{id}.{} = {v} left [{}, {}] at offset {fill}",
                                spec.name,
                                spec.min,
                                spec.max
                            );
                            let span = (spec.max - spec.min).abs().max(1e-6);
                            let moved = (v - base[at + k]).abs() / span;
                            let cap = SWING * spec.drive.clamp(0.0, 1.0);
                            assert!(
                                moved <= cap + 1e-3,
                                "{id}.{} moved {:.0}% of its range (cap {:.0}%) at offset {fill}",
                                spec.name,
                                moved * 100.0,
                                cap * 100.0
                            );
                        }
                    }
                    at += block.width();
                }
            }
        }
    }

    #[test]
    fn an_empty_grade_leaves_the_row_where_the_operator_set_it() {
        // Offset zero is "this frame under this music asks for nothing", which
        // is what an untrained-and-unmatched Colourist produces. It must mean
        // *the operator's row*, not the middle of the range.
        let (_, schema) = schemas()[0];
        let base = schema.defaults();
        let mut sim = Simulated::new(19);
        let n = base.len();
        let out = row_with(
            schema,
            &base,
            &sim.poll(2.0),
            &Drive {
                colour: false,
                ..Drive::default()
            },
            Some(&vec![0.0; n]),
        );
        let mut at = 0;
        for block in schema.blocks {
            if let Block::Floats(specs) = block {
                for k in 0..specs.len() {
                    assert_eq!(out[at + k], base[at + k]);
                }
            }
            at += block.width();
        }
    }

    /// The row the Colourist reads: onset raw, the continuous columns
    /// interpolated. If this collapsed to one or the other, a stutter would
    /// land an eighth-note behind the hit or a colour would stairstep — the two
    /// failures `smooth_default` exists to keep apart.
    #[test]
    fn the_blended_row_lands_onset_raw_and_the_envelopes_smoothed() {
        let frame = Frame {
            f: [1.0; FEATURES],
            raw: [0.0; FEATURES],
            rate: metal_schema::TickRate::Base,
            voice: true,
        };
        let full = blended(&frame, 1.0);
        assert_eq!(full[2], 0.0, "onset must not be interpolated");
        assert_eq!(full[0], 1.0, "rms must be");
        // And the operator's global control only ever scales *down*.
        let none = blended(&frame, 0.0);
        assert_eq!(none, [0.0; FEATURES]);
    }

    /// M8: a material glitch and a strobe hit land on the same beat. The row's
    /// `mat_glitch` and the `fx.glitch` effect share one column, and that column
    /// is onset — so the same transient that opens a beat flash opens the
    /// broken-surface gate. Read the column from the shared map, not a guess.
    #[test]
    fn a_material_glitch_rides_the_strobes_beat_column() {
        let map = metal_schema::effect_column_map();
        let grey = metal_schema::effect_index("glitch").expect("fx.glitch is an effect");
        let local = metal_schema::MATERIAL_FLOATS
            .iter()
            .position(|s| s.name == "mat_glitch")
            .expect("mat_glitch is a material float");
        let flat = metal_schema::CHAIN_FLOATS + metal_schema::N_ADVANCED + local;
        assert_eq!(map[flat], map[grey], "mat_glitch and fx.glitch must share a column");
        assert_eq!(map[flat], metal_schema::ONSET_COLUMN, "and it is onset, the beat");

        // The mapping in action: an onset spike raises `mat_glitch` (it rested
        // at the operator's value) and a silent row leaves it at rest.
        let (_, schema) = schemas().into_iter().next().expect("a model");
        let base = schema.defaults();
        let at = base.len() - metal_schema::EFFECTS_WIDTH + metal_schema::CHAIN_FLOATS + metal_schema::N_ADVANCED;
        let drive = Drive { depth: 1.0, ..Drive::default() };
        let mut f = [0.0f32; FEATURES];
        f[metal_schema::ONSET_COLUMN] = 1.0;
        let hit = Frame { f, raw: f, rate: metal_schema::TickRate::Base, voice: true };
        let rest = Frame { f: [0.0; FEATURES], raw: [0.0; FEATURES], rate: metal_schema::TickRate::Base, voice: true };
        let on_hit = row(schema, &base, &hit, &drive);
        let at_rest = row(schema, &base, &rest, &drive);
        let glitch_hit = metal_schema::read_material(&on_hit).unwrap()[local];
        let glitch_rest = metal_schema::read_material(&at_rest).unwrap()[local];
        assert!((glitch_hit - (base[at + local] + 0.6)).abs() < 1e-5, "the onset hit moved mat_glitch by the swing");
        assert!(glitch_hit > glitch_rest, "the onset hit did not raise the material glitch");
    }

    #[test]
    fn onset_reaches_more_floats_than_any_other_column() {
        // A visual that does not move on the hit is the one thing an audience
        // notices. This pins the weighting so a later edit to `column_for`
        // cannot quietly drop it.
        let counts = (0..60).map(column_for).fold([0usize; 4], |mut acc, c| {
            acc[c] += 1;
            acc
        });
        assert!(
            counts[2] > counts[0],
            "onset must drive more floats than rms"
        );
        assert!(counts[2] > counts[1] && counts[2] > counts[3]);
    }

    // --- loops (T7) -------------------------------------------------------

    static LOOP_FLOATS: [metal_schema::FloatSpec; 2] =
        [metal_schema::spec("speed", 0.0, 2.0, 1.0), metal_schema::spec("size", 0.0, 1.0, 0.5)];
    static LOOP_WHITE: [f32; 12] = [1.0; 12];
    static LOOP_BLOCKS: [Block; 3] = [
        Block::Floats(&LOOP_FLOATS),
        Block::ColorSlot("colors", &LOOP_WHITE),
        Block::Camera([0.0, 0.6, 3.2, 0.0, 0.0, 0.0, 45.0]),
    ];
    static LOOP_SCHEMA: Schema = Schema { blocks: &LOOP_BLOCKS };

    /// A scratch library with one model loop (`speed` + palette `colors`,
    /// plus a track naming a camera-ish param that must hit nothing) and a
    /// two-group palette set.
    fn loop_lib(tag: &str) -> (pattern::Library, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("fw-drive-loops-{tag}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        let factory = dir.join("factory");
        metal_vfs::create_dir_all(factory.join("palettes")).unwrap();
        metal_vfs::write(
            factory.join("m-pulse.json"),
            r##"{"id":"m-pulse","name":"Pulse","target":{"model":"m"},"tracks":[
              {"target":{"kind":"float","param":"speed"},
               "steps":[0,null,null,null,1,null,null,null,0.5,null,null,null,0.25,null,null,null]},
              {"target":{"kind":"float","param":"fov"},
               "steps":[1,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null]},
              {"target":{"kind":"palette","slot":"colors","palette":"two"},
               "steps":[{"colour":0},null,null,null,{"colour":2,"group":1},null,null,null,null,null,null,null,null,null,null,null]}]}"##,
        )
        .unwrap();
        metal_vfs::write(
            factory.join("palettes/two.json"),
            r##"{"id":"two","name":"Two","groups":[
              [{"srgb":"#ff0000"},{"srgb":"#00ff00"},{"srgb":"#0000ff"},{"srgb":"#ffffff"}],
              [{"srgb":"#000000"},{"srgb":"#808080"},{"srgb":"#ffff00"},{"srgb":"#00ffff"}]]}"##,
        )
        .unwrap();
        let lib = pattern::Library::load(&factory, &dir.join("user"));
        assert!(lib.problems.is_empty(), "{:?}", lib.problems);
        (lib, dir)
    }

    fn at_step(step: u8) -> Position {
        Position::from_beats(step as f64 / 4.0)
    }

    #[test]
    fn a_looped_float_follows_the_steps_and_the_rest_stays_driven() {
        let (lib, dir) = loop_lib("float");
        let l = lib.get("m-pulse").unwrap();
        let base = LOOP_SCHEMA.defaults();
        let frame = Simulated::new(3).poll(1.25);
        let driven = row(&LOOP_SCHEMA, &base, &frame, &Drive::default());
        for (step, want) in [(0u8, 0.0f32), (3, 0.0), (4, 2.0), (9, 1.0), (15, 0.5)] {
            let mut r = driven.clone();
            apply_loop(&LOOP_SCHEMA, &mut r, l, at_step(step), &lib);
            assert!((r[0] - want).abs() < 1e-5, "step {step}: speed {} != {want}", r[0]);
            assert_eq!(r[1], driven[1], "untargeted `size` keeps the audio drive");
            assert_eq!(r[14..], driven[14..], "cameras are never looped");
            // Same tick, same row.
            let mut again = driven.clone();
            apply_loop(&LOOP_SCHEMA, &mut again, l, at_step(step), &lib);
            assert_eq!(r, again);
        }
        let _ = metal_vfs::remove_dir_all(dir);
    }

    #[test]
    fn a_palette_track_sets_the_slot_to_the_group_colour() {
        let (lib, dir) = loop_lib("palette");
        let l = lib.get("m-pulse").unwrap();
        let mut r = LOOP_SCHEMA.defaults();
        apply_loop(&LOOP_SCHEMA, &mut r, l, at_step(1), &lib);
        assert_eq!(&r[2..5], &[1.0, 0.0, 0.0], "group 0 colour 0 held from step 0");
        assert_eq!(&r[5..8], &[0.0, 1.0, 0.0], "then the rest of the group in order");
        let mut r = LOOP_SCHEMA.defaults();
        apply_loop(&LOOP_SCHEMA, &mut r, l, at_step(6), &lib);
        let want = lib
            .colour(&l.tracks[2].target, pattern::Value::Pick { group: 1, colour: 2 })
            .unwrap();
        assert_eq!(&r[2..5], &want);
        let _ = metal_vfs::remove_dir_all(dir);
    }
}
