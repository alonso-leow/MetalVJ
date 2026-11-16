//! `AuxLogic` — the one contract for logic that runs on the CPU before a frame
//! is drawn.
//!
//! **Why this crate exists.** The prototype solved the same problem three times
//! in three different ways before this was written: `metal-rig` (rotations →
//! pose/skin), `metal-sense::Detector` (an image → control, with state between
//! frames) and `cam.track` (a track → reserved floats on a generator). Each
//! had its own answer for how floats are declared, how state is kept, how an
//! error is reported and how cost is bounded. That three answers exist is the
//! evidence this is not "the rig's abstraction" — it is the generalisation of
//! something the suite already needed three times. See
//! `docs/plan-architecture.md` §8.1 and D1.
//!
//! **What the contract is.** The design that `plan-opus.md` fixed for the
//! future script node (`O.6`) is exactly this, so it is defined here once
//! instead of being re-invented as a second boundary per runtime:
//!
//! 1. A `FloatSpec`-shaped declaration of the floats the logic writes — a
//!    script with hard-coded constants is a picture, not a node.
//! 2. Reads a row plus a [`LogicContext`], writes a row. **Never pixels.**
//! 3. Validates, and returns a failure as a normal [`AuxError`] value, not a
//!    panic — the same treatment `metal-forge` gives a broken WGSL shader.
//! 4. Cost carries a [`Budget`]; a hung piece of logic does not bring down the
//!    show. [`run_with_budget`] is the shared way to enforce it, and an
//!    implementation that loops internally may also check it itself.
//!
//! `metal-rig` is the first implementation and the reference; `O.6`/`S.G` (the
//! script runtimes) write against this same trait, not against a new one.

#![forbid(unsafe_code)]

use web_time::Instant;

/// One float a piece of logic declares: a name, a legal range, and where it
/// starts. Mirrors `metal_schema::FloatSpec` deliberately — same idea, kept a
/// separate type so the seam does not inherit the manifest's shape (a
/// `FloatSpec` also carries `drive` and `smooth`, which are the renderer's).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatDecl {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

impl FloatDecl {
    pub const fn new(name: &'static str, min: f32, max: f32, default: f32) -> FloatDecl {
        FloatDecl { name, min, max, default }
    }

    /// The declared default, clamped into the declared range — the value a
    /// fresh row starts from. A default outside its own range is a bug in the
    /// declaration, so this clamps rather than returning it unchanged.
    pub fn start(&self) -> f32 {
        self.default.clamp(self.min, self.max)
    }
}

/// How long a single call may take, in microseconds.
///
/// A hard number rather than a comment because the plan's whole point is that
/// the cost is a property *of the logic*, enforced by the caller, not a hope
/// that the logic is fast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub micros: u64,
}

impl Budget {
    /// The default: 2 ms. Generous for a pose, a detector box or a camera
    /// track, and far below a frame at 60 Hz (16.6 ms) so one slow call is
    /// visible as a stutter rather than as a missed frame.
    pub const DEFAULT: Budget = Budget { micros: 2_000 };

    pub const fn micros(micros: u64) -> Budget {
        Budget { micros }
    }

    pub fn exceeded_by(&self, elapsed: std::time::Duration) -> bool {
        elapsed.as_micros() as u64 > self.micros
    }
}

impl Default for Budget {
    fn default() -> Self {
        Budget::DEFAULT
    }
}

/// What the logic is told about the frame it is running for: wall-clock time
/// its own clock, the tick, and the budget. No pixels — see the crate doc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicContext {
    /// Seconds since the show's start, the same clock models are driven on.
    pub time: f64,
    /// Seconds since the previous call, so logic can integrate rather than
    /// assume a fixed step.
    pub dt: f32,
    /// Monotonic frame counter, for logic that must act every N frames.
    pub frame: u64,
    pub budget: Budget,
}

impl LogicContext {
    pub const fn new(time: f64, dt: f32, frame: u64, budget: Budget) -> LogicContext {
        LogicContext { time, dt, frame, budget }
    }
}

