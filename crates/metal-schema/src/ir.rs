//! I.3: the typed IR that sits between "plan" and "build".
//!
//! `the design notes` PART V, §I.3: *"A typed IR between agents, not
//! prose... the AI assembles a 3D scene or a mapping, first emits a plan in
//! JSON with a schema, and only then calls the tools that materialize it.
//! It makes the intent inspectable —and correctable by a human— before ten
//! nodes exist."* This module is that schema:
//! [`ShotIR`] is the only thing that crosses between the model that plans and
//! the code that builds. Nobody receives a paragraph hoping it gets understood.
//!
//! Two rules close the loop with `metal-forge`:
//!
//! - **Closed vocabulary** (§I.4): every [`OpCall::op`] has to be in
//!   [`KNOWN_OPS`] — the same list of names that `metal-forge::vocab`
//!   implements and tests. Nothing invented passes [`validate`].
//! - **Floats declared, never baked** (H.2): every [`OpCall`] has to declare
//!   at least one float name — an op without floats is a baked constant
//!   disguised as an operation.

use serde::{Deserialize, Serialize};

/// The closed vocabulary of WGSL operations for which `metal-forge::vocab`
/// has a tested implementation — see `the design notes` §I.4:
/// "SDF raymarching, noise, palette, mix, deformation". This list and
/// `metal_forge::vocab::VOCABULARY` are tested to be equal in
/// `crates/metal-forge/tests/vocab.rs`, so a name added on one side without
/// the other breaks the build instead of drifting silently.
pub const KNOWN_OPS: &[&str] = &["raymarch_sdf", "noise", "palette", "mix", "warp"];

/// A scene/clip plan, as the model emits it before touching
/// any construction tool. Serializable to JSON with a fixed schema: that is
/// literally the point — a paragraph can't be validated, this can.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShotIR {
    pub id: String,
    /// Fixed clip duration, in seconds. §I.3, hard rule: once set, a repair
    /// may not touch this field to cover up a defect — see [`check_repair`].
    pub duration_secs: f32,
    pub ops: Vec<OpCall>,
}

/// A call to an operation in the closed vocabulary, with its floats
/// declared by name (never by baked value — the concrete values are resolved
/// later, in the `raster` step).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpCall {
    pub op: String,
    #[serde(default)]
    pub floats: Vec<String>,
}

