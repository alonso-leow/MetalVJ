// cymatics: a Chladni plate. Original WGSL; the parameter list follows the
// built-in Chladni sketch. The plate is a square modal field; sand settles on
// the nodal lines (where the displacement crosses zero), with an optional
// n-fold rotational symmetry, a slow drift and a few extra ripple modes.

fn cy_mode(u: vec2<f32>, t: f32, hx: f32, hy: f32) -> f32 {
    let fq = max(f_frequency(), 0.25);
    let m = 1.0 + fq * 0.58 + 0.22 * sin(t * 0.23);
    let n = 1.0 + fq * 0.41 + 0.18 * cos(t * 0.19 + 1.3);
    let pat = clamp(f_pattern(), 0.0, 5.0);

    let x = u.x * 0.5 + 0.5 + (hx - 0.5) * 0.30;
    let y = u.y * 0.5 + 0.5 + (hy - 0.5) * 0.30;

    let a = cos(m * PI * x) * cos(n * PI * y);
    let b = cos(n * PI * x) * cos(m * PI * y);
    let c = cos((m + 1.0) * PI * x + 0.5 * sin(n * PI * y));
    let d = cos((n + 1.0) * PI * y + 0.5 * sin(m * PI * x));

    var z = a - b;
    if (pat < 0.5) {
        z = a - b;
    } else if (pat < 1.5) {
        z = a + b;
    } else if (pat < 2.5) {
        let r = length(vec2<f32>(x - 0.5, y - 0.5)) * 2.0;
        z = cos(m * PI * r) - 0.65 * cos(n * PI * x) * cos(n * PI * y);
    } else if (pat < 3.5) {
        z = cos(m * PI * x + n * PI * y) - cos(n * PI * x - m * PI * y);
    } else if (pat < 4.5) {
        z = a * c - b * d;
    } else {
        let h1 = p_hash(vec2<f32>(f_seed() * 13.0, 3.0));
        let h2 = p_hash(vec2<f32>(f_seed() * 7.0, 11.0));
        z = a - b + (h1 - 0.5) * c + (h2 - 0.5) * d;
    }
    return z;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time() * f_speed();
    let hx = clamp(f_handle_x() * 0.01, 0.0, 1.0);
    let hy = clamp(f_handle_y() * 0.01, 0.0, 1.0);
    let vib = clamp(f_vibration() * 0.01, 0.0, 1.0);
    let wv = clamp(f_waves() * 0.01, 0.0, 1.0);

    var u = p * 2.0;
    u = p_rot(u, f_rotation() * PI + f_sway() * t * 0.18);
    u = u + wv * 0.09 * vec2<f32>(
        sin(u.y * 3.0 * PI + t * 0.9),
        cos(u.x * 3.0 * PI - t * 0.7)
    );
    u = u + vib * 0.012 * vec2<f32>(
        sin(t * 5.7 + u.y * 17.0),
        cos(t * 6.1 + u.x * 19.0)
    );

    let sym = clamp(floor(f_symmetry() + 0.5), 1.0, 8.0);
    var field = 0.0;
    for (var k: i32 = 0; k < 8; k = k + 1) {
        if (f32(k) < sym) {
            let r = p_rot(u, f32(k) * TAU / sym);
            field = field + cy_mode(r, t, hx, hy);
        }
    }
    field = field / sym;
    field = field + wv * 0.25 * sin((u.x + u.y) * 4.0 * PI + t * 1.3);

    // A rotated square plate; outside it the scaffold's background shows.
    let plate = 1.0 - smoothstep(0.98, 1.06, max(abs(u.x), abs(u.y)));

    let az = abs(field);
    let lw = clamp(f_line_width(), 0.0, 1.0);
    let nodal = exp(-az * mix(24.0, 7.0, lw));
    let hot = exp(-az * 6.0) * clamp(f_glow(), 0.0, 1.0);

    // Sand: hash-distributed grains in screen space, denser on nodal lines.
    let gsz = max(f_grain_size(), 0.5);
    let gc = uv * vec2<f32>(max(p_aspect(), 1.0), 1.0) * (320.0 / gsz);
    let cell = floor(gc);
    let fp = fract(gc) - vec2<f32>(0.5);
    let sc = clamp(f_scatter(), 0.0, 1.0);
    let jit = (p_hash2(cell + vec2<f32>(f_seed() * 13.1, f_seed() * 7.3)) - vec2<f32>(0.5))
        * (0.42 + 0.58 * sc);
    let dist = length(fp - jit);
    let grain = 1.0 - smoothstep(0.10, 0.30, dist);

    let amount = clamp(f_grain_amount(), 0.0, 1.0);
    let hp = p_hash(cell + vec2<f32>(f_seed() * 3.7, 9.1));
    let present = step(1.0 - amount, hp);
    let sand = grain * present;
    let dust = grain * step(1.0 - sc * 0.35, p_hash(cell + vec2<f32>(71.0, f_seed())));

    let grain_tint = clamp(0.15 + 0.70 * hp + 0.15 * sin(t * 0.7 + cell.x * 0.1), 0.0, 1.0);
    let spark = clamp(0.5 + 0.5 * field, 0.0, 1.0);

    var ink = p_pal(0u, grain_tint) * (nodal * (0.35 + 0.85 * sand) + dust * 0.30) * 1.55;
    ink = ink + p_pal(1u, spark) * (hot * 0.75 + nodal * sand * 0.22);

    let cover = plate * clamp(nodal * (0.25 + 0.75 * sand) + dust * 0.25 + hot * 0.45, 0.0, 1.0);
    return vec4<f32>(ink * plate, cover);
}
