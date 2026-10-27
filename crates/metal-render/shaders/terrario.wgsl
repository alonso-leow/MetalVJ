// Model 29 — Terrarium (`terrario`). A glass box filled from above by up to ten
// mouths of coloured sand, where the sand actually piles up.
//
// Sandstorm's sand never lands. It could not: that model is an *inversion* of
// the fall — a pixel's height is an age, so the whole curtain is a closed form
// of `(floats, time)` and nothing is stored. A pile is the opposite kind of
// object. Where a grain comes to rest depends on every grain that landed
// before it, which is a history, and a history cannot be inverted out of a
// pixel's coordinates. So this model keeps a grid between frames, like Turing
// and unlike everything else, and pays for it with the same invariant Turing
// pays with: *the simulation advances when time advances*. Same instant twice,
// same frame; a fresh instance always starts from the same empty box and the
// same burn-in.
//
// # The rule is a Margolus block, and that is the whole reason it is correct
//
// The obvious falling-sand automaton — every cell looks below itself and moves
// if there is room — is wrong on a GPU and wrong in a way that looks fine.
// Two cells can both decide to move into the same hole (a grain is duplicated)
// and a cell can move away while another moves in (a grain is destroyed). On a
// CPU the sequential sweep hides it; here every cell decides at once. The
// error is not a crash and not a NaN: it is sand slowly appearing out of
// nothing, which reads as "the pour rate is a bit high".
//
// So the grid is partitioned into 2x2 **blocks**, each updated by one
// invocation, and a step is a *permutation of the four cells inside its own
// block*. Mass is conserved by construction — there is no arithmetic that
// could fail to conserve it, because nothing is created or read across a block
// edge. The block grid alternates its origin between (0,0) and (1,1) on
// successive steps, which is what lets a grain travel further than one block:
// the Margolus neighbourhood. `models::terrario::settle` is this same
// permutation on the CPU, where it can be unit tested against exactly the two
// failures above.
//
// # Angle of repose is a probability, not an angle
//
// `reposo` is the chance that a grain sitting on top of another one slides off
// it diagonally instead of staying put. Near 0 the sand stacks in vertical
// towers; near 1 it collapses to a flat floor almost as fast as it lands. The
// familiar cone of a sand pile is what the middle of that range looks like,
// and it is emergent — there is no slope anywhere in this shader.
//
// `deriva` and `viento` bias which of the two diagonals wins, so a pile leans
// downwind, and `vibracion` shakes the bottom row sideways so old piles slump
// instead of standing for ever.
//
// # Where the colours come from, and where they mix
//
// A grain carries one number: its position in the `arena` gradient, taken from
// the mouth that poured it. Ten mouths spread across the gradient by `matiz`
// give ten colours of sand falling at once, and the pattern in the box is
// where they landed on each other. `mezcla` pulls neighbouring grains' numbers
// towards each other a little on every step, so the seams between two colours
// bleed rather than staying a hard line — the difference between stacked
// powders and stacked inks.

struct TerrarioUniform {
    sand: ColorSlot,
    crystal: ColorSlot,
    background: ColorSlot,
    // aspect, time, grid_w, grid_h
    a: vec4<f32>,
    // fuentes, origen_x, reparto, ancho
    b: vec4<f32>,
    // caudal, vaiven, vaiven_vel, desfase
    c: vec4<f32>,
    // bias (deriva + viento of the moment), reposo, vibracion, drenaje
    d: vec4<f32>,
    // mezcla, veta, matiz, caja
    e: vec4<f32>,
    // marco, brillo, estorbos, semilla
    f: vec4<f32>,
    // The `body` part's picture and `mat_*`, one `MaterialU` — see
    // material.wgsl, prepended ahead of this file.
    mat_body: MaterialU,
};

