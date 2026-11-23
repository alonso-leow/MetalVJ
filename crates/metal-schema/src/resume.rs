//! I.3: the resume table by directory state.
//!
//! `the design notes` §I.3: *"a resume table that says, for each state of the
//! directory, which step to continue from. An interrupted agent resumes
//! instead of redoing."* [`resume_step`] is that table: it looks only at what
//! is on disk — `ir.json`, `shader.wgsl`, `gate_report.json` — never at
//! conversation memory, because the conversation is exactly what an
//! interrupted agent does not have.

use metal_vfs as fs;
use std::path::Path;

use crate::ir::{validate, ShotIR};

/// Where to resume from, given what is in a working directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeStep {
    /// There is no `ir.json`, or the one there does not validate against [`validate`]: the
    /// plan must be (re)emitted before touching any build tool.
    Plan,
    /// There is a valid IR but no `shader.wgsl`: choose from the closed
    /// vocabulary (or compose one) and build.
    Build,
    /// There is a shader but no `gate_report.json`: run lint -> check ->
    /// raster at the three moments.
    Gate,
    /// `gate_report.json` exists and records a failure: a repair pass,
    /// subject to the duration rule (`ir::check_repair`).
    Repair,
    /// `gate_report.json` exists and passed: nothing to resume.
    Done,
}

/// What a gate pass (`metal-forge::gate`) writes to `gate_report.json` so that
/// [`resume_step`] can read it without re-running `raster`. Deliberately minimal — `passed` plus the list of
/// facts, the same convention of "never a lone boolean" that
/// `metal_graph::lint` and `ir::validate` already follow, except that here the
/// aggregate boolean is needed to decide the next step without re-parsing the
/// messages.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GateOutcome {
    pub passed: bool,
    #[serde(default)]
    pub issues: Vec<String>,
}

/// Decides the resume step by looking only at `dir` — no in-memory state.
/// File names are fixed on purpose: they are the contract between whoever
/// writes the IR, whoever builds the shader, and whoever runs the gates.
pub fn resume_step(dir: &Path) -> ResumeStep {
    let ir_ok = fs::read_to_string(dir.join("ir.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<ShotIR>(&s).ok())
        .map(|ir| validate(&ir).is_ok())
        .unwrap_or(false);
    if !ir_ok {
        return ResumeStep::Plan;
    }

    if !metal_vfs::exists(&dir.join("shader.wgsl")) {
        return ResumeStep::Build;
    }

    match fs::read_to_string(dir.join("gate_report.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<GateOutcome>(&s).ok())
    {
        None => ResumeStep::Gate,
        Some(outcome) if outcome.passed => ResumeStep::Done,
        Some(_) => ResumeStep::Repair,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::OpCall;

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("metal-schema-resume-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create tmp dir");
        dir
    }

    fn valid_ir() -> ShotIR {
        ShotIR {
            id: "s".to_string(),
            duration_secs: 3.0,
            ops: vec![OpCall { op: "noise".to_string(), floats: vec!["seed".to_string()] }],
        }
    }

    #[test]
    fn empty_directory_resumes_at_plan() {
        let dir = tmp_dir("empty");
        assert_eq!(resume_step(&dir), ResumeStep::Plan);
    }

    #[test]
    fn an_invalid_ir_on_disk_still_resumes_at_plan() {
        let dir = tmp_dir("bad-ir");
        fs::write(dir.join("ir.json"), r#"{"id":"","duration_secs":0.0,"ops":[]}"#).unwrap();
        assert_eq!(resume_step(&dir), ResumeStep::Plan);
    }

    #[test]
    fn a_valid_ir_with_no_shader_resumes_at_build() {
        let dir = tmp_dir("valid-ir-no-shader");
        fs::write(dir.join("ir.json"), serde_json::to_string(&valid_ir()).unwrap()).unwrap();
        assert_eq!(resume_step(&dir), ResumeStep::Build);
    }

    #[test]
    fn a_shader_with_no_gate_report_resumes_at_gate() {
        let dir = tmp_dir("shader-no-gate");
        fs::write(dir.join("ir.json"), serde_json::to_string(&valid_ir()).unwrap()).unwrap();
        fs::write(dir.join("shader.wgsl"), "struct U { floats: vec4<f32> };").unwrap();
        assert_eq!(resume_step(&dir), ResumeStep::Gate);
    }

    #[test]
    fn a_failed_gate_report_resumes_at_repair() {
        let dir = tmp_dir("failed-gate");
        fs::write(dir.join("ir.json"), serde_json::to_string(&valid_ir()).unwrap()).unwrap();
        fs::write(dir.join("shader.wgsl"), "struct U { floats: vec4<f32> };").unwrap();
        let outcome = GateOutcome { passed: false, issues: vec!["breaks at the maximum".to_string()] };
        fs::write(dir.join("gate_report.json"), serde_json::to_string(&outcome).unwrap()).unwrap();
        assert_eq!(resume_step(&dir), ResumeStep::Repair);
    }

    #[test]
    fn a_passed_gate_report_resumes_at_done() {
        let dir = tmp_dir("passed-gate");
        fs::write(dir.join("ir.json"), serde_json::to_string(&valid_ir()).unwrap()).unwrap();
        fs::write(dir.join("shader.wgsl"), "struct U { floats: vec4<f32> };").unwrap();
        let outcome = GateOutcome { passed: true, issues: vec![] };
        fs::write(dir.join("gate_report.json"), serde_json::to_string(&outcome).unwrap()).unwrap();
        assert_eq!(resume_step(&dir), ResumeStep::Done);
    }
}
