// Model 18 — Puzzle. One instanced rounded tile, drawn once per piece on the
// board plus once per piece currently sliding off it. Everything about where a
// piece is and what colour it is was decided on the CPU (see `puzzle.rs`); this
// file only knows how to draw a tile with a four-stop gradient across it.
//
// The four stops are per instance rather than from a `ColorSlot` uniform,
// because that is the whole point of the model: a plain piece carries the two
// colours its insertion was given, and a piece made by two insertions colliding
// carries all four of theirs. One shader draws both — a plain piece is handed
// its two colours with the two midpoints filled in, so "two colours" and "four
// colours" are the same code path and cannot drift apart.

struct PuzzleUniform {
    // x: aspect, y: bisel, z: unused, w: unused
    misc: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: PuzzleUniform;

struct VIn {
    // cx, cy in the aspect-corrected space (x in ±aspect, y in ±1), then the
    // half-extent the tile is drawn at.
    @location(0) rect: vec4<f32>,
    // gradient axis xy, alpha, corner radius (world units, already scaled)
    @location(1) axis: vec4<f32>,
    @location(2) c0: vec4<f32>,
    @location(3) c1: vec4<f32>,
    @location(4) c2: vec4<f32>,
    @location(5) c3: vec4<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,   // world units, tile-centred
    @location(1) half: vec2<f32>,
    @location(2) axis: vec2<f32>,
    @location(3) fade_radius: vec2<f32>,
    @location(4) c0: vec4<f32>,
    @location(5) c1: vec4<f32>,
    @location(6) c2: vec4<f32>,
    @location(7) c3: vec4<f32>,
};

@vertex
fn vs_main(in: VIn, @builtin(vertex_index) vi: u32) -> VOut {
    // A triangle strip of four: (-1,-1) (1,-1) (-1,1) (1,1).
    let corner = vec2<f32>(
        select(-1.0, 1.0, (vi & 1u) == 1u),
        select(-1.0, 1.0, (vi & 2u) == 2u),
    );
    let half = in.rect.zw;
    let local = corner * half;
    let world = in.rect.xy + local;

    var out: VOut;
    out.clip = vec4<f32>(world.x / u.misc.x, world.y, 0.0, 1.0);
    out.local = local;
    out.half = half;
    out.axis = in.axis.xy;
    out.fade_radius = in.axis.zw;
    out.c0 = in.c0;
    out.c1 = in.c1;
    out.c2 = in.c2;
    out.c3 = in.c3;
    return out;
}

// `grad4` with the stops given rather than read from a uniform slot. Same
// piecewise walk as `shared.wgsl`, so a piece and a background gradient bend
// the same way.
fn grad4v(a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, d: vec3<f32>, t: f32) -> vec3<f32> {
    let tt = clamp(t, 0.0, 1.0) * 3.0;
    let seg = clamp(floor(tt), 0.0, 2.0);
    let f = tt - seg;
    if (seg < 0.5) { return mix(a, b, f); }
    if (seg < 1.5) { return mix(b, c, f); }
    return mix(c, d, f);
}

// Signed distance to a rounded box, negative inside.
fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// Which way the tile's surface faces at `p`. Used only for the bevel, so the
// straight-edge case (push out along the nearer axis) is enough — a bevel that
// is exact on the corners and approximate along the sides is a bevel.
fn box_normal(p: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let q = abs(p) - b;
    if (max(q.x, q.y) > 0.0) {
        return normalize(max(q, vec2<f32>(0.0, 0.0)) * sign(p) + vec2<f32>(1e-6, 1e-6));
    }
    if (q.x > q.y) { return vec2<f32>(sign(p.x), 0.0); }
    return vec2<f32>(0.0, sign(p.y));
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let b = max(in.half, vec2<f32>(1e-4, 1e-4));
    let r = clamp(in.fade_radius.y, 0.0, min(b.x, b.y));
    let d = sd_round_box(in.local, b, r);

    // Antialiased by the actual pixel footprint, so a tile is as crisp in a
    // 160x90 rack cell as it is at 1080p.
    let aa = max(fwidth(d), 1e-5);
    let cover = 1.0 - smoothstep(-aa, aa, d);
    if (cover <= 0.001) { discard; }

    // The gradient runs along the axis the insertion travelled, normalised by
    // the tile's own extent so it fills the tile rather than the frame.
    let n = normalize(in.axis + vec2<f32>(1e-6, 1e-6));
    let g = clamp(dot(in.local / b, n) * 0.5 + 0.5, 0.0, 1.0);
    var col = grad4v(in.c0.rgb, in.c1.rgb, in.c2.rgb, in.c3.rgb, g);

    // A rim bevel: light from the upper left, only within a band of the edge,
    // so the board reads as tiles sitting in it rather than as a flat mosaic.
    let bevel = clamp(u.misc.y, 0.0, 1.0);
    if (bevel > 0.001) {
        let band = min(b.x, b.y) * 0.30;
        let rim = 1.0 - smoothstep(0.0, band, -d);
        let nrm = box_normal(in.local, b - vec2<f32>(r, r));
        let lit = dot(nrm, normalize(vec2<f32>(-0.6, 0.8)));
        col = clamp(col * (1.0 + bevel * 0.55 * lit * rim), vec3<f32>(0.0), vec3<f32>(1.0));
    }

    return vec4<f32>(col, cover * clamp(in.fade_radius.x, 0.0, 1.0));
}
