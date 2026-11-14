// `fx.hardstutter`'s compose pass: the finished picture (`live_tex`, the post
// chain's output) against the frame the stutter is holding (`held_tex`).
//
// The CPU (`shared::stutter::gate`) has already decided *when* and *which
// stored frame*; this only mixes. `a.x` is the share of the live picture
// replaced by the hidden look (0 = untouched, 1 = a hard cut), `a.y` is
// hold (1) against black (0) inside that look, `a.z` how far the held frame
// is tinted by `tint.rgb`.

struct StutterUniform {
    a: vec4<f32>,
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: StutterUniform;
@group(0) @binding(1) var live_tex: texture_2d<f32>;
@group(0) @binding(2) var held_tex: texture_2d<f32>;
@group(0) @binding(3) var stutter_sampler: sampler;

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
    let live = textureSample(live_tex, stutter_sampler, in.uv);
    let held = textureSample(held_tex, stutter_sampler, in.uv);
    let blend = u.a.x;
    if (blend <= 0.0) {
        return live;
    }
    let held_rgb = held.rgb * mix(vec3<f32>(1.0), u.tint.rgb, u.a.z);
    let hidden_rgb = mix(vec3<f32>(0.0), held_rgb, u.a.y);
    let rgb = mix(live.rgb, hidden_rgb, blend);
    let alpha = mix(live.a, mix(live.a, held.a, u.a.y), blend);
    return vec4<f32>(rgb, alpha);
}
