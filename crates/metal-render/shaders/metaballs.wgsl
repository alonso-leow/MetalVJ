// metaballs: soft blobs orbiting a centre and merging like liquid.
//;
// everything here is original.
//
// Each ball i gets stable random numbers from its index and `semilla`: orbit
// speed, radius, ellipse squash and tilt, phase, size. The field is the sum of
// a power-law kernel (size^2 / distance^2)^e per ball; `viscosidad` lowers the
// exponent so the tails are long and blobs merge early. The surface is where
// the field crosses a threshold set by `umbral`. Ball colours are blended by
// their share of the field, so merging blobs mix colour. Slot 0 colours the
// balls, slot 1 the core and rim, slot 2 the outer glow.

fn mb_rand(i: f32, k: f32, seed: f32) -> f32 {
    return p_hash(vec2<f32>(i * 7.0 + k * 131.0, seed * 17.0 + k));
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let n = clamp(i32(floor(f_count() + 0.5)), 1, 16);
    let seed = floor(f_seed() + 0.5);
    let ex = mix(2.2, 0.8, clamp(f_viscosity(), 0.0, 1.0));
    let thr = exp(mix(-1.2, 1.2, clamp(f_threshold(), 0.0, 1.0)));
    let centre = vec2<f32>(f_center_x(), f_center_y());
    var q = p - centre;
    // Jelly: the sampling point wobbles, so the outline ripples.
    let g = f_gelatin() * 0.05;
    q = q + g * vec2<f32>(sin(q.y * 9.0 + t * 2.1), cos(q.x * 9.0 - t * 1.7));

    var field = 0.0;
    var grad = vec2<f32>(0.0);
    var acc = vec3<f32>(0.0);
    for (var i = 0; i < 16; i = i + 1) {
        if (i >= n) { break; }
        let fi = f32(i);
        let r0 = mb_rand(fi, 0.0, seed);
        let r1 = mb_rand(fi, 1.0, seed);
        let r2 = mb_rand(fi, 2.0, seed);
        let r3 = mb_rand(fi, 3.0, seed);
        let r4 = mb_rand(fi, 4.0, seed);
        let r5 = mb_rand(fi, 5.0, seed);
        let w = (0.35 + 0.9 * r0) * f_speed();
        let rad = f_orbit_r() * 0.55 * mix(1.0, 0.25 + 0.95 * r1, f_orbit_variation());
        let squash = mix(1.0, 0.3 + 0.7 * r2, f_eccentricity());
        let ph = r3 * TAU + t * w;
        var o = vec2<f32>(cos(ph) * rad, sin(ph) * rad * squash);
        o = p_rot(o, r4 * PI);
        // Second, faster wobble on top of the orbit.
        o = o + f_chaos() * 0.18 * vec2<f32>(sin(t * w * 2.3 + r5 * TAU), cos(t * w * 1.7 + r0 * TAU));
        // Slow breathing toward / away from the centre.
        o = o * (1.0 - 0.85 * f_attraction() * (0.5 + 0.5 * sin(t * 0.5 * f_speed() + r1 * TAU)));
        let s = f_size() * 0.4 * mix(1.0, 0.35 + 0.95 * r5, f_size_variation())
            * (1.0 + f_pulse_size() * 0.45 * sin(t * 2.0 + r2 * TAU));
        let d = q - o;
        let d2 = dot(d, d) + 1e-4;
        let x = max(s * s, 1e-5) / d2;
        let k = pow(x, ex);
        field = field + k;
        grad = grad - 2.0 * ex * k * d / d2;
        let ct = fract(fi / f32(n) * f_color_ball() + r0 * 0.15 + f_color_drift() * t * 0.1);
        acc = acc + k * p_pal(0u, ct);
    }

    let soft = thr * mix(0.03, 0.5, clamp(f_softness(), 0.0, 1.0));
    let cov = smoothstep(thr - soft, thr + soft, field);
    let depth = clamp((field - thr) / (thr * 3.0), 0.0, 1.0);
    var col = acc / max(field, 1e-4);

    // Inner glow toward the core colour.
    col = mix(col, p_pal(1u, depth), f_shine_core() * depth);
    // Bright rim just inside the edge.
    let rim = cov * (1.0 - smoothstep(thr, thr * 1.9, field));
    col = col + p_pal(1u, 1.0) * rim * f_edge() * 0.8;
    // Fake lighting: a height field from the sum, lit from a direction.
    let nrm = normalize(vec3<f32>(-grad / (field + thr) * 0.35, 1.0));
    let la = f_light_direction() * TAU;
    let ld = normalize(vec3<f32>(cos(la) * 0.6, sin(la) * 0.6, 0.7));
    let lam = clamp(dot(nrm, ld), 0.0, 1.0);
    col = col * mix(1.0, 0.35 + 1.0 * lam, f_shading());
    col = col + vec3<f32>(pow(lam, 24.0)) * f_shading() * 0.35 * cov;

    // Outside glow.
    let gl = pow(clamp(field / thr, 0.0, 1.0), 2.0) * (1.0 - cov) * f_halo();
    let halo_c = p_pal(2u, clamp(field / thr, 0.0, 1.0));
    let a = max(cov, gl);
    let rgb = (col * cov + halo_c * gl) / max(cov + gl, 1e-4);
    return vec4<f32>(rgb, a);
}
