// halftonecmyk: a picture reprinted as four overprinted halftone screens
// (cyan, magenta, yellow, black), each on its own rotated grid. Behaviour and
// parameter list written from the halftone CMYK" effect; everything
// here is original.
//
// The picture (source `imagen`, or the procedural test image) is sampled once
// per halftone cell, converted to four ink amounts (with under-colour removal
// `ucr`), shaped by contrast / gain / flood, and drawn as a dot, ink blob,
// sharp dot, line or square whose size follows the amount. The four screens are
// multiplied over the paper (subtractive mixing), so overprints go dark.
// Misregistration shifts each screen differently; grid noise jitters cells.

fn ch_angle(ch: i32) -> f32 {
    if (ch == 0) { return f_angle_c(); }
    if (ch == 1) { return f_angle_m(); }
    if (ch == 2) { return f_angle_y(); }
    return f_angle_k();
}
fn ch_flood(ch: i32) -> f32 {
    if (ch == 0) { return f_flood_c(); }
    if (ch == 1) { return f_flood_m(); }
    if (ch == 2) { return f_flood_y(); }
    return f_flood_k();
}
fn ch_gain(ch: i32) -> f32 {
    if (ch == 0) { return f_gain_c(); }
    if (ch == 1) { return f_gain_m(); }
    if (ch == 2) { return f_gain_y(); }
    return f_gain_k();
}

// Ink amounts (c, m, y, k) of a linear rgb colour.
fn to_cmyk(rgb_in: vec3<f32>) -> vec4<f32> {
    var c = rgb_in * f_shine_img();
    let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    c = mix(vec3<f32>(l), c, f_sat_img());
    c = clamp((c - vec3<f32>(0.18)) * f_contrast_img() + vec3<f32>(0.18), vec3<f32>(0.0), vec3<f32>(1.0));
    // Work in a perceptual-ish space so mid greys print as half-covered dots.
    let g = pow(c, vec3<f32>(0.4545));
    let cmy = vec3<f32>(1.0) - g;
    let k = min(cmy.x, min(cmy.y, cmy.z)) * f_ucr();
    let rest = clamp((cmy - vec3<f32>(k)) / max(1.0 - k, 0.02), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rest, k);
}

fn ink_amount(v0: f32, ch: i32) -> f32 {
    var v = clamp(v0 * (1.0 + ch_gain(ch)) + ch_flood(ch), 0.0, 1.0);
    let post = f_posterize();
    if (post > 0.0) {
        let n = mix(16.0, 3.0, post);
        v = mix(v, floor(v * n + 0.5) / n, min(post * 2.0, 1.0));
    }
    return v;
}

// Coverage (0..1) of channel `ch` at picture point p.
fn screen_cov(p: vec2<f32>, uv: vec2<f32>, ch: i32, cellsz: f32, t: f32, grain: f32) -> f32 {
    let fch = f32(ch);
    // Misregistration: each screen drifts differently.
    let rd = (f_register_direction() + fch * 0.31) * TAU;
    let wob = 0.6 + 0.4 * sin(t * f_register_speed() * 2.0 + fch * 1.9);
    let off = vec2<f32>(cos(rd), sin(rd)) * f_register() * cellsz * 1.5 * wob * (0.4 + 0.2 * fch);
    let ang = (ch_angle(ch) + f_rotate_red() + f_rotate_speed() * t * 0.05) * TAU;
    let q = p_rot(p - off, -ang) / cellsz;
    let base = floor(q);
    let tipo = clamp(i32(floor(f_type() + 0.5)), 0, 4);
    let sd = f_seed() * 3.7 + fch * 19.0;
    var best = 0.0;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let cid = base + vec2<f32>(f32(i), f32(j));
            let h = p_hash2(cid + vec2<f32>(sd, sd * 0.37));
            let ctr = cid + vec2<f32>(0.5) + (h - vec2<f32>(0.5)) * f_noise_grid();
            let d = q - ctr;
            if (dot(d, d) > 1.3) { continue; }
            // Sample the picture at the cell centre (back in picture space).
            let pc = p_rot(ctr * cellsz, ang) + off;
            let v = ink_amount(to_cmyk(p_src(pc).rgb)[ch], ch);
            let dens = 1.0 - f_stain() * p_noise(pc * 5.0 + vec2<f32>(sd, t * 0.05)) * 0.8;
            let r = sqrt(v) * 0.7071 * (0.8 + 0.4 * h.x * f_noise_grid()) ;
            var distance = 0.0;
            if (tipo == 3) {
                distance = abs(d.y) - v * 0.5;
            } else if (tipo == 4) {
                distance = max(abs(d.x), abs(d.y)) - sqrt(v) * 0.5;
            } else {
                distance = length(d) - r;
            }
            if (v < 0.02) { distance = 1.0; }
            var soft = 0.0;
            if (tipo == 1) {
                let bl = p_noise(q * 7.0 + vec2<f32>(sd, 0.0)) - 0.5;
                distance = distance + bl * (0.12 + f_indent() * 0.5);
                soft = 0.03 + f_indent() * 0.08;
            }
            if (tipo == 2) { soft = 0.0; }
            distance = distance + grain * f_grain_mix() * 0.35;
            let aa = max(fwidth(distance), 0.002) + soft + f_softness() * 0.2;
            let cov = (1.0 - smoothstep(-aa, aa, distance)) * dens;
            best = max(best, cov);
        }
    }
    return best;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    var cell = max(f_size() * 0.5, 0.008);
    cell = cell * (1.0 + f_ripple_size() * 0.6 * sin(p.x * 2.5 + p.y * 1.7 - t * 0.6));
    cell = cell * (1.0 + f_breathe() * 0.35 * sin(t * f_breathe_speed() * 1.5));
    cell = max(cell, 0.006);

    let gs = mix(900.0, 90.0, f_grain_size());
    let grain = p_hash(floor(uv * vec2<f32>(p_aspect(), 1.0) * gs) + vec2<f32>(floor(t * 12.0) * 13.0, 5.0)) - 0.5;

    // Paper: base colour with fibre mottling.
    let fib = p_fbm(p_rot(p, 0.4) * vec2<f32>(90.0, 8.0), 3) * 0.6 + p_noise(p * 30.0) * 0.4;
    var col = p_pal(1u, clamp((fib - 0.35) * f_fiber() * 2.5, 0.0, 1.0));
    col = col + vec3<f32>(grain * f_grain_surface() * 0.25);

    var inkcov = 0.0;
    for (var ch = 0; ch < 4; ch = ch + 1) {
        let cov = screen_cov(p, uv, ch, cell, t, grain) * f_density();
        inkcov = max(inkcov, clamp(cov, 0.0, 1.0));
        let ink = p_pal(0u, f32(ch) / 3.0);
        col = col * mix(vec3<f32>(1.0), ink, clamp(cov, 0.0, 1.0));
    }
    // `papel_op` < 1 lets the backdrop (fondo) show through bare paper.
    return vec4<f32>(max(col, vec3<f32>(0.0)), max(f_paper_opacity(), inkcov));
}
