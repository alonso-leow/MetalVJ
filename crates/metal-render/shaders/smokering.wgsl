// smokering: a ring of turbulent smoke around an empty centre.
//;
// everything here is original.
//
// The picture is an annulus in polar terms: centre radius `radio`, band width
// `grosor`, inner edge shaped by `forma_in` (0 = crisp, 1 = long soft fade
// into the hollow). The ring may be stretched (`elipse`), lobed (`lobulos`,
// `ondulacion`), uneven (`asimetria`), breathing (`latido`) and expanding
// (`expansion`). Smoke is a domain-warped, swirling fbm with `iteraciones`
// octaves of lacunarity `lacunaridad`, scaled by `ruido_escala`, drifting at
// `vel`, curling with `remolino` and stretched along the ring by `filamentos`.
// Density erodes at `desgaste`. Slot 0 inks the smoke, slot 1 tints the inner
// core, slot 2 colours the thin bright thread (`hilo`) and the outer halo.

fn sr_fbm(p0: vec2<f32>, oct: i32, lac: f32) -> f32 {
    var p = p0;
    var tot = 0.0;
    var amp = 0.5;
    var norm = 0.0;
    for (var o = 0; o < 8; o = o + 1) {
        if (o >= oct) { break; }
        tot = tot + p_noise(p) * amp;
        norm = norm + amp;
        amp = amp * 0.5;
        p = p_rot(p, 0.9) * lac + vec2<f32>(3.1, 1.7);
    }
    return tot / max(norm, 1e-3);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let seed = f_seed();
    var q = p_rot(p, f_angle() * TAU);
    q.x = q.x / f_ellipse();
    let rr = length(q);
    let a = atan2(q.y, q.x);

    // Ring geometry.
    let breath = 1.0 + f_beat() * 0.15 * sin(t * 2.5);
    var big = (0.08 + f_radius() * 1.1) * breath;
    big = big * (1.0 + f_expansion() * 0.25 * sin(t * 0.8 - rr * 6.0));
    let lob = max(floor(f_lobes() + 0.5), 1.0);
    big = big * (1.0 + f_ripple() * 0.18 * sin(a * lob + t * f_speed() * 0.7));
    var w = 0.03 + f_thickness() * 0.30;
    w = w * (1.0 + f_asymmetry() * 0.7 * sin(a + t * 0.3 + seed));
    w = max(w, 0.01);
    let s = (rr - big) / w; // -1 inner edge .. +1 outer edge

    // Smoke field: swirl by radius, flow along the ring, warp, fbm.
    let oct = clamp(i32(floor(f_iterations() + 0.5)), 1, 8);
    let lac = f_lacunarity();
    let sc = f_noise_scale();
    var n = q * sc;
    n = p_rot(n, f_swirl() * (1.2 - min(rr * 1.8, 1.2)) + t * f_turn_smoke() * 0.3);
    // Stretch the noise along the tangent: sample angle stretched.
    let tang = vec2<f32>(-sin(a), cos(a));
    n = n + tang * dot(n, tang) * f_filaments() * 1.2;
    n = n + vec2<f32>(seed * 3.7, seed * 1.3);
    n = n + vec2<f32>(0.0, t * f_speed() * 0.5) + q / max(rr, 1e-3) * (-t * f_expansion() * 0.6);
    let wo = min(oct, 4);
    let wv = vec2<f32>(sr_fbm(n * 0.8 + vec2<f32>(0.0, t * 0.12), wo, lac), sr_fbm(n * 0.8 + vec2<f32>(5.2, 1.3 - t * 0.12), wo, lac));
    let f = sr_fbm(n + (wv - 0.5) * 2.2, oct, lac);
    let ink = clamp((f - 0.5) * f_contrast_smoke() + 0.5, 0.0, 1.0);

    // Band profile with soft inner shape.
    let inner_w = 0.05 + f_shape_in() * 0.95;
    let inner = smoothstep(-1.0, -1.0 + 2.0 * inner_w, s);
    let outer = 1.0 - smoothstep(0.0, 1.0, s);
    var band = inner * outer;
    band = pow(band, 0.7 + (1.0 - f_shape_in()) * 0.8);
    var dens = band * (0.25 + 1.1 * ink) - f_wear() * (1.0 - ink) * 0.8;
    dens = clamp(dens, 0.0, 1.0);

    // Colour.
    let cyc = ink * 0.8 + band * 0.2 + t * f_color_speed() * 0.1;
    let tri = 1.0 - abs(fract(cyc) * 2.0 - 1.0);
    let ab = f_aberration() * 0.15;
    var col = vec3<f32>(
        p_pal(0u, clamp(tri + ab, 0.0, 1.0)).r,
        p_pal(0u, tri).g,
        p_pal(0u, clamp(tri - ab, 0.0, 1.0)).b);
    // Core tint hugging the inner edge.
    let core = (1.0 - smoothstep(-1.0, 0.2, s)) * band;
    col = mix(col, p_pal(1u, ink), clamp(core * f_shine_core(), 0.0, 1.0));
    col = col * (0.55 + 0.9 * ink);

    // Thin bright thread on the centre line.
    let hw = max(f_thread_width(), 0.002);
    let thread = exp(-pow((rr - big) / hw, 2.0)) * (0.4 + 0.8 * ink);
    let glowc = p_pal(2u, 0.5);
    col = col + glowc * thread * f_thread() * 1.5;

    // Outer halo.
    let hr = 0.02 + f_halo_radius() * 0.5;
    let out_d = max(rr - big - w, 0.0);
    let halo = exp(-out_d / hr) * f_halo() * (0.3 + 0.7 * ink) * (1.0 - outer * 0.0);
    let halo_a = select(0.0, halo, s > 0.0) * (1.0 - dens);
    col = mix(col, glowc, clamp(halo_a * 0.7, 0.0, 1.0));

    let cov = clamp(dens + halo_a + thread * f_thread() * 0.6, 0.0, 1.0);
    return vec4<f32>(col, cov);
}
