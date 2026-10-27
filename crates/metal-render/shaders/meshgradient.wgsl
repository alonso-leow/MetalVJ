// meshgradient: a mesh of colour points blended by inverse-distance
// weights, with a swirl and an iterated sine distortion of the plane. Behaviour
//; the code is original.
//
// Up to 10 points. Point colours come from slot 0 blended with slot 1 along the
// point index; slot 2 tints the seams between points and the surface grain.

const MG_MAX: i32 = 10;

fn mg_point(i: i32, n: i32, t: f32) -> vec2<f32> {
    let fi = f32(i);
    let h = p_hash2(vec2<f32>(fi, 7.0));
    let hv = p_hash2(vec2<f32>(fi, 19.0));
    let asp = p_aspect();
    // Free orbit around a hashed home position.
    let home = (h - 0.5) * vec2<f32>(asp * 0.9, 0.9) * f_radius_dot();
    let ph = hv * TAU;
    let sp = 0.5 + hv.x * 0.9;
    let orb = f_orbit() * 0.45 * vec2<f32>(sin(t * sp + ph.x), cos(t * (sp * 0.83) + ph.y));
    let free_pos = home + orb;
    // Static layout: points on a Lissajous of two sine waves.
    let s = (fi + 0.5) / f32(n);
    let wx = sin(f_waves_x() * s * TAU + f_phase_x() * TAU + t * 0.3 * f_static_speed());
    let wy = sin(f_waves_y() * s * TAU * 1.0 + f_phase_y() * TAU + t * 0.27 * f_static_speed());
    let stat_pos = vec2<f32>(wx * asp * 0.42, wy * 0.42) * f_radius_dot();
    return mix(free_pos, stat_pos, f_static());
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time() * f_speed();
    var q = p_rot(p, f_rotation() * TAU) / max(f_scale(), 0.05) + vec2<f32>(f_off_x(), f_off_y());

    // Marbling veins: an fbm displacement before everything else.
    if (f_vein() > 0.0) {
        let vn = vec2<f32>(p_fbm(q * 2.2 + vec2<f32>(t * 0.1, 0.0), 3), p_fbm(q * 2.2 + vec2<f32>(5.0, t * 0.12), 3));
        q = q + (vn - 0.5) * f_vein() * 0.9;
    }
    // Iterated sine distortion.
    let distance = f_distortion();
    for (var k = 0; k < 3; k = k + 1) {
        let fk = f32(k + 1);
        q = q + distance * 0.16 / fk * vec2<f32>(
            sin(q.y * (1.8 + fk * f_turbulence() * 2.0) * fk + t * (0.5 + 0.2 * fk)),
            cos(q.x * (1.6 + fk * f_turbulence() * 2.0) * fk - t * (0.4 + 0.25 * fk)));
    }
    // Swirl: angle falls off with the distance from the centre.
    let r = length(q);
    let sw = f_turn_swirl() * 4.0 * exp(-r * f_swirl_fall()) + f_fold() * sin(r * 6.0 - t);
    q = p_rot(q, sw);

    let n = clamp(i32(floor(f_dots() + 0.5)), 2, MG_MAX);
    let power = 1.0 + f_sharpness() * 4.0;
    let eps = mix(0.002, 0.12, f_mix());
    let gs = max(f_grain_size(), 0.1);
    let gn = p_hash(floor(uv * vec2<f32>(P.a.x, 1.0) * 700.0 / gs) + vec2<f32>(floor(p_time() * 12.0) * 13.0, 5.0));

    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    var wmax = 0.0;
    let dr = f_drift_color() * p_time() * 0.1;
    for (var i = 0; i < MG_MAX; i = i + 1) {
        if (i >= n) { break; }
        let c = mg_point(i, n, t);
        let d = q - c;
        // Grain mixer: jitters the distances so seams dissolve into noise.
        let d2 = dot(d, d) * (1.0 + (gn - 0.5) * f_grain_mix() * 2.0);
        let w = pow(1.0 / (max(d2, 0.0) + eps), power);
        let u = fract((f32(i) + 0.5) / f32(n) + dr);
        let pc = p_pal2(0u, u, 1u, u, f_palette_mix());
        acc = acc + pc * w;
        wsum = wsum + w;
        wmax = max(wmax, w);
    }
    var col = acc / max(wsum, 1e-9);

    // Seams between points glow in slot 2.
    let seam = 1.0 - wmax / max(wsum, 1e-9);
    col = mix(col, p_pal(2u, clamp(seam, 0.0, 1.0)), clamp(seam * seam * f_edges() * 2.0, 0.0, 1.0));

    // Posterise and iso-lines on luminance.
    var lum = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
    if (f_bands() > 0.0) {
        let nb = mix(48.0, 3.0, f_bands());
        let ql = floor(clamp(lum, 0.0, 0.999) * nb + 0.5) / nb;
        col = col * mix(1.0, ql / max(lum, 0.01), f_bands());
        lum = ql;
    }
    let iso = abs(fract(lum * 7.0) - 0.5);
    col = col * (1.0 - f_outline() * 0.4 * (1.0 - smoothstep(0.0, 0.07, iso)));

    // Surface grain overlay: signed film grain, sparkle tinted by slot 2.
    col = col + (gn - 0.5) * f_grain_surface() * 0.6;
    let sp = step(0.95, p_hash(floor(uv * 480.0 / gs) + vec2<f32>(floor(p_time() * 8.0), 9.0)));
    col = mix(col, p_pal(2u, fract(lum + dr)), clamp(sp * f_shine_grain() * 2.0, 0.0, 1.0));

    // Soft centre light.
    col = col * (1.0 + f_light() * (0.6 - r * 0.8));
    // Vignette: the picture fades into the background (uncovered edge).
    let vr = length((uv - 0.5) * vec2<f32>(p_aspect(), 1.0));
    let cov = 1.0 - f_vignette() * smoothstep(0.25, 0.85, vr);
    return vec4<f32>(max(col, vec3<f32>(0.0)), cov);
}
