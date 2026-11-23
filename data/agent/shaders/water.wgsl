// water — the blue space: a depth gradient, god rays from the surface, and
// motes drifting through it.
//
// A `forge` generator. One shader, several layers: put it on each object's
// mask and set each layer's `opacity` by how deep that object should read —
// the far wall faint and dark (the deep), the floor in between, the nearest
// objects full. The cut outside every mask is black, so a lower opacity is a
// deeper blue.
//
// What the ingest does to it:
//   rms    — the rays brighten with the music;
//   onset  — a hit lights the motes.

struct Floats {
    floats: vec4<f32>,   // x time (s), y aspect (w/h)
    floats2: vec4<f32>,  // rms, bandwidth, onset, centroid
    floats3: vec4<f32>,  // chroma C  C#  D   D#
    floats4: vec4<f32>,  // chroma E  F   F#  G
    floats5: vec4<f32>,  // chroma G# A   A#  B
};
@group(0) @binding(0) var<uniform> u: Floats;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let t = u.floats.x;
    let aspect = max(u.floats.y, 0.01);
    let rms = clamp(u.floats2.x, 0.0, 1.0);
    let onset = clamp(u.floats2.z, 0.0, 1.0);
    let uv = in.uv;

    // Depth: the surface (top) lighter, the bottom deep.
    let shallow = vec3<f32>(0.02, 0.32, 0.55);
    let deep = vec3<f32>(0.0, 0.05, 0.16);
    var col = mix(shallow, deep, smoothstep(0.0, 1.0, uv.y));

    // God rays: slanted bands from the surface, swaying, fading with depth.
    let x = uv.x * aspect + uv.y * 0.35;
    let rays = pow(0.5 + 0.5 * sin(x * 9.0 + sin(t * 0.3) * 2.0), 6.0) * 0.5
        + pow(0.5 + 0.5 * sin(x * 15.0 - t * 0.2), 8.0) * 0.35;
    col = col + vec3<f32>(0.25, 0.55, 0.70) * rays * (1.0 - uv.y) * (0.35 + 0.65 * rms);

    // The surface itself, shimmering.
    let shimmer = 0.5 + 0.5 * sin(uv.x * aspect * 40.0 + t * 3.0);
    col = col + vec3<f32>(0.30, 0.60, 0.80) * smoothstep(0.08, 0.0, uv.y) * shimmer * 0.3;

    // Motes: one in a few cells of a drifting grid.
    let g = vec2<f32>(uv.x * aspect, uv.y + t * 0.02) * 24.0;
    let cell = floor(g);
    let f = fract(g) - vec2<f32>(0.5, 0.5);
    let h = hash(cell);
    let at = vec2<f32>(h - 0.5, fract(h * 7.3) - 0.5) * 0.6;
    let mote = (1.0 - smoothstep(0.0, 0.08, length(f - at))) * step(0.82, h);
    col = col + vec3<f32>(0.5, 0.8, 0.9) * mote * (0.4 + 0.6 * onset);

    return vec4<f32>(col, 1.0);
}
