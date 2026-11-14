//! I.3/I.4: the closed vocabulary of WGSL operations.
//!
//! `the design notes` §I.4: *"When writing a new shader, the AI picks
//! from a named list of operations already written and tested — SDF
//! raymarch, noise, palette, mix, warp — before inventing."* Each entry here
//! is exactly that: a complete shader (it declares its own float block,
//! never baked-in constants — the same contract [`crate::compile`] demands
//! of any WGSL) with a name and documented floats, tested in
//! `tests/vocab.rs` against a real `compile` + `raster`, not just parsed.
//!
//! The names here and [`metal_schema::ir::KNOWN_OPS`] are the same list by
//! definition — `tests/vocab.rs` compares them and fails if someone adds one
//! without the other. Picking from this list is what replaces "writing free
//! WGSL" in the IR construction step (I.3).

/// One operation of the closed vocabulary: its name, its complete WGSL
/// (uniform + `fs_main`), and the names its floats are documented under in
/// the IR — no baked-in values on either side.
pub struct VocabOp {
    pub name: &'static str,
    pub wgsl: &'static str,
    pub float_names: &'static [&'static str],
}

const RAYMARCH_SDF: &str = r#"
struct U { floats: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;

fn sdf_sphere(p: vec3<f32>, r: f32) -> f32 {
    return length(p) - r;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let ndc = in.uv * 2.0 - vec2<f32>(1.0, 1.0);
    let ro = vec3<f32>(0.0, 0.0, u.floats.y);
    let rd = normalize(vec3<f32>(ndc, -1.0));
    var t = 0.0;
    var hit = false;
    for (var i = 0; i < 64; i = i + 1) {
        let p = ro + rd * t;
        let d = sdf_sphere(p, u.floats.x);
        if (d < 0.001) {
            hit = true;
            break;
        }
        t = t + d;
        if (t > 20.0) {
            break;
        }
    }
    if (hit) {
        return vec4<f32>(1.0 - t * 0.05, 0.4, 0.8, 1.0);
    }
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
"#;

const NOISE: &str = r#"
struct U { floats: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233)) + u.floats.y) * 43758.5453);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let n = hash(in.uv * (u.floats.x * 40.0 + 1.0));
    return vec4<f32>(n, n, n, 1.0);
}
"#;

const PALETTE: &str = r#"
struct U { floats: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let t = in.uv.x + u.floats.x;
    let freq = vec3<f32>(u.floats.y, u.floats.y, u.floats.y);
    let col = vec3<f32>(0.5, 0.5, 0.5)
        + vec3<f32>(0.5, 0.5, 0.5) * cos(6.28318 * (freq * t + vec3<f32>(0.0, 0.33, 0.67)));
    return vec4<f32>(col, 1.0);
}
"#;

const MIX: &str = r#"
struct U { floats: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let a = vec3<f32>(in.uv, 0.5);
    let b = vec3<f32>(1.0 - in.uv.x, 1.0 - in.uv.y, 0.2);
    let c = mix(a, b, clamp(u.floats.x, 0.0, 1.0));
    return vec4<f32>(c, 1.0);
}
"#;

const WARP: &str = r#"
struct U { floats: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let warped = in.uv + vec2<f32>(
        sin(in.uv.y * u.floats.y * 10.0) * u.floats.x,
        cos(in.uv.x * u.floats.y * 10.0) * u.floats.x,
    );
    return vec4<f32>(warped.x, warped.y, 0.5, 1.0);
}
"#;

/// The whole vocabulary. The order carries no meaning — lookup is by name,
/// not by index (unlike `registry::all_models()`, which is positional and
/// append-only; nothing here is stored by position).
pub const VOCABULARY: &[VocabOp] = &[
    VocabOp { name: "raymarch_sdf", wgsl: RAYMARCH_SDF, float_names: &["radius", "camera_z"] },
    VocabOp { name: "noise", wgsl: NOISE, float_names: &["scale", "seed"] },
    VocabOp { name: "palette", wgsl: PALETTE, float_names: &["phase", "frequency"] },
    VocabOp { name: "mix", wgsl: MIX, float_names: &["factor"] },
    VocabOp { name: "warp", wgsl: WARP, float_names: &["amplitude", "frequency"] },
];

/// Looks up a vocabulary operation by name — the entry point an IR builder
/// would use instead of accepting free WGSL.
pub fn find(name: &str) -> Option<&'static VocabOp> {
    VOCABULARY.iter().find(|op| op.name == name)
}
