// cup-caustics — an `fx.forge` effect: whatever picture it is given, seen
// under water.
//
// Wired after a source (source → fx.forge → output), with the source cut to
// a secondary object's contour — a cup on the table — so the object reads
// as sunk: its picture wobbles as if seen through moving water, veins of
// light drift over it, and it goes bluer toward its bottom.
//
// What the ingest does to it:
//   rms    — more wobble, brighter caustics;
//   onset  — a hit flashes the caustics.

struct Floats {
    floats: vec4<f32>,   // x time (s), y aspect (w/h)
    floats2: vec4<f32>,  // rms, bandwidth, onset, centroid
    floats3: vec4<f32>,  // chroma C  C#  D   D#
    floats4: vec4<f32>,  // chroma E  F   F#  G
    floats5: vec4<f32>,  // chroma G# A   A#  B
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

// Caustic veins: a few rounds of domain warping, each adding a thin bright
// line where a sine crosses zero.
fn caustic(p: vec2<f32>, t: f32) -> f32 {
    var q = p;
    var c = 0.0;
    for (var i: i32 = 0; i < 4; i = i + 1) {
        let fi = f32(i);
        q = q + 0.3 * vec2<f32>(sin(q.y * 1.7 + t * 0.8 + fi * 1.3), cos(q.x * 1.9 - t * 0.7 + fi * 2.1));
        c = c + 1.0 / (1.0 + 28.0 * abs(sin(q.x * 1.3 + q.y * 0.9)));
    }
    return c * 0.25;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let t = u.floats.x;
    let aspect = max(u.floats.y, 0.01);
    let rms = clamp(u.floats2.x, 0.0, 1.0);
    let onset = clamp(u.floats2.z, 0.0, 1.0);

    // Refraction: the picture wobbles as if seen through moving water.
    let amp = 0.004 + 0.014 * rms;
    let wobble = amp * vec2<f32>(sin(in.uv.y * 21.0 + t * 2.3), cos(in.uv.x * 17.0 * aspect - t * 1.9));
    let base = textureSample(src, samp, clamp(in.uv + wobble, vec2<f32>(0.0), vec2<f32>(1.0))).rgb;

    // Caustics drifting over the object.
    let light = caustic(in.uv * vec2<f32>(aspect, 1.0) * 7.0, t) * (0.25 + 0.8 * rms + 0.7 * onset);

    // Deeper is bluer: red and green fade down the object.
    let depth = in.uv.y;
    let water = vec3<f32>(0.45 - 0.25 * depth, 0.78 - 0.2 * depth, 1.0);
    let col = base * water + light * vec3<f32>(0.55, 0.95, 1.0);
    return vec4<f32>(col, 1.0);
}
