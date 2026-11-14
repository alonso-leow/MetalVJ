// Ghosting: what an old frame leaves behind.
//
// `out = max(current, history * decay)`, and the max is the point. A `mix`
// would dim the frame that is actually on screen in proportion to how long the
// trail is, so the whole picture would fade as the effect came up. Taking the
// brighter of the two leaves the live frame untouched at full brightness and
// lets everything behind it decay — which is what a trail is.
//
// `decay` is computed on the CPU as `exp(-dt / tau)` from the wall clock, not
// from a frame count: both apps render on demand, so a per-frame fade would
// give a different trail length on a busy machine than on an idle one, and a
// different one in the grid than in solo at the same setting.

struct TrailUniform {
    // x: decay 0..1, y, z, w spare
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: TrailUniform;
@group(0) @binding(1) var current_tex: texture_2d<f32>;
@group(0) @binding(2) var history_tex: texture_2d<f32>;
@group(0) @binding(3) var trail_sampler: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let xy = p[vi];
    var out: VOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let decay = u.params.x;
    let current = textureSample(current_tex, trail_sampler, in.uv).rgb;
    let history = textureSample(history_tex, trail_sampler, in.uv).rgb;
    return vec4<f32>(max(current, history * decay), 1.0);
}
