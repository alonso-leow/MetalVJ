// Shared thick 2D line pipeline: aspect-corrected NDC lines coloured by
// arc-length through grad4. Geoterrain (spiral math) and Escape (Verlet
// strings) both generate a vertex list on the CPU and draw it through this
// one pipeline instance.

struct LineUniform {
    color: ColorSlot,
    misc: vec4<f32>, // x: aspect (width/height)
};

@group(0) @binding(0) var<uniform> u: LineUniform;

struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) t: f32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) t: f32,
};

@vertex
fn vs_main(in: VIn) -> VOut {
    var out: VOut;
    var p = in.pos;
    p.x = p.x / u.misc.x;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.t = in.t;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(grad4(u.color, in.t), 1.0);
}
