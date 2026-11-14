// imagedithering: the SOURCE picture (`imagen`) quantised to a few
// levels through a threshold map, on chunky cells. Behaviour and parameter
// list written from the image dithering" effect; the WGSL is original.
//
// Pipeline per pixel: p -> grid frame (rotated by giro_malla, scrolled by
// desplaza_x/y) -> cell -> read the picture at the cell centre (bloque) or at
// the pixel -> sharpen (realce) -> luma in a perceptual (sqrt) space -> invert,
// solarize, curve, bias -> optional error carried in from the 3 earlier cells
// (difusion; a stateless stand-in for error diffusion) -> threshold from the
// chosen map (tipo) + wave + noise -> level -> colour from the `tinta` ramp
// (optionally mixed with `acento`) or the picture's own colours
// (colores_orig), with per-channel threshold offsets (cromatico).

fn id_bayer(c: vec2<i32>, n: i32) -> f32 {
    let x = u32(c.x & 255);
    let y = u32(c.y & 255);
    var v = 0u;
    for (var l = 0; l < n; l = l + 1) {
        let bx = (x >> u32(l)) & 1u;
        let by = (y >> u32(l)) & 1u;
        v = v * 4u + (((bx ^ by) << 1u) | by);
    }
    return (f32(v) + 0.5) / f32(1u << u32(2 * n));
}

fn id_cell_world(cell: vec2<f32>, cs: f32, shift: vec2<f32>) -> vec2<f32> {
    return p_rot((cell + 0.5) * cs - shift, -f_turn_mesh());
}

// Perceptual luma 0..1 of a linear colour.
fn id_luma(c: vec3<f32>) -> f32 {
    return sqrt(clamp(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0));
}

// invert, solarize, curve, bias applied to one 0..1 value.
fn id_prep(v0: f32) -> f32 {
    var v = mix(v0, 1.0 - v0, f_invert());
    v = mix(v, abs(v * 2.0 - 1.0), f_solarize());
    v = pow(clamp(v, 0.0, 1.0), f_curve());
    return clamp(v + f_threshold(), 0.0, 1.0);
}

fn id_quant(v: f32, th: f32, n: f32) -> f32 {
    return clamp(floor(v * (n - 1.0) + th), 0.0, n - 1.0) / (n - 1.0);
}

