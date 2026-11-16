// godrays: light shafts fanning out of a point, with a soft core glow and
// a coloured bloom halo; everything here is original.
//
// Around the origin the picture is a function of angle and radius. The angle is
// optionally bent by distance (curvatura, a spiral) and by a slow sway (vaiven),
// optionally snapped to discrete beams (pasillo), then looked up in fractal
// noise sampled on a circle (so there is no seam). A threshold set by densidad
// turns the noise into shafts, anchura softens their edges. Along the shaft the
// light breaks into pulses (moteado), fine streaks (estrias) and drifting dust
// (polvo), and fades with distance (largo). The three colour channels sample
// the shaft pattern at slightly different angles (aberracion). A cone (abanico,
// dir) limits the fan. Core glow uses slot 2, bloom halo slot 1, shafts slot 0.

fn ray_field(ang: f32, r: f32, t: f32, off: f32) -> f32 {
    let n = max(f_rays(), 1.0);
    var a = ang + off;
    a = a + f_curvature() * r * 2.5;
    a = a + f_swing() * 0.35 * sin(t * 0.7 + r * 3.0);
    let step_a = TAU / floor(n + 0.5);
    let aq = floor(a / step_a + 0.5) * step_a;
    a = mix(a, aq, f_corridor());
    let dir = vec2<f32>(cos(a), sin(a));
    let drift = t * f_speed() * 0.25;
    let base = p_fbm(dir * n * 0.6 + vec2<f32>(drift, -drift * 0.6), 3);
    let thr = mix(0.78, 0.30, clamp(f_density(), 0.0, 1.0));
    let soft = mix(0.015, 0.35, clamp(f_width(), 0.0, 1.0));
    var v = smoothstep(thr - soft, thr + soft, base);

    // Pulses travelling outwards along each shaft.
    let pul = p_noise(dir * n * 0.45 + vec2<f32>(r * 7.0 - t * f_speed() * 1.4, 13.0));
    v = v * mix(1.0, smoothstep(0.2, 0.8, pul), f_speckle());
    // Fine streaks along the shaft.
    let str = p_noise(dir * n * 2.2 + vec2<f32>(r * 24.0 - t * f_speed() * 2.5, 41.0));
    v = v * (1.0 + f_streaks() * (str - 0.5) * 2.2);
    // Per-shaft flicker.
    let cell = floor(a / TAU * n * 3.0);
    let fl = p_hash(vec2<f32>(cell, floor(t * (4.0 + 6.0 * f_speed()))));
    v = v * (1.0 - f_blink() * fl);
    return max(v, 0.0);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let q = p - vec2<f32>(f_origin_x(), f_origin_y());
    let r = length(q);
    let ang = atan2(q.y, q.x);

    // Cone mask.
    var d = ang - f_direction() * TAU;
    d = d - TAU * floor(d / TAU + 0.5);
    let half_w = clamp(f_fan(), 0.02, 1.0) * PI;
    let cone = 1.0 - smoothstep(half_w * 0.75, half_w, abs(d));

    // Fall-off along the shafts; also fades right at the source.
    let reach = 0.08 + f_long() * 1.8;
    let fall = exp(-r / reach) * smoothstep(0.0, 0.05 + 0.1 * f_mix_color(), r);

    // Shafts, three angular taps for the colour split.
    let ab = f_aberration() * 0.08;
    let vr = ray_field(ang, r, t, -ab);
    let vg = ray_field(ang, r, t, 0.0);
    let vb = ray_field(ang, r, t, ab);
    let tcol = clamp(r * 0.9 * f_mix_color() + vg * (1.0 - 0.6 * f_mix_color()), 0.0, 1.0);
    let c0 = p_pal(0u, tcol);
    let shafts0 = c0 * vec3<f32>(vr, vg, vb);
    let shafts = shafts0 * fall * cone;

    // Dust motes lit by the shafts.
    var dust = 0.0;
    if (f_dust() > 0.001) {
        let g = 38.0;
        let s = p * g + vec2<f32>(0.0, t * 1.5);
        let cl = floor(s);
        let h = p_hash2(cl);
        let c = (fract(s) - 0.5) - (h - 0.5) * 0.6;
        let tw = 0.5 + 0.5 * sin(t * 3.0 + h.x * TAU);
        let m = (1.0 - smoothstep(0.03, 0.12, length(c))) * tw * step(0.55, h.y);
        dust = m * f_dust() * (0.15 + 3.0 * vg) * cone * exp(-r / (reach * 2.0));
    }

    // Core glow (slot 2) and bloom halo (slot 1).
    let cr = max(f_medium_size(), 0.005) * 0.6;
    let core = exp(-(r * r) / (cr * cr)) * f_medium_intensity();
    let hr = 0.04 + f_halo_radius() * 0.9;
    let pulse = 1.0 + f_halo_pulse() * 0.5 * sin(t * 2.6 + r * 4.0);
    let halo = exp(-r / hr) * f_bloom() * pulse;
    let glow = p_pal(2u, clamp(r / (cr * 3.0), 0.0, 1.0)) * core
        + p_pal(1u, clamp(r / (hr * 3.0), 0.0, 1.0)) * halo;

    var col = (shafts * f_intensity() * 1.6 + glow) + p_pal(0u, 0.9) * dust;
    // Soft tonemap so strong intensidad saturates instead of clipping hard.
    col = vec3<f32>(1.0) - exp(-col * 1.4);
    let cov = clamp(max(col.r, max(col.g, col.b)), 0.0, 1.0);
    return vec4<f32>(col / max(cov, 1e-4) * cov, cov);
}