/// Validates a [`ShotIR`] against the closed rules of §I.3/§I.4. Never
/// returns a boolean — it follows the same convention as `metal_graph::lint`:
/// "invalid" is a list of facts, not a single bit, because an invented `op`
/// and a zero duration are two different repairs that an operator needs to
/// see at the same time.
pub fn validate(ir: &ShotIR) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    if ir.id.trim().is_empty() {
        errors.push("ShotIR.id must not be empty".to_string());
    }
    if !(ir.duration_secs.is_finite()) || ir.duration_secs <= 0.0 {
        errors.push(format!("duration_secs must be finite and positive, got {}", ir.duration_secs));
    }
    if ir.ops.is_empty() {
        errors.push("ShotIR.ops must not be empty — a plan without operations builds nothing".to_string());
    }
    for (i, op) in ir.ops.iter().enumerate() {
        if !KNOWN_OPS.contains(&op.op.as_str()) {
            errors.push(format!(
                "ops[{i}]: {:?} is not in the closed vocabulary {KNOWN_OPS:?}",
                op.op
            ));
        }
        if op.floats.is_empty() {
            errors.push(format!(
                "ops[{i}] ({:?}): declares no float — an op without floats is a baked constant, \
                 and \"the soul of each animation here is variability\" (H.2)",
                op.op
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// One construction attempt — the IR plus the WGSL it produced — as it
/// exists before and after a repair pass. [`check_repair`] compares two of
/// these to enforce the hard rule of §I.3/§I.4/H.7 in code.
#[derive(Debug, Clone, PartialEq)]
pub struct RepairAttempt {
    pub ir: ShotIR,
    pub wgsl: String,
}

/// **The rule that must live in code, not in prose** (I.3, and already cited in
/// `data/agent/system_prompt.md` §9): *"never change a fixed duration
/// to cover up a defect... touching it to hide a problem elsewhere
/// silently breaks that contract."*
///
/// Mechanically: if `duration_secs` is the only thing that changed between `before` and
/// `after` — neither the shader nor the IR `ops` were touched — the repair
/// is rejected. Stretching or shortening the duration without touching the shader, the floats or
/// the ops is, by definition, covering up the symptom instead of fixing the
/// defect.
pub fn check_repair(before: &RepairAttempt, after: &RepairAttempt) -> Result<(), String> {
    let duration_changed = before.ir.duration_secs != after.ir.duration_secs;
    let shader_changed = before.wgsl != after.wgsl;
    let ops_changed = before.ir.ops != after.ir.ops;

    if duration_changed && !shader_changed && !ops_changed {
        return Err(format!(
            "repair rejected: the only change was duration_secs ({} -> {}). \
             The hard rule forbids stretching or shortening a fixed duration to cover up a defect — \
             the fix has to touch the shader or the IR ops, not the duration contract.",
            before.ir.duration_secs, after.ir.duration_secs
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_ir() -> ShotIR {
        ShotIR {
            id: "shot-1".to_string(),
            duration_secs: 4.0,
            ops: vec![OpCall { op: "warp".to_string(), floats: vec!["amp".to_string(), "freq".to_string()] }],
        }
    }

    #[test]
    fn a_valid_ir_is_accepted() {
        assert_eq!(validate(&valid_ir()), Ok(()));
    }

    #[test]
    fn an_op_outside_the_closed_vocabulary_is_rejected() {
        let mut ir = valid_ir();
        ir.ops[0].op = "invent_a_shader".to_string();
        let errors = validate(&ir).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("closed vocabulary")), "{errors:?}");
    }

    #[test]
    fn an_op_with_no_declared_floats_is_rejected() {
        let mut ir = valid_ir();
        ir.ops[0].floats.clear();
        let errors = validate(&ir).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("baked constant")), "{errors:?}");
    }

    #[test]
    fn a_nonpositive_duration_is_rejected() {
        let mut ir = valid_ir();
        ir.duration_secs = 0.0;
        let errors = validate(&ir).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("duration_secs")), "{errors:?}");

        let mut ir2 = valid_ir();
        ir2.duration_secs = -1.0;
        let errors2 = validate(&ir2).unwrap_err();
        assert!(errors2.iter().any(|e| e.contains("duration_secs")), "{errors2:?}");
    }

    #[test]
    fn ops_empty_is_rejected() {
        let mut ir = valid_ir();
        ir.ops.clear();
        let errors = validate(&ir).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("ops")), "{errors:?}");
    }

    #[test]
    fn empty_id_is_rejected() {
        let mut ir = valid_ir();
        ir.id = "".to_string();
        let errors = validate(&ir).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("id")), "{errors:?}");
    }

    #[test]
    fn multiple_problems_are_all_reported_at_once() {
        let ir = ShotIR { id: "".to_string(), duration_secs: 0.0, ops: vec![] };
        let errors = validate(&ir).unwrap_err();
        // empty id + invalid duration + empty ops: three facts, not one.
        assert_eq!(errors.len(), 3, "{errors:?}");
    }

    #[test]
    fn a_repair_that_only_stretches_duration_is_rejected() {
        let before = RepairAttempt { ir: valid_ir(), wgsl: "shader v1".to_string() };
        let mut after_ir = valid_ir();
        after_ir.duration_secs = 8.0; // stretched to double, nothing else changed
        let after = RepairAttempt { ir: after_ir, wgsl: "shader v1".to_string() };

        let err = check_repair(&before, &after).unwrap_err();
        assert!(err.contains("duration_secs"), "{err}");
    }

    #[test]
    fn a_repair_that_fixes_the_shader_and_also_adjusts_duration_is_allowed() {
        // The rule forbids covering up the symptom by stretching ONLY the duration; if
        // the shader also changed, it is not "covering up", it is a real repair
        // that also adjusted the duration contract.
        let before = RepairAttempt { ir: valid_ir(), wgsl: "broken shader".to_string() };
        let mut after_ir = valid_ir();
        after_ir.duration_secs = 8.0;
        let after = RepairAttempt { ir: after_ir, wgsl: "fixed shader".to_string() };

        assert_eq!(check_repair(&before, &after), Ok(()));
    }

    #[test]
    fn a_repair_that_only_fixes_the_shader_is_allowed() {
        let before = RepairAttempt { ir: valid_ir(), wgsl: "broken shader".to_string() };
        let after = RepairAttempt { ir: valid_ir(), wgsl: "fixed shader".to_string() };
        assert_eq!(check_repair(&before, &after), Ok(()));
    }
}
