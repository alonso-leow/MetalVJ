// dithering: a procedural field (7 selectable shapes) quantised to a few
// ink levels through an ordered (Bayer 2/4/8) or random threshold, on chunky
// cells. Written from the visible behaviour only; nothing here is upstream code.
//
// Pipeline: p -> centred/rotated/scaled -> warp (deformacion) -> shape field
// 0..1 (+ ondas ripple, detalle fbm, vineta) -> curve/threshold -> per-cell
// threshold from the matrix -> level -> colour from the two slots.

fn d_shape(q: vec2<f32>, t: f32, k: i32) -> f32 {
    let r = length(q);
    let a = atan2(q.y, q.x);
    if (k == 0) {
        return p_fbm(q * 1.3 + vec2<f32>(t * 0.3, -t * 0.2), 3);
    } else if (k == 1) {
        let w = vec2<f32>(p_fbm(q * 1.1 + vec2<f32>(t * 0.25, 3.1), 3), p_fbm(q * 1.1 + vec2<f32>(7.7, -t * 0.22), 3));
        return p_fbm(q + (w - 0.5) * 2.4, 3);
    } else if (k == 2) {
        let g = fract(q * 2.2 + 0.5) - 0.5;
        let id = floor(q * 2.2 + 0.5);
        let rad = 0.18 + 0.28 * (0.5 + 0.5 * sin(t * 1.3 + p_hash(id) * TAU));
        return 1.0 - smoothstep(rad * 0.4, rad + 0.12, length(g));
    } else if (k == 3) {
        return 0.5 + 0.5 * sin(q.x * 3.4 + 1.6 * sin(q.y * 2.3 + t * 0.9) + t * 0.7);
    } else if (k == 4) {
        return 0.5 + 0.5 * sin(r * 7.0 - t * 2.0);
    } else if (k == 5) {
        return 0.5 + 0.5 * sin(a * 3.0 + r * 4.5 - t * 1.4);
    }
    // sphere: lit ball with a moving light
    let rr = 0.85;
    let z = sqrt(max(rr * rr - r * r, 0.0));
    let n = vec3<f32>(q, z) / rr;
    let l = normalize(vec3<f32>(cos(t * 0.6) * 0.7, -0.6 + 0.2 * sin(t * 0.5), 0.8));
    let lit = clamp(dot(n, l), 0.0, 1.0);
    return select(0.04, 0.12 + 0.88 * lit, r < rr);
}

// Ordered threshold 0..1 for cell (ix, iy), n = 2^levels.
fn d_bayer(ix: i32, iy: i32, levels: i32) -> f32 {
    let x = u32(ix & 255);
    let y = u32(iy & 255);
    var v = 0u;
    for (var l = 0; l < levels; l = l + 1) {
        let bx = (x >> u32(l)) & 1u;
        let by = (y >> u32(l)) & 1u;
        v = v * 4u + 2u * (bx ^ by) + by;
    }
    return (f32(v) + 0.5) / f32(1u << u32(2 * levels));
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time() * f_speed() + f_phase() * 10.0;
    let cs = max(f_size(), 0.2) * 0.012;

    // Dither grid, optionally turned against the picture.
    let gq = p_rot(p, f_turn_mesh());
    let cell = floor(gq / cs);
    let cc = p_rot((cell + 0.5) * cs, -f_turn_mesh());

    // Field sampled at the cell centre so each cell is one flat block.
    var q = (cc - vec2<f32>(f_center_x(), f_center_y())) * f_scale();
    if (f_deformation() > 0.0001) {
        q = q + f_deformation() * vec2<f32>(
            sin(q.y * 2.6 + t * 0.8) + 0.5 * sin(q.y * 6.1 - t),
            cos(q.x * 2.3 - t * 0.7) + 0.5 * cos(q.x * 5.7 + t));
    }
    var f = d_shape(q, t, i32(round(f_shape())));
    f = f + f_waves() * 0.5 * sin(length(q) * 9.0 - t * 2.2);
    f = f + f_detail() * (p_fbm(q * 5.0 + vec2<f32>(0.0, t * 0.4), 3) - 0.5);
    f = f - f_vignette() * smoothstep(0.2, 1.3, length(cc / vec2<f32>(p_aspect(), 1.0)) * 1.4);
    f = clamp(f, 0.0, 1.0);
    f = pow(f, f_curve());
    f = clamp(f + f_threshold(), 0.0, 1.0);

    // Threshold from the matrix.
    let ix = i32(cell.x);
    let iy = i32(cell.y);
    let step_t = floor(p_time() * 8.0);
    let shift = i32(step_t * f_animation_mesh());
    var th: f32;
    let m = i32(round(f_matrix()));
    let sx = ix + shift;
    let sy = iy + shift * 2;
    if (m == 0) { th = d_bayer(sx, sy, 1); }
    else if (m == 1) { th = d_bayer(sx, sy, 2); }
    else if (m == 2) { th = d_bayer(sx, sy, 3); }
    else { th = p_hash(vec2<f32>(f32(sx), f32(sy)) + 11.0); }
    th = clamp(th + f_noise() * (p_hash(vec2<f32>(f32(ix), f32(iy)) + step_t * 3.0) - 0.5), 0.0, 1.0);

    // Quantise: n levels, threshold decides rounding up.
    let n = max(round(f_levels()), 2.0);
    let sc = f * (n - 1.0);
    var lv = (floor(sc) + select(0.0, 1.0, fract(sc) > th)) / (n - 1.0);
    lv = mix(lv, f, f_mix());
    if (f_invert() > 0.5) { lv = 1.0 - lv; }

    var col = mix(p_pal(1u, f), p_pal(0u, f), lv);
    // Cell grid lines.
    let fc = abs(fract(gq / cs) - 0.5) * 2.0;
    let edge = smoothstep(0.78, 1.0, max(fc.x, fc.y));
    col = col * (1.0 - f_grid() * edge);
    return vec4<f32>(col, 1.0 - f_empty() * (1.0 - lv));
}
