// Model 7 — Entrance. No walls, no tunnel: N luminous shapes drifting
// toward the camera on a wrapped depth axis, entirely GPU-generated —
// every particle's base position comes from hashing its instance index, so
// there is no per-frame CPU buffer to build.

struct ZoomUniform {
    color: ColorSlot,
    floats: vec4<f32>,  // vel_avance, densidad (informational), escala, variacion_paleta
    floats2: vec4<f32>, // time, aspect, depth_range, unused
};

@group(0) @binding(0) var<uniform> u: ZoomUniform;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) t: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let id = f32(ii);
    let depth_range = max(u.floats2.z, 0.5);

    let x0 = (hash11(id * 13.73) * 2.0 - 1.0) * 1.6;
    let y0 = (hash11(id * 91.31 + 7.0) * 2.0 - 1.0) * 1.6;
    let z0 = hash11(id * 3.77 + 5.0) * depth_range;

    let speed = u.floats.x;
    let time = u.floats2.x;
    var z = z0 - time * speed;
    z = z - floor(z / depth_range) * depth_range;
    z = max(z, 0.05);

    let scale = u.floats.z / z;

    var corner = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, 1.0)
    );
    let local = corner[vi];
    let center = vec2<f32>(x0, y0) * 0.9 / z;
    var p = center + local * scale;
    p.x = p.x / u.floats2.y;

    var out: VOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.local = local;
    let variation = clamp(u.floats.w, 0.0, 1.0);
    out.t = fract(id * 0.061 + (z / depth_range) * variation);
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let d = length(in.local);
    let mask = clamp(1.0 - smoothstep(0.6, 1.0, d), 0.0, 1.0);
    if (mask <= 0.001) {
        discard;
    }
    let col = grad4(u.color, in.t) * mask;
    return vec4<f32>(col, mask);
}
