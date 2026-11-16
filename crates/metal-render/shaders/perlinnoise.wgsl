// perlinnoise: a two-colour (or ramped) field cut from fractal gradient
// noise; the noise,
// warp, ridge/billow shaping, banding and relief are original.
//
// field(q): fractal sum of hashed-gradient (Perlin) noise, each octave
// optionally ridged (1-|n|) or billowed (|n|), after a domain warp built from
// two further noise lookups and a slow advection `flujo`. The field is cut at
// `proporcion` with a `suavidad`-wide edge; slot 0 paints the low side, slot 1
// the high side, slot 2 the outline / rim light. `rampa` lets the raw field
// walk the slot gradients instead of a flat two-colour cut.

fn pn_grad(i: vec2<f32>, q: vec2<f32>) -> f32 {
    let a = p_hash(i) * TAU;
    return dot(vec2<f32>(cos(a), sin(a)), q);
}

// 2D gradient noise, about -0.7..0.7.
fn pn(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let w = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = pn_grad(i, f);
    let b = pn_grad(i + vec2<f32>(1.0, 0.0), f - vec2<f32>(1.0, 0.0));
    let c = pn_grad(i + vec2<f32>(0.0, 1.0), f - vec2<f32>(0.0, 1.0));
    let d = pn_grad(i + vec2<f32>(1.0, 1.0), f - vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

// Fractal sum in 0..1. `oct` is fractional: the last octave fades in so a
// stepped control never pops.
fn pn_fbm(p: vec2<f32>, oct: f32, pers: f32, lac: f32, ridge: f32, billow: f32) -> f32 {
    var sum = 0.0;
    var norm = 0.0;
    var amp = 1.0;
    var fr = 1.0;
    var off = vec2<f32>(0.0);
    for (var o = 0; o < 8; o = o + 1) {
        let w = clamp(oct - f32(o), 0.0, 1.0);
        if (w <= 0.0) { break; }
        var n = pn(p * fr + off) * 1.45;
        let r = 1.0 - abs(n) * 1.6;
        let b = abs(n) * 1.6 - 0.5;
        n = mix(n, r * r - 0.4, ridge);
        n = mix(n, b, billow * (1.0 - ridge));
        sum = sum + n * amp * w;
        norm = norm + amp * w;
        amp = amp * pers;
        fr = fr * lac;
        off = off + vec2<f32>(17.3, 9.1);
    }
    return clamp(0.5 + 0.5 * sum / max(norm, 0.0001), 0.0, 1.0);
}

fn pn_field(q0: vec2<f32>, t: f32) -> f32 {
    var q = q0;
    // Anisotropic stretch.
    let an = f_anisotropy();
    q = vec2<f32>(q.x * (1.0 + max(an, 0.0) * 3.0), q.y * (1.0 + max(-an, 0.0) * 3.0));
    // Slow advection along a noise direction (flow).
    let fa = pn(q * 0.6 + vec2<f32>(t * 0.1, 3.0)) * TAU * 1.5;
    q = q + f_flow() * 0.8 * vec2<f32>(cos(fa), sin(fa)) * (0.5 + 0.5 * sin(t * 0.3));
    // Domain warp from two noise lookups.
    let ws = f_warp_scale();
    let wv = vec2<f32>(pn(q * ws + vec2<f32>(t * 0.15, 0.0)), pn(q * ws + vec2<f32>(5.2, 1.3 - t * 0.12)));
    q = q + wv * f_warp_domain() * 1.6;
    // Time as a third dimension: slide the lattice diagonally and morph by a
    // second, offset sample of the same field.
    let sl = vec2<f32>(t * 0.35, t * 0.21);
    let oct = clamp(f_octaves(), 1.0, 8.0);
    let a = pn_fbm(q + sl, oct, f_persistence(), f_lacunarity(), f_crest(), f_wave());
    let b = pn_fbm(q - sl.yx + vec2<f32>(11.7, 4.3), oct, f_persistence(), f_lacunarity(), f_crest(), f_wave());
    return mix(a, b, 0.5 + 0.5 * sin(t * 0.4 * f_morph() * 3.0));
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time() * f_speed();
    let q = p_rot(p, f_rotation() * TAU) * f_scale() + vec2<f32>(f_off_x(), f_off_y()) * 3.0;
    var v = pn_field(q, t);

    // Contrast about the middle so octave sums (which cluster at 0.5) still
    // reach both colours.
    v = clamp(0.5 + (v - 0.5) * f_contrast_field(), 0.0, 1.0);

    // Contour bands on the raw field.
    let nb = floor(f_n_bands() + 0.5);
    let bd = floor(v * nb) / nb + 0.5 / nb;
    let vb = mix(v, bd, f_bands());

    // Cut at `proporcion` (more proportion = more of the front colour).
    let thr = 1.0 - clamp(f_proportion(), 0.0, 1.0);
    let wd = max(f_softness() * 0.5, 0.0015);
    var m = smoothstep(thr - wd, thr + wd, vb);

    // Outline hugging the cut.
    let distance = abs(vb - thr);
    let ol = (1.0 - smoothstep(0.0, 0.02 + f_outline_width() * 0.06, distance)) * f_outline();

    // Relief: light from the field's slope.
    var lit = 0.0;
    if (f_relief() > 0.001) {
        let e = 0.02;
        let vx = pn_field(q + vec2<f32>(e, 0.0), t) - v;
        let vy = pn_field(q + vec2<f32>(0.0, e), t) - v;
        let la = f_light_angle() * TAU;
        lit = clamp((vx * cos(la) + vy * sin(la)) / e * 0.5, -1.0, 1.0) * f_relief();
    }

    // Colour: flat cut, or the raw field walking each slot gradient.
    let drift = fract(f_drift_color() * p_time() * 0.1);
    let lo = p_pal(0u, fract(mix(0.0, v, f_ramp()) + drift));
    let hi = p_pal(1u, fract(mix(1.0, v, f_ramp()) + drift));
    var col = mix(lo, hi, m);
    col = col * (1.0 + lit * 0.8);
    col = mix(col, p_pal(2u, clamp(0.5 + lit * 0.5, 0.0, 1.0)), ol);

    // Fine film grain, kept in screen space.
    col = col + p_grain(uv, p_time()) * f_grain() * 0.5;
    // `hueco` lets the low side fall through to the fondo slot.
    let cov = 1.0 - f_hole() * (1.0 - m);
    return vec4<f32>(max(col, vec3<f32>(0.0)), cov);
}