/// Per step, not per frame: several steps run inside one encoder and each one
/// needs its own clock and its own hash salt. Without this the sand poured by
/// the four steps of a 60 Hz frame would be four identical rows.
struct StepUniform {
    // step_time, step_index, block origin offset (0 or 1), unused
    s: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: TerrarioUniform;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var src: texture_2d<f32>;
@group(0) @binding(3) var<uniform> st: StepUniform;
// The sand's material, read by the fragment stage only. At 4/5 so the compute
// entry points, which share this module, keep their 0..=3 layout.
@group(0) @binding(4) var surf_body_tex: texture_2d<f32>;
@group(0) @binding(5) var surf_body_samp: sampler;

// ---------------------------------------------------------------------------
// the box
// ---------------------------------------------------------------------------

/// The walls, the floor, and whatever is standing in the middle of the box.
///
/// Everything outside the grid is solid, which is what makes the terrarium a
/// terrarium: a grain can never be permuted into a cell that does not exist,
/// so nothing leaves through the sides and the floor holds the first pile up.
/// `drenaje` is the only way out, and it is a hole in the floor rather than an
/// absence of one.
fn solid_at(c: vec2<i32>, w: i32, h: i32) -> bool {
    if (c.x < 0 || c.y < 0 || c.x >= w || c.y >= h) {
        return true;
    }
    let obstacles = clamp(u.f.z, 0.0, 1.0);
    if (obstacles <= 0.001) {
        return false;
    }
    // Never in the top rows: an obstacle under a mouth would cork it, and a
    // corked mouth is a source that silently pours nothing.
    if (c.y < 16) {
        return false;
    }
    let cell = vec2<f32>(f32(c.x), f32(c.y));
    let sp = 58.0;
    let g = floor(cell / sp);
    let seed = u.f.w;
    if (lattice(g + vec2<f32>(seed * 17.3, seed * 29.1)) > obstacles * 0.85) {
        return false;
    }
    let wobble = vec2<f32>(lattice(g + 3.1), lattice(g - 7.3)) - 0.5;
    let centre = (g + 0.5 + wobble * 0.6) * sp;
    let rad = sp * 0.13 * (0.6 + 0.9 * lattice(g + 13.7));
    return length(cell - centre) < rad;
}

/// Where mouth `i` of `n` is pouring at time `t`, in cells across the grid.
fn mouth_x(i: f32, n: f32, t: f32, w: f32) -> f32 {
    // One mouth pours down the middle rather than at one end of the spread.
    var spread = 0.0;
    if (n > 1.5) {
        spread = (i / (n - 1.0) - 0.5) * u.b.z;
    }
    let sway = u.c.y * 0.5 * sin(6.28318 * (t * u.c.z + i * u.c.w));
    let x = u.b.y + spread + sway;
    return (x * 0.5 + 0.5) * w;
}

// ---------------------------------------------------------------------------
// the block rule
// ---------------------------------------------------------------------------

/// The permutation. `map[k]` is which of the block's four cells ends up in
/// slot `k`; slots are 0=TL, 1=TR, 2=BL, 3=BR.
///
/// The twin of `models::terrario::settle`, line for line, and the twin is not
/// a convenience: this is the one piece of the model that can be wrong without
/// producing anything out of range, and a WGSL function cannot be unit tested.
/// The Rust copy is where "it is always a permutation" and "no grain ever
/// rises" are actually checked.
fn settle(occ: array<bool, 4>, sun: array<bool, 4>, rest: f32, bias: f32, vibra: f32,
          r: vec3<f32>) -> array<i32, 4> {
    var map = array<i32, 4>(0, 1, 2, 3);
    var o = occ;
    // Copied into a `var` because the loop below indexes them with a value the
    // compiler cannot fold, and WGSL only allows that on memory.
    var s = sun;

    // 1. Straight down. Both cells have to be free of wall for a grain to
    //    cross between them, so a floor block simply never moves anything.
    for (var c = 0; c < 2; c = c + 1) {
        let top = c;
        let bot = c + 2;
        if (o[top] && !o[bot] && !s[top] && !s[bot]) {
            let keep = map[top];
            map[top] = map[bot];
            map[bot] = keep;
            o[top] = false;
            o[bot] = true;
        }
    }

    // 2. Off the side of the pile. Only reachable when the cell directly below
    //    is taken and the diagonal one is not — which is exactly "this grain
    //    is resting on the shoulder of another".
    let slide = clamp(rest * 0.9 + vibra * 0.3, 0.0, 1.0);
    let right = clamp(slide * (0.5 + 0.5 * bias), 0.0, 1.0);
    let left = clamp(slide * (0.5 - 0.5 * bias), 0.0, 1.0);
    if (o[0] && o[2] && !o[3] && !s[0] && !s[3] && r.x < right) {
        let keep = map[0];
        map[0] = map[3];
        map[3] = keep;
        o[0] = false;
        o[3] = true;
    }
    if (o[1] && o[3] && !o[2] && !s[1] && !s[2] && r.y < left) {
        let keep = map[1];
        map[1] = map[2];
        map[2] = keep;
        o[1] = false;
        o[2] = true;
    }

    // 3. Shuffling along the floor. What keeps a settled pile from being a
    //    permanent monument: a shaken or windblown pile levels itself.
    let shake = clamp(vibra * 0.25 + abs(bias) * 0.18, 0.0, 1.0);
    if (!s[2] && !s[3]) {
        if (o[2] && !o[3] && r.z < shake * (0.5 + 0.5 * bias)) {
            let keep = map[2];
            map[2] = map[3];
            map[3] = keep;
        } else if (o[3] && !o[2] && (1.0 - r.z) < shake * (0.5 - 0.5 * bias)) {
            let keep = map[3];
            map[3] = map[2];
            map[2] = keep;
        }
    }

    return map;
}

@compute @workgroup_size(8, 8, 1)
fn cs_seed(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(u.a.z);
    let h = i32(u.a.w);
    let c = vec2<i32>(i32(gid.x), i32(gid.y));
    if (c.x >= w || c.y >= h) {
        return;
    }
    // An empty box. The first thing an operator sees is the burn-in's worth of
    // sand already falling and already piled — see `BURN_IN` on the Rust side.
    textureStore(dst, c, vec4<f32>(0.0, 0.0, 0.0, 0.0));
}

@compute @workgroup_size(8, 8, 1)
fn cs_step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(u.a.z);
    let h = i32(u.a.w);
    let off = i32(st.s.z);
    // Block origins run from -off so that, on the shifted step, the cells in
    // row and column 0 are still covered by somebody.
    let base = vec2<i32>(i32(gid.x) * 2 - off, i32(gid.y) * 2 - off);
    if (base.x >= w || base.y >= h) {
        return;
    }

