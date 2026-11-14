// dotgrid: a lattice of small shapes (circle, diamond, square, triangle,
// cross) on a background, each with its own size and opacity jitter. Behaviour
//; everything
// here is original.
//
// Cell (col,row) of pitch (gap_x, gap_y) holds one shape centred in the cell,
// optionally staggered (odd rows shift by desfase/2 cells) and scattered by a
// per-cell hash. Its radius is tam/2 shrunk by tam_rango*h, bent by a travelling
// sine wave (ola), a bump at the middle (lente) and a fall-off to the edge
// (vineta). Opacity drops by op_rango*h and twinkles (destello). The shape is a
// signed distance, rounded by `redondez`, rotated per dot by `giro_punto`, and
// drawn as an `trazo`-wide outline in slot 1 around a fill from slot 0. Because
// sizes and scatter can exceed a cell, the 3x3 neighbourhood is composited.

fn sd_shape(q: vec2<f32>, r: f32, kind: i32) -> f32 {
    if (kind == 0) { return length(q) - r; }
    if (kind == 1) { return (abs(q.x) + abs(q.y)) * 0.70710678 - r * 0.70710678 * 1.41421356 * 0.8; }
    if (kind == 2) { return max(abs(q.x), abs(q.y)) - r * 0.85; }
    if (kind == 3) {
        // Equilateral triangle, apex up, circumradius ~ r.
        let k = 1.7320508;
        let a = abs(q.x) * k * 0.5 + q.y * 0.5;
        return max(a, -q.y) - r * 0.5;
    }
    // Plus sign: union of two bars.
    let b = abs(q);
    let w = r * 0.32;
    return min(max(b.x - r, b.y - w), max(b.x - w, b.y - r));
}

fn dot_at(p: vec2<f32>, cell: vec2<f32>, shift: f32, gx: f32, gy: f32, acc: ptr<function, vec4<f32>>) {
    let h = p_hash2(cell + vec2<f32>(11.0, 5.0));
    let h2 = p_hash2(cell + vec2<f32>(71.0, 29.0));
    let t = p_time();

    var c = vec2<f32>((cell.x + 0.5) * gx + shift, (cell.y + 0.5) * gy);
    c = c + (h2 - 0.5) * vec2<f32>(gx, gy) * f_dispersion();

    // Radius.
    var r = 0.5 * f_size() * (1.0 - f_size_range() * h.x);
    let wph = dot(c, vec2<f32>(cos(f_wave_direction() * TAU), sin(f_wave_direction() * TAU))) * f_wave_freq() * 6.0 - t * f_wave_speed() * 2.0;
    r = r * max(1.0 + f_wave() * sin(wph), 0.0);
    let rad = length(c) / (0.5 * p_aspect() + 0.5);
    r = r * (1.0 + f_lens() * exp(-rad * rad * 3.0));
    r = r * mix(1.0, clamp(1.0 - rad * 0.8, 0.0, 1.0), f_vignette());
    if (r <= 1e-4) { return; }

    // Opacity.
    var op = 1.0 - f_opacity_range() * h.y;
    let tw = 0.5 + 0.5 * sin(t * f_flash_speed() * 4.0 + h2.y * TAU * 3.0);
    op = op * mix(1.0, tw, f_flash());

    // Local frame, rotated per dot.
    let ang = f_turn_dot() * TAU * (0.5 + h2.x) + f_turn_dot() * t * 0.4 * (h.x - 0.5);
    let q = p_rot(p - c, -ang);

    let kind = clamp(i32(floor(f_shape() + 0.5)), 0, 4);
    let red = f_roundness();
    let d = sd_shape(q, r * (1.0 - red * 0.9), kind) - r * red * 0.9;
    let aa = max(fwidth(d), 1e-4) + f_softness() * r;
    let outer = 1.0 - smoothstep(-aa, aa, d);
    if (outer <= 0.001) { return; }
    let inner = select(1.0 - smoothstep(-aa, aa, d + f_stroke()), 1.0, f_stroke() < 1e-4);

    let fill = p_pal(0u, clamp(f_mix_color() * h.x, 0.0, 1.0));
    let line = p_pal(1u, clamp(f_mix_color() * h2.y, 0.0, 1.0));
    let col = mix(line, fill, inner);

    let a = outer * op;
    let prev = *acc;
    *acc = vec4<f32>(prev.rgb * (1.0 - a) + col * a, prev.a * (1.0 - a) + a);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let gx = max(f_gap_x(), 0.01);
    let gy = max(f_gap_y(), 0.01);
    let off = vec2<f32>(f_speed_x(), f_speed_y()) * p_time() * 0.1;
    let s = p - off;
    var acc = vec4<f32>(0.0);

    let row0 = floor(s.y / gy);
    for (var j = -1; j <= 1; j = j + 1) {
        let row = row0 + f32(j);
        let odd = row - 2.0 * floor(row * 0.5);
        let shift = f_shift() * 0.5 * gx * odd;
        let col0 = floor((s.x - shift) / gx);
        for (var i = -1; i <= 1; i = i + 1) {
            dot_at(s, vec2<f32>(col0 + f32(i), row), shift, gx, gy, &acc);
        }
    }
    let cov = clamp(acc.a, 0.0, 1.0);
    return vec4<f32>(acc.rgb / max(cov, 1e-4), cov);
}