/// The row the logic reads. Borrowed so a caller can pass the model's own row
/// without copying it first.
#[derive(Debug, Clone, Copy)]
pub struct AuxInput<'a> {
    pub row: &'a [f32],
}

impl<'a> AuxInput<'a> {
    pub const fn new(row: &'a [f32]) -> AuxInput<'a> {
        AuxInput { row }
    }
}

/// The row the logic writes, aligned to [`AuxLogic::floats`]: entry `i` is the
/// value of declared float `i`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuxOutput {
    pub row: Vec<f32>,
}

impl AuxOutput {
    pub fn new(row: Vec<f32>) -> AuxOutput {
        AuxOutput { row }
    }

    /// A row of the declared defaults, for logic that only fills some entries.
    pub fn defaults(floats: &[FloatDecl]) -> AuxOutput {
        AuxOutput { row: floats.iter().map(FloatDecl::start).collect() }
    }

    /// The value of a declared float by name — how a downstream stage (the
    /// plan's `Mask` reading a script's `control`) consumes the row without
    /// knowing the logic that wrote it. `None` if the logic did not declare
    /// that name, which is a wiring mistake the caller can report, not a panic.
    pub fn get(&self, floats: &[FloatDecl], name: &str) -> Option<f32> {
        let i = floats.iter().position(|f| f.name == name)?;
        self.row.get(i).copied()
    }
}

/// Why a call failed. A value, not a panic and not a thread death — the caller
/// decides whether a failed tick is a dropped frame or a stopped node.
#[derive(Debug, Clone, PartialEq)]
pub enum AuxError {
    /// The call took longer than [`LogicContext::budget`] allowed.
    BudgetExceeded,
    /// The input row was the wrong shape for this logic.
    BadInput(String),
    /// The logic itself failed. Free-form because the runtime that produced it
    /// (Rust today, Python or JS later) knows its own failure best.
    Failed(String),
}

impl std::fmt::Display for AuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuxError::BudgetExceeded => write!(f, "logic exceeded its budget"),
            AuxError::BadInput(m) => write!(f, "bad input: {m}"),
            AuxError::Failed(m) => write!(f, "logic failed: {m}"),
        }
    }
}

impl std::error::Error for AuxError {}

/// A piece of logic that runs before a frame is drawn.
///
/// The rig is the reference implementation (`metal-rig::logic`); a detector, a
/// camera track, a CPU-evaluated mask and a user script are all meant to fit
/// this same shape. A type implementing it can be run without knowing whether
/// it is written in Rust, Python or JS — that is the seam `O.6`/`S.G` land on.
pub trait AuxLogic: Send {
    /// The stable id the logic is addressed by, e.g. `rig`, `sensor.yolo`.
    fn id(&self) -> &'static str;

    /// The floats this logic writes, in row order. Empty is legal (logic that
    /// produces only its own internal state), but a logic that writes a row
    /// must declare it here — a hard-coded output is a picture, not a node.
    fn floats(&self) -> &'static [FloatDecl];

    /// The width of the row this logic reads. `0` means "takes no input row"
    /// and the caller may pass anything.
    fn input_width(&self) -> usize {
        0
    }

    /// Runs the logic for one tick. Must not panic on bad input: validate and
    /// return [`AuxError::BadInput`] instead.
    fn run(&mut self, input: &AuxInput, ctx: &LogicContext) -> Result<AuxOutput, AuxError>;

    /// Resets any state the logic keeps between frames.
    fn reset(&mut self) {}
}

