// Trafico, the step pass: one tick of the Biham-Middleton-Levine traffic
// automaton on a torus, as a pull-based fragment pass (each cell decides what
// it becomes from the previous state alone).
//
// State per cell, RGBA16F: x = rank, y = trail, z = trail type, w = unused.
// `rank` is a fixed random number in [0,1) that travels with the cell's
// *contents*: a cell holds a car iff rank < densidad, and its type (red, blue,
// diagonal) is a hash of the rank. Moving a car swaps the two ranks, so the
// population of ranks is conserved, and densidad / p_rojo / tercer_tipo are
// live: moving them adds, removes or recolours cars with no reseed and no
// neighbourhood estimate.
//
// mode 0 seeds, mode 2 advances one step. (1 is reserved.)
// Phase 0: red cars move right. Phase 1: blue cars move down. Phase 2 (only
// when diagonal cars exist): diagonal cars move down-right.

struct StepU {
    mode: u32,
    step: u32,
    w: u32,
    h: u32,
    seed: u32,
    nph: u32,
    pad0: u32,
    pad1: u32,
    // densidad, p_rojo, ruido_giro, tercer_tipo
    a: vec4<f32>,
    // trail decay per step, 0, 0, 0
    b: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: StepU;
@group(0) @binding(1) var prev: texture_2d<f32>;
@group(0) @binding(2) var prev_smp: sampler;

fn tr_pcg(v: u32) -> u32 {
    let st = v * 747796405u + 2891336453u;
    let w = ((st >> ((st >> 28u) + 4u)) ^ st) * 277803737u;
    return (w >> 22u) ^ w;
}

fn tr_rnd4(a: u32, b: u32, c: u32, d: u32) -> f32 {
    let h = tr_pcg(a ^ tr_pcg(b ^ tr_pcg(c ^ tr_pcg(d))));
    return f32(h >> 8u) / 16777216.0;
}

fn tr_load(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(i32(u.w), i32(u.h));
    let q = ((p % size) + size) % size;
    return textureLoad(prev, q, 0);
}

// 0 = empty, 1 = red, 2 = blue, 3 = diagonal. Pure function of the rank.
fn tr_type(rank: f32) -> i32 {
    if (rank >= u.a.x) { return 0; }
    let bits = bitcast<u32>(rank);
    if (tr_rnd4(bits, 7u, 1u, 0u) < u.a.w * 0.5) { return 3; }
    if (tr_rnd4(bits, 7u, 2u, 0u) < u.a.y) { return 1; }
    return 2;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
    let ux = u32(p.x);
    let uy = u32(p.y);

    if (u.mode == 0u) {
        return vec4<f32>(tr_rnd4(ux, uy, 0u, u.seed), 0.0, 0.0, 0.0);
    }

    let ph = i32(u.step % u.nph);
    var dir = vec2<i32>(1, 0);
    if (ph == 1) { dir = vec2<i32>(0, 1); }
    if (ph == 2) { dir = vec2<i32>(1, 1); }
    let mover = ph + 1;

    let me = tr_load(p);
    let tme = tr_type(me.x);
    let decay = u.b.x;

    if (tme == mover) {
        let dst = tr_load(p + dir);
        let go = tr_rnd4(ux, uy, u.step, u.seed ^ 0x9e3779b9u) >= u.a.z;
        if (tr_type(dst.x) == 0 && go) {
            // The car leaves: take the empty cell's rank, leave a trail.
            return vec4<f32>(dst.x, 1.0, f32(mover), 0.0);
        }
    }
    if (tme == 0) {
        let sp = p - dir;
        let src = tr_load(sp);
        if (tr_type(src.x) == mover) {
            let go = tr_rnd4(u32((sp.x + i32(u.w)) % i32(u.w)), u32((sp.y + i32(u.h)) % i32(u.h)), u.step, u.seed ^ 0x9e3779b9u) >= u.a.z;
            if (go) {
                // A car arrives: it brings its rank; the cell has no trail.
                return vec4<f32>(src.x, 0.0, 0.0, 0.0);
            }
        }
    }
    var trail = me.y * decay;
    if (trail < 0.03) { trail = 0.0; }
    return vec4<f32>(me.x, trail, select(0.0, me.z, trail > 0.0), 0.0);
}
