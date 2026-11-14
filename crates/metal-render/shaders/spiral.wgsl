// spiral: multi-arm spiral strokes. Written from first principles;
//
// Phase of the pattern is ph = radial(r) * pitch - angle * arms / TAU, so one
// full turn shifts the phase by exactly `arms` bands and the seam is invisible.
// radial() blends linear (Archimedean) and logarithmic spacing (mezcla_log).
// The fractional part of ph is the position across a stroke; its integer part
// (mod arms) names the arm, which drives per-arm colour, width and flicker.
// A second spiral winding the other way (doble) uses slot 1; glow adds a halo.

fn sp_radial(r: f32) -> f32 {
    let lg = log(1.0 + r * 6.0) / log(7.0);
    return mix(r, lg * 0.9, clamp(f_mix_log(), 0.0, 1.0));
}

// Returns (coverage, arm colour key, across-stroke position, glow).
fn sp_layer(q: vec2<f32>, r: f32, ang: f32, dir: f32, t: f32, n: f32, seed: f32) -> vec4<f32> {
    let pitch = mix(1.5, 14.0, clamp(f_density(), 0.0, 1.0))
        * (1.0 + f_squeeze_swing() * 0.35 * sin(t * 0.6));
    var rr = sp_radial(r);
    // Breathing ripple travelling along the radius.
    rr = rr + f_breath() * 0.03 * sin(t * 2.2 - r * 9.0);
    var ph = rr * pitch * dir - ang * n / TAU * dir - t * f_speed() * 0.5 * dir;
    // Arms drift against each other.
    ph = ph + f_turn_arms() * rr * 3.0 * sin(ang * n + t * 0.4) * 0.2;
    // Lobed waves bending the strokes.
    ph = ph + f_waves() * 0.45 * sin(ang * floor(f_waves_lobes() + 0.5) + t * 0.8 + r * 5.0);
    // Domain distortion by fractal noise.
    if (f_distortion() > 0.001) {
        let nf = 1.0 + f_noise_freq() * 5.0;
        let wn = p_fbm(q * nf * 1.6 + vec2<f32>(t * 0.12 + seed, -t * 0.09), 3);
        ph = ph + (wn - 0.5) * f_distortion() * 5.0;
    }
    let band = floor(ph);
    let fr = ph - band;
    let arm = band - n * floor(band / n);
    let arm_h = p_hash(vec2<f32>(arm + seed, 7.0));

    // Stroke width: base, cone taper along radius, alternating arms, tip.
    var w = clamp(f_stroke(), 0.0, 1.0);
    w = w * clamp(1.0 - f_stroke_cone() * clamp(r * 1.2, 0.0, 1.2), 0.0, 2.0);
    let odd = select(-1.0, 1.0, (arm - 2.0 * floor(arm * 0.5)) > 0.5);
    w = w * (1.0 + f_alternate() * 0.6 * odd);
    // Rounded tip: strokes thin out as they reach the centre.
    w = w * mix(1.0, smoothstep(0.0, 0.25, r), f_stroke_tip());
    // Edge noise.
    if (f_noise() > 0.001) {
        let en = p_noise(vec2<f32>(ang * 6.0 + ph * 2.0, r * 40.0 * (0.5 + f_noise_freq()) + t * 0.5));
        w = w + (en - 0.5) * f_noise() * 0.6;
    }
    // Per-arm flicker.
    let fl = p_hash(vec2<f32>(arm + seed * 3.0, floor(t * 6.0)));
    w = w * (1.0 - f_blink_arm() * fl);
    w = clamp(w, 0.0, 1.0);

    let d = abs(fr - 0.5) * 2.0;
    let soft = 0.02 + f_softness() * 0.45;
    let cov = 1.0 - smoothstep(w - soft, w + soft, d);
    let cov2 = cov * smoothstep(0.0, 0.02 + soft * 0.4, w);
    let glo = exp(-max(d - w, 0.0) * 6.0) * step(0.01, w);
    return vec4<f32>(cov2, arm_h, fr, glo);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let q = p - vec2<f32>(f_center_x(), f_center_y());
    let r = length(q);
    let ang = atan2(q.y, q.x);
    let n = max(floor(f_arms() + 0.5), 1.0);

    let a = sp_layer(q, r, ang, 1.0, t, n, 0.0);
    var tc = mix(0.5, a.y, f_color_arm());
    tc = tc + f_color_radius() * (r * 1.5 - 0.5);
    tc = mix(tc, a.z, f_color_stroke());
    var col = p_pal(0u, clamp(tc, 0.0, 1.0)) * a.x;
    var cov = a.x;
    var glow = a.w;

    if (f_double() > 0.001) {
        let b = sp_layer(q, r, ang, -1.0, t * 0.8, n, 11.0);
        let bc = b.x * f_double();
        let c1 = p_pal(1u, clamp(mix(0.5, b.y, f_color_arm()) + f_color_radius() * (r * 1.5 - 0.5), 0.0, 1.0));
        col = mix(col, c1, bc) ;
        cov = max(cov, bc);
        glow = max(glow, b.w * f_double());
    }

    // Glow halo around strokes, coloured from slot 1.
    let gl = glow * f_glow() * 0.6 * (1.0 - cov);
    col = col + p_pal(1u, 0.3 + 0.4 * r) * gl;
    cov = clamp(cov + gl, 0.0, 1.0);

    // Hole at the centre and fade at the outer edge.
    let hole = smoothstep(f_hole() * 0.5, f_hole() * 0.5 + 0.06, r);
    let edge = 1.0 - smoothstep(1.2 - f_edge() * 0.9, 1.3 - f_edge() * 0.9 + 0.2, r);
    cov = cov * hole * edge;
    return vec4<f32>(col * hole * edge, cov);
}
