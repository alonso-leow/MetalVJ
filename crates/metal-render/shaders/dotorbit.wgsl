// dotorbit: dots that circle their own cell, each on a private orbit
// (radius, phase, speed, direction, tilt, squash), coloured from a stepped
// palette; everything here is
// original.
//
// Space is cut into square cells of pitch `celda`. Every cell holds
// round(`cantidad`) dots. Dot k of cell c has hashes that give its orbit
// radius (`dispersion`), speed (`velocidad` +- `vel_rango`), direction
// (`sentido` reverses a share), plane tilt and squash (`inclina`, `elipse`).
// Its colour is a hash quantised to `pasos_color` steps per palette gap, then
// optionally biased by distance to the centre and cycled over time. Optional
// extras: motion trail (re-evaluating the orbit at past times), pulsing and
// twinkling radius, fake depth from the orbit phase, a halo and the orbit ring.

fn orbit_pos(base: vec2<f32>, h: vec4<f32>, h2: vec4<f32>, k: f32, tm: f32, cell: f32) -> vec3<f32> {
    let reach = f_dispersion() * cell * (0.12 + 0.38 * h.x);
    let dir = select(1.0, -1.0, h.y < f_sense() * 0.5);
    let om = f_speed() * (1.0 + f_speed_range() * (h.z * 2.0 - 1.0)) * (0.6 + 0.8 * h2.x);
    let th = h.w * TAU + dir * om * tm * 1.3;
    let sq = 1.0 - f_ellipse() * (0.4 + 0.6 * h2.y);
    let tilt = (h2.z - 0.5) * TAU * f_tilt();
    let o = p_rot(vec2<f32>(cos(th) * reach, sin(th) * reach * sq), tilt);
    return vec3<f32>(base + o, th);
}

fn dot_cell(p: vec2<f32>, cell: vec2<f32>, celda: f32, acc: ptr<function, vec4<f32>>) {
    let n = clamp(i32(floor(f_amount() + 0.5)), 1, 6);
    let t = p_time();
    let steps = max(floor(f_steps_color() + 0.5), 1.0) * 3.0 + 1.0;
    let ntrail = select(0, 4, f_wake() > 0.01);
    for (var k = 0; k < 6; k = k + 1) {
        if (k >= n) { break; }
        let kf = f32(k);
        let key = cell * 7.0 + vec2<f32>(kf * 13.0, kf * 5.0);
        let h = vec4<f32>(p_hash(key), p_hash(key + vec2<f32>(3.1, 7.7)), p_hash(key + vec2<f32>(9.3, 1.9)), p_hash(key + vec2<f32>(5.5, 4.4)));
        let h2 = vec4<f32>(p_hash(key + vec2<f32>(21.0, 8.0)), p_hash(key + vec2<f32>(33.0, 2.0)), p_hash(key + vec2<f32>(17.0, 29.0)), p_hash(key + vec2<f32>(41.0, 13.0)));
        let jit = (p_hash2(cell * 3.0 + vec2<f32>(kf, 0.0) + vec2<f32>(50.0, 50.0)) - 0.5) * celda * 0.5 * f_jitter();
        let centre = (cell + 0.5) * celda + jit;

        var rad = 0.5 * f_size() * celda * (1.0 - f_size_range() * h2.w);
        rad = rad * (1.0 + f_pulse_dot() * sin(t * f_pulse_speed() * 3.0 + h.x * TAU));
        let tw = 0.5 + 0.5 * sin(t * (1.5 + 3.0 * h.y) + h.z * TAU * 4.0);
        let glint = mix(1.0, tw, f_flash());

        // Colour: hash (or radius from the middle) quantised to palette steps.
        let rr = length(centre) / (0.5 * p_aspect() + 0.5);
        var u = mix(h2.x, clamp(rr * 0.6, 0.0, 1.0), f_color_radius());
        u = fract(u + t * f_color_speed() * 0.1);
        let ci = min(floor(u * steps), steps - 1.0) / (steps - 1.0);
        let col = p_pal(0u, ci);

        // Orbit ring (a thin line around the path).
        if (f_ring() > 0.01) {
            let reach = f_dispersion() * celda * (0.12 + 0.38 * h.x);
            let sq = 1.0 - f_ellipse() * (0.4 + 0.6 * h2.y);
            let tilt = (h2.z - 0.5) * TAU * f_tilt();
            let q = p_rot(p - centre, -tilt);
            let d = abs(length(vec2<f32>(q.x, q.y / max(sq, 0.1))) - reach);
            let w = max(fwidth(d), 1e-4) + 0.0015;
            let a = (1.0 - smoothstep(0.0, w * 1.5, d)) * f_ring() * 0.5;
            let prev0 = *acc;
            *acc = vec4<f32>(prev0.rgb * (1.0 - a) + col * a, prev0.a * (1.0 - a) + a);
        }

        for (var s = 0; s <= 4; s = s + 1) {
            if (s > ntrail) { break; }
            let sf = f32(s);
            let tm = t - sf * f_wake_long() * 0.12;
            let o = orbit_pos(centre, h, h2, kf, tm, celda);
            let depth = 1.0 + f_depth() * 0.6 * sin(o.z);
            let fade = select(1.0, (1.0 - sf / (f32(ntrail) + 1.0)) * f_wake(), s > 0);
            let r = rad * depth * (1.0 - 0.12 * sf * f_wake());
            if (r <= 1e-4) { continue; }
            let d = length(p - o.xy) - r;
            let aa = max(fwidth(d), 1e-4) + f_softness() * r;
            var a = (1.0 - smoothstep(-aa, aa, d)) * fade * glint;
            a = max(a, f_halo() * 0.6 * exp(-max(d, 0.0) / (r * 1.5 + 1e-4)) * fade * glint * 0.5);
            if (a <= 0.002) { continue; }
            let shade = col * mix(1.0, 0.7 + 0.5 * (0.5 + 0.5 * sin(o.z)), f_depth());
            let prev = *acc;
            *acc = vec4<f32>(prev.rgb * (1.0 - a) + shade * a, prev.a * (1.0 - a) + a);
        }
    }
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let cell = max(f_cell(), 0.05);
    let s = p - vec2<f32>(f_speed_x(), f_speed_y()) * p_time() * 0.1;
    let c0 = floor(s / cell);
    var acc = vec4<f32>(0.0);
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            dot_cell(s, c0 + vec2<f32>(f32(i), f32(j)), cell, &acc);
        }
    }
    let cov = clamp(acc.a, 0.0, 1.0);
    return vec4<f32>(acc.rgb / max(cov, 1e-4), cov);
}
