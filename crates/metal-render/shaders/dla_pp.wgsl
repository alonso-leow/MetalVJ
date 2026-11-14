// Dla on a device with no compute shaders or storage buffers (WebGL2): the
// same `walk` as `cs_step`, as fragment passes over textures.
//
//  - `grid_tex` (Rgba32Float, GRID x GRID): the occupancy and bead metadata,
//    (jitter x, jitter y, age, occupancy age); occupancy is `w > 0`.
//  - the walkers live in a WALKER_TEX_W x WALKER_TEX_H Rgba32Uint pair
//    (pos.x bits, pos.y bits, rng, alive), stepped A to B; the same texture
//    is what the sparks read, so no bake pass.
//  - a step draws `fs_step_walkers` over the walkers (reading the grid as it
//    stood at the start of the step: the compute path's one-step lag, made
//    exact), then scatters every fresh claim into the grid as a point.

@group(0) @binding(10) var grid_tex: texture_2d<f32>;
@group(0) @binding(11) var walker_in: texture_2d<u32>;
@group(0) @binding(12) var stick_tex: texture_2d<u32>;

fn grid_at(k: u32) -> vec4<f32> {
    return textureLoad(grid_tex, vec2<i32>(i32(k % GRID), i32(k / GRID)), 0);
}

fn occ_sim(k: u32) -> bool { return grid_at(k).w > 0.0; }
fn occ_view(k: u32) -> bool { return grid_at(k).w > 0.0; }
fn bead_view(k: u32) -> vec4<f32> { return vec4<f32>(grid_at(k).xyz, 0.0); }

fn walker_texel(i: u32) -> vec4<u32> {
    return textureLoad(walker_in, vec2<i32>(i32(i % WALKER_TEX_W), i32(i / WALKER_TEX_W)), 0);
}

fn load_walker(i: u32) -> Walker {
    let t = walker_texel(i);
    var w: Walker;
    w.pos = vec2<f32>(bitcast<f32>(t.x), bitcast<f32>(t.y));
    w.rng = t.z;
    w.alive = t.w;
    return w;
}

fn pack_walker(w: Walker) -> vec4<u32> {
    return vec4<u32>(bitcast<u32>(w.pos.x), bitcast<u32>(w.pos.y), w.rng, w.alive);
}

// (position, alive) of walker `i`, read straight from the state.
fn walker_view(i: u32) -> vec4<f32> {
    let w = load_walker(i);
    return vec4<f32>(w.pos, f32(w.alive), 0.0);
}

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn texel_index(pos: vec4<f32>) -> u32 {
    return u32(pos.y) * WALKER_TEX_W + u32(pos.x);
}

@fragment
fn fs_seed_walkers(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<u32> {
    let i = texel_index(pos);
    var r = seed_hash(u32(u.f3.w) * 7919u + 1u, i);
    let alive = select(0u, 1u, i < u32(u.f1.w));
    return pack_walker(spawn_walker(&r, u.f3.y, alive));
}

// The cleared grid with the origin cell claimed, aged 1.
@fragment
fn fs_seed_grid(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let k = u32(pos.y) * GRID + u32(pos.x);
    if (k == cell_of(vec2<f32>(0.0, 0.0))) {
        return vec4<f32>(0.0, 0.0, 1.0, 1.0);
    }
    return vec4<f32>(0.0);
}

struct StepOut {
    @location(0) walker: vec4<u32>,
    // x: claimed cell + 1 (0: no fresh claim), then the bead's jitter x/y and
    // the age as f32 bits.
    @location(1) stick: vec4<u32>,
};

@fragment
fn fs_step_walkers(@builtin(position) pos: vec4<f32>) -> StepOut {
    let i = texel_index(pos);
    let o = walk(i, load_walker(i));
    var out: StepOut;
    out.walker = pack_walker(o.w);
    out.stick = vec4<u32>(0u);
    if (o.stuck == 1u && o.fresh == 1u) {
        out.stick = vec4<u32>(o.cell + 1u, bitcast<u32>(o.bead.x), bitcast<u32>(o.bead.y), bitcast<u32>(o.bead.z));
    }
    return out;
}

struct ClaimOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) bead: vec3<f32>,
};

// One point per walker, parked off-screen unless it made a fresh claim.
@vertex
fn vs_claim(@builtin(instance_index) ii: u32) -> ClaimOut {
    let t = textureLoad(stick_tex, vec2<i32>(i32(ii % WALKER_TEX_W), i32(ii / WALKER_TEX_W)), 0);
    var out: ClaimOut;
    out.bead = vec3<f32>(bitcast<f32>(t.y), bitcast<f32>(t.z), bitcast<f32>(t.w));
    out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    if (t.x > 0u) {
        let k = t.x - 1u;
        let cx = f32(k % GRID) + 0.5;
        let cy = f32(k / GRID) + 0.5;
        out.pos = vec4<f32>(cx / f32(GRID) * 2.0 - 1.0, 1.0 - cy / f32(GRID) * 2.0, 0.0, 1.0);
    }
    return out;
}

@fragment
fn fs_claim(in: ClaimOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.bead.x, in.bead.y, in.bead.z, in.bead.z);
}
