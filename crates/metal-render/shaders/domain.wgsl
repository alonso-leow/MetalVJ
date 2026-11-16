// Model 3 — Domain: old-TV glitches. No input texture — the
// "picture" being torn is itself generated, since this model has nothing
// upstream of it to disturb.

struct InterUniform {
    color: ColorSlot,
    floats: vec4<f32>,  // scanline, distortion, rgb_split, prob
    floats2: vec4<f32>, // duration, time, unused, unused
};

@group(0) @binding(0) var<uniform> u: InterUniform;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let xy = p[vi];
    var out: VOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let scanline = u.floats.x;
    let distortion = u.floats.y;
    let rgb_split = u.floats.z;
    let prob = u.floats.w;
    let duration = max(u.floats2.x, 0.05);
    let time = u.floats2.y;

    var uv = in.uv;

    // Rows tear independently: a per-row, per-burst hash decides whether
    // that row is glitching this burst, and by how much.
    let row = floor(uv.y * 240.0);
    let burst = floor(time * duration * 10.0);
    let row_roll = hash21(vec2<f32>(row, burst));
    let glitching = step(1.0 - prob, row_roll);
    let shift = (hash21(vec2<f32>(row, burst + 71.0)) * 2.0 - 1.0) * distortion * 0.15 * glitching;
    uv.x = uv.x + shift;

    // The signal being torn: this model has no upstream frame, so the
    // "picture" is a soft moving band field — enough structure for the
    // glitches and split to read as damage rather than noise on nothing.
    let base = clamp(0.5 + 0.5 * sin((uv.x * 9.0 + uv.y * 3.0 + time * 0.6) * 6.2831), 0.0, 1.0);

    let split = rgb_split * 0.02;
    let r = grad4(u.color, clamp(base + split, 0.0, 1.0)).r;
    let g = grad4(u.color, base).g;
    let b = grad4(u.color, clamp(base - split, 0.0, 1.0)).b;
    var col = vec3<f32>(r, g, b);

    let scan = 0.5 + 0.5 * cos(uv.y * 700.0);
    col = col * (1.0 - scanline * 0.6 * (1.0 - scan));

    let grain = hash21(uv * 400.0 + time);
    col = mix(col, vec3<f32>(grain, grain, grain), glitching * 0.5 * distortion);

    return vec4<f32>(col, 1.0);
}
