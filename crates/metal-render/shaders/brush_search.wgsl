// fx.brush -- search module: best-of-K candidates scored at the 64x64 proxy,
// the way primitive's core.go picks each shape (random candidates, then a
// hill-climb step), with the error reduced on the GPU. `brush_common.wgsl` is
// prepended.

@group(0) @binding(1) var tgt: texture_2d<f32>; // the frame, 64x64
@group(0) @binding(2) var cur: texture_2d<f32>; // the canvas so far, 64x64
@group(0) @binding(3) var<storage, read_write> scores: array<vec4<f32>>; // x = error change
@group(0) @binding(4) var<storage, read_write> cols: array<vec4<f32>>;  // rgb, alpha
@group(0) @binding(5) var<storage, read_write> chosen: array<Stroke>;

const PROXY: u32 = 64u;

// Candidate `k` of `round` at `stage`: stage 0 is random, stage 1 mutates the
// winner of stage 0 (candidate 0 is the winner itself, so it cannot get worse).
fn make_cand(round: u32, stage: u32, k: u32) -> Stroke {
    let alea = u.a.z;
    let seed = u32(u.f.x) * 977u + round * 131u + k * 7u + stage * 7919u;
    if (stage == 1u) {
        var s = chosen[round];
        if (k == 0u) {
            return s;
        }
        let sz = max(s.hw, 0.004);
        s.c = clamp(s.c + (vec2<f32>(rnd(seed, 1u), rnd(seed, 2u)) - 0.5) * sz * 1.2 / vec2<f32>(u.a.x, 1.0), vec2<f32>(0.0), vec2<f32>(1.0));
        s.ang = s.ang + (rnd(seed, 3u) - 0.5) * 0.9;
        s.hl = max(s.hl * mix(0.8, 1.25, rnd(seed, 4u)), 0.004);
        s.hw = max(s.hw * mix(0.8, 1.25, rnd(seed, 5u)), 0.004);
        s.bend = clamp(s.bend + (rnd(seed, 6u) - 0.5) * 0.6, -1.0, 1.0);
        return s;
    }
    let rounds = max(u.h.y, 1.0);
    let t = f32(round) / max(rounds - 1.0, 1.0);
    let size = u.d.w * mix(u.b.x, u.b.y, t) * (1.0 + (rnd(seed, 3u) - 0.5) * (0.4 + alea)) * mix(0.7, 1.3, rnd(seed, 4u)) * 0.55;
    let stretch = sqrt(u.b.z);
    var s: Stroke;
    s.c = vec2<f32>(rnd(seed, 1u), rnd(seed, 2u));
    // Random direction, turned toward the frame's edge direction at the
    // candidate's centre by `sigue`.
    let base = u.c.x + (rnd(seed, 5u) - 0.5) * 2.0 * PI;
    let px = clamp(vec2<i32>(s.c * f32(PROXY)), vec2<i32>(1), vec2<i32>(i32(PROXY) - 2));
    let g = vec2<f32>(
        lum(textureLoad(tgt, px + vec2<i32>(1, 0), 0).rgb) - lum(textureLoad(tgt, px - vec2<i32>(1, 0), 0).rgb),
        lum(textureLoad(tgt, px + vec2<i32>(0, 1), 0).rgb) - lum(textureLoad(tgt, px - vec2<i32>(0, 1), 0).rgb),
    );
    let edge_ang = atan2(g.y, g.x) + 0.5 * PI;
    s.ang = base + wrap_ang(edge_ang - base) * u.b.w * smoothstep(0.02, 0.15, length(g));
    s.hl = size * 0.5 * stretch;
    s.hw = size * 0.5 / stretch;
    s.bend = (rnd(seed, 9u) - 0.5) * 2.0 * (0.3 + alea * 0.7);
    s.shape = pick_shape(u.e.x, seed);
    s.pad = 0.0;
    s.col = vec4<f32>(0.0, 0.0, 0.0, u.a.y);
    return s;
}

