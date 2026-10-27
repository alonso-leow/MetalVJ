//! I.3/I.4: the verification gates — `lint -> check -> raster at three
//! moments`, applied to an already compiled shader.
//!
//! `the design notes` §I.3/§I.4: *"test snapshots at chosen times (the
//! opening, the characteristic movement, the close)"*, translated into our
//! float vocabulary as **minimum / default / maximum**. The reason for three
//! and not one, quoted as-is in the task: a shader that looks right at its
//! default value and breaks at an extreme is the common failure, and a
//! single sample hides it.
//!
//! `check` here is [`crate::compile`] (naga + the float block); `raster` is
//! [`crate::raster`] called three times, once per moment. [`verify`] never
//! collapses the three into a boolean — it returns a [`GateReport`] with each
//! moment's result separately, the same "`lint` never returns a bare bool"
//! convention `metal_graph::lint` already follows.

use crate::{compile, raster, CompileError, Pipeline, RasterError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    Min,
    Default,
    Max,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MomentResult {
    pub moment: Moment,
    pub ok: bool,
    /// Human-readable detail: "ok" if it passed, or the `raster` error if not.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GateReport {
    /// `Err` if it did not even compile — in that case there are no moments
    /// to trace, because there is no pipeline to rasterise any of them with.
    pub compile: Result<(), CompileError>,
    pub moments: Vec<MomentResult>,
}

impl GateReport {
    /// The one place where this report collapses into a boolean — on
    /// purpose, so the caller that only needs "do I go on or repair?" has
    /// it, without that replacing a look at `moments` to know *what* to
    /// repair.
    pub fn passed(&self) -> bool {
        self.compile.is_ok() && !self.moments.is_empty() && self.moments.iter().all(|m| m.ok)
    }

    /// A list of facts, not a bool — for the same use as
    /// `metal_graph::lint::Issue` or `ir::validate`: a human or a repair pass
    /// needs to see everything that failed at once.
    pub fn issues(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Err(e) = &self.compile {
            out.push(format!("compile: {e}"));
        }
        for m in &self.moments {
            if !m.ok {
                out.push(format!("{:?}: {}", m.moment, m.detail));
            }
        }
        out
    }
}

/// Runs `check` (compile) and then `raster` at the three moments on the same
/// already compiled [`Pipeline`] — each moment is independent: one failing
/// does not stop the other two, because the whole point is to see the three
/// results at once.
pub fn verify(
    wgsl: &str,
    floats_min: &[f32],
    floats_default: &[f32],
    floats_max: &[f32],
    w: u32,
    h: u32,
) -> GateReport {
    let pipeline = match compile(wgsl) {
        Ok(p) => p,
        Err(e) => return GateReport { compile: Err(e), moments: vec![] },
    };

    let moments = [(Moment::Min, floats_min), (Moment::Default, floats_default), (Moment::Max, floats_max)]
        .into_iter()
        .map(|(moment, floats)| raster_one_moment(&pipeline, moment, floats, w, h))
        .collect();

    GateReport { compile: Ok(()), moments }
}

fn raster_one_moment(pipeline: &Pipeline, moment: Moment, floats: &[f32], w: u32, h: u32) -> MomentResult {
    match raster(pipeline, floats, w, h) {
        Ok(_) => MomentResult { moment, ok: true, detail: "ok".to_string() },
        Err(e) => MomentResult { moment, ok: false, detail: format_raster_error(&e) },
    }
}

fn format_raster_error(e: &RasterError) -> String {
    format!("{e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SHADER: &str = r#"
struct Uniform {
    floats: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniform;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    return vec4<f32>(u.floats.x, u.floats.y, u.floats.z, 1.0);
}
"#;

    #[test]
    fn a_shader_fine_at_every_moment_passes_all_three() {
        let report = verify(VALID_SHADER, &[0.0, 0.0, 0.0], &[0.5, 0.5, 0.5], &[1.0, 1.0, 1.0], 8, 8);
        assert!(report.compile.is_ok());
        assert_eq!(report.moments.len(), 3);
        assert!(report.passed(), "{:?}", report.issues());
    }

    /// The case that motivates the three samples: a shader that rasterises
    /// fine at its default but breaks at an extreme. It is forced here in the
    /// deterministic way available without depending on the GPU's numerics
    /// (which never fail observably at the `Result` level — WGSL does not
    /// "blow up" on NaN, it just paints wrong): `floats_min` carries one
    /// float more than the declared block holds (capacity 4), so only the
    /// `Min` moment reports a real `raster` error while `Default` and `Max`
    /// pass. It is the same mechanism that would catch any real extreme: the
    /// gate evaluates the three moments independently and does not stop at
    /// the first.
    #[test]
    fn a_shader_broken_only_at_one_extreme_is_caught_without_hiding_the_others() {
        let floats_min = [0.0, 0.0, 0.0, 0.0, 0.0]; // 5 floats > capacity 4
        let floats_default = [0.5, 0.5, 0.5];
        let floats_max = [1.0, 1.0, 1.0];

        let report = verify(VALID_SHADER, &floats_min, &floats_default, &floats_max, 8, 8);

        assert!(report.compile.is_ok());
        assert!(!report.passed());

        let min = report.moments.iter().find(|m| m.moment == Moment::Min).unwrap();
        let default = report.moments.iter().find(|m| m.moment == Moment::Default).unwrap();
        let max = report.moments.iter().find(|m| m.moment == Moment::Max).unwrap();

        assert!(!min.ok, "the broken moment should fail");
        assert!(default.ok, "the default should not be affected by the minimum's failure");
        assert!(max.ok, "the maximum should not be affected by the minimum's failure");

        // The list of issues names the moment that failed, not a loose bool.
        assert!(report.issues().iter().any(|i| i.contains("Min")), "{:?}", report.issues());
    }

    #[test]
    fn a_shader_that_fails_to_compile_reports_no_moments() {
        let broken = "not valid wgsl at all {{{";
        let report = verify(broken, &[0.0], &[0.5], &[1.0], 8, 8);
        assert!(report.compile.is_err());
        assert!(report.moments.is_empty());
        assert!(!report.passed());
    }
}
