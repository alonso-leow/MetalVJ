// `fx.math` (parity P-A) — per-pixel channel math. The CPU stand-in is
// `metal_perform::fx::parity::math_pixel`; keep the two in step.
//
//   floats2  mul r g b a        floats3  add r g b a
//   floats4  pow, abs, invert, clamp (flags 0/1)
//   floats5.x luma → alpha (0/1)
//   floats5.y swizzle, base 6: s0 + 6 s1 + 36 s2 + 216 s3 (0..3 rgba, 4 zero, 5 one)
struct Floats {
    floats: vec4<f32>,
    floats2: vec4<f32>,
    floats3: vec4<f32>,
    floats4: vec4<f32>,
    floats5: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

fn pick(c: vec4<f32>, i: i32) -> f32 {
    if (i < 4) { return c[i]; }
    if (i == 4) { return 0.0; }
    return 1.0;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let c = textureSample(src, samp, in.uv);
    let code = i32(round(u.floats5.y));
    var v = vec4<f32>(
        pick(c, code % 6),
        pick(c, (code / 6) % 6),
        pick(c, (code / 36) % 6),
        pick(c, (code / 216) % 6),
    );
    v = v * u.floats2 + u.floats3;
    if (u.floats4.y >= 0.5) { v = abs(v); }
    var rgb = pow(max(v.rgb, vec3<f32>(0.0)), vec3<f32>(u.floats4.x));
    if (u.floats4.z >= 0.5) { rgb = vec3<f32>(1.0) - rgb; }
    v = vec4<f32>(rgb, v.a);
    if (u.floats4.w >= 0.5) { v = clamp(v, vec4<f32>(0.0), vec4<f32>(1.0)); }
    if (u.floats5.x >= 0.5) {
        v.a = clamp(dot(v.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
    }
    return clamp(v, vec4<f32>(0.0), vec4<f32>(1.0));
}
