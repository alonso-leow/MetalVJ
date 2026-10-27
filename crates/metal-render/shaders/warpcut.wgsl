// warpcut: a periodic pattern bent by an iterated swirl and a noise
// distortion, then cut into four colours. Written from first principles;
//
// The picture coordinate is scaled / rotated / stretched / folded, displaced by
// a value-noise distortion, then by an iterated sinusoidal swirl (iteration i
// has amplitude 1/i and frequency growing with i, so low iterations make broad
// folds and high ones fine marbling). A pattern field v in 0..1 (checks,
// stripes, a single edge, rings, triangular lattice) is read from the bent
// coordinate. `proporcion` shifts which colours dominate, `suavidad` sets the
// transition width; the field is optionally posterised, outlined, lit as
// relief, grain-dithered and mapped through slot 0 blended with slot 1.

fn wp_pattern(q: vec2<f32>, kind: i32, k: f32, ang: f32, t: f32) -> f32 {
    let r = p_rot(q, ang * TAU);
    if (kind == 0) {
        return 0.5 + 0.5 * sin(r.x * k) * sin(r.y * k);
    }
    if (kind == 1) {
        return 0.5 + 0.5 * sin(r.x * k + 0.3 * sin(r.y * k * 0.2 + t));
    }
    if (kind == 2) {
        return clamp(0.5 + r.x * k * 0.16, 0.0, 1.0);
    }
    if (kind == 3) {
        return 0.5 + 0.5 * sin(length(r) * k - t * 0.5);
    }
    // Triangular lattice: three sines at 120 degrees.
    let a = sin(r.x * k);
    let b = sin(dot(r, vec2<f32>(-0.5, 0.866)) * k);
    let c = sin(dot(r, vec2<f32>(-0.5, -0.866)) * k);
    return clamp(0.5 + (a + b + c) * 0.19, 0.0, 1.0);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let tt = p_time();
    let t = tt * f_speed();
    var q = p_rot(p, f_rotation() * TAU) / max(f_scale(), 0.05) + vec2<f32>(f_off_x(), f_off_y());
    q = q * vec2<f32>(exp(f_asymmetry()), exp(-f_asymmetry()));
    q = mix(q, vec2<f32>(abs(q.x), q.y), f_fold());

    // Noise distortion.
    let df = f_distance_freq();
    let nz = vec2<f32>(p_noise(q * df * 0.5 + vec2<f32>(t * 0.3, 0.0)), p_noise(q * df * 0.5 + vec2<f32>(7.1, t * 0.27)));
    q = q + (nz - 0.5) * 0.7 * f_distortion();

    // Iterated swirl.
    let n = clamp(i32(floor(f_iter_swirl() + 0.5)), 0, 20);
    let sw = f_swirl();
    let st = tt * f_speed_swirl() + f_swirl_phase() * TAU;
    let cx = f_crossing();
    for (var i = 1; i <= 20; i = i + 1) {
        if (i > n) { break; }
        let fi = f32(i);
        let a = sw / fi;
        let ph = st + fi * 0.8;
        let nx = q.x + a * cos(fi * (q.y * 1.3 + q.x * 0.3 * cx) + ph);
        let ny = q.y + a * cos(fi * (q.x * 1.1 + q.y * 0.3 * cx) + ph * 1.2 + 1.3);
        q = vec2<f32>(nx, ny);
    }

    let kind = clamp(i32(floor(f_shape() + 0.5)), 0, 4);
    let k = 1.5 / max(f_shape_scale(), 0.01);
    var v = wp_pattern(q, kind, k, f_angle_shape(), t);

    // Relief shading from the screen-space gradient of the field.
    let gx = dpdx(v);
    let gy = dpdy(v);
    let lit = clamp(0.5 + (gx * 0.7 + gy * 0.7) * 40.0, 0.0, 1.0);

    // Grain, posterise.
    let n2 = p_hash(floor(uv * vec2<f32>(P.a.x, 1.0) * 600.0) + vec2<f32>(floor(tt * 12.0) * 13.0, 5.0));
    v = clamp(v + (n2 - 0.5) * f_grain() * 0.5, 0.0, 1.0);
    let nb = mix(64.0, 3.0, f_bands());
    v = mix(v, floor(clamp(v, 0.0, 0.999) * nb + 0.5) / nb, f_bands());

    // Four-colour cut with proportion and softness.
    let x = clamp(v * 3.0 + (f_proportion() - 0.5) * 3.0, 0.0, 2.999);
    let w = mix(0.02, 0.5, f_softness());
    let kf = floor(x);
    let ff = smoothstep(0.5 - w, 0.5 + w, x - kf);
    let pos = clamp((kf + ff) / 3.0 + f_drift_color() * tt * 0.1, -8.0, 8.0);
    let u = clamp(abs(fract(pos * 0.5) * 2.0 - 1.0), 0.0, 1.0);
    let upos = select(clamp(pos, 0.0, 1.0), u, abs(f_drift_color()) > 0.001);
    var col = p_pal2(0u, upos, 1u, upos, f_palette_mix());

    // Outline on the colour boundaries, relief highlight, both tinted by slot 2.
    let edge = abs(fract(x) - 0.5);
    let line = (1.0 - smoothstep(0.0, 0.06 + 0.2 * w, edge)) * f_outline();
    col = mix(col, p_pal(2u, upos), clamp(line, 0.0, 1.0));
    col = col * (1.0 + (lit - 0.5) * f_relief() * 1.6);

    let cov = mix(1.0, smoothstep(0.0, 0.6, v + 0.2), f_empty());
    return vec4<f32>(col, cov);
}