/// Runs `logic` under its budget, validating the input shape first.
///
/// This is the one place the contract's rules 3 and 4 are enforced for every
/// implementation, so no runtime has to remember them: a wrong-width row is a
/// normal [`AuxError::BadInput`], and overrunning the budget is a normal
/// [`AuxError::BudgetExceeded`] even if the logic itself returned `Ok`.
pub fn run_with_budget(
    logic: &mut dyn AuxLogic,
    input: &AuxInput,
    ctx: &LogicContext,
) -> Result<AuxOutput, AuxError> {
    let expected = logic.input_width();
    if expected != 0 && input.row.len() != expected {
        return Err(AuxError::BadInput(format!(
            "{} expects {expected} floats, got {}",
            logic.id(),
            input.row.len()
        )));
    }
    let start = Instant::now();
    let out = logic.run(input, ctx)?;
    if ctx.budget.exceeded_by(start.elapsed()) {
        return Err(AuxError::BudgetExceeded);
    }
    Ok(out)
}

/// The trivially-correct logic: copies its input row to its output row.
///
/// Lives in the crate (not behind `cfg(test)`) so it is the reference every
/// test — and every future non-Rust runtime's conformance suite — can run
/// through the same door without needing the rig. `docs/plan-architecture.md`
/// LAY.13 uses it as the script seam's stand-in until `O.6`/`S.G` exist.
#[derive(Debug, Default, Clone, Copy)]
pub struct IdentityLogic {
    pub width: usize,
}

impl IdentityLogic {
    pub const fn new(width: usize) -> IdentityLogic {
        IdentityLogic { width }
    }
}

/// The identity's declarations can't be `&'static` for an arbitrary width, so
/// the stub declares one named float and copies as many entries as the row has
/// when the width is not set at construction. Kept deliberately simple: its
/// job is to prove the boundary, not to be useful.
static IDENTITY_FLOATS: [FloatDecl; 1] = [FloatDecl::new("passthrough", 0.0, 1.0, 0.0)];

impl AuxLogic for IdentityLogic {
    fn id(&self) -> &'static str {
        "identity"
    }

    fn floats(&self) -> &'static [FloatDecl] {
        &IDENTITY_FLOATS
    }

    fn input_width(&self) -> usize {
        self.width
    }

    fn run(&mut self, input: &AuxInput, _ctx: &LogicContext) -> Result<AuxOutput, AuxError> {
        Ok(AuxOutput::new(input.row.to_vec()))
    }
}

// ---------------------------------------------------------------------------
// the script seam (LAY.13)
// ---------------------------------------------------------------------------

/// A stand-in for a user script, so the boundary can be tested before `O.6`
/// (Python) and `S.G` (Rust/JS) exist.
///
/// **This is not a script runtime.** The plan is explicit that the runtimes
/// are `O.6`/`S.G` and that this batch only leaves the seam they are written
/// against. `GainLogic` exists to prove that a logic which is *not the rig*
/// passes through the same [`AuxLogic`] door and produces a named control a
/// `Mask` can read — the shot of `the design notes:244-247` in miniature.
#[derive(Debug, Default, Clone, Copy)]
pub struct GainLogic {
    /// Multiplied by the first input float, if there is one.
    pub gain: f32,
}

static GAIN_FLOATS: [FloatDecl; 1] = [FloatDecl::new("gain_out", 0.0, 1.0, 0.0)];

impl GainLogic {
    pub const fn new(gain: f32) -> GainLogic {
        GainLogic { gain }
    }
}

impl AuxLogic for GainLogic {
    fn id(&self) -> &'static str {
        "script.gain"
    }

    fn floats(&self) -> &'static [FloatDecl] {
        &GAIN_FLOATS
    }

    fn run(&mut self, input: &AuxInput, _ctx: &LogicContext) -> Result<AuxOutput, AuxError> {
        let x = input.row.first().copied().unwrap_or(0.0);
        Ok(AuxOutput::new(vec![(x * self.gain).clamp(0.0, 1.0)]))
    }
}

// ---------------------------------------------------------------------------
// the trained reaction (S.AM, `ai.colourist`)
// ---------------------------------------------------------------------------

/// How many numbers a Colourist hears: the digest row's music prefix — the
/// first `metal_schema::MUSIC` columns, which is what `metal_reel::grade::INPUTS`
/// and the corpus define. The voice block that follows is not read; a
/// 29-wide row is accepted and its prefix sliced, rather than rejected.
pub const COLOURIST_INPUTS: usize = metal_schema::MUSIC;

