//! O.2: the agent turn as a composition level.
//!
//! # Why this module exists
//!
//! `the design notes` §II.3 found three limits and said they were one:
//! *"the agent turn is a composition level that is not modelled today"*.
//! Before this there were two levels and nothing in between:
//!
//! - the **tool call**, which [`crate::harness::Harness`] executes — synchronous,
//!   stateless between calls and **without access to the model**;
//! - the **loop** of [`crate::agent_loop::run_turn`], which sends messages and
//!   returns the last one.
//!
//! Everything that needs the model to *choose between two calls* falls through
//! that crack. The two-pass `locate` loop (H.5c) is the clear case: the four
//! pieces of [`crate::locate`] are pure and tested, but the protocol asks the
//! model to look at a grid, choose strips, look at a crop, and choose again.
//! `Harness` cannot host it because it has no one to talk to. **It lives in the
//! turn or it does not live.**
//!
//! The same goes for the orchestrator: §I.3 left the four gates built and tested
//! separately — `lint`, [`metal_schema::ir::validate`],
//! [`metal_forge::gate::verify`] at min/default/max, and
//! [`metal_schema::ir::check_repair`] — and what was missing was the live loop
//! that chains them. Chaining is the work; the pieces are not touched.
//!
//! # The two things this module makes structurally impossible
//!
//! 1. **Answering "done" without having looked** (§I.4). It is not a warning in
//!    the system prompt: [`Turn::may_finish`] is consulted before the final
//!    message is returned, and a turn with an unverified construction does not
//!    finish. A model that says "done" gets back what it is missing.
//! 2. **Estimating coordinates in the second `locate` pass** (§H.5c). The
//!    protocol is a state machine — [`Locating`] — and its only input is a list
//!    of strips. There is no way to pass it a number that is not a strip index,
//!    so the 6.5% → 2.3% that hyperframes measured does not depend on the model
//!    remembering the rule.
//!
//! # No conversation memory
//!
//! [`Turn::resume`] looks at `resume_step` over the working directory and
//! nothing else. A turn interrupted halfway resumes from what is on disk, which
//! is exactly what an interrupted agent *has* — the conversation is the only
//! thing it safely lost.

use std::path::{Path, PathBuf};

use metal_schema::ir::{check_repair, validate, RepairAttempt, ShotIR};
use metal_schema::resume::{resume_step, GateOutcome, ResumeStep};

use crate::images::TurnImages;
use crate::locate::{compose, crop_and_scale, draw_box, overlay_grid, strips_to_region, Region};

/// The coarse grid of the first pass. 9×9, as §H.5c.
pub const COARSE_GRID: u32 = 9;
/// The fine grid of the second pass, over the enlarged crop. 6×6.
pub const FINE_GRID: u32 = 6;
/// How much the crop is enlarged before the fine grid is overlaid on it.
/// Enlarging is half the trick: choosing strips over 40 pixels of height is no
/// better than estimating coordinates.
pub const FINE_SIZE: u32 = 512;

// ---------------------------------------------------------------------------
// locate: the state machine of the two passes
// ---------------------------------------------------------------------------

/// What the turn needs from the model right now to keep locating.
///
/// Each variant carries the PNG that must be looked at. The model never returns
/// coordinates: it returns strips ([`Locating::choose`]) or a confirmation
/// ([`Locating::confirm`]).
#[derive(Debug, Clone, PartialEq)]
pub enum LocateStep {
    /// Pass 1: the [`COARSE_GRID`] grid over the whole frame. The model lists
    /// **all** the strips the target touches, vertical and horizontal.
    Coarse { png: Vec<u8>, grid: u32 },
    /// Pass 2: the crop of the chosen region, enlarged, with the [`FINE_GRID`]
    /// grid. Strips again — **not** coordinates, which is where the whole
    /// advantage would be lost.
    Fine { png: Vec<u8>, grid: u32 },
    /// Step 4: the box drawn over the original frame, to look at. This step is
    /// not skipped; it turns a silent failure into a cheap retry, and the model
    /// already knows which way to correct.
    Confirm { png: Vec<u8>, region: Region },
    /// Finished: the region, already composed into the original image space.
    Done { region: Region },
}