    let t = st.s.x;
    let salt = st.s.y;

    var cell: array<vec4<f32>, 4>;
    var occ: array<bool, 4>;
    var sun: array<bool, 4>;
    var pos: array<vec2<i32>, 4>;
    for (var k = 0; k < 4; k = k + 1) {
        let p = base + vec2<i32>(k % 2, k / 2);
        pos[k] = p;
        let s = solid_at(p, w, h);
        sun[k] = s;
        if (s) {
            cell[k] = vec4<f32>(0.0);
            occ[k] = false;
        } else {
            let v = textureLoad(src, p, 0);
            cell[k] = v;
            occ[k] = v.x > 0.5;
        }
    }

    let r = vec3<f32>(
        lattice(vec2<f32>(f32(base.x) + salt * 1.7, f32(base.y) - salt * 3.1)),
        lattice(vec2<f32>(f32(base.x) - salt * 5.3 + 41.0, f32(base.y) + salt * 2.9)),
        lattice(vec2<f32>(f32(base.x) + salt * 0.9 - 17.0, f32(base.y) + salt * 7.7)),
    );

    let map = settle(occ, sun, clamp(u.d.y, 0.0, 1.0), clamp(u.d.x, -1.0, 1.0),
                     clamp(u.d.z, 0.0, 1.0), r);

    // The mean colour of the block's grains, for `mezcla`. Taken before
    // anything is written, so every grain in the block bleeds towards the same
    // number and no grain bleeds towards itself twice.
    var sum = 0.0;
    var count = 0.0;
    for (var k = 0; k < 4; k = k + 1) {
        if (occ[k]) {
            sum = sum + cell[k].y;
            count = count + 1.0;
        }
    }
    let mean = select(0.0, sum / max(count, 1.0), count > 0.0);
    let mezcla = clamp(u.e.x, 0.0, 1.0);

    let sources = clamp(round(u.b.x), 1.0, 10.0);
    let flow = clamp(u.c.x, 0.0, 1.0);
    let half_mouth = max(u.b.w, 0.002) * 0.25 * u.a.z;
    let hue = clamp(u.e.z, 0.0, 1.0);
    let drain = clamp(u.d.w, 0.0, 1.0) * 0.14;

