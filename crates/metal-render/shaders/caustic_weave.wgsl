// caustic-weave: stitched caustic light, after the community tool "Caustic
// Stitch" (behaviour and parameter list only; this WGSL is original).
//
// Two endpoint families are laid out on one of nine closed/strand shapes. Each
// stitch is a chord from index i of the first family to index j = i*multiplier
// of the second. The chord is bent by a slow, shared warp field and drawn as a
// bright core plus a broad halo; the additive overlap of many stitches reads as
// caustic light. A deterministic hash picks which chords get the accent colour
// and width, seeded by accent_variations so a fixed value never flickers.
//
// `points` is the source's stitch density. The fragment loop is fixed (16
// chords), so the index stride comes from `points`: the whole set is resampled
// as points changes, which preserves the density look without a 240-iteration
// per-pixel loop.

fn cw_mod(x: f32, n: f32) -> f32 {
    return x - n * floor(x / n);
}

fn cw_axis_capacity(rot: f32) -> f32 {
    // Frame in picture coordinates: x in +-aspect/2, y in +-0.5.
    let hx = p_aspect() * 0.5;
    let hy = 0.5;
    let sx = abs(sin(rot));
    let cz = abs(cos(rot));
    var cx = 4.0;
    var cy = 4.0;
    if (sx > 1e-5) { cx = hx / sx; }
    if (cz > 1e-5) { cy = hy / cz; }
    return max(1.0, min(cx, cy) * 2.0);
}

fn cw_rect_point(idx: f32, n: f32, r: f32, rot: f32) -> vec2<f32> {
    let u = fract(idx / max(n, 1.0)) * 4.0;
    let seg = floor(u);
    let t = u - seg;
    var x = 0.0;
    var y = 0.0;
    if (seg < 1.0) {
        x = -r + t * 2.0 * r; y = -r;
    } else if (seg < 2.0) {
        x = r; y = -r + t * 2.0 * r;
    } else if (seg < 3.0) {
        x = r - t * 2.0 * r; y = r;
    } else {
        x = -r; y = r - t * 2.0 * r;
    }
    return p_rot(vec2<f32>(x, y), rot);
}

fn cw_tri_point(idx: f32, n: f32, r: f32, rot: f32) -> vec2<f32> {
    let u = fract(idx / max(n, 1.0)) * 3.0;
    let seg = floor(u);
    let t = u - seg;
    let s = sin(PI / 3.0) * r;
    var a = vec2<f32>(0.0, -r);
    var b = vec2<f32>(s, r * 0.5);
    if (seg < 1.0) {
        a = vec2<f32>(0.0, -r); b = vec2<f32>(s, r * 0.5);
    } else if (seg < 2.0) {
        a = vec2<f32>(s, r * 0.5); b = vec2<f32>(-s, r * 0.5);
    } else {
        a = vec2<f32>(-s, r * 0.5); b = vec2<f32>(0.0, -r);
    }
    return p_rot(mix(a, b, t), rot);
}

