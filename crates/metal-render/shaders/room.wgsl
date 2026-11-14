// Model 15 — Room. A room with a couch, a television, a lamp, a table with a
// telephone, a curtained window and a door in it, drawn as grid: everything you
// can read off it is brightness and a position along one gradient.
//
// `models/room.rs` has the argument; this file is the three rules it names.
//
//   1. **The lamp is the only light.** Brightness falls off with distance from
//      the bulb, so the near arm of the couch is bright, the far wall is not,
//      and the room has a shape without a single normal being computed. At
//      `lampara = 0` this collapses to the flat light the empty room had.
//
//   2. **Colour is depth, not material.** The gradient is walked by distance
//      from the *eye*, blended with brightness by `profundidad`. Turn it up
//      and a straight run of grid ramps through the whole slot as it recedes;
//      turn it down and colour goes back to being a brightness read-out. There
//      is no third source of colour, because there are no object colours —
//      that is the whole ask.
//
//   3. **A role is a rate, not a tint.** Four roles: wall, solid, emitter,
//      cloth. A television is not "the blue thing", it is the grid running at
//      a different rate; a curtain is not fabric-coloured, it is the grid with
//      a slow wave along it. Every one of them lands in the same gradient.
//
// Nothing here displaces a vertex. Rule 1 and rule 2 are per-vertex reads of
// static geometry, and the model's founding invariant — see the module doc —
// is that the room never moves and only the light in it and your place in it
// do.
//
// **The filled layer (materials M6).** The weave emits the same cells as
// triangles carrying their real normal, in `fill`/`glass`. At `relleno = 0`
// they are not drawn and the room is exactly its lines; above it they are,
// through the shared `material_apply`, so the room wears a material part by
// part like any other 3D model. A line is not a surface, so the picture and
// the material also reach the lines, where the normal is the triplanar
// fallback `(1,1,1)` the room has always used.

struct RoomUniform {
    color: ColorSlot,
    floats: vec4<f32>,  // brillo, variacion, pulso, barrido
    floats2: vec4<f32>, // time, lampara, profundidad, relleno
    bulb: vec4<f32>,    // xyz where the lamp hangs
    eye: vec4<f32>,     // xyz where the camera ended up after the drift
    view_proj: mat4x4<f32>,
    // Materials (appended), one `MaterialU` per part in `SURFACES` order, then
    // the viewport in pixels for projector mapping. See `material.wgsl`.
    mat_walls: MaterialU,
    mat_couch: MaterialU,
    mat_tv: MaterialU,
    mat_table: MaterialU,
    mat_window: MaterialU,
    mat_door: MaterialU,
    screen: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: RoomUniform;
// The picture laid on each part (plan: surface sources), bound by
// `shared::surface::layout_entries`: texture and sampler per part.
@group(0) @binding(1) var surf_walls_tex: texture_2d<f32>;
@group(0) @binding(2) var surf_walls_samp: sampler;
@group(0) @binding(3) var surf_couch_tex: texture_2d<f32>;
@group(0) @binding(4) var surf_couch_samp: sampler;
@group(0) @binding(5) var surf_tv_tex: texture_2d<f32>;
@group(0) @binding(6) var surf_tv_samp: sampler;
@group(0) @binding(7) var surf_table_tex: texture_2d<f32>;
@group(0) @binding(8) var surf_table_samp: sampler;
@group(0) @binding(9) var surf_window_tex: texture_2d<f32>;
@group(0) @binding(10) var surf_window_samp: sampler;
@group(0) @binding(11) var surf_door_tex: texture_2d<f32>;
@group(0) @binding(12) var surf_door_samp: sampler;

struct VIn {
    // Already in room space — the room is baked, nothing is instanced.
    @location(0) pos: vec3<f32>,
    // Not a normal on a line; the strand's own id (or a model value), how far
    // along it this point is, and the role. `nrm` is the surface's real normal
    // on a filled face and zero on a free strand.
    @location(1) tag: vec3<f32>,
    // Which thing this strand belongs to. A hash seed, nothing else — and the
    // id that picks the part a picture or a material lands on.
    @location(2) obj: f32,
    @location(3) nrm: vec3<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) lum: f32,
    @location(1) depth: f32,
    // Where the point is, for the picture's object mapping.
    @location(2) wpos: vec3<f32>,
    @location(3) obj: f32,
    @location(4) nrm: vec3<f32>,
};

// The picture and the material of the part a face belongs to, through the one
// shared `material_apply`. The room's six parts are keyed by `obj`, the hash
// seed the weave gives each thing: 0 walls, 1 couch, 2 television, 3 lamp
// (which has no part and wears the walls'), 4 table, 5 window, 6 door.
fn part_shaded(base: vec3<f32>, obj: f32, p: vec3<f32>, n: vec3<f32>, screen_uv: vec2<f32>) -> Shaded {
    let t = u.floats2.x;
    let v = normalize(u.eye.xyz - p);
    if (obj < 0.5) {
        return material_apply(u.mat_walls, surf_walls_tex, surf_walls_samp, base, p, n, screen_uv, v, t);
    } else if (obj < 1.5) {
        return material_apply(u.mat_couch, surf_couch_tex, surf_couch_samp, base, p, n, screen_uv, v, t);
    } else if (obj < 2.5) {
        return material_apply(u.mat_tv, surf_tv_tex, surf_tv_samp, base, p, n, screen_uv, v, t);
    } else if (obj < 3.5) {
        return material_apply(u.mat_walls, surf_walls_tex, surf_walls_samp, base, p, n, screen_uv, v, t);
    } else if (obj < 4.5) {
        return material_apply(u.mat_table, surf_table_tex, surf_table_samp, base, p, n, screen_uv, v, t);
    } else if (obj < 5.5) {
        return material_apply(u.mat_window, surf_window_tex, surf_window_samp, base, p, n, screen_uv, v, t);
    }
    return material_apply(u.mat_door, surf_door_tex, surf_door_samp, base, p, n, screen_uv, v, t);
}