/// One declared output float for the Colourist.
///
/// The real row is one entry per **own float of the downstream model**, so its
/// width is only known when the node is built against a schema — exactly the
/// case [`IdentityLogic`] already handles by declaring one float and writing
/// as many entries as the caller's row has. The declaration exists so the
/// contract's "a logic that writes a row must declare it" holds; the width is
/// carried by the weight matrix instead.
static COLOURIST_FLOATS: [FloatDecl; 1] = [FloatDecl::new("offset", -1.0, 1.0, 0.0)];

/// The Colourist as a piece of [`AuxLogic`]: a fitted linear map from the
/// digest's sixteen columns to one signed offset per own float, in −1..1.
///
/// **Why the weights and not the model.** `metal-reel::grade::Grade` owns the
/// fitted rows and the file on disk; this crate cannot depend on it. So the
/// caller hands over the rows (each `COLOURIST_INPUTS` weights followed by the
/// bias) and this runtime only applies them — the same split that keeps the
/// budget, the error-as-a-value and the "never pixels" rule enforced in one
/// place for every runtime. The arithmetic mirrors `Colourist::offsets` for a
/// trained grade exactly, including the final clamp, so a row computed here is
/// the row `metal-reel` would have produced.
#[derive(Debug, Default, Clone)]
pub struct ColouristLogic {
    /// One row per own float: `COLOURIST_INPUTS` weights, then the bias.
    weights: Vec<Vec<f32>>,
}

impl ColouristLogic {
    pub fn new(weights: Vec<Vec<f32>>) -> ColouristLogic {
        ColouristLogic { weights }
    }

    /// How many offsets this logic writes — the downstream model's own-float
    /// count.
    pub fn len(&self) -> usize {
        self.weights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }
}