/// The protocol of §H.5c as a state, not as a function.
///
/// The reason it is a machine and not a call: between the coarse and the fine
/// pass **the model has to speak**, and this module does not call any model
/// (same rule as [`crate::locate`], which does not either). The turn advances it
/// with what the model answered.
#[derive(Debug, Clone)]
pub struct Locating {
    /// The original frame, intact. Everything is composed against this.
    source: Vec<u8>,
    /// What is being searched for. It is kept so the question can be repeated if
    /// the model chooses badly and a retry is needed.
    target: String,
    stage: Stage,
}

#[derive(Debug, Clone, PartialEq)]
enum Stage {
    Coarse,
    /// The region that came out of the coarse pass, in the original's space.
    Fine { coarse: Region },
    Confirm { region: Region },
    Done { region: Region },
}

impl Locating {
    /// Starts the protocol over a camera frame.
    ///
    /// **Ambiguity before geometry** (§H.5c): if `target` can match several
    /// instances — "a tile" when there are forty — no function here fixes it.
    /// That is resolved with the `question` tool *before* arriving here, and the
    /// shot corpus has to show it.
    pub fn begin(source_png: Vec<u8>, target: impl Into<String>) -> Locating {
        Locating { source: source_png, target: target.into(), stage: Stage::Coarse }
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    /// What to look at now. Idempotent: calling it twice without choosing
    /// anything in between returns the same image.
    pub fn step(&self) -> Result<LocateStep, String> {
        match &self.stage {
            Stage::Coarse => Ok(LocateStep::Coarse {
                png: overlay_grid(&self.source, COARSE_GRID)?,
                grid: COARSE_GRID,
            }),
            Stage::Fine { coarse } => {
                let crop = crop_and_scale(&self.source, coarse, FINE_SIZE, FINE_SIZE)?;
                Ok(LocateStep::Fine { png: overlay_grid(&crop, FINE_GRID)?, grid: FINE_GRID })
            }
            Stage::Confirm { region } => {
                Ok(LocateStep::Confirm { png: draw_box(&self.source, region)?, region: *region })
            }
            Stage::Done { region } => Ok(LocateStep::Done { region: *region }),
        }
    }

    /// The model's only input while a grid is on screen: the **1-indexed** indices
    /// of the strips the target touches, equal to the numbers drawn. There is no
    /// convention translation in between, and there is no way to pass a
    /// coordinate.
    ///
    /// Error if called outside a grid pass — reaching confirmation and choosing
    /// strips again is the model lost, and returning that as an error is cheaper
    /// than accepting it.
    pub fn choose(&mut self, cols: &[u32], rows: &[u32]) -> Result<LocateStep, String> {
        match &self.stage {
            Stage::Coarse => {
                let local = strips_to_region(COARSE_GRID, cols, rows)?;
                // The coarse pass is chosen over the whole image, so its local
                // region is already in the global space.
                self.stage = Stage::Fine { coarse: local };
            }
            Stage::Fine { coarse } => {
                let local = strips_to_region(FINE_GRID, cols, rows)?;
                // §H.5c step 3: `global = region_origin + local × region_size`.
                self.stage = Stage::Confirm { region: compose(coarse, &local) };
            }
            Stage::Confirm { .. } => {
                return Err("there is no grid left to choose: look at the drawn box and confirm, or retry with `restart`"
                    .to_string())
            }
            Stage::Done { .. } => return Err("this locate has already finished".to_string()),
        }
        self.step()
    }

    /// Answer to step 4. `true` closes the protocol; `false` goes back to the
    /// coarse pass, which is what "the box came out shifted" means.
    ///
    /// That this is mandatory is the point: without it, `Done` would be reachable
    /// without anyone having looked at the result.
    pub fn confirm(&mut self, ok: bool) -> Result<LocateStep, String> {
        let Stage::Confirm { region } = &self.stage else {
            return Err("there is no box to confirm yet".to_string());
        };
        self.stage = if ok { Stage::Done { region: *region } } else { Stage::Coarse };
        self.step()
    }

    /// The final region, or `None` while the protocol is still open.
    pub fn finished(&self) -> Option<Region> {
        match &self.stage {
            Stage::Done { region } => Some(*region),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// the orchestrator: the four gates chained
// ---------------------------------------------------------------------------

/// The rasterization gate, as an injectable seam.
///
/// The real implementer is [`ForgeGate`], which calls
/// `metal_forge::gate::verify` — it compiles with `naga` and rasterizes at
/// min/default/max on a GPU. Making it a trait is not abstraction for its own
/// sake, there are two concrete reasons: it allows testing **the chaining** —
/// which is the only new thing here — without a GPU, just as
/// [`crate::agent_loop::ToolExecutor`] allows testing the loop without a
/// provider; and it keeps the orchestrator compilable without the `gpu` feature,
/// which is the same reason `metal-forge` is an optional dependency of this crate.
///
/// It returns [`GateOutcome`] and not the `GateReport` of `metal-forge` on
/// purpose: `GateOutcome` is what gets written to `gate_report.json` and what
/// `resume_step` reads, so the trait speaks the vocabulary of the on-disk
/// contract rather than that of one implementation.
pub trait ShaderGate {
    fn verify(&mut self, wgsl: &str, floats: &GateFloats) -> GateOutcome;
}

/// The three moments of §I.3 translated into this repo's float vocabulary. Three
/// and not one because *"a shader that looks right at its default value and
/// breaks at an extreme is the common failure, and a single sample hides it"*.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GateFloats {
    pub min: Vec<f32>,
    pub default: Vec<f32>,
    pub max: Vec<f32>,
    pub width: u32,
    pub height: u32,
}

/// The real gate: `metal-forge` on its own device, which is the one that cannot
/// bring down the show being projected on a wall (H.2).
#[cfg(feature = "gpu")]
pub struct ForgeGate;

#[cfg(feature = "gpu")]
impl ShaderGate for ForgeGate {
    fn verify(&mut self, wgsl: &str, f: &GateFloats) -> GateOutcome {
        let report = metal_forge::gate::verify(wgsl, &f.min, &f.default, &f.max, f.width, f.height);
        // `issues()` already carries the compile error with line and column, or
        // the moment that broke — never a lone boolean, which is the convention
        // that `lint`, `ir::validate` and `GateReport` share.
        GateOutcome { passed: report.passed(), issues: report.issues() }
    }
}

/// What the turn asks of the model after running the gates.
#[derive(Debug, Clone, PartialEq)]
pub enum BuildStep {
    /// There is no valid IR on disk: emit the plan **before** touching any
    /// construction tool. `problems` is empty in a clean directory and full when
    /// the IR present does not validate — and it carries *all* the problems, not
    /// the first, which is the convention of `ir::validate`.
    Plan { problems: Vec<String> },
    /// There is valid IR and no shader: choose from the closed vocabulary and
    /// write it.
    Build { ir: Box<ShotIR> },
    /// The gate ran and failed. `issues` names the moment that broke
    /// (Min/Default/Max) or the compile error with line and column.
    Repair { issues: Vec<String> },
    /// Nothing to do: it compiled and passed the three moments.
    Done,
}

/// Fixed file names, the same ones [`metal_schema::resume::resume_step`] reads —
/// they are the contract between whoever writes the IR, whoever writes the shader
/// and whoever runs the gates.
const IR_FILE: &str = "ir.json";
const SHADER_FILE: &str = "shader.wgsl";
const REPORT_FILE: &str = "gate_report.json";

/// The state that survives a tool call and dies with the turn.
pub struct Turn {
    /// H.3: *"an attached image persists for the whole turn, not for one call"*.
    /// If the AI captures the camera, makes the grid, tests it and captures again,
    /// the four images have to be alive in the same context or the refinement
    /// loop cannot compare before and after.
    pub images: TurnImages,
    /// Where the construction in progress lives: `ir.json`, `shader.wgsl`,
    /// `gate_report.json`.
    workdir: PathBuf,
    /// The in-flight `locate` protocol, if any.
    locating: Option<Locating>,
    /// The last attempt that went through the gate, so that
    /// [`metal_schema::ir::check_repair`] has something to compare the next one
    /// against. It is the only thing this type remembers *beyond* what is on disk,
    /// and it is the minimum needed to apply the duration rule: disk only keeps
    /// the current state, not the previous one.
    last: Option<RepairAttempt>,
    /// Whether this turn started a construction. A turn that only answered a
    /// question or read a parameter has nothing to verify, and requiring a gate
    /// from it would be requiring a GPU to tell the time.
    building: bool,
}

impl Turn {
    pub fn new(workdir: impl Into<PathBuf>) -> Turn {
        Turn {
            images: TurnImages::new(),
            workdir: workdir.into(),
            locating: None,
            last: None,
            building: false,
        }
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    // -- locate ---------------------------------------------------------

    /// Opens the §H.5c protocol over a frame. It replaces any existing one: two
    /// localizations at once in a turn would be two targets competing for the
    /// same strips.
    pub fn locate_begin(&mut self, source_png: Vec<u8>, target: impl Into<String>) -> Result<LocateStep, String> {
        let l = Locating::begin(source_png, target);
        let step = l.step()?;
        self.locating = Some(l);
        Ok(step)
    }

    pub fn locating(&mut self) -> Option<&mut Locating> {
        self.locating.as_mut()
    }

    /// The located region, if the protocol reached the end **with the
    /// confirmation done**.
    pub fn located(&self) -> Option<Region> {
        self.locating.as_ref().and_then(Locating::finished)
    }

    // -- the gates ------------------------------------------------------

    /// Writes the plan and validates it before a single node exists.
    ///
    /// §I.3: *"emit a schema-backed JSON plan first, and only then call the tools
    /// that materialize it"*. An IR that does not validate is not saved — if it
    /// were, `resume_step` would read it on the next turn and still say `Plan`,
    /// but the model would have believed it had moved forward.
    pub fn plan(&mut self, ir: &ShotIR) -> Result<(), Vec<String>> {
        validate(ir)?;
        self.building = true;
        std::fs::create_dir_all(&self.workdir).map_err(|e| vec![e.to_string()])?;
        let text = serde_json::to_string_pretty(ir).map_err(|e| vec![e.to_string()])?;
        std::fs::write(self.workdir.join(IR_FILE), text).map_err(|e| vec![e.to_string()])?;
        Ok(())
    }

    /// Saves the WGSL of the current attempt. It compiles nothing: compiling is the
    /// gate, and keeping them apart is what lets a broken shader be a normal result
    /// of the loop (H.2) instead of a write error.
    pub fn build(&mut self, wgsl: &str) -> Result<(), String> {
        self.building = true;
        std::fs::create_dir_all(&self.workdir).map_err(|e| e.to_string())?;
        std::fs::write(self.workdir.join(SHADER_FILE), wgsl).map_err(|e| e.to_string())
    }

    /// **The live loop.** Runs the gate that corresponds to the state of the
    /// directory and returns what remains to be done.
    ///
    /// The order is that of §I.3 and none is skipped: `resume_step` decides where
    /// we are → the IR has to validate → there has to be a shader → it compiles
    /// and rasterizes at the three moments → if it failed, repair subject to
    /// [`check_repair`].
    ///
    /// The report is written to `gate_report.json` on every pass, so a turn
    /// interrupted here resumes at exactly the same point without rasterizing
    /// again.
    pub fn gate(&mut self, gate: &mut dyn ShaderGate, floats: &GateFloats) -> Result<BuildStep, String> {
        match resume_step(&self.workdir) {
            ResumeStep::Plan => Ok(BuildStep::Plan { problems: self.plan_problems() }),
            ResumeStep::Build => {
                let ir = self.read_ir()?;
                Ok(BuildStep::Build { ir: Box::new(ir) })
            }
            ResumeStep::Gate | ResumeStep::Repair => {
                let ir = self.read_ir()?;
                let wgsl = std::fs::read_to_string(self.workdir.join(SHADER_FILE))
                    .map_err(|e| format!("could not read {SHADER_FILE}: {e}"))?;

                // The hard rule of §I.3, applied **from inside the orchestrator**
                // and not only in its unit test: if the previous attempt exists and
                // the only thing that changed is the duration, the repair is rejected
                // here, before spending a GPU on rasterizing it.
                let attempt = RepairAttempt { ir: ir.clone(), wgsl: wgsl.clone() };
                if let Some(before) = &self.last {
                    check_repair(before, &attempt)?;
                }

                let outcome = gate.verify(&wgsl, floats);
                let text = serde_json::to_string_pretty(&outcome).map_err(|e| e.to_string())?;
                std::fs::write(self.workdir.join(REPORT_FILE), text).map_err(|e| e.to_string())?;
                self.last = Some(attempt);

                if outcome.passed {
                    Ok(BuildStep::Done)
                } else {
                    Ok(BuildStep::Repair { issues: outcome.issues })
                }
            }
            ResumeStep::Done => Ok(BuildStep::Done),
        }
    }

    /// Which step an interrupted turn resumes from, looking **only** at the
    /// directory. No conversation memory, which is what an interrupted agent does
    /// not have.
    pub fn resume(&self) -> ResumeStep {
        resume_step(&self.workdir)
    }

    /// The gate that makes it impossible to answer without having looked (§I.4).
    ///
    /// It is consulted before the turn's final message is returned. A turn that
    /// built nothing always passes; one that built something has to have reached
    /// [`ResumeStep::Done`], and one with a half-finished `locate` has to have
    /// closed it — an unconfirmed box is exactly the silent failure that step 4 of
    /// §H.5c exists to turn into a retry.
    pub fn may_finish(&self) -> Result<(), String> {
        if let Some(l) = &self.locating {
            if l.finished().is_none() {
                return Err(format!(
                    "the turn cannot close: `locate({:?})` was left half-done. \
                     Look at the drawn box and confirm it, or choose strips again.",
                    l.target()
                ));
            }
        }
        if !self.building {
            return Ok(());
        }
        match self.resume() {
            ResumeStep::Done => Ok(()),
            step => Err(format!(
                "the turn cannot close: the construction is stuck at {step:?}. \
                 The gates of I.3 are not skipped — nobody answers \"done\" without having looked \
                 at the raster at the three moments.",
            )),
        }
    }

    fn read_ir(&self) -> Result<ShotIR, String> {
        let text = std::fs::read_to_string(self.workdir.join(IR_FILE))
            .map_err(|e| format!("could not read {IR_FILE}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("{IR_FILE} did not parse: {e}"))
    }

    /// Why `resume_step` said `Plan`: the full list from
    /// [`metal_schema::ir::validate`], or empty if there is simply no file.
    fn plan_problems(&self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(self.workdir.join(IR_FILE)) else {
            return Vec::new();
        };
        match serde_json::from_str::<ShotIR>(&text) {
            Ok(ir) => validate(&ir).err().unwrap_or_default(),
            Err(e) => vec![format!("{IR_FILE} did not parse: {e}")],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_schema::ir::OpCall;

    fn workdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("metal-agent-turn-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create workdir");
        dir
    }

    fn ir(duration: f32) -> ShotIR {
        ShotIR {
            id: "shot-1".to_string(),
            duration_secs: duration,
            ops: vec![OpCall { op: "warp".to_string(), floats: vec!["amp".to_string()] }],
        }
    }

    /// A scripted gate: returns the reports loaded into it, in order. It exists to
    /// test the **chaining** without a GPU — what `metal_forge::gate::verify` does
    /// is already tested in its own crate.
    struct ScriptedGate {
        results: Vec<bool>,
        calls: usize,
    }

    impl ScriptedGate {
        fn new(results: Vec<bool>) -> ScriptedGate {
            ScriptedGate { results, calls: 0 }
        }
    }

    impl ShaderGate for ScriptedGate {
        fn verify(&mut self, _wgsl: &str, _floats: &GateFloats) -> GateOutcome {
            let ok = *self.results.get(self.calls).unwrap_or(&true);
            self.calls += 1;
            GateOutcome {
                passed: ok,
                // In the shape that `GateReport::issues` produces: the moment that
                // broke, named.
                issues: if ok { vec![] } else { vec!["Min: breaks at the minimum".to_string()] },
            }
        }
    }

    // -- the orchestrator -----------------------------------------------

    #[test]
    fn a_turn_that_built_nothing_may_finish() {
        // Not every turn builds something: an answered question or a read
        // parameter has no gate to pass.
        let t = Turn::new(workdir("nothing"));
        assert_eq!(t.may_finish(), Ok(()));
    }

    #[test]
    fn the_orchestrator_walks_plan_build_gate_to_done() {
        let dir = workdir("happy");
        let mut t = Turn::new(&dir);
        let mut gate = ScriptedGate::new(vec![true]);
        let floats = GateFloats { width: 8, height: 8, ..Default::default() };

        // Clean directory: a plan is needed, and there are no problems to report
        // because there is no IR to criticize.
        assert_eq!(
            t.gate(&mut gate, &floats).unwrap(),
            BuildStep::Plan { problems: vec![] }
        );

        t.plan(&ir(4.0)).unwrap();
        assert_eq!(t.gate(&mut gate, &floats).unwrap(), BuildStep::Build { ir: Box::new(ir(4.0)) });

        t.build("shader v1").unwrap();
        assert_eq!(t.gate(&mut gate, &floats).unwrap(), BuildStep::Done);
        assert_eq!(t.may_finish(), Ok(()));
        // The report is on disk, so an interrupted turn here does not rasterize
        // again.
        assert_eq!(t.resume(), ResumeStep::Done);
    }

    #[test]
    fn an_invalid_plan_is_not_written_and_reports_every_problem_at_once() {
        let dir = workdir("bad-plan");
        let mut t = Turn::new(&dir);
        let broken = ShotIR { id: String::new(), duration_secs: 0.0, ops: vec![] };

        let problems = t.plan(&broken).unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(!dir.join(IR_FILE).exists(), "an IR that does not validate is not saved");
    }

    #[test]
    fn a_failed_gate_asks_for_a_repair_and_blocks_the_turn_from_finishing() {
        let dir = workdir("failed-gate");
        let mut t = Turn::new(&dir);
        let mut gate = ScriptedGate::new(vec![false]);
        let floats = GateFloats { width: 8, height: 8, ..Default::default() };

        t.plan(&ir(4.0)).unwrap();
        t.build("broken shader").unwrap();

        match t.gate(&mut gate, &floats).unwrap() {
            BuildStep::Repair { issues } => {
                assert!(issues.iter().any(|i| i.contains("Min")), "{issues:?}");
            }
            other => panic!("expected Repair, got {other:?}"),
        }

        // And this is the half that matters: the turn cannot close.
        let err = t.may_finish().unwrap_err();
        assert!(err.contains("Repair"), "{err}");
    }

    #[test]
    fn a_repair_that_only_stretches_the_duration_is_rejected_from_inside_the_orchestrator() {
        // The negative case that O.2 explicitly asks for: `check_repair` already has
        // its unit test in `metal-schema`; what is tested here is that the
        // orchestrator actually applies it, before spending the gate.
        let dir = workdir("duration-repair");
        let mut t = Turn::new(&dir);
        let mut gate = ScriptedGate::new(vec![false, true]);
        let floats = GateFloats { width: 8, height: 8, ..Default::default() };

        t.plan(&ir(4.0)).unwrap();
        t.build("broken shader").unwrap();
        assert!(matches!(t.gate(&mut gate, &floats).unwrap(), BuildStep::Repair { .. }));

        // "Repair": double the duration and the same shader, word for word.
        t.plan(&ir(8.0)).unwrap();
        let err = t.gate(&mut gate, &floats).unwrap_err();
        assert!(err.contains("duration_secs"), "{err}");
        assert_eq!(gate.calls, 1, "the gate should not have run for the rejected repair");

        // And the turn still cannot close.
        assert!(t.may_finish().is_err());
    }

    #[test]
    fn a_real_repair_that_touches_the_shader_passes_the_gate_and_closes_the_turn() {
        let dir = workdir("real-repair");
        let mut t = Turn::new(&dir);
        let mut gate = ScriptedGate::new(vec![false, true]);
        let floats = GateFloats { width: 8, height: 8, ..Default::default() };

        t.plan(&ir(4.0)).unwrap();
        t.build("broken shader").unwrap();
        assert!(matches!(t.gate(&mut gate, &floats).unwrap(), BuildStep::Repair { .. }));

        t.build("fixed shader").unwrap();
        assert_eq!(t.gate(&mut gate, &floats).unwrap(), BuildStep::Done);
        assert_eq!(gate.calls, 2);
        assert_eq!(t.may_finish(), Ok(()));
    }

    #[test]
    fn an_interrupted_turn_resumes_from_the_directory_and_not_from_memory() {
        let dir = workdir("interrupted");
        {
            let mut t = Turn::new(&dir);
            t.plan(&ir(4.0)).unwrap();
            t.build("shader v1").unwrap();
        } // the turn dies here — no conversation is kept

        let fresh = Turn::new(&dir);
        assert_eq!(fresh.resume(), ResumeStep::Gate, "there is a shader and no report: the gate is missing");
    }

    #[test]
    fn a_directory_with_a_broken_ir_reports_why_rather_than_just_saying_plan() {
        let dir = workdir("broken-ir");
        std::fs::write(dir.join(IR_FILE), r#"{"id":"x","duration_secs":1.0,"ops":[]}"#).unwrap();
        let mut t = Turn::new(&dir);
        let mut gate = ScriptedGate::new(vec![]);

        match t.gate(&mut gate, &GateFloats::default()).unwrap() {
            BuildStep::Plan { problems } => {
                assert!(problems.iter().any(|p| p.contains("ops")), "{problems:?}");
            }
            other => panic!("expected Plan, got {other:?}"),
        }
    }

    // -- locate ---------------------------------------------------------

    /// A small, real RGBA PNG, so that `overlay_grid`/`crop_and_scale` have
    /// something to decode.
    fn png(w: u32, h: u32) -> Vec<u8> {
        use png::{BitDepth, ColorType, Encoder};
        let mut out = Vec::new();
        {
            let mut enc = Encoder::new(&mut out, w, h);
            enc.set_color(ColorType::Rgba);
            enc.set_depth(BitDepth::Eight);
            let mut writer = enc.write_header().unwrap();
            writer.write_image_data(&vec![128u8; (w * h * 4) as usize]).unwrap();
        }
        out
    }

    #[test]
    fn locate_walks_coarse_then_fine_then_confirm() {
        let mut l = Locating::begin(png(64, 64), "the plant");

        match l.step().unwrap() {
            LocateStep::Coarse { grid, .. } => assert_eq!(grid, COARSE_GRID),
            other => panic!("expected Coarse, got {other:?}"),
        }

        // Pass 1: the target touches the central strips.
        match l.choose(&[4, 5, 6], &[4, 5]).unwrap() {
            LocateStep::Fine { grid, .. } => assert_eq!(grid, FINE_GRID),
            other => panic!("expected Fine, got {other:?}"),
        }
        assert!(l.finished().is_none(), "the fine pass cannot finish the protocol");

        // Pass 2: strips again, never coordinates.
        let region = match l.choose(&[2, 3], &[3]).unwrap() {
            LocateStep::Confirm { region, .. } => region,
            other => panic!("expected Confirm, got {other:?}"),
        };
        assert!(l.finished().is_none(), "without looking at the box there is no result");

        l.confirm(true).unwrap();
        assert_eq!(l.finished(), Some(region));
    }

    #[test]
    fn the_fine_pass_is_composed_back_into_the_original_image_space() {
        let mut l = Locating::begin(png(64, 64), "one tile");
        // Upper-left quadrant, coarse: strips 1..=3 of 9 -> 0..1/3.
        l.choose(&[1, 2, 3], &[1, 2, 3]).unwrap();
        // Fine: the last strip of 6 -> 5/6..1 *within that third*.
        let region = match l.choose(&[6], &[6]).unwrap() {
            LocateStep::Confirm { region, .. } => region,
            other => panic!("{other:?}"),
        };
        // global = 0 + 5/6 * 1/3 = 0.2777…, and never beyond the third.
        assert!((region.x0 - 5.0 / 18.0).abs() < 1e-6, "{region:?}");
        assert!(region.x1 <= 1.0 / 3.0 + 1e-6, "the fine pass cannot leave the coarse one: {region:?}");
    }

    #[test]
    fn a_rejected_box_goes_back_to_the_coarse_pass_rather_than_to_an_estimate() {
        let mut l = Locating::begin(png(64, 64), "the front of the house");
        l.choose(&[1], &[1]).unwrap();
        l.choose(&[1], &[1]).unwrap();
        match l.confirm(false).unwrap() {
            LocateStep::Coarse { grid, .. } => assert_eq!(grid, COARSE_GRID),
            other => panic!("a rejection has to reopen the grid, got {other:?}"),
        }
        assert!(l.finished().is_none());
    }

    #[test]
    fn strips_outside_the_grid_are_a_model_answering_badly_and_come_back_as_an_error() {
        let mut l = Locating::begin(png(64, 64), "x");
        assert!(l.choose(&[], &[]).is_err(), "choosing nothing is not a choice");
        assert!(l.choose(&[10], &[1]).is_err(), "strip 10 does not exist in a grid of 9");
    }

    #[test]
    fn a_half_finished_locate_blocks_the_turn_from_finishing() {
        // The other side of the same rule: `may_finish` does not let a turn close
        // with an unlooked-at box.
        let mut t = Turn::new(workdir("locate-open"));
        t.locate_begin(png(64, 64), "the plant").unwrap();
        let err = t.may_finish().unwrap_err();
        assert!(err.contains("locate"), "{err}");

        let l = t.locating().unwrap();
        l.choose(&[5], &[5]).unwrap();
        l.choose(&[3], &[3]).unwrap();
        l.confirm(true).unwrap();
        assert_eq!(t.may_finish(), Ok(()));
        assert!(t.located().is_some());
    }
}
