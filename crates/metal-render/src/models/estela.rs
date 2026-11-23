//! Wake's float layout and stutter clock — **no longer a model**.
//!
//! `wake` was an effect pretending to be a generator, and was retired from the
//! registry on 2026-10-25 (tombstone, rule 1: its row keeps its index). What
//! stays here is only what the post chain's `EstelaParams` and the trail ring
//! still read: the schema defaults, the block/refresh ranges, `TAP_DT`, and
//! the `Stutter` playhead clock. There is no `Model` and no shader.

use metal_reel::Cue;

use crate::model::{color_at, spec, spec_held, Block, FloatSpec, Schema};





/// How far apart the taps of the trail are, in seconds of wall time.
///
/// One rung of the ladder at `metal_reel::index::FPS` and playback speed 1, so at
/// rest each tap is a different source frame and the wake is a wake rather than
/// one frame drawn eight times. The two constants are related but not shared,
/// because this is a property of *the effect* — it is how fast the tube decays
/// — and a library indexed at another rate must not change how the smear looks.
pub const TAP_DT: f32 = 1.0 / 12.0;

/// The shortest and longest a stutter may hold one frame, in seconds.
///
/// The floor is one tap, which is "not holding at all". The ceiling is a bit
/// over half a second: past that the model stops reading as a stutter and
/// starts reading as a still, and a venue cannot tell that from the player
/// having crashed.
pub const MIN_HOLD: f32 = TAP_DT;
pub const MAX_HOLD: f32 = 0.55;

/// The furthest back a stuttering hold may jump, in seconds.
pub const MAX_JUMP: f32 = 0.75;

const FLOATS: &[FloatSpec] = &[
    // Structural. It decides whether the audience is watching footage or a
    // slideshow, and a signal swinging it would do both several times a second.
    spec_held("advance", 0.0, 3.0, 1.0, 0.25),
    spec("stutter", 0.0, 1.0, 0.35),
    spec("retention", 0.0, 1.0, 0.4),
    spec("jump", 0.0, 1.0, 0.3),
    // The mosh. `congelado` is the float this model is about: what fraction of
    // the picture has stopped updating.
    spec("block", MIN_BLOCKS, MAX_BLOCKS, 14.0),
    spec("frozen", 0.0, 1.0, 0.55),
    spec("refresh", MIN_REFRESH, MAX_REFRESH, 0.5),
    // The top of the range is a quarter of the frame by the deepest generation,
    // which is a patch that has marched clear of where it froze — the reference
    // shot's six drummers.
    spec("drag", 0.0, 0.14, 0.035),
    spec("tear", 0.0, 0.06, 0.01),
    spec("indent", 0.0, 1.0, 0.35),
    // The grade — the high-level parameters the Colourist writes. The names are
    // `read::NAMES`' and Viraje's, not this model's own: one word per quantity
    // across the crate is what lets `grade::DEFAULT` carry one line for
    // `contraste` rather than one per transmission.
    spec("exposure", 0.0, 2.0, 1.0),
    spec("contrast", 0.0, 2.5, 1.0),
    spec("saturation", 0.0, 2.5, 1.0),
    spec("r", 0.0, 2.0, 1.0),
    spec("g", 0.0, 2.0, 1.0),
    spec("b", 0.0, 2.0, 1.0),
];

/// How wide the block grid may be, in blocks across the frame.
///
/// The floor is coarse enough to read as damage rather than as noise — four
/// blocks across is the picture in quarters. The ceiling is where a block stops
/// being a block: past about sixty, one is a few pixels on a projector and the
/// mosh reads as grain.
pub const MIN_BLOCKS: f32 = 4.0;
pub const MAX_BLOCKS: f32 = 60.0;

/// How long a block may stay frozen before its keyframe lands, in seconds.
pub const MIN_REFRESH: f32 = 0.1;
pub const MAX_REFRESH: f32 = 1.6;

/// What a block's colour drifts towards as it goes stale.
///
/// A residual carries chroma that was computed against pixels it never met, so
/// the error is a *hue* rotation rather than a fade — which is why these stops
/// stay near full brightness and move around the wheel instead of falling to
/// black. Green and magenta because that is the axis a subsampled chroma plane
/// tears along, and it is the cast the reference has on its worst blocks.
const BLEED_COLOR: [f32; 12] = [
    1.00, 1.00, 1.00, //
    0.72, 0.96, 0.78, //
    0.98, 0.66, 0.92, //
    0.55, 0.72, 1.00,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[
        Block::Floats(FLOATS),
        color_at("blood", &BLEED_COLOR),
        crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK,
        crate::model::EFFECTS_COLOR_BLOCK,
    ],
};

/// The four floats that decide where in the footage the effect is reading.
#[derive(Clone, Copy, Debug)]
pub struct Stutter {
    pub advance: f32,
    pub stutter: f32,
    pub retention: f32,
    pub jump: f32,
}

impl Stutter {
    pub fn from_row(f: &[f32]) -> Stutter {
        Stutter {
            advance: f[0].max(0.0),
            stutter: f[1].clamp(0.0, 1.0),
            retention: f[2].clamp(0.0, 1.0),
            jump: f[3].clamp(0.0, 1.0),
        }
    }

    pub fn hold(&self) -> f32 {
        MIN_HOLD + (MAX_HOLD - MIN_HOLD) * self.retention
    }
}

/// A fixed draw for hold `k`. Deterministic on every machine, and that matters:
/// two screens of one venue render the same wall clock and must stutter
/// together, and they may not be the same binary on the same GPU.
///
/// An integer hash rather than `fract(sin(x))` for exactly that reason — the
/// float version is a different number depending on how `sin` was implemented.
fn draw(k: i64, salt: u64) -> f32 {
    let mut x = (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(salt);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 40) as f32 / 16_777_216.0
}

/// Where in the clip the effect is reading at wall-clock `t`.
///
/// Time is cut into holds of [`Stutter::hold`]. Each hold draws once, from its
/// own index: below `tartamudeo` it **freezes**, at the position the playhead
/// had when the hold began, jumped back by its own second draw. Above it, the
/// footage simply plays.
///
/// # Why a per-hold draw rather than a blend
///
/// The obvious spelling is to interpolate between the smooth position and the
/// frozen one by `tartamudeo`. That is not a stutter, it is *slow motion* — the
/// playhead moves the whole time, just less. What an audience recognises as a
/// stutter is a frame that is completely still followed by one that has caught
/// up, and `tartamudeo` is how *often* that happens, not how far.
pub fn playhead(cue: &Cue, t: f32, s: &Stutter) -> f32 {
    let smooth = cue.position(t, s.advance);
    if s.stutter <= 0.0 {
        return smooth;
    }
    let hold = s.hold();
    let k = (t / hold).floor();
    // The floor of a huge or non-finite time is not a hold index. A clock that
    // has run past this has run for three thousand years.
    if !k.is_finite() || k.abs() > 9e15 {
        return smooth;
    }
    let k = k as i64;
    if draw(k, 0x51ED) >= s.stutter {
        return smooth;
    }
    let start = k as f32 * hold;
    let jump = s.jump * MAX_JUMP * draw(k, 0xB19D);
    cue.position(start - jump, s.advance)
}

