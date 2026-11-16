// `presets/kaleido` — six-fold kaleidoscope around the centre, slowly turning.
// The angle is folded into one sixth of a turn, then mirrored, so the six
// wedges meet without a seam.

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
    let seg = 6.283185307 / 6.0;
    var p = (in.uv * 2.0 - vec2<f32>(1.0, 1.0)) * vec2<f32>(aspect, 1.0);
    let r = length(p);
    var a = atan2(p.y, p.x) + t * 0.1;
    a = a - seg * floor(a / seg);
    a = abs(a - seg * 0.5);
    p = r * vec2<f32>(cos(a), sin(a));
    let uv = (p / vec2<f32>(aspect, 1.0) + vec2<f32>(1.0, 1.0)) * 0.5;
    return textureSample(src, samp, fract(uv));
}

