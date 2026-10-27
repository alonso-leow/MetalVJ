// Model 36 — Dla. Diffusion-limited aggregation: a seed at the origin, and
// a ring of walkers that random-walk inwards until one of them touches the
// structure, freezes where it landed, and the rest go on. The aggregate that
// grows is the animation — a coral of points where the tips capture walkers
// first, which is the whole reason DLA branches: a tip that gets ahead has
// more surface facing the incoming walk than a gap does, so it eats more and
// gets further ahead. One rule, no data structure, all of the shape.
//
// # Where the state lives, and why there are no sparks buffers here
//
// The occupancy grid and the per-cell metadata are `read_write` storage
// bound only to the compute half (bindings 1-3); the render half re-binds the
// same buffers `read` at 7-8 because WGSL declares one type per binding, and
// a fragment stage may not touch a `read_write` buffer. The walkers are
// baked once per frame into `walker_tex` (binding 6 write, binding 9
// sample) because a vertex stage can sample a texture but should not be the
// place to invent per-pixel loops: the bake is a 1024-thread dispatch, and
// it is what makes the sparks a *readout* of walker state rather than an
// accumulation — a frame that advances nothing bakes the same positions and
// draws the same sparks, which is the render gate's determinism promise
// (see `models/turing.rs`, the model that set it).
//
// # The one-step lag, stated honestly
//
// A walker decides "stuck" by looking at the 3x3 cells around itself. Other
// threads in the same dispatch may be claiming cells into that
// neighbourhood at the same moment; WebGPU gives the read the old value or
// the new one, not a promise about which. So a stick can miss a neighbour
// claimed in the same step and land one step later. Visually this is
// nothing — one dispatch is 1/60th of a step budget — and it is what buys
// the whole grid living in one buffer with no ping-pong: the alternative,
// reading only the previous step's grid, is exactly the same lag with two
// buffers instead of one.

const GRID: u32 = 256u;
const CELLS: u32 = GRID * GRID;
const MAX_WALKERS: u32 = 1024u;
const WALKER_TEX_W: u32 = 64u;
const WALKER_TEX_H: u32 = 16u;
// Half-size of the world square the grid covers. `radio` may reach 2.0 and
// the kill ring sits 15% beyond the spawn circle, so the field has to cover
// ±2.3 or a dying walker would spend its last steps clamped onto the edge
// cells — 2.4 covers the kill ring with margin to spare.
const HALF: f32 = 2.4;
// Age is the global step counter at stick time; the render walks it into a
// ramp over this many steps. Passed as a shader constant rather than a float
// because it is a property of the visual (how long "new growth stays new"),
// not of the show.
const AGE_HORIZON: f32 = 4000.0;
const TAU: f32 = 6.2831853;

struct DlaUniform {
    coral: ColorSlot,
    background: ColorSlot,
    // aspect, time, rotacion, tam
    f0: vec4<f32>,
    // luz, brillo, chispa, alive walker count
    f1: vec4<f32>,
    // age_base, deriva, deriva_dir, giro
    f2: vec4<f32>,
    // zancada, radio, cell_world, seed_key
    f3: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: DlaUniform;

// -- the simulation half ----------------------------------------------------

@group(0) @binding(1) var<storage, read_write> occ_rw: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read_write> bead_rw: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> walkers_rw: array<Walker>;

// -- the bake half ----------------------------------------------------------

@group(0) @binding(5) var<storage, read> walkers_ro: array<Walker>;
@group(0) @binding(6) var walker_tex: texture_storage_2d<rgba16float, write>;

// -- the render half --------------------------------------------------------

@group(0) @binding(7) var<storage, read> occ: array<u32>;
@group(0) @binding(8) var<storage, read> bead: array<vec4<f32>>;
@group(0) @binding(9) var walker_sample: texture_2d<f32>;

struct Walker {
    pos: vec2<f32>,
    rng: u32,
    alive: u32,
};

// pcg32 hash, the usual. One state word per walker, advanced on every use,
// seeded from the model's seed key at spawn — the same seed key grows the
// same walk, which is what a preset re-run and the render gate both lean on.
fn pcg(v: u32) -> u32 {
    let n = v * 747796405u + 2891336453u;
    let w = ((n >> ((n >> 28u) + 4u)) ^ n) * 277803737u;
    return (w >> 22u) ^ w;
}

fn rand01(r: ptr<function, u32>) -> f32 {
    *r = pcg(*r);
    return f32(*r) / 4294967295.0;
}

fn seed_hash(key: u32, i: u32) -> u32 {
    return pcg(key * 2654435761u + i * 40503u + 1u);
}

// World -> cell index. The clamp parks off-field positions on the edge,
// which no live walker can reach (see HALF above); it exists so the index
// is always in range, not because a walker ever lands there.
fn cell_of(p: vec2<f32>) -> u32 {
    let g = clamp((p / (2.0 * HALF) + 0.5) * f32(GRID), vec2<f32>(0.0), vec2<f32>(f32(GRID - 1u)));
    let xi = u32(g.x);
    let yi = u32(g.y);
    return yi * GRID + xi;
}

fn cell_center(c: u32) -> vec2<f32> {
    let xi = f32(c % GRID);
    let yi = f32(c / GRID);
    return (vec2<f32>(xi, yi) + 0.5) / f32(GRID) * (2.0 * HALF) - vec2<f32>(HALF);
}

// Whether any of the nine cells around `c` is claimed. This is the stick
// rule — a walker freezes when it comes within one cell of the structure,
// never by overlapping it.
fn neighbour_stuck(c: u32) -> bool {
    let xi = i32(c % GRID);
    let yi = i32(c / GRID);
    for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
        for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
            let x = xi + dx;
            let y = yi + dy;
            if (x < 0 || y < 0 || x >= i32(GRID) || y >= i32(GRID)) {
                continue;
            }
            if (atomicLoad(&occ_rw[u32(y) * GRID + u32(x)]) > 0u) {
                return true;
            }
        }
    }
    return false;
}

