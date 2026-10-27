// `presets/ripple` — concentric waves leave the centre and push the picture
// out of place. Time drives the wave; the ingest onset kicks its amplitude.

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
    let aspect = max(u.floats.y, 0.01);
    let t = u.floats.x;
    let onset = clamp(u.floats2.z, 0.0, 1.0);
    let p = (in.uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);
    let r = length(p);
    let dir = select(vec2<f32>(0.0, 0.0), p / r, r > 0.0001);
    let amp = 0.008 + 0.012 * onset;
    let d = sin(r * 26.0 - t * 4.0) * amp;
    let uv = in.uv + vec2<f32>(dir.x / aspect, dir.y) * d;
    return textureSample(src, samp, clamp(uv, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0)));
}

