// `presets/rgb-split` — red, green and blue read the picture at three offsets
// along x. The offset widens with the music.

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
    let off = vec2<f32>(0.004 + 0.02 * rms, 0.0);
    let r = textureSample(src, samp, in.uv + off).r;
    let g = textureSample(src, samp, in.uv).g;
    let b = textureSample(src, samp, in.uv - off).b;
    let a = textureSample(src, samp, in.uv).a;
    return vec4<f32>(r, g, b, a);
}

