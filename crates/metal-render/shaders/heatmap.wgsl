// heatmap: a picture's shape rendered as a thermal map. Behaviour and
// parameter list written from the heatmap" effect; everything here is
// original.
//
// The source's luminance x alpha is cut at `umbral` into a shape S (edge
// softness `suaviza`, flipped by `invierte`). A blurred copy B of S is built
// from two rings of taps (radii `radio`, 2.2 x `radio`): B is ~1 deep inside,
// ~0.5 on the edge, ~0 far outside. Heat is then
//   inside:  S * (1 - contorno * depth)  scaled by `glow_in`  (hot core or hot rim)
//   outside: (1 - S) * B * 2             scaled by `glow_out` (halo falling off)
// The lookup position is displaced by fractal noise (`ruido`, `ruido_escala`,
// evolving at `ruido_vel`) that also streams along the heat direction `angulo`
// (`fluir`, `velocidad`) and licks upward in tongues (`llama`). Heat can ripple
// (`ondas`, `ondas_freq`), breathe (`pulso_calor`), be curved (`gamma`) and
// quantised into contour bands (`bandas`, `banda_suave`). It indexes slot 0
// (the heat ramp, shifted by `desfase`/`ciclo`, channel-split by `dispersa`);
// slot 1 tints the outer halo (`tinte_glow`); slot 2 is the white-hot core
// (`brasa`).

fn hm_shape(p: vec2<f32>) -> f32 {
    let s = p_src(p);
    var l = dot(s.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)) * s.a;
    l = mix(l, 1.0 - l, step(0.5, f_invert()));
    let w = 0.02 + f_soften() * 0.4;
    return smoothstep(f_threshold() - w, f_threshold() + w, l);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let ang = f_angle() * TAU;
    let dir = vec2<f32>(cos(ang), sin(ang));
    let perp = vec2<f32>(-dir.y, dir.x);
    let sd = f_seed() * 3.7;

    // Displacement noise, streaming along the heat direction.
    let along = dot(p, dir);
    let flow = t * f_speed() * f_flow() * 0.35;
    let np = p * f_noise_scale() + dir * flow + vec2<f32>(sd, t * f_noise_speed() * 0.4);
    let n1 = p_fbm(np, 3) - 0.5;
    let n2 = p_fbm(np + vec2<f32>(7.3, 2.1), 3) - 0.5;
    // Tongues: stretched noise that grows with how far "up" the heat direction we sample.
    let tongue = p_fbm(vec2<f32>(dot(p, perp) * f_noise_scale() * 2.2 + sd, along * f_noise_scale() * 0.6 - flow * 1.5), 3);
    var q = p + vec2<f32>(n1, n2) * f_noise() * 0.5;
    q = q - dir * (tongue - 0.3) * f_flame() * 0.35;

    // Shape and its blurred copy.
    let s0 = hm_shape(q);
    let r1 = max(f_radius(), 0.004);
    var acc = s0 * 2.0;
    var wsum = 2.0;
    for (var i = 0; i < 8; i = i + 1) {
        let a = f32(i) * 0.785398 + 0.3;
        let o = vec2<f32>(cos(a), sin(a));
        acc = acc + hm_shape(q + o * r1) * 1.0;
        acc = acc + hm_shape(q + o * r1 * 2.2 + vec2<f32>(0.0, 0.0)) * 0.6;
        wsum = wsum + 1.6;
    }
    let b = acc / wsum;

    // Heat field.
    let depth = clamp(b * 2.0 - 1.0, 0.0, 1.0);
    let gin = mix(0.25, 1.0, f_glow_in());
    let hin = s0 * (1.0 - f_outline() * depth) * gin;
    let hout = (1.0 - s0) * clamp(b * 2.0, 0.0, 1.0) * f_glow_out() * 1.3;
    var h = hin + hout;
    h = h + f_waves() * 0.18 * sin(b * f_waves_freq() * 12.0 - t * f_speed() * 3.0) * smoothstep(0.02, 0.3, b);
    h = h * (1.0 + f_pulse_heat() * 0.35 * sin(t * 3.0 + b * 4.0));
    h = clamp(h, 0.0, 1.0);
    h = pow(h, exp2(f_gamma()));
    let nb = floor(f_bands() + 0.5);
    if (nb > 0.5) {
        let x = h * nb;
        let fl = floor(x);
        let fr = fract(x);
        let st = (fl + smoothstep(0.5 - f_band_soft() * 0.5, 0.5 + f_band_soft() * 0.5 + 1e-3, fr)) / nb;
        h = st;
    }

    // Colour.
    let rt = h + f_shift() + f_cycle() * 0.25 * sin(t * 0.8 + along * 2.0);
    let ds = f_scattered() * 0.12;
    var col = vec3<f32>(p_pal(0u, rt + ds).r, p_pal(0u, rt).g, p_pal(0u, rt - ds).b);
    col = mix(col, p_pal(1u, h) * (0.4 + h), (1.0 - s0) * f_tint_glow());
    let core = smoothstep(0.68, 1.0, h);
    col = col + p_pal(2u, h) * core * f_ember();
    col = col + vec3<f32>(p_grain(uv, t)) * f_grain() * 0.5 * (0.3 + h);
    let cov = smoothstep(0.0, 0.12, h);
    return vec4<f32>(max(col, vec3<f32>(0.0)), cov);
}
