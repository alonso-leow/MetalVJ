// hairlines: a family of curved strands around a head anchor. Behaviour and
// parameter list follow the community Canvas2D tool "Homer's Hair"; this is an
// original per-pixel field implementation, not a port of its draw calls.
//
// A base curve (arc / sine / cubic / closed loop) is scaled, rotated and
// displaced per strand, then every pixel measures the distance to the nearest
// strand's polyline. The nearest distance drives the line coverage (slot 0),
// a soft halo (slot 2) and a warm glow at the head anchor (slot 1).

// One point and its unit tangent on the base curve, packed as
// (x, y, tangent.x, tangent.y).
fn hl_sample(system: i32, t: f32, radius: f32, tension: f32) -> vec4<f32> {
    let x = (t - 0.5) * radius * 2.0;

    if (system == 0) {
        // Arc: deeper with tension.
        let xn = t * 2.0 - 1.0;
        let arch = max(sqrt(max(0.0, 1.0 - xn * xn)), 0.02);
        let c = 0.16 + tension * 2.88;
        let y = -arch * radius * c;
        let dx = radius * 2.0;
        let dy = radius * c * 2.0 * xn / arch;
        return vec4<f32>(x, y, dx, dy);
    }

    if (system == 1) {
        // Sine: amplitude grows with tension.
        let amp = radius * (0.12 + tension * 2.48);
        let a = t * TAU;
        let y = sin(a) * amp;
        let dx = radius * 2.0;
        let dy = cos(a) * TAU * amp;
        return vec4<f32>(x, y, dx, dy);
    }

    if (system == 2) {
        // Cubic bezier with opposing handles: a controlled S curve.
        let bend = radius * (0.18 + tension * 3.28);
        let p0 = vec2<f32>(-radius, 0.0);
        let p1 = vec2<f32>(-radius * 0.28, -bend);
        let p2 = vec2<f32>(radius * 0.28, bend);
        let p3 = vec2<f32>(radius, 0.0);
        let u = 1.0 - t;
        let uu = u * u;
        let tt = t * t;
        let pt = uu * u * p0 + 3.0 * uu * t * p1 + 3.0 * u * tt * p2 + tt * t * p3;
        let d = 3.0 * uu * (p1 - p0) + 6.0 * u * t * (p2 - p1) + 3.0 * tt * (p3 - p2);
        return vec4<f32>(pt, d);
    }

    // Closed loop: radial lobing grows with tension.
    let a = t * TAU;
    let radial = 1.0 + sin(a * 3.0) * 0.64 * tension + cos(a * 5.0) * 0.28 * tension;
    let drad = (1.92 * cos(a * 3.0) - 1.40 * sin(a * 5.0)) * tension * TAU;
    let cs = cos(a);
    let sn = sin(a);
    let pt = vec2<f32>(cs, sn) * radius * radial;
    let d = vec2<f32>(
        radius * (-sn * TAU * radial + cs * drad),
        radius * (cs * TAU * radial + sn * drad)
    );
    return vec4<f32>(pt, d);
}

fn hl_seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-9), 0.0, 1.0);
    return length(pa - ba * h);
}

