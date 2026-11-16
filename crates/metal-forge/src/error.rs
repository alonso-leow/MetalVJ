//! `naga` validates before `wgpu` sees anything (H.2's first non-negotiable),
//! and its result comes back as this — a normal `Result` value the agent
//! loop's tool-calling layer can hand straight back to the model, never a
//! panic.

use naga::SourceLocation;

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum CompileError {
    /// The WGSL failed to parse — a syntax error, in the caller's own
    /// source. Line/column are already translated back past the vertex
    /// prelude `compile` prepends, so they point at the caller's text.
    #[error("shader parse error at line {line}, column {column}: {message}")]
    Parse { message: String, line: u32, column: u32 },

    /// The WGSL parsed but failed `naga`'s validator — a type error, an
    /// undeclared binding, a stage-mismatch, etc.
    #[error("shader validation error at line {line}, column {column}: {message}")]
    Validation { message: String, line: u32, column: u32 },

    /// Compiled and validated, but declares no `floatsN: vec4<f32>` member
    /// on a `var<uniform>` at `@group(0) @binding(0)` — see the crate docs.
    /// Rejected here rather than left to silently render a static image:
    /// "a shader with baked-in constants is a painting, not a model."
    #[error(
        "shader declares no float-uniform block: expected a `var<uniform>` at \
         @group(0) @binding(0) whose struct has a `floats: vec4<f32>` member \
         (and optionally `floats2`, `floats3`, ...), the same shape every \
         model in metal-render uses"
    )]
    NoFloatBlock,

    /// Declares half of an effect's input: a texture at `@binding(1)` with no
    /// sampler at `@binding(2)`, or the other way round. An effect needs both
    /// (`fx.forge` binds the picture it is given there); a generator needs
    /// neither.
    #[error(
        "an effect shader needs both `@group(0) @binding(1) var src: texture_2d<f32>;` \
         and `@group(0) @binding(2) var samp: sampler;` — this one declares only one of them"
    )]
    EffectBindings,
}

impl CompileError {
    pub(crate) fn from_parse(message: String, loc: Option<SourceLocation>, prelude_lines: u32) -> Self {
        let (line, column) = translate(loc, prelude_lines);
        CompileError::Parse { message, line, column }
    }

    pub(crate) fn from_validation(
        message: String,
        loc: Option<SourceLocation>,
        prelude_lines: u32,
    ) -> Self {
        let (line, column) = translate(loc, prelude_lines);
        CompileError::Validation { message, line, column }
    }
}

/// naga's line numbers are 1-based against the *concatenated* source
/// (prelude + caller's WGSL). Subtract the prelude's line count so a caller
/// who never sees the prelude gets a number that lands in their own text —
/// clamped to 1 so an error that is technically inside the prelude (a stray
/// caller mistake referencing `vs_main`, say) still reports a sane location
/// instead of underflowing.
fn translate(loc: Option<SourceLocation>, prelude_lines: u32) -> (u32, u32) {
    match loc {
        Some(loc) => {
            let line = loc.line_number.saturating_sub(prelude_lines).max(1);
            (line, loc.line_position)
        }
        None => (0, 0),
    }
}
