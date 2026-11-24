// Fineline: the pen strokes. `side` is the signed distance from the centre
// line in pixels, `soft` the true half width in pixels, so a 1 px line is
// anti-aliased by coverage instead of being widened. Vertex colour is the
// ink (depth-graded on the CPU); the look kit grades it like the paper.

struct LineU {
    look: vec4<f32>, // brillo, contraste, tono, saturacion
    misc: vec4<f32>, // aspect, opacidad, 0, 0
};

@group(0) @binding(0) var<uniform> u: LineU;

struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) side: f32,
    @location(2) soft: f32,
    @location(3) color: vec4<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) side: f32,
    @location(1) soft: f32,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_main(in: VIn) -> VOut {
    var out: VOut;
    out.clip = vec4<f32>(in.pos.x / max(u.misc.x, 0.01), in.pos.y, 0.0, 1.0);
    out.side = in.side;
    out.soft = in.soft;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let d = abs(in.side);
    let cov = clamp(in.soft + 0.5 - d, 0.0, 1.0);
    // Neon ink: a hot core, the ink itself, and a soft halo outside it that is
    // stronger on the heavier strokes (silhouettes glow, hatching barely does).
    let out_d = max(d - in.soft, 0.0);
    let halo = exp(-out_d * out_d / 4.5) * 0.32 * clamp(in.soft / 1.2, 0.25, 1.0);
    let core = clamp(1.0 - d / max(in.soft, 0.5), 0.0, 1.0);
    var ink = kit_look(in.color.rgb, u.look.x, u.look.y, u.look.z, u.look.w);
    ink = mix(ink, vec3<f32>(1.0), core * core * 0.28 * clamp(in.soft - 0.4, 0.0, 1.0));
    let a = max(cov, halo);
    return vec4<f32>(ink, in.color.a * a * u.misc.y);
}
