// water: a picture seen through a rippling, caustic water surface.
//; the
// maths here is original.
//
// A height field H(q) is built from up to six directional wave layers
// (`capas` fades them in, each layer higher in frequency and lower in
// amplitude, headings fanned by `dir_var`), a domain warp (`turbulencia`), a
// slow swell (`marea`), fine noise ripples (`ondulas`) and rain rings
// (`gotas`). Central differences give the surface slope g and its Laplacian.
// The slope bends where the picture is read (refraction, optionally split per
// channel), the Laplacian gives light focusing (caustics), crests give
// highlights and foam, the slope against a sun vector gives the specular glint.
// Slot `agua` tints the depth, slot `luz` is highlights, foam and glints.

fn wh(i: f32) -> f32 { return p_hash(vec2<f32>(i, 5.0 + f_seed() * 3.1)); }

// Height of the water at q (q-space units, ~1 unit per big wave).
fn water_h(q0: vec2<f32>, t: f32) -> f32 {
    var q = q0;
    let sp = f_speed();
    let warp = f_turbulence();
    if (warp > 0.0) {
        q = q + vec2<f32>(
            p_noise(q * 0.7 + vec2<f32>(t * 0.11 * sp, 3.0)),
            p_noise(q * 0.7 + vec2<f32>(9.0, t * 0.09 * sp))) * warp * 2.4 - vec2<f32>(warp * 1.2);
    }
    let base = f_direction() * TAU;
    let spread = f_direction_variation() * 2.6;
    var h = 0.0;
    var norm = 0.0;
    for (var i = 0; i < 6; i = i + 1) {
        let fi = f32(i);
        let w = clamp(f_layers() * 6.0 + 0.5 - fi, 0.0, 1.0);
        if (w <= 0.0) { break; }
        let a = base + (wh(fi) - 0.5) * spread + fi * 0.4 * f_direction_variation();
        let d = vec2<f32>(cos(a), sin(a));
        let k = 1.1 * pow(1.55, fi);
        let ph = dot(q, d) * k + t * sp * (0.8 + 0.35 * fi) * (0.6 + wh(fi + 20.0)) + wh(fi + 40.0) * TAU;
        let s = sin(ph);
        // pliegue sharpens the crests (trochoid-like) and flattens the troughs.
        let sharp = 1.0 - 2.0 * abs(s * 0.5 + 0.5 - 0.5 * s * s * s * s * 0.0);
        let crest = pow(1.0 - abs(s), 1.6) * 2.0 - 1.0;
        let wave = mix(s, crest, f_fold());
        let amp = pow(0.62, fi) * w;
        h = h + wave * amp;
        norm = norm + amp;
    }
    h = h / max(norm, 0.001);
    // Slow swell along the same heading.
    h = h + sin(dot(q, vec2<f32>(cos(base), sin(base))) * 0.35 + t * f_tide_speed()) * f_tide() * 1.2;
    // Fine ripples.
    if (f_ripples() > 0.0) {
        let rq = q * f_ripples_freq() * 0.25;
        h = h + (p_noise(rq + vec2<f32>(t * sp * 0.7, -t * sp * 0.4)) - 0.5) * f_ripples() * 1.6;
    }
    // Rain rings: one jittered ring per cell, expanding and fading.
    if (f_drops() > 0.0) {
        let c = q * 0.8;
        let id = floor(c);
        let f = fract(c) - vec2<f32>(0.5);
        let jit = (p_hash2(id) - vec2<f32>(0.5)) * 0.3;
        let ph = fract(t * f_drops_speed() * 0.35 + p_hash(id + vec2<f32>(3.0, 8.0)));
        let on = step(0.35, p_hash(id + vec2<f32>(7.0, 1.0)));
        let distance = length(f - jit);
        let r = ph * 0.42;
        let ring = sin((distance - r) * 38.0) * exp(-pow((distance - r) * 9.0, 2.0)) * (1.0 - ph);
        h = h + ring * on * f_drops() * 0.9;
    }
    return h;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let q = p * (3.0 / max(f_size(), 0.05));
    let e = 0.035;
    let h0 = water_h(q, t);
    let hx1 = water_h(q + vec2<f32>(e, 0.0), t);
    let hx0 = water_h(q - vec2<f32>(e, 0.0), t);
    let hy1 = water_h(q + vec2<f32>(0.0, e), t);
    let hy0 = water_h(q - vec2<f32>(0.0, e), t);
    let g = vec2<f32>(hx1 - hx0, hy1 - hy0) / (2.0 * e);
    let lap = (hx1 + hx0 + hy1 + hy0 - 4.0 * h0) / (e * e);

    // Refraction: the picture is read through the slope.
    let k = f_waves() * f_refract() * 0.045;
    let off = g * k;
    let cr = f_chromatic() * 1.4;
    let r = p_src(p + off * (1.0 + cr));
    let gg = p_src(p + off);
    let b = p_src(p + off * (1.0 - cr));
    var col = vec3<f32>(r.r, gg.g, b.b);
    let alpha = gg.a;

    // Depth tint from the `agua` slot: deeper where the surface is low.
    let hn = clamp(h0 * 0.35 + 0.5, 0.0, 1.0);
    let water = p_pal(0u, hn);
    col = mix(col, water * (0.35 + 0.9 * (1.0 - f_depth() * 0.5)), f_depth() * (0.55 + 0.45 * (1.0 - hn)));

    // Caustics: light gathers where the surface is concave (lap < 0).
    let focus = clamp(-lap * 0.045 - 0.1, 0.0, 1.0);
    let caus = focus * focus * (3.0 - 2.0 * focus);
    col = col + p_pal(1u, 0.15) * caus * f_caustic() * 3.0;
    col = col * (1.0 + (caus - 0.15) * f_caustic() * 0.8);

    // Highlights on the wave crests.
    let crestm = smoothstep(0.35, 0.95, hn);
    col = mix(col, p_pal(1u, 0.35), clamp(crestm * f_highlights() * 2.5, 0.0, 1.0));

    // Edges: slope lines (sharp refraction edges), shaded dark/light.
    let sl = length(g);
    let edge = smoothstep(0.6, 2.6, sl);
    col = mix(col, p_pal(1u, 0.7), edge * f_edges() * 0.45);
    col = col * (1.0 - edge * f_edges() * 0.18 * (1.0 - hn));

    // Sun glint: normal against a fixed sun vector.
    if (f_reflection() > 0.0) {
        let n = normalize(vec3<f32>(-g * 0.35, 1.0));
        let sun = normalize(vec3<f32>(0.35, -0.4, 0.85));
        let hv = normalize(sun + vec3<f32>(0.0, 0.0, 1.0));
        let spec = pow(max(dot(n, hv), 0.0), mix(24.0, 220.0, f_reflection_duration()));
        col = col + p_pal(1u, 0.0) * spec * f_reflection() * 2.0;
    }

    // Foam where crests break, broken up by noise.
    if (f_foam() > 0.0) {
        let fn1 = p_fbm(q * 3.0 + vec2<f32>(t * 0.25, 0.0), 3);
        let fm = smoothstep(f_foam_threshold(), f_foam_threshold() + 0.12, hn * 0.8 + fn1 * 0.35 + edge * 0.2);
        col = mix(col, p_pal(1u, 0.9), clamp(fm * f_foam(), 0.0, 1.0));
    }

    // Tiny sparkles riding the slopes.
    if (f_flashes() > 0.0) {
        let sp2 = floor(uv * vec2<f32>(320.0, 180.0) + vec2<f32>(floor(t * 8.0) * 3.0, 0.0));
        let tw = step(0.992 - 0.02 * f_flashes(), p_hash(sp2)) * smoothstep(0.4, 1.8, sl);
        col = col + p_pal(1u, 0.0) * tw * f_flashes() * 1.6;
    }

    let cov = alpha * (1.0 - f_clear());
    return vec4<f32>(max(col, vec3<f32>(0.0)), cov);
}