fn cw_endpoint(
    idx: f32,
    n: f32,
    side: f32,
    mode: i32,
    base_r: f32,
    spread_px: f32,
    rot: f32,
) -> vec2<f32> {
    // Loom / fissure: own long axis, fitted to the frame at this rotation.
    if (mode == 2 || mode == 3) {
        let long_cap = cw_axis_capacity(rot);
        let short_cap = cw_axis_capacity(rot + PI * 0.5);
        let scale_n = clamp(f_scale() / 100.0, 0.0, 1.0);
        let long_size = long_cap * mix(0.16, 0.90, scale_n);
        let short_size = short_cap * mix(0.14, 0.62, scale_n) + spread_px;
        let t = idx / max(n - 1.0, 1.0);
        let y = -long_size * 0.5 + t * long_size;
        if (mode == 2) {
            let wave = sin(t * TAU) * spread_px * 0.28;
            let x = side * short_size * 0.5 + wave;
            return p_rot(vec2<f32>(x, y), rot);
        }
        let kink = (p_hash(vec2<f32>(idx * 8.311 + side * 19.7, 3.0)) - 0.5) * spread_px * 0.55;
        let x = side * (short_size * 0.14 + sin(t * PI * 3.0) * short_size * 0.05) + kink;
        return p_rot(vec2<f32>(x, y), rot);
    }

    let a = (idx / max(n, 1.0)) * TAU + rot;

    if (mode == 1) {
        return cw_rect_point(idx, n, base_r * (0.80 + side * 0.10), rot);
    }
    if (mode == 4) {
        let wob = 0.04 + spread_px / max(base_r, 0.05) * 0.08;
        let rr = base_r * (1.0 + wob * sin(a * 3.0 + idx * 0.137));
        return vec2<f32>(cos(a), sin(a)) * rr;
    }
    if (mode == 5) {
        let k = 1.0 + 0.02 * sin(a * 4.0 + idx * 0.19);
        let rx = base_r * (1.12 + side * 0.12);
        let ry = base_r * 0.62;
        return vec2<f32>(cos(a) * rx * k, sin(a) * ry * k);
    }
    if (mode == 6) {
        return cw_tri_point(idx, n, base_r * (0.92 + side * 0.08), rot + side * 0.10);
    }
    if (mode == 7) {
        return cw_rect_point(idx, n, base_r * (0.90 + side * 0.08), rot + PI * 0.25 + side * 0.08);
    }
    if (mode == 8) {
        let petals = 5.0;
        let rr = base_r * (0.98 + side * 0.06) * (0.74 + 0.26 * cos(a * petals));
        return vec2<f32>(cos(a), sin(a)) * rr;
    }

    // mode 0: a clean circle.
    return vec2<f32>(cos(a), sin(a)) * base_r;
}

fn cw_spread(pt: vec2<f32>, side: f32, spread_px: f32) -> vec2<f32> {
    let l = max(length(pt), 1e-5);
    return pt + (pt / l) * (spread_px * 0.55 * side);
}

fn cw_seg_dist(pt: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = pt - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-9), 0.0, 1.0);
    return length(pa - ba * h);
}

// Distance to the quadratic through a, control c, b, as a 3-segment polyline.
fn cw_curve_dist(pt: vec2<f32>, a: vec2<f32>, c: vec2<f32>, b: vec2<f32>) -> f32 {
    let q1 = a * (4.0 / 9.0) + c * (4.0 / 9.0) + b * (1.0 / 9.0);
    let q2 = a * (1.0 / 9.0) + c * (4.0 / 9.0) + b * (4.0 / 9.0);
    let d0 = cw_seg_dist(pt, a, q1);
    let d1 = cw_seg_dist(pt, q1, q2);
    let d2 = cw_seg_dist(pt, q2, b);
    return min(min(d0, d1), d2);
}