impl AuxLogic for ColouristLogic {
    fn id(&self) -> &'static str {
        "ai.colourist"
    }

    fn floats(&self) -> &'static [FloatDecl] {
        &COLOURIST_FLOATS
    }

    fn input_width(&self) -> usize {
        metal_schema::FEATURES
    }

    fn run(&mut self, input: &AuxInput, _ctx: &LogicContext) -> Result<AuxOutput, AuxError> {
        let Some(music) = input.row.get(..COLOURIST_INPUTS) else {
            return Err(AuxError::BadInput(format!(
                "ai.colourist expects at least {COLOURIST_INPUTS} digest floats, got {}",
                input.row.len()
            )));
        };
        let mut out = Vec::with_capacity(self.weights.len());
        for row in &self.weights {
            let mut v = row.get(COLOURIST_INPUTS).copied().unwrap_or(0.0);
            for k in 0..COLOURIST_INPUTS {
                v += row.get(k).copied().unwrap_or(0.0) * music[k];
            }
            out.push(if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 });
        }
        Ok(AuxOutput::new(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_float_decl_clamps_its_default_into_its_own_range() {
        assert_eq!(FloatDecl::new("x", 0.0, 1.0, 0.5).start(), 0.5);
        assert_eq!(FloatDecl::new("x", 0.0, 1.0, 2.0).start(), 1.0);
        assert_eq!(FloatDecl::new("x", 0.0, 1.0, -3.0).start(), 0.0);
    }

    #[test]
    fn the_identity_runs_and_declares_its_floats() {
        let mut logic = IdentityLogic::new(0);
        let ctx = LogicContext::new(0.0, 1.0 / 60.0, 0, Budget::DEFAULT);
        let out = run_with_budget(&mut logic, &AuxInput::new(&[0.25, 0.75]), &ctx).unwrap();
        assert_eq!(out.row, vec![0.25, 0.75]);
        assert_eq!(logic.floats().len(), 1);
    }

    #[test]
    fn a_wrong_width_row_is_a_normal_error_not_a_panic() {
        let mut logic = IdentityLogic::new(3);
        let ctx = LogicContext::new(0.0, 0.0, 0, Budget::DEFAULT);
        let err = run_with_budget(&mut logic, &AuxInput::new(&[1.0, 2.0]), &ctx).unwrap_err();
        assert!(matches!(err, AuxError::BadInput(_)), "{err:?}");
    }

    struct Sleeps(std::time::Duration);

    impl AuxLogic for Sleeps {
        fn id(&self) -> &'static str {
            "sleeps"
        }
        fn floats(&self) -> &'static [FloatDecl] {
            &[]
        }
        fn run(&mut self, _input: &AuxInput, _ctx: &LogicContext) -> Result<AuxOutput, AuxError> {
            std::thread::sleep(self.0);
            Ok(AuxOutput::default())
        }
    }

    #[test]
    fn a_call_that_overruns_its_budget_is_a_normal_error() {
        // The budget is enforced by the caller, not hoped for from the logic:
        // the implementation returns `Ok` and the contract still reports the
        // overrun as a value the caller can act on.
        let mut logic = Sleeps(std::time::Duration::from_millis(1));
        let ctx = LogicContext::new(0.0, 0.0, 0, Budget::micros(100));
        let err = run_with_budget(&mut logic, &AuxInput::new(&[]), &ctx).unwrap_err();
        assert_eq!(err, AuxError::BudgetExceeded);
    }

    // -- LAY.13: the seam a non-rig runtime is written against -------------

    #[test]
    fn the_same_door_runs_a_logic_that_is_not_the_rig() {
        // Two unrelated implementations, one call signature. This is the
        // property `O.6`/`S.G` depend on: the frame engine does not know
        // whether it holds the rig or a script.
        let ctx = LogicContext::new(0.0, 0.0, 0, Budget::DEFAULT);
        let mut identity: Box<dyn AuxLogic> = Box::new(IdentityLogic::new(0));
        let mut script: Box<dyn AuxLogic> = Box::new(GainLogic::new(0.5));

        let a = run_with_budget(&mut *identity, &AuxInput::new(&[0.8]), &ctx).unwrap();
        assert_eq!(a.get(identity.floats(), "passthrough"), Some(0.8));

        let b = run_with_budget(&mut *script, &AuxInput::new(&[0.8]), &ctx).unwrap();
        assert_eq!(b.get(script.floats(), "gain_out"), Some(0.4));

        assert_ne!(identity.id(), script.id());
    }

    #[test]
    fn two_passes_of_a_non_rig_logic_write_a_control_a_mask_can_read() {
        // The shot of `the design notes:244-247`: a script produces a
        // Control row, a later stage reads one channel by name. Two passes
        // because the contract has to hold across frames, not only once.
        let mut logic: Box<dyn AuxLogic> = Box::new(GainLogic::new(0.5));
        for frame in 0..2u64 {
            let ctx = LogicContext::new(frame as f64 / 60.0, 1.0 / 60.0, frame, Budget::DEFAULT);
            let out = run_with_budget(&mut *logic, &AuxInput::new(&[0.6, 0.0]), &ctx).unwrap();
            let alpha = out.get(logic.floats(), "gain_out").expect("declared control present");
            assert!((alpha - 0.3).abs() < 1e-6, "frame {frame}: {alpha}");
        }
    }

    // -- S.AM: the trained reaction -----------------------------------------

    /// A weight row of `COLOURIST_INPUTS` weights plus a bias, then a digest
    /// row that lights up exactly one column, so the expected offset is the
    /// arithmetic done by hand.
    #[test]
    fn the_colourist_logic_applies_its_weights_and_bias() {
        let mut w = vec![0.0f32; COLOURIST_INPUTS + 1];
        w[0] = 2.0; // rms
        w[COLOURIST_INPUTS] = 0.25; // bias
        let mut logic = ColouristLogic::new(vec![w]);
        let ctx = LogicContext::new(0.0, 0.0, 0, Budget::DEFAULT);
        let mut digest = [0.0f32; metal_schema::FEATURES];
        digest[0] = 0.5;
        let out = run_with_budget(&mut logic, &AuxInput::new(&digest), &ctx).unwrap();
        assert_eq!(out.row, vec![1.25f32.clamp(-1.0, 1.0)], "2.0 * 0.5 + 0.25 clamps to 1.0");
        assert_eq!(logic.len(), 1);
    }

    /// The clamp is the seam's guarantee: a saturated fit can only produce a
    /// row a slider could have produced.
    #[test]
    fn a_colourist_offset_never_leaves_minus_one_to_one() {
        let mut w = vec![0.0f32; COLOURIST_INPUTS + 1];
        w[0] = 100.0;
        let mut logic = ColouristLogic::new(vec![w.clone(), {
            let mut r = w.clone();
            r[0] = -100.0;
            r
        }]);
        let ctx = LogicContext::new(0.0, 0.0, 0, Budget::DEFAULT);
        let mut digest = [0.0f32; metal_schema::FEATURES];
        digest[0] = 1.0;
        let out = run_with_budget(&mut logic, &AuxInput::new(&digest), &ctx).unwrap();
        assert_eq!(out.row, vec![1.0, -1.0]);
    }

    /// A digest row of the wrong width is a normal `BadInput`, never a panic —
    /// the same rule `IdentityLogic`'s wrong-width test pins.
    #[test]
    fn a_colourist_digest_of_the_wrong_width_is_a_normal_error() {
        let mut logic = ColouristLogic::new(vec![vec![0.0; COLOURIST_INPUTS + 1]]);
        let ctx = LogicContext::new(0.0, 0.0, 0, Budget::DEFAULT);
        let err = run_with_budget(&mut logic, &AuxInput::new(&[0.5, 0.5]), &ctx).unwrap_err();
        assert!(matches!(err, AuxError::BadInput(_)), "{err:?}");
    }

    /// The colourist passes through the same door as the rig and the script —
    /// the property `metal-perform` relies on when it holds a `dyn AuxLogic`.
    #[test]
    fn the_colourist_is_an_aux_logic_like_any_other() {
        let mut logic: Box<dyn AuxLogic> =
            Box::new(ColouristLogic::new(vec![vec![0.0; COLOURIST_INPUTS + 1]]));
        assert_eq!(logic.id(), "ai.colourist");
        assert_eq!(logic.input_width(), metal_schema::FEATURES);
        let ctx = LogicContext::new(0.0, 0.0, 0, Budget::DEFAULT);
        let digest = [0.5f32; metal_schema::FEATURES];
        let out = run_with_budget(&mut *logic, &AuxInput::new(&digest), &ctx).unwrap();
        assert_eq!(out.row, vec![0.0], "an all-zero fit answers a zero offset");
    }

    /// The whole point of the prefix rule: a 29-wide row and the same row with
    /// its voice block set to garbage answer the same offsets. A model trained
    /// on the corpus must not be moved by columns it never saw.
    #[test]
    fn the_colourist_ignores_the_voice_block() {
        let mut w = vec![0.0f32; COLOURIST_INPUTS + 1];
        w[0] = 2.0; // rms
        w[2] = -1.0; // onset
        w[COLOURIST_INPUTS] = 0.1; // bias
        let ctx = LogicContext::new(0.0, 0.0, 0, Budget::DEFAULT);

        let mut music = [0.0f32; metal_schema::FEATURES];
        music[0] = 0.25;
        music[2] = 0.75;
        for (k, v) in music[metal_schema::MUSIC..].iter_mut().enumerate() {
            *v = k as f32 / metal_schema::VOICE as f32;
        }

        let mut with_voice = ColouristLogic::new(vec![w.clone()]);
        let a = run_with_budget(&mut with_voice, &AuxInput::new(&music), &ctx).unwrap();

        let mut clean = music;
        clean[metal_schema::MUSIC..].fill(0.0);
        let mut without_voice = ColouristLogic::new(vec![w]);
        let b = run_with_budget(&mut without_voice, &AuxInput::new(&clean), &ctx).unwrap();

        assert_eq!(a.row, b.row, "the voice block moved the Colourist");
    }
}
