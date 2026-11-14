// inkline: a sheet of hand-inked doodle strokes. Behaviour and controls follow
// the built-in Doodle tool (pencil / chalk / watercolour / paint, boil flicker,
// stroke thickness and softness, shape stamps); this WGSL is original.
//
// Every mark is a distance-to-curve field. Free mode lays down a family of
// tapered, wavy strokes; stamp mode turns the same machinery on the outline of
// a circle, rectangle or triangle. Boil is a discrete per-tick variant chosen
// from `variations`, exactly like the tool's flicker. Brush families differ in
// how the coverage is broken up: pencil grain, chalk dryness, watercolour
// pooling and paint bristles. All of it is evaluated per pixel, no paths.

fn ink_mod(a: f32, b: f32) -> f32 {
    return a - floor(a / b) * b;
}

fn ink_seg(pp: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = pp - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn ink_box(pp: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = abs(pp) - b;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
}

struct InkHit {
    d: f32,
    hw: f32,
    u: f32,
    id: f32,
};

fn ink_free(q: vec2<f32>, i: f32, t: f32) -> InkHit {
    let sd = f_seed();
    let vn = max(floor(f_variations() + 0.5), 2.0);
    let tick = floor(t * abs(f_speed()));
    let bidx = ink_mod(tick, vn);

    let a0 = p_hash(vec2<f32>(i * 2.37 + sd * 0.71, sd * 1.13));
    let a1 = p_hash(vec2<f32>(i * 4.11 + sd * 1.37, 11.0 + sd * 0.29));
    let a2 = p_hash(vec2<f32>(i * 7.73 + sd * 2.11, 23.0 + sd * 0.53));
    let b0 = p_hash(vec2<f32>(i * 5.19 + sd * 3.71, bidx * 3.17 + tick * 0.73 + vn * 11.0));
    let b1 = p_hash(vec2<f32>(i * 8.23 + sd * 1.97, bidx * 5.31 + tick * 1.13 + vn * 7.0));
    let b2 = p_hash(vec2<f32>(i * 3.31 + sd * 4.13, bidx * 7.91 + tick * 0.37 + vn * 3.0));

    let count = max(floor(f_count() + 0.5), 1.0);
    let lane = ((i + 0.5) / count - 0.5) * f_span();
    let amp = f_wave() * (0.35 + 0.9 * a1);
    let frq = f_frequency() * TAU * (0.55 + 0.9 * a2);
    let ph = a0 * TAU + (b0 - 0.5) * f_wobble() * 2.4;

    let x = q.x + (b2 - 0.5) * f_wobble() * 0.05;
    let y0 = lane + (b1 - 0.5) * f_wobble() * 0.035;
    let y = y0 + amp * sin(x * frq + ph) + amp * 0.33 * sin(x * frq * 2.7 + ph * 1.7);
    let dy = amp * frq * cos(x * frq + ph)
        + amp * 0.33 * 2.7 * frq * cos(x * frq * 2.7 + ph * 1.7);

    let ext = 0.56 * p_aspect() + 0.18;
    let x0 = -ext + a1 * 0.34 * ext;
    let x1 = ext - a2 * 0.34 * ext;
    let raw = (q.x - x0) / max(x1 - x0, 1e-3);
    let inside = step(0.0, raw) * step(raw, 1.0);
    let u = clamp(raw, 0.0, 1.0);

    let pointy = pow(max(sin(PI * u), 0.0), 0.35 + f_pressure() * 1.7);
    let prof = mix(0.22, pointy, clamp(f_taper(), 0.0, 1.0));
    let w = max(f_thickness() * prof, 0.0008);
    let d = abs(y - q.y) / sqrt(1.0 + dy * dy) + (1.0 - inside) * 8.0;
    return InkHit(d, w * 0.5, u, i);
}

fn ink_shape(q: vec2<f32>, shape_mode: i32, t: f32) -> InkHit {
    let sd = f_seed();
    let vn = max(floor(f_variations() + 0.5), 2.0);
    let tick = floor(t * abs(f_speed()));
    let bidx = ink_mod(tick, vn);
    let b0 = p_hash(vec2<f32>(sd * 11.0 + f32(shape_mode), bidx * 4.1 + tick * 0.9 + vn * 5.0));
    let b1 = p_hash(vec2<f32>(sd * 5.0 + 7.0, bidx * 2.3 + tick * 1.7 + vn * 2.0));
    let ph = b0 * TAU;
    let rad = 0.16 + 0.18 * f_span();
    let amt = f_wave() * 0.8;
    let pp = q + vec2<f32>(
        sin(q.y * f_frequency() * TAU + ph),
        cos(q.x * f_frequency() * TAU + ph * 1.3),
    ) * amt * 0.12 * (0.5 + b1);

    var d = 20.0;
    if (shape_mode == 1) {
        d = abs(length(pp) - rad);
    } else if (shape_mode == 2) {
        d = abs(ink_box(pp, vec2<f32>(rad * 1.25, rad * 0.9)));
    } else {
        let rt = rad * 1.15;
        let p0 = vec2<f32>(0.0, -rt);
        let p1 = vec2<f32>(rt * 0.866, rt * 0.5);
        let p2 = vec2<f32>(-rt * 0.866, rt * 0.5);
        d = min(ink_seg(pp, p0, p1), min(ink_seg(pp, p1, p2), ink_seg(pp, p2, p0)));
    }

    let pressure = mix(0.55, 0.95 + 0.15 * sin(PI * clamp((q.x + 0.7) / 1.4, 0.0, 1.0)), f_pressure());
    let prof = mix(pressure, 1.0, f_taper());
    let w = max(f_thickness() * prof, 0.0008);
    return InkHit(d, w * 0.5, 0.5, 0.0);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let sd = f_seed();
    let q = p_rot(p, f_angle());
    let shape_mode = i32(floor(clamp(f_shape(), 0.0, 3.0) + 0.5));

    var hit = InkHit(20.0, 0.005, 0.5, 0.0);
    if (shape_mode == 0) {
        let cn = i32(max(floor(f_count() + 0.5), 1.0));
        for (var i: i32 = 0; i < cn; i = i + 1) {
            let s = ink_free(q, f32(i), t);
            if (s.d < hit.d) {
                hit = s;
            }
        }
    } else {
        hit = ink_shape(q, shape_mode, t);
    }

    let brush = i32(floor(clamp(f_brush(), 0.0, 3.0) + 0.5));
    let d = hit.d;
    let hw = hit.hw;
    let soft = clamp(f_softness(), 0.0, 1.0);
    let aa = max(0.0012, hw * 0.18 + soft * 0.012);
    let base = 1.0 - smoothstep(hw - aa, hw + aa, d);

    let tex = max(f_texture(), 1.0);
    let n1 = p_noise(uv * tex * 22.0 + vec2<f32>(sd * 13.1 + hit.id * 1.7, sd * 7.7));
    let n2 = p_noise(uv * tex * 5.0 + vec2<f32>(3.3, sd * 2.1));
    let fib = 0.5 + 0.5 * sin((uv.x * 0.8 + uv.y * 0.35) * tex * 14.0 + n2 * 7.0);

    let grain = clamp(f_grain(), 0.0, 1.0);
    let dry = clamp(f_dry(), 0.0, 1.0);
    let bristle = clamp(f_bristle(), 0.0, 1.0);
    let wash = clamp(f_wash(), 0.0, 1.0);
    let bleed = clamp(f_bleed(), 0.0, 1.0);
    let splat = clamp(f_splatter(), 0.0, 1.0);

    let breakup = mix(1.0, smoothstep(0.18, 0.72, n1), grain * 0.75 + dry * 0.35);

    var body = base;
    var rim = 0.0;
    var gloss = 0.0;
    if (brush == 0) {
        body = base * (0.42 + 0.58 * breakup);
        rim = exp(-max(d - hw, 0.0) * 22.0) * 0.25;
    } else if (brush == 1) {
        let chalk = smoothstep(0.28, 0.66, n1 * (0.6 + 0.4 * fib));
        body = base * (0.25 + 0.75 * chalk) * (0.75 + 0.25 * fib);
        rim = exp(-max(d - hw, 0.0) * 12.0) * 0.35;
    } else if (brush == 2) {
        let wide = 1.0 - smoothstep(hw * 0.45, hw * 1.35 + soft * 0.02, d);
        body = mix(wide * 0.55, base * 0.65, 0.45);
        rim = exp(-abs(d - hw * 0.95) * 18.0) * 0.65;
    } else {
        body = base * (0.82 + 0.18 * fib);
        rim = exp(-abs(d - hw * 1.08) * 30.0) * 0.85;
        gloss = exp(-abs(d - hw * 0.35) * 40.0) * bristle * 0.5;
    }

    body = body * (0.78 + 0.22 * mix(1.0, fib, bristle));
    body = body * mix(1.0, breakup, dry * 0.6 + grain * 0.4);

    let halo = exp(-max(d - hw, 0.0) * (9.0 + 18.0 * (1.0 - soft)));
    let wash_amt = halo * wash * (0.35 + 0.65 * n2) * (0.4 + 0.6 * bleed);
    let bleed_amt = halo * bleed * 0.3;

    let gs = 45.0 + tex * 4.0;
    let cell = floor(uv * gs) + vec2<f32>(hit.id * 37.0, sd * 11.0);
    let rnd = p_hash(cell);
    let rnd2 = p_hash(cell + vec2<f32>(19.0, 51.0));
    let dot_r = (0.12 + 0.55 * rnd2) * (0.004 + f_thickness() * 0.25);
    let fp = fract(uv * gs) - vec2<f32>(0.5);
    let spot = 1.0 - smoothstep(dot_r * gs * 0.7, dot_r * gs, length(fp));
    let near = exp(-max(d - hw, 0.0) * 16.0);
    let dots = spot * step(0.86 - 0.35 * splat, rnd) * near * splat;

    let paper_col = p_pal(1u, clamp(0.28 + 0.5 * n2 + 0.12 * (uv.y - 0.5), 0.0, 1.0));
    let ink_col = p_pal(0u, clamp(0.15 + 0.7 * hit.u + 0.15 * n1, 0.0, 1.0));
    let wash_col = p_pal(2u, clamp(0.2 + 0.7 * n2, 0.0, 1.0));

    let amt = clamp(body + rim * 0.4 + dots, 0.0, 1.0) * clamp(f_ink_opacity(), 0.0, 1.0);
    var c0 = paper_col * (0.93 + 0.07 * fib);
    c0 = mix(c0, wash_col, clamp(wash_amt * 0.55 + bleed_amt * 0.35, 0.0, 0.8));
    c0 = mix(c0, ink_col, amt);
    c0 = mix(c0, paper_col * 1.02, clamp(gloss, 0.0, 0.5));

    let edge_a = 1.0 - smoothstep(0.47, 0.5, max(abs(uv.x - 0.5), abs(uv.y - 0.5)));
    return vec4<f32>(c0, edge_a);
}