// A walker's re-entry: somewhere on the spawn circle, at a fresh angle. This
// is the "reappear at a new angle" of the reference video — every walker
// that sticks or wanders out comes back in from the ring, never from where
// it left.
fn spawn_walker(r: ptr<function, u32>, radius: f32, alive: u32) -> Walker {
    let ang = rand01(r) * TAU;
    var w: Walker;
    w.pos = vec2<f32>(cos(ang), sin(ang)) * radius;
    w.rng = *r;
    w.alive = alive;
    return w;
}

@compute @workgroup_size(64)
fn cs_seed_cells(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= CELLS) {
        return;
    }
    atomicStore(&occ_rw[i], 0u);
    bead_rw[i] = vec4<f32>(0.0);
}

@compute @workgroup_size(64)
fn cs_seed_walkers(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= MAX_WALKERS) {
        return;
    }
    // The zero position: one claimed cell at the origin, aged 1, so the
    // first frame already has a thing to stick to and the ramp has a dark
    // core to grow out of. Planted here rather than in `cs_seed_cells`
    // because a clear and a claim on the same cell inside one dispatch
    // race, and the clear wins as often as not — a coral with no seed is
    // a frame of sparks and nothing else.
    if (i == 0u) {
        let c = cell_of(vec2<f32>(0.0, 0.0));
        atomicExchange(&occ_rw[c], 1u);
        bead_rw[c] = vec4<f32>(0.0, 0.0, 1.0, 0.0);
    }
    var r = seed_hash(u32(u.f3.w) * 7919u + 1u, i);
    let alive = select(0u, 1u, i < u32(u.f1.w));
    walkers_rw[i] = spawn_walker(&r, u.f3.y, alive);
}

@compute @workgroup_size(64)
fn cs_step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= MAX_WALKERS) {
        return;
    }
    var r = pcg(walkers_rw[i].rng);

    // Beyond the alive count: parked on the circle, invisible, one step
    // away from wandering if the operator raises `caminantes` again.
    let alive = select(0u, 1u, i < u32(u.f1.w));
    if (alive == 0u) {
        walkers_rw[i] = spawn_walker(&r, u.f3.y, 0u);
        return;
    }
    var p = walkers_rw[i].pos;

    // One drunkard step, biased by the row: a drift direction pushes the
    // whole growth sideways, a swirl bends it into a spiral, and both are
    // ordinary driven floats so the digest can steer the walk live.
    let step_len = u.f3.x * u.f3.z;
    let ang = rand01(&r) * TAU;
    var move_v = vec2<f32>(cos(ang), sin(ang));
    let dd = u.f2.z * TAU;
    move_v = move_v + u.f2.y * vec2<f32>(cos(dd), sin(dd));
    let radial = p / max(length(p), 0.25);
    move_v = move_v + u.f2.w * vec2<f32>(-radial.y, radial.x);
    p = p + step_len * move_v;

    // Wandered off the field: discarded, re-entered at a new angle.
    if (length(p) > u.f3.y * 1.15) {
        walkers_rw[i] = spawn_walker(&r, u.f3.y, 1u);
        return;
    }

    let c = cell_of(p);
    if (neighbour_stuck(c)) {
        // Touch: frozen. The exchange claims the cell — the first claimant
        // writes the metadata, a later one on the same cell this step just
        // joins the structure without overwriting it.
        let age = max(u.f2.x, 1.0);
        let prev = atomicExchange(&occ_rw[c], u32(age));
        if (prev == 0u) {
            bead_rw[c] = vec4<f32>(
                (rand01(&r) - 0.5) * 0.6,
                (rand01(&r) - 0.5) * 0.6,
                age,
                0.0,
            );
        }
        walkers_rw[i] = spawn_walker(&r, u.f3.y, 1u);
        return;
    }

    var w: Walker;
    w.pos = p;
    w.rng = r;
    w.alive = 1u;
    walkers_rw[i] = w;
}