// One sample of strand `fi`, including the per-strand scale, rotation,
// displacement waves and propagation drift.
fn hl_point(
    system: i32,
    fi: f32,
    n: f32,
    t: f32,
    radius: f32,
    tension: f32,
    phase_time: f32,
    phase_offset: f32,
    scale_step: f32,
    rotation_step: f32,
    displacement_norm: f32,
    warp_norm: f32,
    propagation_norm: f32,
    spacing_norm: f32,
    seed: f32
) -> vec2<f32> {
    let centered = fi - (n - 1.0) * 0.5;
    let halfn = max((n - 1.0) * 0.5, 1.0);
    let rel = select(0.0, centered / halfn, n > 1.0);

    let offset_x = rel * 0.38 * spacing_norm;
    let scale_factor = max(0.08, 1.0 + centered * scale_step);
    let rotation = centered * rotation_step;
    let propagation_phase = rel * propagation_norm * PI * 1.65 + centered * propagation_norm * 0.11;

    // `seed` jitters the phase of each strand without changing its path family.
    let jitter = (p_hash(vec2<f32>(fi + 0.37, seed * 3.1 + 0.71)) - 0.5) * seed * 0.35;
    let local_time = phase_time + centered * phase_offset + propagation_phase + jitter;

    let st = hl_sample(system, t, radius, tension);
    let base = st.xy;
    let tan_len = max(length(st.zw), 1e-6);
    let tangent = st.zw / tan_len;
    let nrm = vec2<f32>(-tangent.y, tangent.x);

    let primary = sin(t * TAU + local_time + centered * phase_offset * 0.35);
    let secondary = sin(
        t * PI * (5.0 + abs(rel) * 3.0)
        - phase_time * 0.42
        - propagation_phase * 0.72
        + centered * 0.71
    );
    let disp = primary * radius * 0.20 * displacement_norm
        + secondary * radius * 0.095 * warp_norm;

    var q = (base + nrm * disp) * scale_factor;
    q = p_rot(q, rotation);

    let transverse = sin(
        phase_time * 0.55 + propagation_phase + t * TAU + fi * 0.17
    ) * radius * 0.055 * propagation_norm * rel;

    return vec2<f32>(q.x + offset_x, q.y + transverse);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let system = i32(clamp(floor(f_system() + 0.5), 0.0, 3.0));
    let n = clamp(floor(f_repetitions() + 0.5), 2.0, 32.0);
    let radius = 0.25;
    let tension = clamp(f_curve_tension() / 400.0, 0.0, 1.0);
    let phase_offset = f_phase_offset() * PI / 180.0;
    let scale_step = f_scale_step() / 100.0;
    let rotation_step = f_rotation_step() * PI / 180.0;
    let spacing_norm = max(f_spacing(), 0.0) / 100.0;
    let displacement_norm = max(f_displacement(), 0.0) / 100.0;
    let warp_norm = max(f_warp(), 0.0) / 100.0;
    let propagation_norm = max(f_propagation(), 0.0) / 100.0 * 5.0;
    let seed = floor(f_seed() + 0.5);

    let bpm = clamp(f_motion_bpm(), 0.0, 260.0);
    let phase_time = t * (bpm / 60.0) * PI * 0.78 * f_speed();

    var best = 1e6;
    var best_i = 0.0;
    for (var i = 0; i < 32; i = i + 1) {
        if (f32(i) >= n) {
            break;
        }
        var pts: array<vec2<f32>, 7>;
        for (var j = 0; j < 7; j = j + 1) {
            let tt = f32(j) / 6.0;
            pts[j] = hl_point(
                system, f32(i), n, tt, radius, tension, phase_time,
                phase_offset, scale_step, rotation_step, displacement_norm,
                warp_norm, propagation_norm, spacing_norm, seed
            );
        }
        for (var j = 0; j < 6; j = j + 1) {
            let d = hl_seg(p, pts[j], pts[j + 1]);
            if (d < best) {
                best = d;
                best_i = f32(i);
            }
        }
    }

    // Pixel footprint keeps the line one pixel wide whatever the resolution.
    let px = max(max(fwidth(p.x), fwidth(p.y)), 1e-5);
    let half_w = px * (0.35 + f_line_width() * 0.35);
    let aa = max(px * 0.75, 1e-5);

    let line = 1.0 - smoothstep(half_w - aa, half_w + aa, best);
    let core = 1.0 - smoothstep(half_w * 0.12, half_w * 0.85, best);

    // Head anchor glow and the soft halo around every strand.
    let root_d = length(p);
    let root = exp(-root_d * root_d * 420.0);
    let halo = exp(-best * 220.0) * (1.0 - line);

    // Per-strand colour drift along the gradient, plus a little depth shading.
    let tint = fract(best_i * 0.173 + 0.31 + p.y * 0.12);
    var col = p_pal(0u, tint);
    let sheen = p_pal(2u, clamp(0.5 + p.y * 0.6 + best_i * 0.07, 0.0, 1.0));
    col = mix(col, sheen, core * 0.45);
    col = col + sheen * halo * 0.22;

    let root_col = p_pal(1u, clamp(root_d * 5.0, 0.0, 1.0));
    col = mix(col, root_col, root * 0.72);

    let root_cov = root * 0.55;
    let cov = clamp(max(line, root_cov), 0.0, 1.0);
    let denom = max(line + root_cov, 1e-3);
    let rgb = (col * line + root_col * root_cov) / denom;
    return vec4<f32>(rgb, cov);
}
