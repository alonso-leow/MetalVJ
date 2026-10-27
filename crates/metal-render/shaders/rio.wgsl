// Model 30 — River (`rio`). A river running away from you between two banks,
// stones in it and beside it, bushes on the bank that answer the wind, and a
// moon low at the far end that is the only light in the valley.
//
// `models/rio.rs` has the argument; this file is Matrix's four rules with a
// fifth added, and the fifth is the whole difference between the two models.
//
//   1. **The moon is the only light.** Brightness falls off with distance from
//      it, so the far end of the water glows and the bank at your feet is the
//      dark part of the frame. At `luna = 0` the valley lights flat.
//
//   2. **Colour is depth, not material.** The gradient is walked by distance
//      from the eye, blended with brightness by `profundidad`. Identical to
//      the room, and for the same reason: there are no object colours here.
//
//   3. **A role is a rate, not a tint.** Bank, stone, moon, leaf, water. A
//      bush is not green, it is the grid that bends; the river is not blue, it
//      is the grid that runs.
//
//   4. **The weave is one weave.** Nothing in here is drawn at its own line
//      density — see `shared::weave`.
//
//   5. **Two roles are displaced, and only from the clock.** This is what
//      Matrix does not do. A leaf moves with the gust field, water moves with
//      the wave trains and the wakes, and both are closed-form functions of
//      position and time — no state, no history, so the same instant twice is
//      the same river. Every other role is where the mesh put it.
//
// The two displaced roles need one number each that the geometry knows and
// this file cannot recover: `tag.x` carries water depth for the surface and
// height-above-root for a leaf. Recomputing either here would mean a second
// copy of the terrain function in WGSL, which is two valleys.

struct RioUniform {
    color: ColorSlot,
    floats: vec4<f32>,  // brightness, variation, pulse, sweep
    floats2: vec4<f32>, // time, moon, depth, water level
    aire: vec4<f32>,    // wind, gusts, heading (radians), how many wakes
    agua: vec4<f32>,    // current, swell, foam, flow rate
    moon: vec4<f32>,    // xyz, where the moon hangs
    eye: vec4<f32>,     // xyz, where the camera ended up after the drift
    view_proj: mat4x4<f32>,
    // The stones standing in the stream: xz, radius, strength. Only these
    // ones — a stone on the bank does not shape the water and does not take a
    // slot. `aire.w` is how many are live.
    wakes: array<vec4<f32>, 10>,
    // Materials (appended), one `MaterialU` per part in `SURFACES` order, then
    // the viewport in pixels for projector mapping, with `relleno` in `.z`.
    // See `material.wgsl`.
    mat_ground: MaterialU,
    mat_water: MaterialU,
    mat_stones: MaterialU,
    mat_bushes: MaterialU,
    screen: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: RioUniform;
// The pictures laid on each part (plan: surface sources), bound by
// `shared::surface::layout_entries`: texture and sampler per part.
@group(0) @binding(1) var surf_ground_tex: texture_2d<f32>;
@group(0) @binding(2) var surf_ground_samp: sampler;
@group(0) @binding(3) var surf_water_tex: texture_2d<f32>;
@group(0) @binding(4) var surf_water_samp: sampler;
@group(0) @binding(5) var surf_stones_tex: texture_2d<f32>;
@group(0) @binding(6) var surf_stones_samp: sampler;
@group(0) @binding(7) var surf_bushes_tex: texture_2d<f32>;
@group(0) @binding(8) var surf_bushes_samp: sampler;

struct VIn {
    // Already in world space — the valley is baked, nothing is instanced.
    @location(0) pos: vec3<f32>,
    // Not a normal on a line; the strand's own id (or a role coordinate), how
    // far along it this point is, and the role. `nrm` is the surface's real
    // normal on a filled face and zero on a free strand.
    @location(1) tag: vec3<f32>,
    // Which thing this strand belongs to. A hash seed, nothing else — but
    // every stone and every bush has its own, which is what stops a hillside
    // of bushes from going over as one object.
    @location(2) obj: f32,
    @location(3) nrm: vec3<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) lum: f32,
    @location(1) depth: f32,
    // Where the point is after displacement, and its role, for the surfaces.
    @location(2) wpos: vec3<f32>,
    @location(3) role: f32,
    @location(4) nrm: vec3<f32>,
};

// Which way the wind is going, as a unit vector in the ground plane.
fn heading() -> vec2<f32> {
    let a = u.aire.z;
    return vec2<f32>(cos(a), sin(a));
}