// The bake: current walker positions copied into the texture the sparks
// pipeline samples. Runs every frame, steps or no steps — that is what makes
// the sparks a readout instead of an accumulation.
@compute @workgroup_size(64)
fn cs_bake_walkers(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= MAX_WALKERS) {
        return;
    }
    let w = walkers_ro[i];
    textureStore(walker_tex, vec2<i32>(i32(i % WALKER_TEX_W), i32(i / WALKER_TEX_W)), vec4<f32>(w.pos, f32(w.alive), 0.0));
}

// -- the field: aggregate dots over the fondo --------------------------------

fn rot2(p: vec2<f32>, ang: f32) -> vec2<f32> {
    let cr = cos(ang);
    let sr = sin(ang);
    return vec2<f32>(cr * p.x - sr * p.y, sr * p.x + cr * p.y);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.f0.x, 0.01);
    var col = backdrop_from_uv(u.background, in.uv, aspect);

    // World coordinates, the field's own frame, spun by `rotacion`.
    var p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    let rot = u.f0.z * u.f0.y * 0.6;
    p = rot2(p, rot);

    if (abs(p.x) < HALF && abs(p.y) < HALF) {
        let cell_world = u.f3.z;
        let dot_r = max(u.f0.w, 0.05) * cell_world * 0.85;
        let c = cell_of(p);
        let xi = i32(c % GRID);
        let yi = i32(c / GRID);
        var acc = vec3<f32>(0.0);
        for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
            for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
                let x = xi + dx;
                let y = yi + dy;
                if (x < 0 || y < 0 || x >= i32(GRID) || y >= i32(GRID)) {
                    continue;
                }
                let k = u32(y) * GRID + u32(x);
                if (occ[k] > 0u) {
                    let m = bead[k];
                    let center = cell_center(k) + m.xy * cell_world;
                    let d = length(p - center);
                    // The bead: a smooth round core with a wider, fainter
                    // bloom around it, so branch tips where beads overlap
                    // read brighter than single strands without any
                    // per-tip bookkeeping.
                    let core = max(1.0 - d / dot_r, 0.0);
                    let t = clamp(m.z / AGE_HORIZON, 0.0, 1.0);
                    let c3 = grad4(u.coral, t);
                    let shape = core * core * (3.0 - 2.0 * core);
                    acc = acc + c3 * u.f1.x * shape;
                    let glow = max(1.0 - d / (dot_r * 3.0), 0.0);
                    acc = acc + c3 * u.f1.y * glow * glow * 0.25;
                }
            }
        }
        col = col + acc;
    }

    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

// -- the sparks: the walkers themselves --------------------------------------

struct SparkVOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) bright: f32,
};

@vertex
fn vs_sparks(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
) -> SparkVOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let w = textureLoad(walker_sample, vec2<i32>(i32(ii % WALKER_TEX_W), i32(ii / WALKER_TEX_W)), 0);
    var out: SparkVOut;
    out.local = corners[vi];
    // Inactive slots collapse to a degenerate quad at the origin: no
    // fragment is shaded, no branch needed.
    let live = step(0.5, w.z) * u.f1.z;
    let half_q = u.f3.z * 0.55;
    var p = rot2(w.xy, u.f0.z * u.f0.y * 0.6) + corners[vi] * half_q;
    out.pos = vec4<f32>(p.x / max(u.f0.x, 0.01), -p.y, 0.0, 1.0);
    out.bright = live;
    if (live <= 0.0) {
        out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    return out;
}

@fragment
fn fs_sparks(in: SparkVOut) -> @location(0) vec4<f32> {
    let d = length(in.local);
    let m = max(1.0 - d, 0.0);
    let c3 = grad4(u.coral, 1.0) * in.bright * (m * m);
    return vec4<f32>(c3, 1.0);
}
