// liquidmetal: a chrome-like liquid metal poured over a shape. Behaviour
//; the
// maths here is original.
//
// The shape is the `imagen` source (alpha blended with luminance by `mascara`)
// or, with nothing connected, a procedural silhouette picked by `forma`
// (0 disc, 1 daisy, 2 diamond, 3 metaballs). A bevel height field and its
// gradient come from 16 taps of that mask in two rings (`bisel` = ring radius)
// with no distance transform: height = mean of the mask, gradient = the first
// moment of the taps. The metal is a band pattern (`repeticion` stripes along
// `angulo`) phase-shifted by the height (`contorno`), by the gradient
// (`distorsion`), by ripples and a noise warp; red and blue read the pattern
// at shifted phases (`desplaza_r/_b`) for the chromatic split. The bands are
// coloured by the `metal` slot, the rim and `tinte` by the `tinte` slot.

fn lm_shape(p: vec2<f32>) -> f32 {
    let t = p_time();
    let s = max(f_size(), 0.05);
    let q = p / s;
    let kind = clamp(i32(floor(f_shape() + 0.5)), 0, 3);
    var sd = 0.0;
    if (kind == 0) {
        sd = length(q) - 0.5;
    } else if (kind == 1) {
        let a = atan2(q.y, q.x);
        sd = length(q) - (0.34 + 0.12 * cos(a * 7.0 + t * 0.2)) ;
    } else if (kind == 2) {
        sd = (abs(q.x) + abs(q.y)) - 0.55;
    } else {
        var f = 0.0;
        for (var i = 0; i < 4; i = i + 1) {
            let fi = f32(i);
            let c = vec2<f32>(cos(t * (0.3 + fi * 0.13) + fi * 1.7), sin(t * (0.37 + fi * 0.11) + fi * 2.9)) * 0.28;
            let d = length(q - c);
            f = f + 0.045 / (d * d + 0.01);
        }
        sd = (1.0 - f) * 0.2;
    }
    return 1.0 - smoothstep(-0.012, 0.012, sd * s);
}

fn lm_mask(p: vec2<f32>) -> f32 {
    if (p_has_src()) {
        let c = p_src(p);
        let lum = dot(c.rgb, vec3<f32>(0.299, 0.587, 0.114));
        return clamp(mix(c.a, c.a * lum, f_mask()), 0.0, 1.0);
    }
    // Procedural: `mascara` carves the shape with the stand-in picture's luminance.
    let lum = dot(p_src(p).rgb, vec3<f32>(0.299, 0.587, 0.114));
    return lm_shape(p) * mix(1.0, lum, f_mask());
}

fn lm_bands(v: f32, rep: f32, soft: f32, hard: f32) -> f32 {
    let x = fract(v * rep);
    let tri = abs(x * 2.0 - 1.0);
    let sm = 0.5 + 0.5 * cos(tri * PI);
    let st = smoothstep(0.5 - soft * 0.5 - 0.002, 0.5 + soft * 0.5 + 0.002, tri);
    return mix(sm, st, hard);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let m0 = lm_mask(p);

    // Bevel height and gradient from two rings of mask taps.
    let rad = max(f_bevel(), 0.01) * (1.0 + f_pulses() * 0.6 * sin(t * 1.3));
    var hsum = m0;
    var g = vec2<f32>(0.0);
    for (var k = 0; k < 8; k = k + 1) {
        let a = f32(k) * TAU / 8.0 + 0.3;
        let d = vec2<f32>(cos(a), sin(a));
        let ma = lm_mask(p + d * rad * 0.5);
        let mb = lm_mask(p + d * rad);
        hsum = hsum + ma + mb;
        g = g + d * (ma * 0.6 + mb * 0.4);
    }
    let h = hsum / 17.0;
    g = g / 8.0 * 2.0;
    // Dome: broad swell towards the middle so the interior is not flat.
    let dome = f_dome() * (1.0 - smoothstep(0.0, 0.7, length(p) / max(f_size(), 0.05))) ;
    let hh = h + dome * m0;
    let gz = f_relief() * 3.0;
    let n = normalize(vec3<f32>(-g * gz, 1.0));

    // Band phase.
    let ang = f_angle() * TAU;
    let adir = vec2<f32>(cos(ang), sin(ang));
    let aperp = vec2<f32>(-adir.y, adir.x);
    let size = max(f_size(), 0.05);
    let aniso = 1.0 + f_anisotropy() * 3.0;
    var v = dot(p, adir) / size * 0.5 + dot(p, aperp) / size * 0.5 / aniso;
    v = v + hh * f_outline() * 2.0;
    v = v + dot(g, adir) * f_distortion() * 12.0;
    v = v + sin(dot(p, aperp) / size * f_ripple_freq() + t * 1.5) * f_ripple() * 0.25;
    let w = vec2<f32>(p_noise(p / size * 1.7 + vec2<f32>(t * 0.15, f_seed())),
                      p_noise(p / size * 1.7 + vec2<f32>(7.3, t * 0.12 - f_seed()))) - 0.5;
    v = v + (w.x + w.y) * f_flow() * 1.5;
    v = v + t * f_speed() * 0.2;
    v = v + sin(t * 0.9) * f_pulses() * 0.3;

    let rep = max(f_repetition(), 0.1);
    let soft = f_softness();
    let hard = f_bands_hard();
    let vr = v + f_displace_r() * 0.25;
    let vb = v - f_displace_b() * 0.25;
    let br = lm_bands(vr, rep, soft, hard);
    let bg = lm_bands(v, rep, soft, hard);
    let bb = lm_bands(vb, rep, soft, hard);
    var col = vec3<f32>(p_pal(0u, br).r, p_pal(0u, bg).g, p_pal(0u, bb).b);

    // Tint, environment horizon, fresnel rim, specular glints.
    let tn = p_pal(1u, bg);
    col = mix(col, col * tn, f_tint());
    let env = smoothstep(-0.35, 0.35, n.y + (bg - 0.5) * 0.3);
    col = col * mix(1.0, 0.35 + 0.9 * env, f_background_reflection());
    let fres = pow(clamp(1.0 - n.z, 0.0, 1.0), 1.5);
    col = col + p_pal(1u, 0.85) * fres * f_fresnel();
    let L = normalize(vec3<f32>(cos(t * 0.5) * 0.6, sin(t * 0.37) * 0.6, 0.8));
    col = col + vec3<f32>(pow(max(dot(n, L), 0.0), 48.0)) * f_shine_specular() * 1.5;

    let cov = smoothstep(0.0, max(f_edge(), 0.02), m0);
    return vec4<f32>(max(col, vec3<f32>(0.0)), cov);
}
