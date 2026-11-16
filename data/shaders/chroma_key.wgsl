// `fx.chroma_key` (parity P-A) — key a colour out to alpha. The CPU stand-in
// is `metal_perform::fx::parity::chroma_key`; keep the two in step.
//
//   floats2.xyz  key colour (0..1 sRGB)   floats2.w  tolerance
//   floats3.x    softness                 floats3.y  spill
//   floats3.z    invert (0/1)
struct Floats {
    floats: vec4<f32>,
    floats2: vec4<f32>,
    floats3: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

fn cbcr(c: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(
        -0.168736 * c.r - 0.331264 * c.g + 0.5 * c.b,
        0.5 * c.r - 0.418688 * c.g - 0.081312 * c.b,
    );
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    var c = textureSample(src, samp, in.uv);
    let key = u.floats2.xyz;
    let tol = u.floats2.w;
    let soft = u.floats3.x;
    let d = distance(cbcr(c.rgb), cbcr(key));
    var a = smoothstep(tol * 0.5, (tol + soft) * 0.5 + 0.0001, d);
    if (u.floats3.z >= 0.5) {
        a = 1.0 - a;
    }
    // Spill: pull the key's dominant channel down toward the other two.
    var rgb = c.rgb;
    if (key.g >= key.r && key.g >= key.b) {
        let o = max(rgb.r, rgb.b);
        if (rgb.g > o) { rgb.g = rgb.g - (rgb.g - o) * u.floats3.y; }
    } else if (key.r >= key.b) {
        let o = max(rgb.g, rgb.b);
        if (rgb.r > o) { rgb.r = rgb.r - (rgb.r - o) * u.floats3.y; }
    } else {
        let o = max(rgb.r, rgb.g);
        if (rgb.b > o) { rgb.b = rgb.b - (rgb.b - o) * u.floats3.y; }
    }
    return vec4<f32>(rgb, c.a * a);
}