    for (var k = 0; k < 4; k = k + 1) {
        let p = pos[k];
        if (p.x < 0 || p.y < 0 || p.x >= w || p.y >= h) {
            continue;
        }
        if (sun[k]) {
            // Written, not skipped. The destination is the other half of a
            // ping-pong pair, so a cell nobody writes keeps whatever it held
            // two steps ago — an obstacle turned on under a pile would leave a
            // ghost of that pile flickering at half the step rate.
            textureStore(dst, p, vec4<f32>(0.0));
            continue;
        }
        var out = cell[map[k]];
        let moved = map[k] != k;
        if (out.x > 0.5) {
            out.y = mix(out.y, mean, mezcla * 0.06);
            // The motion channel: 1 the instant a grain moves, decaying while
            // it rests. The render pass lights the falling sand with it, which
            // is the whole reason a stream reads as a stream and a pile reads
            // as a pile when they are the same colour.
            out.w = select(max(out.w * 0.90, 0.0), 1.0, moved);
        }

        // The mouths. They pour into whatever is empty in the top band, so a
        // stream that backs up against its own pile stops pouring instead of
        // overwriting it.
        if (p.y < 3 && out.x <= 0.5) {
            let jitter = lattice(vec2<f32>(f32(p.x) + salt * 11.1, f32(p.y) - salt * 4.3));
            let grain = lattice(vec2<f32>(f32(p.x) - salt * 6.1, f32(p.y) + salt * 8.9 + 23.0));
            for (var i = 0.0; i < sources; i = i + 1.0) {
                let mx = mouth_x(i, sources, t, u.a.z);
                if (abs(f32(p.x) + 0.5 - mx) <= half_mouth && jitter < flow) {
                    var tone = 0.0;
                    if (sources > 1.5) {
                        tone = (i / (sources - 1.0)) * hue;
                    }
                    out = vec4<f32>(1.0, clamp(tone, 0.0, 1.0), grain, 1.0);
                    break;
                }
            }
        }

        // The drain. Without it the box fills, the rule runs out of holes to
        // move grains into, and the model becomes a still image of a full box
        // — legal, and what `drenaje` at 0 is for.
        if (p.y == h - 1 && out.x > 0.5) {
            let d = lattice(vec2<f32>(f32(p.x) + salt * 3.3, salt * 1.9 + 77.0));
            if (d < drain) {
                out = vec4<f32>(0.0);
            }
        }

        textureStore(dst, p, out);
    }
}

// ---------------------------------------------------------------------------
// looking at it
// ---------------------------------------------------------------------------

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
    let aspect = max(u.a.x, 0.01);
    let w = u.a.z;
    let h = u.a.w;
    let p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    var col = backdrop_from_uv(u.background, in.uv, aspect);

    // The grid is shaped like the viewport (`grid_dims` on the Rust side), so
    // the box is simply a fraction of whatever the view is: no letterbox at
    // any aspect.
    let caja = clamp(u.e.w, 0.05, 1.0);
    let half_h = caja;
    let half_w = caja * aspect;

    let inside = abs(p.x) <= half_w && abs(p.y) <= half_h;
    let frame = clamp(u.f.x, 0.0, 1.0);
    let shine = clamp(u.f.y, 0.0, 1.0);

    if (inside) {
        let gx = (p.x + half_w) / (2.0 * half_w) * w;
        let gy = (half_h - p.y) / (2.0 * half_h) * h;
        let c = vec2<i32>(i32(floor(gx)), i32(floor(gy)));
        let v = textureLoad(src, clamp(c, vec2<i32>(0), vec2<i32>(i32(w) - 1, i32(h) - 1)), 0);

        // A grain is drawn round inside its own cell rather than as the square
        // the simulation stores. At a 1080p wall a cell is a few pixels across
        // and the difference is whether a pile has a grain texture or a
        // staircase.
        let f = fract(vec2<f32>(gx, gy)) - 0.5;
        let d = length(f);
        let cover = 1.0 - smoothstep(0.45, 0.72, d);

        if (solid_at(c, i32(w), i32(h))) {
            let peg = grad4(u.crystal, 0.75);
            col = mix(col, peg * (0.25 + 0.45 * frame), 0.85);
        } else if (v.x > 0.5) {
            let vein = clamp(u.e.y, 0.0, 1.0);
            let tone = grad4(u.sand, clamp(v.y, 0.0, 1.0));
            let shade = 1.0 - vein * 0.45 * (1.0 - v.z);
            // The shared material, off at mix 0 (a migrated row). Sand is a
            // top-down surface: the normal faces the viewer, and the grain's
            // own cell is the object space a kind patterns over.
            let shaded = material_apply(
                u.mat_body, surf_body_tex, surf_body_samp, tone,
                vec3<f32>(gx, gy, v.y), vec3<f32>(0.0, 0.0, 1.0), in.uv,
                vec3<f32>(0.0, 0.0, 1.0), u.a.y,
            );
            var sand = shaded.albedo * shade;
            // Falling sand is lit; resting sand is not. `v.w` is the motion
            // channel the step wrote.
            sand = sand + shaded.albedo * v.w * shine * 0.40;
            col = mix(col, sand, cover);
        }
    }

    // The glass. Drawn after the sand so a pile leaning on a wall reads as
    // being behind it.
    if (frame > 0.001) {
        let edge = max(abs(p.x) / half_w, abs(p.y) / half_h);
        let wall = (1.0 - smoothstep(0.0, 0.012, abs(edge - 1.0))) * frame;
        let sheen = exp(-max(1.0 - edge, 0.0) * 26.0) * frame * 0.30;
        let glass = grad4(u.crystal, clamp((p.y / half_h) * 0.5 + 0.5, 0.0, 1.0));
        col = col + glass * (wall * 0.9 + sheen);
    }

    return vec4<f32>(col, 1.0);
}
