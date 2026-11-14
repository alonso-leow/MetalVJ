// `fx.level` (parity P-A, TD Level TOP). The CPU stand-in is
// `metal_perform::fx::parity::level`; keep the two in step.
//
//   floats2  black_in, white_in, black_out, white_out
//   floats3  gamma, brightness, contrast, opacity
//   floats4  invert (0/1), channel (0 all, 1 r, 2 g, 3 b)
struct Floats {
    floats: vec4<f32>,
    floats2: vec4<f32>,
    floats3: vec4<f32>,
    floats4: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

fn level(c: f32) -> f32 {
    var v = clamp((c - u.floats2.x) / max(u.floats2.y - u.floats2.x, 0.0001), 0.0, 1.0);
    v = pow(v, 1.0 / max(u.floats3.x, 0.001));
    v = u.floats2.z + v * (u.floats2.w - u.floats2.z);
    v = (v - 0.5) * u.floats3.z + 0.5 + u.floats3.y;
    if (u.floats4.x >= 0.5) {
        v = 1.0 - v;
    }
    return clamp(v, 0.0, 1.0);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let c = textureSample(src, samp, in.uv);
    let ch = i32(round(u.floats4.y));
    var out = c.rgb;
    let l = mix(c.rgb, vec3<f32>(level(c.r), level(c.g), level(c.b)), u.floats3.w);
    if (ch == 0 || ch == 1) { out.r = l.r; }
    if (ch == 0 || ch == 2) { out.g = l.g; }
    if (ch == 0 || ch == 3) { out.b = l.b; }
    return vec4<f32>(out, c.a);
}