// Coverage of the stroke over proxy pixel `px`.
fn proxy_cov(s: Stroke, px: vec2<i32>) -> f32 {
    let uv = (vec2<f32>(px) + 0.5) / f32(PROXY);
    let p = vec2<f32>(uv.x * u.a.x, uv.y) - vec2<f32>(s.c.x * u.a.x, s.c.y);
    let c = cos(s.ang);
    let sn = sin(s.ang);
    let local = vec2<f32>(c * p.x + sn * p.y, -sn * p.x + c * p.y);
    let d = stroke_d(s.shape, local, s.hl, s.hw, s.bend);
    return clamp(0.5 - d * f32(PROXY), 0.0, 1.0);
}

var<workgroup> sh: array<vec4<f32>, 256>;

fn reduce(tid: u32) {
    workgroupBarrier();
    for (var s = 128u; s > 0u; s = s >> 1u) {
        if (tid < s) {
            sh[tid] = sh[tid] + sh[tid + s];
        }
        workgroupBarrier();
    }
}

@compute @workgroup_size(256)
fn cs_score(@builtin(local_invocation_index) tid: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    let k = wg.x;
    let s = make_cand(u.g.x, u.g.y, k);
    let alpha = max(u.a.y, 0.02);

    // The colour that best explains the frame under the stroke (primitive's
    // computeColor): the mean of (target - canvas) / alpha + canvas.
    var acc = vec4<f32>(0.0);
    for (var j = 0u; j < 16u; j = j + 1u) {
        let i = tid + j * 256u;
        let px = vec2<i32>(i32(i % PROXY), i32(i / PROXY));
        let cov = proxy_cov(s, px);
        if (cov > 0.0) {
            let t = textureLoad(tgt, px, 0).rgb;
            let c = textureLoad(cur, px, 0).rgb;
            acc = acc + vec4<f32>(((t - c) / alpha + c) * cov, cov);
        }
    }
    sh[tid] = acc;
    reduce(tid);
    let tot = sh[0];
    let col = clamp(tot.rgb / max(tot.w, 1e-4), vec3<f32>(0.0), vec3<f32>(1.0));
    workgroupBarrier();

    // Error change if the stroke were laid down in that colour.
    var e = 0.0;
    for (var j = 0u; j < 16u; j = j + 1u) {
        let i = tid + j * 256u;
        let px = vec2<i32>(i32(i % PROXY), i32(i / PROXY));
        let cov = proxy_cov(s, px);
        if (cov > 0.0) {
            let t = textureLoad(tgt, px, 0).rgb;
            let c = textureLoad(cur, px, 0).rgb;
            let n = c + (col - c) * alpha * cov;
            e = e + dot(n - t, n - t) - dot(c - t, c - t);
        }
    }
    sh[tid] = vec4<f32>(e, 0.0, 0.0, 0.0);
    reduce(tid);
    if (tid == 0u) {
        scores[k] = vec4<f32>(select(sh[0].x, 1.0e9, tot.w < 0.5), tot.w, 0.0, 0.0);
        cols[k] = vec4<f32>(col, alpha);
    }
}

// One thread picks the lowest error and writes that stroke to chosen[round].
@compute @workgroup_size(1)
fn cs_best() {
    var bk = 0u;
    var be = scores[0].x;
    for (var k = 1u; k < u.g.z; k = k + 1u) {
        if (scores[k].x < be) {
            be = scores[k].x;
            bk = k;
        }
    }
    var s = make_cand(u.g.x, u.g.y, bk);
    // The same finish a dab gets: lightness/colour jitter, then saturation.
    let k = u32(u.f.x) * 977u + u.g.x * 131u + 55u;
    let jl = (rnd(k, 5u) - 0.5) * u.c.y * 1.2;
    let jc = (vec3<f32>(rnd(k, 6u), rnd(k, 7u), rnd(k, 10u)) - 0.5) * u.c.y * 0.5;
    var col = cols[bk].rgb * (vec3<f32>(1.0 + jl) + jc) + vec3<f32>(jl * 0.1);
    let l = lum(col);
    col = clamp(vec3<f32>(l) + (col - vec3<f32>(l)) * u.c.z, vec3<f32>(0.0), vec3<f32>(1.0));
    s.col = vec4<f32>(col, cols[bk].a);
    chosen[u.g.x] = s;
}