@vertex
fn vs_main(in: VIn) -> VOut {
    let variation = u.floats.y;
    let pulse = u.floats.z;
    let barrido = u.floats.w;
    let time = u.floats2.x;
    let lamp = u.floats2.y;

    let world = in.pos;
    let role = in.tag.z;

    // The cell flicker: a hash per (strand, cell along it, object, slot of
    // time), held for the slot rather than drifting, so the room reads as a
    // weave switching on and off and not as a slow shimmer.
    let cell = vec3<f32>(in.tag.x, floor(in.tag.y * 12.0), in.obj);
    let slot = floor(time * (0.5 + pulse * 3.0));
    let flick = hash31(cell + vec3<f32>(slot, slot * 1.7, 0.0));

    // Rule 1. Inverse-square from the bulb, mixed against flat light so
    // `lampara` is a dial between "an empty cube, evenly lit" and "one lamp in
    // a dark room", rather than a brightness slider with a second job.
    let reach = world - u.bulb.xyz;
    let bulb = 2.2 / (1.0 + dot(reach, reach) * 2.4);
    let lit = mix(1.0, bulb, clamp(lamp, 0.0, 1.0));

    var lum = (1.0 - variation + variation * flick) * lit;

    // Rule 3. Ordered low to high and each branch closed at the top, so adding
    // a role never silently falls into the one below it — `glass` did exactly
    // that when the cloth branch was an open `role > 2.5`.
    if (role < 0.5) {
        // Wall: a little less than the things standing in front of it, so a
        // room reads as a room and not as a tangle at one depth.
        lum = lum * 0.78;
    } else if (role < 1.5) {
        // Solid. Lit by the lamp and nothing else — the default case, and the
        // one the whole model is calibrated around.
    } else if (role < 2.5) {
        // An emitter makes its own light. The television's scanline runs down
        // the screen and the lamp shade's runs around it — the same expression,
        // because both are grid at a rate of their own, which is all either of
        // them is here.
        let scan = fract(world.y * 22.0 - time * 1.7);
        lum = lum + 0.55 + 0.75 * scan * scan;
    } else if (role < 3.5) {
        // Cloth: a slow wave along the panel. The curtain does not move — the
        // light in it does.
        lum = lum * (0.72 + 0.5 * sin(world.x * 11.0 + time * 0.55));
    } else {
        // Glass: a steady lift, not a scanline. A window is the one light in
        // here that is not a device, and giving it the television's flicker
        // would make it read as a second screen.
        lum = lum + 0.9;
    }

    // The sweep: a plane travelling through the room along its diagonal, and
    // the one thing in here that crosses the couch, the lamp and the far wall
    // as if they were the same object. They are.
    let front = fract(time * (0.05 + pulse * 0.25)) * 6.0 - 3.0;
    let along = world.x * 0.5 + world.y * 0.5 + world.z;
    let sweep = exp(-pow(along - front, 2.0) * 2.2);
    lum = clamp(lum + barrido * sweep * 1.6, 0.0, 2.4);

    var out: VOut;
    out.clip = u.view_proj * vec4<f32>(world, 1.0);
    out.lum = lum;
    out.wpos = world;
    out.obj = in.obj;
    out.nrm = in.nrm;
    // Distance from the camera, not from the room's centre. The centre was a
    // fair stand-in for the eye while the room was empty and is simply wrong
    // once there is a couch between you and a wall.
    out.depth = clamp(length(world - u.eye.xyz) / 3.2, 0.0, 1.0);
    return out;
}

// Rule 2's colour, shared by the line and filled fragments.
fn room_colour(lum: f32, depth: f32) -> vec3<f32> {
    let by_light = clamp(lum * 0.55, 0.0, 1.0);
    let by_depth = 1.0 - depth;
    return grad4(u.color, mix(by_light, by_depth, clamp(u.floats2.z, 0.0, 1.0)));
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let shine = u.floats.x;
    let base = room_colour(in.lum, in.depth);
    // A line carries no normal: the triplanar fallback the room always used.
    let shaded = part_shaded(base, in.obj, in.wpos, vec3<f32>(0.57735), in.clip.xy / u.screen.xy);

    let fade = 1.0 - 0.5 * in.depth;
    let col = shaded.albedo * shine * in.lum * fade;
    return vec4<f32>(col, clamp(0.30 + 0.62 * in.lum, 0.0, 1.0));
}

// The filled cell of the weave: a real normal, the picture and the material of
// its part, and `relleno` as its opacity so the fill is a mix rather than a
// second model.
@fragment
fn fs_fill(in: VOut) -> @location(0) vec4<f32> {
    let shine = u.floats.x;
    let fill = clamp(u.floats2.w, 0.0, 1.0);
    let base = room_colour(in.lum, in.depth);
    let screen_uv = in.clip.xy / u.screen.xy;
    let shaded = part_shaded(base, in.obj, in.wpos, normalize(in.nrm), screen_uv);

    let fade = 1.0 - 0.5 * in.depth;
    let col = shaded.albedo * shine * in.lum * fade;
    return vec4<f32>(col, clamp(shaded.alpha * fill, 0.0, 1.0));
}
