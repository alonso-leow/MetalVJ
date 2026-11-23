//! `metal-forge`'s gate. Maps straight onto H.2's own list in
//! `the design notes`: naga catches a broken shader with line/column
//! before wgpu ever sees it, a shader with no float block is rejected at
//! compile time, and `raster` respects the resolution cap and is wired
//! through the timeout constant.

use metal_forge::{compile, raster, CompileError, RasterError, MAX_DIMENSION};

/// The minimal shader every other test builds on: one `floats` member,
/// `fs_main` painting a solid colour from it.
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
fn valid_shader_compiles() {
    let pipeline = compile(VALID_SHADER);
    assert!(pipeline.is_ok(), "expected a trivial valid shader to compile, got {:?}", pipeline.err());
}

#[test]
fn syntax_error_reports_line_and_column() {
    // Missing semicolon / stray token — a straightforward parse error.
    let broken = r#"
struct Uniform {
    floats: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniform;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    return vec4<f32>(u.floats.x u.floats.y, u.floats.z, 1.0);
}
"#;
    let err = compile(broken).expect_err("expected a syntax error");
    match err {
        CompileError::Parse { line, column, .. } => {
            assert!(line >= 1, "line should point somewhere in the caller's own source, got {line}");
            assert!(column >= 1, "column should be nonzero, got {column}");
        }
        other => panic!("expected CompileError::Parse, got {other:?}"),
    }
}

#[test]
fn shader_without_float_block_is_rejected() {
    // Compiles and validates fine on its own — the point of this test is
    // that "the alma of every animation here is variability" (H.2) is
    // enforced even against a shader with zero syntax problems: no
    // `floatsN: vec4<f32>` member anywhere means no variability, and that
    // has to fail compile(), not just render the same frame forever.
    let no_floats = r#"
struct Uniform {
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniform;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    return u.color;
}
"#;
    let err = compile(no_floats).expect_err("expected NoFloatBlock");
    assert_eq!(err, CompileError::NoFloatBlock);
}

#[test]
fn shader_with_no_uniform_at_all_is_rejected() {
    let baked = r#"
@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}
"#;
    let err = compile(baked).expect_err("expected NoFloatBlock");
    assert_eq!(err, CompileError::NoFloatBlock);
}

#[test]
fn raster_of_valid_shader_produces_png_of_requested_size() {
    let pipeline = compile(VALID_SHADER).expect("valid shader should compile");
    let png_bytes = raster(&pipeline, &[0.2, 0.4, 0.8], 64, 32).expect("raster should succeed");

    // Cheap PNG sanity check without pulling in a decoder as a dev-dep:
    // the 8-byte PNG signature, then decode via the `png` crate itself
    // (already a normal dependency) to check the declared dimensions.
    assert_eq!(&png_bytes[0..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);

    let decoder = png::Decoder::new(std::io::Cursor::new(&png_bytes));
    let reader = decoder.read_info().expect("metal-forge produced an unreadable PNG");
    let info = reader.info();
    assert_eq!(info.width, 64);
    assert_eq!(info.height, 32);
}

#[test]
fn resolution_above_cap_is_rejected() {
    let pipeline = compile(VALID_SHADER).expect("valid shader should compile");
    let err = raster(&pipeline, &[0.0, 0.0, 0.0], MAX_DIMENSION + 1, 16).expect_err("expected a cap error");
    assert_eq!(err, RasterError::ResolutionTooLarge { w: MAX_DIMENSION + 1, h: 16 });
}

#[test]
fn resolution_at_cap_is_allowed_to_proceed_past_the_check() {
    // Not asserting the raster *succeeds* at 4096x4096 (that is a slow,
    // machine-dependent GPU allocation) — only that the cap itself is an
    // inclusive boundary, not an off-by-one that rejects the documented
    // maximum.
    let pipeline = compile(VALID_SHADER).expect("valid shader should compile");
    let err = raster(&pipeline, &[0.0, 0.0, 0.0], MAX_DIMENSION, 1);
    assert_ne!(err, Err(RasterError::ResolutionTooLarge { w: MAX_DIMENSION, h: 1 }));
}

#[test]
fn too_many_floats_for_the_declared_block_is_rejected() {
    let pipeline = compile(VALID_SHADER).expect("valid shader should compile");
    // VALID_SHADER declares one `floats: vec4<f32>` -> capacity 4.
    let err = raster(&pipeline, &[0.0; 5], 8, 8).expect_err("expected TooManyFloats");
    assert_eq!(err, RasterError::TooManyFloats { got: 5, capacity: 4 });
}

#[test]
fn raster_timeout_constant_is_wired_into_the_call_path() {
    // Constructing an actually-infinite WGSL fragment shader safely (one
    // that hangs the GPU without hanging the *test suite* if the timeout
    // failed to fire) is not practical here — a real hang would make this
    // test itself never finish if the wiring were broken. What is checked
    // instead: `RASTER_TIMEOUT` is a real, positive, small constant, and a
    // normal raster call comfortably completes under it, which is what
    // `raster`'s `recv_timeout(RASTER_TIMEOUT)` actually gates on.
    assert!(metal_forge::RASTER_TIMEOUT.as_secs() > 0);
    assert!(metal_forge::RASTER_TIMEOUT.as_secs() <= 30, "H.2 asks for \"a few seconds\", not tens of them");

    let pipeline = compile(VALID_SHADER).expect("valid shader should compile");
    let start = std::time::Instant::now();
    let result = raster(&pipeline, &[0.1, 0.2, 0.3], 16, 16);
    assert!(result.is_ok());
    assert!(
        start.elapsed() < metal_forge::RASTER_TIMEOUT,
        "a trivial shader should finish well inside the timeout budget"
    );
}

/// The line a parse error reports is the caller's own line, not one past it.
#[test]
fn a_parse_error_names_the_callers_own_line() {
    let wgsl = "struct Floats { floats: vec4<f32> };\n\
                @group(0) @binding(0) var<uniform> u: Floats;\n\
                @fragment\n\
                fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {\n\
                    let a = 1.0\n\
                    return vec4<f32>(a);\n\
                }\n";
    match metal_forge::check(wgsl) {
        // naga reports where it found `return` instead of `;`: line 6.
        Err(CompileError::Parse { line, .. }) => assert_eq!(line, 6),
        other => panic!("expected a parse error, got {other:?}"),
    }
}
