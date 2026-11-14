// Model 14 — Vaporgrid. The receding 3D grid of the vapourwave poster: a
// ground plane of lines running to a horizon, a sun behind it, and the whole
// thing scrolling toward the camera forever.
//
// Two pipelines out of one module. `vs_bg`/`fs_bg` lay the sky and the sun
// down as a fullscreen triangle with no depth, then `vs_main`/`fs_main` draw
// the grid over it. A grid alone is lines on black; the horizon is most of
// what makes this the aesthetic it was asked for.
//
// **It is asked as Entrance's child and it is Chamber's.** The ask says "based
// on super zoom", and what it takes from Entrance is the feeling — infinite
// forward motion, things arriving out of the horizon. What it takes from
// Chamber is the code: a displaced height field evaluated per vertex. There
// are no particles in it, so starting from `entrance.rs` would have started
// from billboards. See `docs/NEXT.md` §5.
//
// The scroll is an offset plus a wrap, not recycled geometry: the crossing
// lines are generated one spacing longer than the frame needs and slid by
// `fract(time * speed)`, so nothing is ever rebuilt and the grid recedes for
// as long as the set lasts.

struct VaporUniform {
    grid: ColorSlot,
    horizon: ColorSlot,
    floats: vec4<f32>,  // speed, spacing, perturbacion, glitch
    floats2: vec4<f32>, // sol, techo, time, half_width
    floats3: vec4<f32>, // z_far, z_near, aspect, unused
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: VaporUniform;

// ---------------------------------------------------------------- background

struct BgOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_bg(@builtin(vertex_index) vi: u32) -> BgOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let xy = p[vi];
    var out: BgOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}

@fragment
fn fs_bg(in: BgOut) -> @location(0) vec4<f32> {
    let sun = u.floats2.x;
    let time = u.floats2.z;
    let aspect = max(u.floats3.z, 0.01);

    // The sky is the horizon slot walked top to bottom, so a four-stop slot is
    // a four-band sunset and a solid one is a flat wall — the same range every
    // other slot in the suite gives.
    var col = grad4(u.horizon, clamp(in.uv.y * 1.15, 0.0, 1.0));

    // The sun: a disc sitting on the horizon line, cut by the horizontal bars
    // that are half of what makes this the aesthetic. The bars widen toward
    // the bottom, which is the poster convention and also what stops the disc
    // reading as a plain circle.
    if (sun > 0.001) {
        let centre = vec2<f32>(0.5, 0.52);
        let d = (in.uv - centre) * vec2<f32>(aspect, 1.0);
        let radius = 0.10 + 0.22 * sun;
        let disc = 1.0 - smoothstep(radius * 0.92, radius, length(d));
        // Bars below the middle of the disc only, spaced so they open out.
        let below = clamp((in.uv.y - centre.y) / max(radius, 1e-3), 0.0, 1.0);
        let bar = step(0.5 + 0.5 * cos(in.uv.y * 260.0 + time * 0.6), 0.35 + 0.55 * below);
        let lit = disc * (1.0 - below * bar);
        col = mix(col, grad4(u.horizon, 0.85) + vec3<f32>(0.35, 0.22, 0.15), lit * sun);
    }

    return vec4<f32>(col, 1.0);
}

// ---------------------------------------------------------------------- grid

// A travelling ridge plus a slow swell, the shape Chamber's pulse has, in the
// ground plane's own coordinates. Everything is a function of (x, z, time), so
// asking for the same instant twice draws the same grid.
fn height_at(x: f32, z: f32) -> f32 {
    let amount = u.floats.z;
    if (amount <= 0.0) {
        return 0.0;
    }
    let time = u.floats2.z;
    // The ridge travels out from under the camera toward the horizon.
    let d = length(vec2<f32>(x * 0.35, z + 6.0));
    let front = fract(time * 0.11) * 26.0;
    let ring = exp(-pow(d - front, 2.0) * 0.09);
    // The swell is what stops a still frame looking like a flat floor with one
    // wave on it.
    let swell = sin(x * 0.7 + time * 0.4) * cos(z * 0.31 - time * 0.27);
    return amount * (ring * 1.6 + swell * 0.35);
}

struct VIn {
    @location(0) pos: vec3<f32>,
    // Not a normal: this mesh is lines and nothing here is shaded. x carries
    // the line's own index (so a glitch can pick a line), y is 1 for the
    // crossing lines that scroll and 0 for the rails, z is unused.
    @location(1) tag: vec3<f32>,
    @location(2) t: f32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) t: f32,
    @location(1) glitched: f32,
};

@vertex
fn vs_main(in: VIn, @builtin(instance_index) inst: u32) -> VOut {
    let speed = u.floats.x;
    let spacing = max(u.floats.y, 0.001);
    let glitch = u.floats.w;
    let time = u.floats2.z;

    var p = in.pos;

    // The scroll. One offset for the whole mesh, wrapped into a single
    // spacing, so the grid arrives forever out of geometry that never moves.
    let scroll = fract(time * speed / spacing) * spacing;
    p.z = p.z + scroll;

    // The glitch: a crossing line picked by a per-burst hash tears sideways
    // and lights up. Held for the burst rather than drifting, the same shape
    // Domain's row tearing has.
    var glitched = 0.0;
    if (glitch > 0.0 && in.tag.y > 0.5) {
        let burst = floor(time * 6.0);
        let roll = hash21(vec2<f32>(in.tag.x, burst));
        if (roll > 1.0 - glitch * 0.45) {
            let shove = (hash21(vec2<f32>(in.tag.x, burst + 31.0)) * 2.0 - 1.0);
            p.x = p.x + shove * glitch * 1.4;
            glitched = 1.0;
        }
    }

    p.y = p.y + height_at(p.x, p.z);

    // Instance 1 is the ceiling: the same grid, flipped and lifted. A room,
    // not a floor — which is what a wide frame needs, since a ground plane
    // alone leaves the whole top half to the sky.
    if (inst == 1u) {
        p.y = 3.4 - p.y;
    }

    var out: VOut;
    out.clip = u.view_proj * vec4<f32>(p, 1.0);
    // Depth, 0 at the horizon and 1 underfoot: what the colour ramp and the
    // fog are both keyed to.
    let z_far = u.floats3.x;
    let z_near = u.floats3.y;
    out.t = clamp((p.z - z_far) / max(z_near - z_far, 0.001), 0.0, 1.0);
    out.glitched = glitched;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let base = grad4(u.grid, in.t);
    // Into the horizon at distance, so the grid ends in the sky rather than at
    // a hard edge across the frame.
    let fog = pow(1.0 - in.t, 2.2);
    let col = mix(base, grad4(u.horizon, 0.55), fog * 0.9) + vec3<f32>(0.6, 0.3, 0.5) * in.glitched;
    // The far end fades out entirely instead of stacking into a bright band
    // where a hundred lines converge.
    let alpha = clamp(0.25 + 0.75 * in.t, 0.0, 1.0);
    return vec4<f32>(col, alpha);
}