// How hard the wind is blowing at a point, right now.
//
// The field travels *across the valley in the wind's own direction*, which is
// what makes `rachas` worth having: at 0 every bush holds the same lean and
// the frame reads as a photograph of a windy day, and at 1 a gust arrives at
// the near bushes before the far ones and you can see the wind cross the
// ground. `lattice_fbm` rather than `fbm` because the coordinate walks with
// time and never comes back — see the plateau note in shared.wgsl.
fn racha(p: vec2<f32>, t: f32) -> f32 {
    let q = p * 0.30 - heading() * (t * 0.85);
    let g = lattice_fbm(q, 3) * 2.2 - 1.0;
    return mix(1.0, max(0.30 + g * 1.6, -0.45), clamp(u.aire.y, 0.0, 1.0));
}

// The surface, as a height above the water plane. Three trains running
// downstream at wavelengths well apart, a chop that runs along the *wind*
// rather than along the flow, and an advected texture under all of it.
//
// The chop term is the only place the two halves of the ask meet, and it is
// the one that makes them read as one weather: turn `viento` up and the river
// roughens across its own current.
fn crests(p: vec2<f32>, t: f32) -> f32 {
    let current = u.agua.x;
    let swell = u.agua.y;
    let wind = clamp(u.aire.x, 0.0, 1.5);
    let flow = t * current;

    var h = sin(p.y * 2.42 - flow * 1.6 + p.x * 0.50) * 0.55;
    h = h + sin(p.y * 4.85 - flow * 2.7 - p.x * 0.90) * 0.28;
    h = h + sin(p.y * 7.30 - flow * 4.1 + p.x * 1.70) * 0.11;
    // One train that is mostly *across* the river. Without it every wave was a
    // function of `z` alone, so the strands that run bank to bank stayed dead
    // flat and half the mesh never showed the water moving at all.
    h = h + sin(p.x * 4.30 + p.y * 1.10 - flow * 1.2) * 0.30;
    h = h + sin(dot(p, heading()) * 10.0 - t * (2.0 + current)) * 0.22 * wind;
    h = h + (lattice_fbm(vec2<f32>(p.x * 1.5, p.y * 0.9 - flow * 0.8), 2) - 0.38) * 0.70;
    return h * swell * 0.30;
}

// What the stones in the stream do to the surface: a bulge held in front of
// each one and a standing V trailing off behind it.
//
// **Standing, not travelling.** A wake is the one thing on a river that does
// not move downstream — it is fixed to the obstacle while the water goes
// through it — so this term has no `flow` in it at all, and that absence is
// what makes a rock read as a rock rather than as a bright patch drifting.
fn wake(p: vec2<f32>) -> f32 {
    let n = i32(u.aire.w);
    var w = 0.0;
    for (var k = 0; k < n; k = k + 1) {
        let s = u.wakes[k];
        let r = max(s.z, 0.03);
        let dx = p.x - s.x;
        let dz = p.y - s.y;
        if (dz > 0.0) {
            // The V opens downstream, and dies out along it.
            let arm = abs(dx) - dz * 0.42;
            w = w + exp(-arm * arm / (r * r * 1.6)) * exp(-dz * 1.1) * 1.10;
        } else {
            w = w + exp(-(dx * dx + dz * dz) / (r * r * 3.0)) * 0.85;
        }
    }
    return w * (0.35 + u.agua.x * 0.65);
}

// The whole surface at a point: waves plus wakes, tapered by how deep the
// water is there so it flattens to nothing at the shoreline.
fn surface(p: vec2<f32>, t: f32, deep: f32) -> f32 {
    return (crests(p, t) + wake(p) * 0.045) * deep;
}

// The picture and the material of the part a face belongs to, through the one
// shared `material_apply`. The valley's four parts are keyed by `role`, which
// is how the weave lights a strand and, here, what it is: 0 bank (ground),
// 1 stone, 2 moon (which has no part and wears the ground's), 3 leaf (bushes),
// 4 water.
fn part_shaded(base: vec3<f32>, role: f32, p: vec3<f32>, n: vec3<f32>, screen_uv: vec2<f32>) -> Shaded {
    let t = u.floats2.x;
    let v = normalize(u.eye.xyz - p);
    if (role < 0.5) {
        return material_apply(u.mat_ground, surf_ground_tex, surf_ground_samp, base, p, n, screen_uv, v, t);
    } else if (role < 1.5) {
        return material_apply(u.mat_stones, surf_stones_tex, surf_stones_samp, base, p, n, screen_uv, v, t);
    } else if (role < 2.5) {
        return material_apply(u.mat_ground, surf_ground_tex, surf_ground_samp, base, p, n, screen_uv, v, t);
    } else if (role < 3.5) {
        return material_apply(u.mat_bushes, surf_bushes_tex, surf_bushes_samp, base, p, n, screen_uv, v, t);
    }
    return material_apply(u.mat_water, surf_water_tex, surf_water_samp, base, p, n, screen_uv, v, t);
}

