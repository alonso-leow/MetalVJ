// Particulas on a device with no compute shaders or storage buffers (WebGL2):
// the same `seed_body` / `step_body` as the compute entry points, as fragment
// passes over body textures.
//
//  - bodies live in two BODY_TEX x BODY_TEX Rgba32Float textures (position,
//    half-step velocity), body i at texel (i % BODY_TEX, i / BODY_TEX), stepped
//    A to B by `fs_step` (a fragment per body, reading only the previous state).
//  - the workgroup tile is recomputed per body: the same hashed 64 sources the
//    compute path shares through workgroup memory.
//  - the collision grid is an R32Uint texture of "highest body index + 1" per
//    cell: one point per body drawn with a depth that grows with the index and
//    a Greater test, so the highest index wins exactly like `atomicMax`.

const BODY_TEX: u32 = 256u;

@group(0) @binding(6) var tex_pos: texture_2d<f32>;
@group(0) @binding(7) var tex_vel: texture_2d<f32>;
@group(0) @binding(8) var tex_grid: texture_2d<u32>;

fn body_texel(j: u32) -> vec2<i32> {
    return vec2<i32>(i32(j % BODY_TEX), i32(j / BODY_TEX));
}

fn pos_at(j: u32) -> vec4<f32> { return textureLoad(tex_pos, body_texel(j), 0); }
fn vel_at(j: u32) -> vec4<f32> { return textureLoad(tex_vel, body_texel(j), 0); }
fn rep_at(c: i32) -> u32 {
    return textureLoad(tex_grid, vec2<i32>(c % GRID, c / GRID), 0).x;
}
fn tile_pos(wid: u32, k: u32) -> vec3<f32> {
    let n = u.ints.x;
    let pick = pcg(wid * 2654435761u + u.ints.z * 40503u + k * 97u + u.ints.y) % n;
    return pos_at(pick).xyz;
}

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

struct BodyOut {
    @location(0) pos: vec4<f32>,
    @location(1) vel: vec4<f32>,
};

fn body_index(pos: vec4<f32>) -> u32 {
    return u32(pos.y) * BODY_TEX + u32(pos.x);
}

@fragment
fn fs_seed(@builtin(position) pos: vec4<f32>) -> BodyOut {
    let i = body_index(pos);
    var out: BodyOut;
    out.pos = vec4<f32>(0.0);
    out.vel = vec4<f32>(0.0);
    if (i < u.ints.x) {
        let b = seed_body(i);
        out.pos = vec4<f32>(b.p, 0.0);
        out.vel = vec4<f32>(b.v, 0.0);
    }
    return out;
}

@fragment
fn fs_step(@builtin(position) pos: vec4<f32>) -> BodyOut {
    let i = body_index(pos);
    var out: BodyOut;
    out.pos = vec4<f32>(0.0);
    out.vel = vec4<f32>(0.0);
    if (i < u.ints.x) {
        let b = step_body(i, i / 64u);
        out.pos = vec4<f32>(b.p, 0.0);
        out.vel = vec4<f32>(b.v, 0.0);
    }
    return out;
}

struct CellOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
};

// One point per body in its collision cell, deeper for a higher index; a body
// outside the grid, or beyond the live count, is parked off-screen.
@vertex
fn vs_build(@builtin(instance_index) ii: u32) -> CellOut {
    var out: CellOut;
    out.id = ii + 1u;
    out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    if (ii < u.ints.x) {
        let c = cell_of(pos_at(ii).xyz);
        if (c >= 0) {
            let cx = f32(c % GRID) + 0.5;
            let cy = f32(c / GRID) + 0.5;
            out.pos = vec4<f32>(
                cx / f32(GRID) * 2.0 - 1.0,
                1.0 - cy / f32(GRID) * 2.0,
                f32(ii + 1u) / 65537.0,
                1.0,
            );
        }
    }
    return out;
}

@fragment
fn fs_build(in: CellOut) -> @location(0) vec4<u32> {
    return vec4<u32>(in.id, 0u, 0u, 0u);
}
