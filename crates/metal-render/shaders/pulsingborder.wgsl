// pulsingborder: a glowing rounded frame. Behaviour and parameter list
// written from the pulsing border" effect; everything here is original.
//
// The frame is a signed distance to a rounded box (blendable towards an
// ellipse). The border line is a soft band around the zero of that distance,
// its width breathing with the pulse. Colour spots orbit the perimeter by
// angle, each a gaussian bump in angle that lights the line and the smoke.
// Smoke is fbm advected along the frame, bloom is an exponential halo outside
// and inside. Slot 0 colours spots and line, slot 1 smoke and fill, slot 2 the
// bloom. Coverage is the brightest channel, so `fondo` shows through elsewhere.

fn box_sdf(q: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let d = abs(q) - b + vec2<f32>(r);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

fn frame_ratio() -> f32 {
    let k = floor(f_aspect() + 0.5);
    if (k < 0.5) { return p_aspect(); }
    if (k < 1.5) { return 1.0; }
    if (k < 2.5) { return 1.3333; }
    if (k < 3.5) { return 1.7778; }
    return 0.5625;
}

fn wrap01(x: f32) -> f32 { return x - floor(x + 0.5); }

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time() * f_speed();
    let tr = p_time();

    // Frame geometry.
    let ratio = frame_ratio();
    var hh = 0.46 * (1.0 - 1.6 * f_margin());
    var hx = ratio * hh;
    let maxx = p_aspect() * 0.5 * 0.94 * (1.0 - 1.6 * f_margin());
    if (hx > maxx) { hx = maxx; hh = hx / ratio; }
    let b = vec2<f32>(hx, hh);
    let rad = f_roundness() * min(b.x, b.y);

    let ang = atan2(p.y, p.x);
    let u = ang / TAU + 0.5;
    var distance = box_sdf(p, b, rad);
    let de = (length(p / b) - 1.0) * min(b.x, b.y);
    distance = mix(distance, de, f_ellipse());
    distance = distance + f_wave() * 0.02 * sin(ang * floor(f_wave_freq() + 0.5) + t * 2.0);

    // Breathing: thickness and brightness, optionally travelling round the frame.
    let ph = sin(tr * 2.4 - f_phase_pulse() * u * TAU * 2.0);
    let breath = 1.0 + f_pulse_b() * 0.6 * ph;
    let th = (0.006 + f_thickness() * 0.16) * breath;
    let w = th * 0.5;
    let sft = clamp(f_softness(), 0.0, 1.0);
    let ab = f_aberration() * 0.02;

    // Spots orbiting the perimeter.
    let n = clamp(floor(f_dots() + 0.5), 0.0, 8.0);
    let sw = 0.015 + f_dots_size() * 0.22;
    var spots = 0.0;
    var spot_col = vec3<f32>(0.0);
    for (var i = 0u; i < 8u; i = i + 1u) {
        if (f32(i) >= n) { break; }
        let fi = f32(i);
        let dirn = 1.0 - 2.0 * f_counterflow() * (fi - 2.0 * floor(fi * 0.5));
        let pos = fi / max(n, 1.0) + t * f_orbit() * dirn * 0.5;
        let da = wrap01(u - pos);
        let g = exp(-(da * da) / (sw * sw));
        let cm = fi / max(n - 1.0, 1.0);
        spots = spots + g;
        spot_col = spot_col + g * p_pal(0u, cm);
    }
    let spot_tint = spot_col / max(spots, 1e-3);

    // Optional dashes and sparkles along the line.
    var dash = 1.0;
    if (f_cuts() > 0.001) {
        let f = fract(u * floor(f_cuts_density() + 0.5) - t * 0.1);
        dash = mix(1.0, smoothstep(0.35, 0.55, f) * (1.0 - smoothstep(0.8, 0.95, f)), f_cuts());
    }
    var spark = 1.0;
    if (f_sparks() > 0.001) {
        let cell = vec2<f32>(floor(u * 90.0), floor(tr * 7.0));
        let h = p_hash(cell);
        spark = 1.0 + f_sparks() * 2.5 * step(0.9, h) * (h - 0.9) * 10.0;
    }

    // Line, per channel offset for the split.
    let lvl = clamp(f_base(), 0.0, 1.0);
    let line_tint = p_pal(0u, fract(u + f_cycle() * tr * 0.1));
    var line = vec3<f32>(0.0);
    var chan = vec3<f32>(0.0);
    for (var c = 0; c < 3; c = c + 1) {
        let off = (f32(c) - 1.0) * ab;
        let a = abs(distance + off + w);
        let band = 1.0 - smoothstep(w * (1.0 - 0.9 * sft), w * (1.0 + 2.2 * sft) + 0.0005, a);
        chan[c] = band;
    }
    line = chan * (lvl * line_tint + spots * 1.2 * spot_tint) * dash * spark;

    // Second, fainter ring further in.
    if (f_double() > 0.001) {
        let a2 = abs(distance + w + (0.02 + f_double_separation() * 0.18));
        let b2 = 1.0 - smoothstep(w * 0.4, w * (1.0 + 2.0 * sft), a2);
        line = line + b2 * f_double() * (0.5 * lvl * p_pal(0u, fract(u + 0.5)) + 0.7 * spots * spot_tint);
    }

    // Smoke, fbm drifting around the frame, strongest near the line.
    let ssz = mix(7.0, 1.2, clamp(f_smoke_size(), 0.0, 1.0));
    let sm_n = p_fbm(p * ssz + vec2<f32>(t * 0.2, -t * 0.13) + vec2<f32>(cos(ang), sin(ang)) * t * 0.1, 4);
    let reach = 0.05 + 0.25 * f_smoke_size();
    let inside = smoothstep(0.02, -0.02, distance);
    let smoke_m = smoothstep(0.3, 0.8, sm_n) * exp(-abs(distance) / reach) * (0.35 + 0.65 * inside);
    let smoke = p_pal(1u, sm_n) * smoke_m * f_smoke() * (0.5 + 1.8 * spots + 0.5 * ph * f_pulse_b());

    // Fill.
    let fill = p_pal(1u, 0.5 + 0.5 * sm_n) * inside * f_empty() * 0.6;

    // Bloom halos.
    let hr = 0.015 + f_halo_radius() * 0.2;
    let outer = exp(-max(distance, 0.0) / hr) * (1.0 - inside);
    let inner = exp(-max(-distance, 0.0) / (hr * 1.8)) * inside * f_halo_interior();
    let hcol = p_pal(2u, fract(u + f_cycle() * tr * 0.1));
    let glow = hcol * (outer * f_bloom() + inner * f_bloom() * 0.6) * (0.25 + 0.75 * lvl + spots) * (1.0 + 0.3 * f_pulse_b() * ph);

    var col = (line * f_intensity() * 2.0 + smoke + fill + glow) ;
    col = vec3<f32>(1.0) - exp(-col * 1.5);
    col = col + f_grain() * p_grain(uv, tr) * 0.5 * clamp(max(col.r, max(col.g, col.b)) * 4.0, 0.0, 1.0);
    col = max(col, vec3<f32>(0.0));
    let cov = clamp(max(col.r, max(col.g, col.b)), 0.0, 1.0);
    return vec4<f32>(col, cov);
}