@vertex
fn vs_main(in: VIn) -> VOut {
    let variation = u.floats.y;
    let pulse = u.floats.z;
    let barrido = u.floats.w;
    let time = u.floats2.x;
    let moon = u.floats2.y;
    let level = u.floats2.w;
    let wind = clamp(u.aire.x, 0.0, 1.5);
    let foam = u.agua.z;

    var world = in.pos;
    let role = in.tag.z;

    // Rule 5, first half. Both branches are ordered low to high and closed at
    // the top for the reason matrix.wgsl records: an open comparison lets a
    // new role fall silently into the one below it.
    var bend = 0.0;
    var crest = 0.0;
    var glint = 0.0;
    if (role > 2.5 && role < 3.5) {
        // A leaf. Bent by the gust that is over this bush at this instant,
        // scaled by the square of its height above the root — so the root
        // stays planted, the crown goes over, and the bush hinges rather than
        // sliding. The flutter on top is per bush and per leaf, and it is what
        // keeps a still frame from looking like a bent wire sculpture.
        let high = clamp(in.tag.x, 0.0, 1.0);
        bend = racha(world.xz, time) * wind;
        let flutter = sin(time * 6.3 + in.obj * 2.7 + in.tag.y * 9.0) * 0.30 * wind;
        let lay = (bend + flutter) * high * high;
        world = world + vec3<f32>(heading().x, 0.0, heading().y) * (lay * 0.20);
        // Laid over, a twig no longer reaches as high. Without this the bush
        // stretches instead of bending, which reads as growing.
        world.y = world.y - abs(lay) * high * 0.045;
    } else if (role > 3.5) {
        // Water. The height field, and the surface normal taken from the same
        // field a finger's width either side — which is what the moon's path
        // on the river is computed from, and the only reason it wavers.
        let deep = clamp(in.tag.x, 0.0, 1.0);
        let e = 0.06;
        let h = surface(world.xz, time, deep);
        let hx = surface(world.xz + vec2<f32>(e, 0.0), time, deep);
        let hz = surface(world.xz + vec2<f32>(0.0, e), time, deep);
        world.y = world.y + h;
        // A surface that only bobs reads as a lake in a breeze. Sliding the
        // grid itself downstream, a little, is what says the water is going
        // somewhere — and it is bounded, so the mesh never leaves its banks.
        world.z = world.z + sin(world.x * 3.1 + time * u.agua.x * 1.3) * 0.02 * deep;

        let normal = normalize(vec3<f32>(-(hx - h) / e, 1.0, -(hz - h) / e));
        let to_moon = normalize(u.moon.xyz - world);
        let to_eye = normalize(u.eye.xyz - world);
        let half_v = normalize(to_moon + to_eye);
        glint = pow(max(dot(normal, half_v), 0.0), 26.0);
        // Where this point is between the trough and the crest, normalised
        // against what the trains can actually reach at this `oleaje`.
        // Dividing by the *displacement* instead — which is what this did
        // first — saturates at ±1 everywhere the moment the amplitude is
        // non-zero, and then `foam` is not foam on the crests, it is a flat
        // white lid on the river.
        crest = clamp(crests(world.xz, time) / max(u.agua.y * 0.55, 0.001), -1.0, 1.0);
    }

    // The cell flicker, exactly the room's: a hash per (strand or role
    // coordinate, cell along the strand, object, slot of time), held for the
    // slot rather than drifting.
    let cell = vec3<f32>(in.tag.x * 7.3 + in.obj, floor(in.tag.y * 12.0), in.obj);
    let slot = floor(time * (0.5 + pulse * 3.0));
    let flick = hash31(cell + vec3<f32>(slot, slot * 1.7, 0.0));

    // Rule 1. Distance from the moon, over a valley-sized falloff rather than
    // a room-sized one — a bulb three metres away and a moon at the end of the
    // river are the same rule and nothing like the same number.
    let reach = world - u.moon.xyz;
    let moonlight = 2.4 / (1.0 + dot(reach, reach) * 0.035);
    let lit = mix(1.0, moonlight, clamp(moon, 0.0, 1.0));

    var lum = (1.0 - variation + variation * flick) * lit;

    // Rule 3.
    // How far under the waterline a point is. Every role that can be partly
    // submerged asks this, and it is the answer to the first thing that went
    // wrong in this model: the bed is drawn *through* the water, so the
    // channel came out as two grids in the same place, and two grids in the
    // same place do not read as depth, they read as moiré. Drowning what is
    // under the surface fixes it and is the honest reading anyway — at night
    // you do not see the bed of a river, you see the surface of one.
    let under = smoothstep(0.0, 0.10, level - world.y);

    if (role < 0.5) {
        // Bank. Dimmer than the things standing on it, so the valley reads as
        // ground with things on it and not as one tangle at one depth — plus a
        // damp band right at the waterline, which is the only mark on the
        // whole landscape that says where the water currently is.
        let wet = exp(-pow((world.y - level) * 11.0, 2.0));
        lum = lum * (0.62 + 0.55 * wet) * mix(1.0, 0.16, under);
    } else if (role < 1.5) {
        // Stone. The default case — and if it is standing in the river it
        // wears a collar of white water at the line the surface cuts it, and
        // the part of it below that line goes under with the bed.
        let collar = exp(-pow((world.y - level) * 15.0, 2.0));
        lum = lum * mix(1.0, 0.30, under)
            + foam * collar * (0.30 + u.agua.x * 0.55);
    } else if (role < 2.5) {
        // The moon. An emitter, and deliberately without the television's
        // scanline: the one light in the valley is not a device. What breathing
        // it does is on `pulso`, the model's own rate float, so that at
        // `pulso = 0` the frame is genuinely still — which is a property the
        // gate in tests/render.rs checks, and a free-running sine here would
        // have quietly broken it.
        lum = lum + 1.5 + 0.30 * pulse * sin(time * 0.6);
    } else if (role < 3.5) {
        // Leaf. It does not change colour in the wind — the grid running
        // through it brightens as the gust arrives and settles after it, which
        // is the only way this model is allowed to say "that one just moved".
        lum = lum * (0.72 + 0.50 * clamp(abs(bend), 0.0, 1.4));
    } else {
        // Water. Foam on the crests and in the wakes, and the moon's own path
        // laid down the river toward the camera.
        lum = lum * 1.05
            + foam * smoothstep(0.12, 0.80, crest) * 1.15
            + foam * wake(world.xz) * 0.55
            + glint * 2.2 * clamp(moon, 0.0, 1.0);
    }

    // The sweep, running downstream rather than across a room's diagonal — the
    // one thing that crosses the near bank, the water and the far stones as if
    // they were the same object. They are.
    let front = fract(time * (0.05 + pulse * 0.25)) * 20.0 - 3.5;
    let sweep = exp(-pow((world.z - front) * 0.55, 2.0));
    lum = clamp(lum + barrido * sweep * 1.6, 0.0, 2.6);

    var out: VOut;
    out.clip = u.view_proj * vec4<f32>(world, 1.0);
    out.lum = lum;
    out.wpos = world;
    out.role = role;
    out.nrm = in.nrm;
    // Distance from the camera, over the length of the valley rather than the
    // width of a room.
    out.depth = clamp(length(world - u.eye.xyz) / 15.0, 0.0, 1.0);
    return out;
}