const CW_CHORDS: i32 = 16;

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time() * f_speed();
    let mode = i32(floor(clamp(f_mode(), 0.0, 8.0) + 0.5));
    let n = clamp(floor(f_points() + 0.5), 12.0, 240.0);
    let mult = clamp(f_multiplier(), 1.0, 24.0);
    let scale_n = clamp(f_scale() / 100.0, 0.0, 1.0);
    let spread_n = clamp(f_spread() / 200.0, 0.0, 1.0);
    let line_n = clamp(f_line_width() / 100.0, 0.0, 1.0);
    let accent_n = clamp(f_accent() / 100.0, 0.0, 1.0);
    let accent_w_n = clamp(f_accent_width() / 100.0, 0.0, 1.0);
    let var_n = clamp(f_accent_variations() / 100.0, 0.0, 1.0);
    let rot = f_rotation() * PI / 180.0;
    let warp_n = clamp(f_warp() / 100.0, 0.0, 1.0);
    let soft_n = clamp(f_softness(), 0.0, 1.0);
    let glow_n = clamp(f_glow(), 0.0, 1.0);

    let base_r = mix(0.08, 0.48, scale_n);
    let spread_px = mix(0.0, 0.36, spread_n);
    let lw = mix(0.0009, 0.012, line_n);
    let aw = mix(max(0.001, lw), 0.032, accent_w_n);

    let variation_step = floor(var_n * 100.0);
    let accent_seed = variation_step * 97.331 + f32(mode) * 31.71 + n * 0.137
        + f_seed() * 13.7;

    // Shared slow field: every stitch bends with the same phase, so the weave
    // moves as one fabric rather than as unrelated noise.
    let global_phase = t * 0.16 + sin(t * 0.071) * 0.65;
    let global_x = cos(global_phase);
    let global_y = sin(global_phase);

    var line_em = vec3<f32>(0.0);
    var acc_em = vec3<f32>(0.0);

    for (var k: i32 = 0; k < CW_CHORDS; k = k + 1) {
        let idx = floor(f32(k) * n / f32(CW_CHORDS));
        var j = 0.0;
        if (mode == 2 || mode == 3) {
            j = floor((n - 1.0) - cw_mod(idx * mult, n));
        } else {
            j = floor(cw_mod(idx * mult, n));
        }
        j = clamp(j, 0.0, n - 1.0);

        var a = cw_endpoint(idx, n, -1.0, mode, base_r, spread_px, rot);
        var b = cw_endpoint(j, n, 1.0, mode, base_r, spread_px, rot);
        a = cw_spread(a, -1.0, spread_px);
        b = cw_spread(b, 1.0, spread_px);

        let d = b - a;
        let len = max(length(d), 1e-4);
        let nrm = vec2<f32>(-d.y, d.x) / len;
        let mid = (a + b) * 0.5;

        let h1 = p_hash(vec2<f32>(idx * 3.71 + j * 7.13, f32(mode) * 2.9));
        let h2 = p_hash(vec2<f32>(idx * 5.17 - j * 2.31, f32(mode) * 4.3 + 11.0));

        // Bend the straight chord: a slow shared pull plus a per-stitch drift.
        let side_global = global_x * nrm.x + global_y * nrm.y;
        let drift = sin(t * (0.11 + 0.25 * h1) + h2 * TAU);
        let bend = (side_global * 0.55 + drift * 0.45) * 0.5 * warp_n;
        let ctrl = mid + nrm * (len * bend);

        let dist = cw_curve_dist(p, a, ctrl, b);

        // Caustic beads travelling along the stitch.
        let spark = 0.62 + 0.38 * sin(dot(mid, vec2<f32>(9.0, -6.0)) + t * 2.2 + h1 * TAU);

        let halo_r = lw * (4.0 + 6.0 * soft_n) + 0.008 + soft_n * 0.03;
        let z = dist / max(lw, 1e-5);
        let core = exp(-z * z) * spark * (0.55 + 0.45 * accent_n);
        let halo = exp(-dist / max(halo_r, 1e-4)) * (0.22 + 0.55 * glow_n);

        let lc = p_pal(0u, clamp(0.20 + 0.60 * h1, 0.0, 1.0));
        line_em = line_em + lc * (core + halo * 0.55);

        // Deterministic accent selection; changing variations reshuffles it.
        let score = p_hash(vec2<f32>(idx * 47.17 + accent_seed, 3.7));
        if (score < accent_n) {
            let za = dist / max(aw, 1e-5);
            let acore = exp(-za * za) * spark;
            let ahalo_r = aw * (4.0 + 6.0 * soft_n) + 0.010 + soft_n * 0.04;
            let ahalo = exp(-dist / max(ahalo_r, 1e-4)) * (0.35 + 0.75 * glow_n);
            let ac = p_pal(1u, clamp(0.30 + 0.70 * h2, 0.0, 1.0));
            acc_em = acc_em + ac * (acore + ahalo * 0.45);
        }
    }

    // A faint caustic wash keeps the cloth between stitches alive.
    let wash = 0.03 + 0.05 * p_fbm(p * 3.0 + vec2<f32>(t * 0.02, 0.0), 3);
    var col = p_pal(0u, 0.18) * wash;
    let emit = line_em + acc_em;
    col = col + emit;
    // Crossings and hot cores gain an extra caustic focus.
    let caustic = clamp(dot(emit, vec3<f32>(0.3333)), 0.0, 1.0);
    col = col + emit * caustic * 0.55;
    col = col / (1.0 + col * 0.22);

    // Opaque field: the stitch light sits on its own dark caustic ground.
    let edge_a = 1.0 - smoothstep(0.47, 0.5, max(abs(uv.x - 0.5), abs(uv.y - 0.5)));
    return vec4<f32>(col, edge_a);
}
