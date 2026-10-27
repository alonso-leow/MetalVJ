// Model 2 — Cyberspace. K layers of tiled glass panels with neon edges,
// each drifting at its own depth-scaled speed. One fullscreen pass, a
// bounded loop over layers inside the fragment shader — no per-layer draw
// call or geometry, since every layer is the same tiling function at a
// different scale and offset.

struct CyberUniform {
    panel_color: ColorSlot,
    neon_color: ColorSlot,
    background: ColorSlot,
    floats: vec4<f32>,  // n_capas, profundidad, vel_parallax, neon
    floats2: vec4<f32>, // glass, time, aspect, unused
};

@group(0) @binding(0) var<uniform> u: CyberUniform;

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
    let n = clamp(u.floats.x, 1.0, 8.0);
    let n_layers = i32(n);
    let depth = max(u.floats.y, 0.05);
    let speed = u.floats.z;
    let neon = u.floats.w;
    let glass = u.floats2.x;
    let time = u.floats2.y;
    let aspect = max(u.floats2.z, 0.01);

    var uv = (in.uv - 0.5) * vec2<f32>(aspect, 1.0) + 0.5;

    var accum = vec3<f32>(0.0, 0.0, 0.0);
    for (var i = 0; i < n_layers; i = i + 1) {
        let depth_factor = (f32(i) + 1.0) / n;
        let cell_size = mix(0.32, 0.06, pow(depth_factor, depth));
        let speed = mix(0.03, 0.35, pow(depth_factor, 1.0 / depth)) * speed;

        var p = uv;
        p.x = p.x + time * speed + f32(i) * 3.7;
        p.y = p.y + sin(time * 0.08 + f32(i) * 1.3) * 0.03;

        let cell = floor(p / cell_size);
        let local = fract(p / cell_size) - 0.5;
        let id = hash21(cell + f32(i) * 17.0);
        let present = step(0.35, id);

        let edge = max(abs(local.x), abs(local.y));
        let panel = present * (1.0 - smoothstep(0.30, 0.48, edge)) * glass * 0.30 * depth_factor;
        let ring = present * smoothstep(0.43, 0.47, edge) * (1.0 - smoothstep(0.47, 0.50, edge));

        let panel_col = grad4(u.panel_color, fract(id * 3.1 + depth_factor));
        let neon_col = grad4(u.neon_color, fract(id * 5.3));

        accum = accum + panel_col * panel + neon_col * ring * neon * 1.6 * depth_factor;
    }

    // The layers are already an accumulation of emissive panels and rings, so
    // the background joins them the same way — it is what `accum` used to
    // start from as an implicit black.
    let bg = backdrop_from_uv(u.background, in.uv, aspect);
    return vec4<f32>(bg + accum, 1.0);
}
