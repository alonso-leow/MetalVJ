struct Cf {
    f1: f32,
    f2: f32,
    id: vec2<f32>,
}

fn cf_softstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / max(0.00001, e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn cf_cellular(p: vec2<f32>, phase: f32, motion: f32) -> Cf {
    let ip = floor(p);
    let fp = p - ip;
    var r: Cf;
    r.f1 = 9.0;
    r.f2 = 9.0;
    r.id = ip;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let g = vec2<f32>(f32(i), f32(j));
            let rnd = p_hash2(ip + g + vec2<f32>(f_seed() * 13.7, f_seed() * 5.3));
            let a = phase * (0.16 + 0.12 * rnd.x) + TAU * rnd + vec2<f32>(0.0, 1.7);
            let orbit = vec2<f32>(0.5) + 0.5 * sin(a);
            let feature = mix(rnd, orbit, motion);
            let d = g + feature - fp;
            let dist2 = dot(d, d);
            if (dist2 < r.f1) {
                r.f2 = r.f1;
                r.f1 = dist2;
                r.id = ip + g;
            } else if (dist2 < r.f2) {
                r.f2 = dist2;
            }
        }
    }
    return Cf(sqrt(r.f1), sqrt(r.f2), r.id);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let speed = clamp(f_speed(), -2.0, 2.0);
    let bpm = max(1.0, f_bpm());
    let drift = clamp(f_warp_drift(), 0.0, 1.0);
    let drift_phase = t * speed * (bpm / 60.0) * TAU * drift;
    let warp_time = drift_phase * 0.22;

    let density = mix(2.0, 22.0, clamp(f_cells() / 50.0, 0.0, 1.0));
    let core_size = clamp(f_core() / 3.0, 0.0, 1.0);
    let inner_gap = clamp(f_inner_gap(), 0.0, 100.0) * 0.0012;
    let membrane = clamp(f_membrane(), 0.0, 200.0) * 0.0012;
    let warp = clamp(f_warp(), 0.0, 1.0);
    let warp_scale = clamp(f_warp_scale(), 0.0, 1.0);
    let warp_complexity = clamp(f_warp_complexity(), 0.0, 1.0);
    let px = 0.0014;

    var q = p;
    let warp_freq = mix(0.65, 5.80, warp_scale);
    let wp = q * warp_freq;

    let warp_a = vec2<f32>(
        sin(wp.y * 1.35 + warp_time + sin(wp.x * 0.90 - warp_time * 0.70)),
        cos(wp.x * 1.25 - warp_time * 0.82 + cos(wp.y * 0.95 + warp_time * 0.45))
    );
    let warp_b = vec2<f32>(
        sin(dot(wp, vec2<f32>(1.75, -1.15)) + warp_time * 1.35),
        cos(dot(wp, vec2<f32>(-1.05, 1.85)) - warp_time * 1.18)
    );
    let warp_c = vec2<f32>(
        sin(wp.x * 3.10 + sin(wp.y * 2.15 + warp_time * 1.60) - warp_time * 2.10),
        cos(wp.y * 2.85 + cos(wp.x * 1.95 - warp_time * 1.40) + warp_time * 1.75)
    );

    let mid_c = smoothstep(0.12, 0.62, warp_complexity);
    let high_c = smoothstep(0.52, 1.00, warp_complexity);
    let warp_field = warp_a * 0.23 + warp_b * (0.17 * mid_c) + warp_c * (0.08 * high_c);
    q = q + warp_field * warp;
    q = p_rot(q, 0.045 * sin(warp_time * 0.37 + dot(q, vec2<f32>(0.7, -0.4))) * warp * (0.30 + 0.70 * warp_complexity));

    let cell_space = q * density;
    let cell_motion = warp * mix(0.10, 0.68, warp_complexity);
    let c = cf_cellular(cell_space, drift_phase, cell_motion);

    let nearest = c.f1 / density;
    let gap = (c.f2 - c.f1) / density;

    var membrane_mask = 0.0;
    if (membrane > 0.0) {
        let variation = 0.5 + 0.5 * sin(cell_space.x * 9.0 + sin(cell_space.y * 7.3) + drift_phase * 0.14);
        let local_width = membrane * mix(1.0, 0.35 + 1.75 * variation, warp * 0.65);
        let aa = max(px * 1.5, 0.0015);
        membrane_mask = 1.0 - cf_softstep(local_width - aa, local_width + aa, gap);
    }

    var core_mask = 0.0;
    if (core_size > 0.0001) {
        let aa = max(px * 1.5, 0.0018);
        let circle_radius = mix(0.0, 0.24, core_size);
        let circle_mask = 1.0 - cf_softstep(circle_radius - aa, circle_radius + aa, nearest);
        let membrane_norm = clamp(f_membrane() / 200.0, 0.0, 1.0);
        let morph_driver = clamp(pow(core_size, 0.78) + 0.35 * membrane_norm, 0.0, 1.35);
        let morph = smoothstep(0.14, 0.95, morph_driver);
        let clearance = inner_gap + px * 1.5;
        let required = clearance + circle_radius * morph * 0.90;
        let interior = cf_softstep(required - aa, required + aa, gap);
        core_mask = clamp(circle_mask * interior, 0.0, 1.0);
    }

    let hid = p_hash(c.id + vec2<f32>(f_seed() * 13.7, f_seed() * 5.3));
    let hid2 = p_hash(c.id + vec2<f32>(101.0, 57.0));

    let core_col = p_pal(0u, clamp(0.15 + 0.70 * hid, 0.0, 1.0));
    let membrane_col = p_pal(1u, clamp(0.25 + 0.75 * hid2, 0.0, 1.0));

    var col = mix(membrane_col, core_col, clamp(core_mask, 0.0, 1.0));
    var cov = clamp(core_mask + membrane_mask * (1.0 - core_mask), 0.0, 1.0);

    let aspect = p_aspect();
    let vig = 1.0 - smoothstep(0.20, 1.25, length(p / vec2<f32>(max(0.65, aspect), 1.0)));
    col = col * mix(1.0, mix(0.70, 1.02, vig), clamp(f_vignette(), 0.0, 1.0));
    col = col + vec3<f32>(p_grain(uv, t) * f_grain() * (0.20 + warp) * cov);
    return vec4<f32>(clamp(col * (1.0 + 0.06 * clamp(f_core() / 3.0, 0.0, 1.0)), vec3<f32>(0.0), vec3<f32>(1.0)), cov);
}
