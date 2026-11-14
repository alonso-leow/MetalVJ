// graingradient: a colour gradient driven by a scalar field and roughened
// by film grain; everything here is original.
//
// A field f in 0..1 is built from one of seven shapes (wave, dots, truchet,
// corners, ripple, blob, sphere) after scale / rotation / offset / a sinusoidal
// domain distortion. A focal radial gradient r (radius, focal offset + angle,
// fall-off exponent) can be blended in. The result is contrasted, softened,
// banded, grain-dithered, optionally outlined, quantised against `mezcla`, then
// mapped through slot 0 blended with slot 1; slot 2 tints the sparkle grain.

fn gg_field(q: vec2<f32>, kind: i32, t: f32) -> f32 {
    if (kind == 0) {
        // Layered travelling waves.
        let a = sin(q.x * 3.0 + sin(q.y * 2.3 + t * 0.7) * 1.4 + t);
        let b = sin(q.y * 2.0 - q.x * 1.1 - t * 0.6);
        return 0.5 + 0.28 * a + 0.22 * b;
    }
    if (kind == 1) {
        // Dot lattice: soft distance to the nearest cell centre, with drift.
        let g = q * 2.2 + vec2<f32>(t * 0.15, 0.0);
        let c = fract(g) - 0.5;
        let h = p_hash(floor(g));
        return clamp(1.0 - length(c) * (1.6 + h), 0.0, 1.0);
    }
    if (kind == 2) {
        // Truchet arcs: two quarter circles per cell, flipped by a hash.
        let g = q * 2.0;
        let id = floor(g);
        var c = fract(g);
        if (p_hash(id) > 0.5) { c.x = 1.0 - c.x; }
        let d1 = abs(length(c) - 0.5);
        let d2 = abs(length(c - 1.0) - 0.5);
        return clamp(1.0 - min(d1, d2) * 3.2, 0.0, 1.0);
    }
    if (kind == 3) {
        // Corners: four hash-weighted corner glows that breathe.
        var s = 0.0;
        for (var i = 0; i < 4; i = i + 1) {
            let fx = f32(i & 1) * 2.0 - 1.0;
            let fy = f32(i >> 1) * 2.0 - 1.0;
            let w = 0.6 + 0.4 * sin(t * 0.5 + f32(i) * 1.7);
            s = s + w * exp(-length(q - vec2<f32>(fx, fy) * 0.9) * 1.3);
        }
        return clamp(s * 0.8, 0.0, 1.0);
    }
    if (kind == 4) {
        // Ripples from a drifting source.
        let c = vec2<f32>(sin(t * 0.3) * 0.3, cos(t * 0.23) * 0.3);
        return 0.5 + 0.5 * sin(length(q - c) * 9.0 - t * 2.0);
    }
    if (kind == 5) {
        // Metaball blob field.
        var s = 0.0;
        for (var i = 0; i < 4; i = i + 1) {
            let fi = f32(i);
            let c = vec2<f32>(sin(t * (0.4 + 0.13 * fi) + fi * 2.1), cos(t * (0.33 + 0.11 * fi) + fi * 1.3)) * 0.7;
            let d = length(q - c);
            s = s + 0.18 / (d * d + 0.12);
        }
        return clamp(s * 0.5, 0.0, 1.0);
    }
    // Sphere: height of a lit ball with a moving light.
    let r2 = dot(q, q);
    let z = sqrt(max(1.0 - r2 * 0.7, 0.0));
    let l = normalize(vec3<f32>(cos(t * 0.5) * 0.6, sin(t * 0.5) * 0.6, 0.8));
    let n = normalize(vec3<f32>(q * 0.84, z));
    return clamp(dot(n, l) * 0.9 + 0.1, 0.0, 1.0) * select(0.15, 1.0, r2 < 1.43);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time() * f_speed();
    var q = p_rot(p, f_rotation() * TAU) / max(f_scale(), 0.05) + vec2<f32>(f_off_x(), f_off_y());

    // Sinusoidal domain distortion (shared by the shape field and the radial).
    let df = f_distance_freq();
    let dsh = f_distance_phase() * TAU + t * 0.5;
    q = q + f_distortion() * 0.12 * vec2<f32>(sin(q.y * df * 0.35 + dsh), cos(q.x * df * 0.35 - dsh));

    let kind = clamp(i32(floor(f_shape() + 0.5)), 0, 6);
    var v = gg_field(q, kind, t);

    // Focal radial gradient (the folded-in static variant).
    let fa = f_focus_angle() * TAU;
    let focus = vec2<f32>(cos(fa), sin(fa)) * f_focus_distance() * f_radius() * 0.9;
    let rr = length(q - focus) / max(f_radius(), 0.05);
    let rad = 1.0 - pow(clamp(rr, 0.0, 4.0), max(f_fall(), 0.05) * 2.0) * 0.5;
    v = mix(v, clamp(rad, 0.0, 1.0), f_radial());

    // Contrast, softness.
    v = clamp(0.5 + (v - 0.5) * (0.35 + f_intensity() * 1.5), 0.0, 1.0);
    v = mix(smoothstep(0.35, 0.65, v), v, f_softness());

    // Grain dither.
    let gs = max(f_grain_size(), 0.1);
    let n = p_hash(floor(uv * vec2<f32>(P.a.x, 1.0) * 700.0 / gs) + vec2<f32>(floor(p_time() * 12.0) * 13.0, 5.0));
    v = v + (n - 0.5) * f_noise() * (1.2 - f_softness() * 0.6);

    // Posterise and outline.
    let nb = mix(64.0, 3.0, f_bands());
    v = mix(v, floor(clamp(v, 0.0, 0.999) * nb + 0.5) / nb, f_bands());
    let iso = abs(fract(v * 6.0) - 0.5);
    v = v - f_outline() * 0.25 * (1.0 - smoothstep(0.0, 0.08, iso));

    // Quantise against `mezcla` (stepped across the four slot colours -> smooth).
    let st = (floor(clamp(v, 0.0, 0.999) * 4.0) + 0.5) / 4.0;
    let u = fract(mix(st, v, f_mix()) + f_drift_color() * p_time() * 0.1);
    let uc = clamp(u, 0.0, 1.0);

    var col = p_pal2(0u, uc, 1u, uc, f_palette_mix());
    // Sparkle grain tinted by slot 2.
    let sp = step(0.93, p_hash(floor(uv * 500.0 / gs) + vec2<f32>(floor(p_time() * 8.0), 9.0)));
    col = mix(col, p_pal(2u, uc), clamp(sp * f_shine_grain() * 2.0, 0.0, 1.0));
    return vec4<f32>(col, clamp(0.3 + 1.1 * uc, 0.0, 1.0));
}
