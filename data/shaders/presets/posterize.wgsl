// `presets/posterize` — each channel reduced to a few flat levels. Fewer
// levels on loud passages, a smoother picture on quiet ones.

struct Floats {
    floats: vec4<f32>,   // x time (s), y aspect (w/h)
    floats2: vec4<f32>,  // rms, bandwidth, onset, centroid
    floats3: vec4<f32>,
    floats4: vec4<f32>,
    floats5: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let rms = clamp(u.floats2.x, 0.0, 1.0);
    let levels = mix(9.0, 3.0, rms);
    let c = textureSample(src, samp, in.uv);
    let q = floor(c.rgb * levels) / (levels - 1.0);
    return vec4<f32>(clamp(q, vec3<f32>(0.0, 0.0, 0.0), vec3<f32>(1.0, 1.0, 1.0)), c.a);
}

