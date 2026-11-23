// Corales: the variable-width curve strips (halo and stroke). Vertex colour is
// computed on the CPU from the trazo slot; the look kit is applied here so the
// strokes grade exactly like the fill pass behind them.

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
    let s = abs(in.side);
    let crisp = 1.0 - smoothstep(0.6, 1.0, s);
    let glow = exp(-3.5 * s * s) * (1.0 - smoothstep(0.85, 1.0, s));
    let w = mix(crisp, glow, in.soft);
    let rgb = kit_look(in.color.rgb, u.look.x, u.look.y, u.look.z, u.look.w);
    return vec4<f32>(rgb, in.color.a * w * u.misc.y);
}