// Rule 2's colour, shared by the line and filled fragments.
fn rio_colour(lum: f32, depth: f32) -> vec3<f32> {
    let by_light = clamp(lum * 0.55, 0.0, 1.0);
    let by_depth = 1.0 - depth;
    return grad4(u.color, mix(by_light, by_depth, clamp(u.floats2.z, 0.0, 1.0)));
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let shine = u.floats.x;
    let base = rio_colour(in.lum, in.depth);
    // A line carries no normal: the triplanar fallback the valley always used.
    let shaded = part_shaded(base, in.role, in.wpos, vec3<f32>(0.57735), in.clip.xy / u.screen.xy);

    let fade = 1.0 - 0.55 * in.depth;
    let col = shaded.albedo * shine * in.lum * fade;
    return vec4<f32>(col, clamp(0.26 + 0.62 * in.lum, 0.0, 1.0));
}

// The filled cell of the weave: a real normal, the picture and the material of
// its part, and `relleno` as its opacity so the fill is a mix rather than a
// second model.
@fragment
fn fs_fill(in: VOut) -> @location(0) vec4<f32> {
    let shine = u.floats.x;
    let fill = clamp(u.screen.z, 0.0, 1.0);
    let base = rio_colour(in.lum, in.depth);
    let screen_uv = in.clip.xy / u.screen.xy;
    let shaded = part_shaded(base, in.role, in.wpos, normalize(in.nrm), screen_uv);

    let fade = 1.0 - 0.55 * in.depth;
    let col = shaded.albedo * shine * in.lum * fade;
    return vec4<f32>(col, clamp(shaded.alpha * fill, 0.0, 1.0));
}
