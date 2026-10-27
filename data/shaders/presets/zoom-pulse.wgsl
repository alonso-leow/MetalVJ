// `presets/zoom-pulse` — the picture pushes in on every beat and settles back.
// The ingest onset is the pulse; with no onset the frame sits still.

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
    let onset = clamp(u.floats2.z, 0.0, 1.0);
    let z = 1.0 + 0.10 * onset;
    let uv = (in.uv - vec2<f32>(0.5, 0.5)) / z + vec2<f32>(0.5, 0.5);
    return textureSample(src, samp, clamp(uv, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0)));
}