// Threshold 0..1 of the chosen map at a cell.
fn id_th(cell: vec2<f32>, kind: i32, seed: f32) -> f32 {
    let ci = vec2<i32>(i32(cell.x), i32(cell.y));
    if (kind == 0) { return p_hash(cell + vec2<f32>(seed, 3.7)); }
    if (kind == 1) { return id_bayer(ci, 1); }
    if (kind == 2) { return id_bayer(ci, 2); }
    if (kind == 3) { return id_bayer(ci, 3); }
    if (kind == 4) {
        // Interleaved-gradient noise: smooth, blue-ish spectrum.
        return fract(52.9829189 * fract(dot(cell + seed, vec2<f32>(0.06711056, 0.00583715))));
    }
    if (kind == 5) {
        // Error-diffusion style: the map is flat, the carried error does the work.
        return 0.5 + (p_hash(cell + vec2<f32>(seed, 9.1)) - 0.5) * 0.12;
    }
    if (kind == 6) {
        // Clustered-dot screen: rank by distance to a 4x4-cell centre.
        let f = fract(cell / 4.0) - 0.5;
        return clamp(1.0 - length(f) * 1.6, 0.0, 1.0) * 0.94 + p_hash(cell + seed) * 0.05;
    }
    // Line screen: rows of rising threshold, a half step apart per column.
    return fract((cell.y + 0.5 * f32(ci.x & 1)) / 5.0 + 0.1);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let kind = clamp(i32(floor(f_type() + 0.5)), 0, 7);
    let n = max(2.0, floor(f_steps_color() + 0.5));
    let cs = max(f_size(), 0.3) * 0.012;
    let seed = f_seed() * 1.7;
    let shift = vec2<f32>(f_displace_x(), f_displace_y()) * t * cs * 6.0;

    let gq = p_rot(p, f_turn_mesh()) + shift;
    let cell = floor(gq / cs);
    let cu = fract(gq / cs);

    // Where the picture is read: the cell centre or the pixel itself.
    let wc = id_cell_world(cell, cs, shift);
    let sp = mix(p, wc, f_block());
    var s = p_src(sp);
    if (f_highlight() > 0.0) {
        let d = cs * 1.6;
        let bl = (p_src(sp + vec2<f32>(d, 0.0)).rgb + p_src(sp - vec2<f32>(d, 0.0)).rgb
            + p_src(sp + vec2<f32>(0.0, d)).rgb + p_src(sp - vec2<f32>(0.0, d)).rgb) * 0.25;
        s = vec4<f32>(max(s.rgb + (s.rgb - bl) * f_highlight() * 2.0, vec3<f32>(0.0)), s.a);
    }
    var lv = id_prep(id_luma(s.rgb));

    // Error carried in from the left, upper and upper-left cells.
    let dk = select(f_diffusion(), 1.0, kind == 5);
    if (dk > 0.001) {
        let nl = id_prep(id_luma(p_src(id_cell_world(cell + vec2<f32>(-1.0, 0.0), cs, shift)).rgb));
        let nu = id_prep(id_luma(p_src(id_cell_world(cell + vec2<f32>(0.0, -1.0), cs, shift)).rgb));
        let nd = id_prep(id_luma(p_src(id_cell_world(cell + vec2<f32>(-1.0, -1.0), cs, shift)).rgb));
        let err = (nl - id_quant(nl, 0.5, n)) * 0.44 + (nu - id_quant(nu, 0.5, n)) * 0.31
            + (nd - id_quant(nd, 0.5, n)) * 0.19;
        lv = clamp(lv + err * dk * 1.6, 0.0, 1.0);
    }

    // Threshold: map, serpentine scan, travelling wave, flicker.
    var th = id_th(cell, kind, seed);
    if (f_serpentine() > 0.001) {
        let odd = (i32(cell.y) & 1) == 1;
        let mc = vec2<f32>(select(cell.x, -cell.x - 1.0, odd), cell.y);
        th = mix(th, id_th(mc, kind, seed), f_serpentine());
    }
    let dir = vec2<f32>(0.8, 0.6);
    th = th + f_wave() * 0.5 * sin(dot(cell, dir) * f_wave_freq() * 0.25 - t * f_wave_speed() * 2.0);
    let fl = floor(t * f_noise_speed());
    th = th + (p_hash(cell + vec2<f32>(fl * 7.0, seed + 1.3)) - 0.5) * f_noise();
    th = clamp(th, 0.0, 1.0);

    // Levels per channel; cromatico spreads the threshold across r, g, b.
    let sh = f_chromatic() * 0.3333;
    let t3 = vec3<f32>(th, fract(th + sh), fract(th + 2.0 * sh));
    let lq = vec3<f32>(id_quant(lv, t3.x, n), id_quant(lv, t3.y, n), id_quant(lv, t3.z, n));

    // Palette path: `tinta` ramp, optionally swapped for `acento` per cell.
    let pick = step(p_hash(cell + vec2<f32>(seed, 21.0)), f_tint());
    let pr = mix(p_pal(0u, lq.x), p_pal(1u, lq.x), pick).x;
    let pg = mix(p_pal(0u, lq.y), p_pal(1u, lq.y), pick).y;
    let pb = mix(p_pal(0u, lq.z), p_pal(1u, lq.z), pick).z;
    let pal = vec3<f32>(pr, pg, pb);

    // Own-colour path: each channel of the (prepared) picture quantised.
    let cg = vec3<f32>(id_prep(sqrt(clamp(s.r, 0.0, 1.0))), id_prep(sqrt(clamp(s.g, 0.0, 1.0))),
        id_prep(sqrt(clamp(s.b, 0.0, 1.0))));
    let oq = vec3<f32>(id_quant(cg.x, t3.x, n), id_quant(cg.y, t3.y, n), id_quant(cg.z, t3.z, n));
    var col = mix(pal, oq * oq, f_colors_origin());

    // Gaps between cells.
    let ed = max(abs(cu.x - 0.5), abs(cu.y - 0.5));
    col = col * (1.0 - smoothstep(0.5 - f_grid() * 0.45, 0.5, ed) * min(f_grid() * 1.5, 1.0));

    // vacio: the darker levels let the background through.
    let on = clamp((lq.x + lq.y + lq.z) * 0.3333 * 1.5, 0.0, 1.0);
    let a = s.a * mix(1.0, on, f_empty());
    return vec4<f32>(col, a);
}
