// ripples: rows of undulating lines, written from first principles.
//
//
// The picture plane is rotated (angulo), optionally bent into rings (radial),
// perspective-squeezed (inclina) and sheared (curva). Row i sits at i*pitch
// and is displaced by a wave whose phase depends on x, time and the row
// (fase_y, desfase_par, cruce braiding). The wave shape (forma) morphs between
// sine, zigzag, soft square and noise; armonicos adds a second harmonic and
// ruido a wobble. Because rows can cross into their neighbours, four candidate
// rows are tested per pixel and the nearest wins. Slot 0 draws the rows (graded
// by degrade), slot 1 tints the band between rows (relleno); anything left
// uncovered shows the fondo.

fn wv_tri(x: f32) -> f32 {
    return 1.0 - 4.0 * abs(fract(x / TAU + 0.25) - 0.5);
}

fn wv_shape(k: i32, ph: f32, row: f32) -> f32 {
    if (k <= 0) { return sin(ph); }
    if (k == 1) { return wv_tri(ph); }
    if (k == 2) { return clamp(sin(ph) * 3.0, -1.0, 1.0); }
    return p_noise(vec2<f32>(ph / TAU * 1.7, row * 0.37 + 5.0)) * 2.0 - 1.0;
}

// Displacement (in picture units) of row `row` at horizontal position x.
fn wv_disp(x: f32, y: f32, row: f32, t: f32, pitch: f32) -> f32 {
    let par = select(0.0, PI, (i32(abs(row)) & 1) == 1);
    var ph = x * f_frequency() * TAU * 3.0 + t * f_speed() * 1.5
        + row * f_phase_y() * 1.2 + par * f_shift_par();
    ph = ph + f_crossing() * 2.0 * sin(y / pitch * 0.8 + t * 0.5);
    let fm = clamp(f_shape(), 0.0, 3.0);
    let k = i32(floor(fm));
    let m = smoothstep(0.0, 1.0, fm - floor(fm));
    var s = wv_shape(k, ph, row);
    if (m > 0.001) { s = mix(s, wv_shape(min(k + 1, 3), ph, row), m); }
    s = s + f_harmonics() * 0.6 * sin(2.03 * ph + row * 1.3);
    s = s / (1.0 + 0.6 * f_harmonics());
    s = s + f_noise() * 1.6 * (p_noise(vec2<f32>(x * (0.5 + f_noise_freq() * 8.0), row * 0.7 + t * 0.2)) - 0.5);
    let env = 1.0 - f_modulation() + f_modulation() * (0.5 + 0.5 * sin(x * 1.7 + t * 0.3 + row * 0.2));
    let br = 1.0 + f_breath() * 0.6 * sin(t * 1.3);
    return s * f_amplitude() * 0.12 * env * br + f_curve() * x * x * 0.5;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    var q = p_rot(p, f_angle() * TAU);
    let rr = length(p);
    q.y = mix(q.y, rr, f_radial());
    let den = max(1.0 + f_tilt() * 0.8 * (q.y - 0.5), 0.25);
    let yy = q.y / den;
    let aa = max(fwidth(yy), 1e-4);
    let pitch = 0.08 * max(f_spacing(), 0.1);

    let w0 = wv_disp(q.x, yy, 0.0, t, pitch);
    let base = floor((yy - w0) / pitch);

    var best_d = 1e9;
    var best_row = 0.0;
    var best_s = 0.0;
    for (var k = -1; k <= 2; k = k + 1) {
        let row = base + f32(k);
        let c = row * pitch + wv_disp(q.x, yy, row, t, pitch);
        let sd = yy - c;
        if (abs(sd) < best_d) {
            best_d = abs(sd);
            best_row = row;
            best_s = sd;
        }
    }

    let tv = 1.0 + f_thickness_variation() * 1.6 * (p_noise(vec2<f32>(q.x * 3.0 + t * 0.3, best_row * 0.9)) - 0.5);
    let thick = (0.15 + f_proportion() * 0.85) * pitch * 0.5 * max(tv, 0.05);
    let soft = aa + f_softness() * thick * 1.5;
    var line = 1.0 - smoothstep(thick - soft, thick + soft, best_d);
    let ring = (1.0 - smoothstep(0.0, soft + thick * 0.3, abs(best_d - thick)));
    line = mix(line, ring, f_outline());

    // Dashes along the row.
    let dash = smoothstep(0.35, 0.65, abs(fract(q.x * (2.0 + f_dash_freq() * 18.0) + best_row * 0.37) - 0.5) * 2.0);
    line = line * mix(1.0, 1.0 - dash, f_dash());
    // Per-row flicker.
    let fl = p_hash(vec2<f32>(best_row, floor(t * 6.0)));
    line = line * (1.0 - f_blink() * fl);
    // Fade towards the frame edges.
    let ed = 1.0 - f_fade() * smoothstep(0.25, 0.85, abs(uv.x - 0.5) * 2.0);
    line = line * ed;

    let tcol = clamp(f_erode() * (0.5 + 0.5 * sin(best_row * 0.5 + q.x * 1.5 + t * 0.3)), 0.0, 1.0);
    let lc = p_pal(0u, tcol);

    // Band between rows: tint by position inside the band, shadow under rows.
    let inb = clamp(0.5 + best_s / pitch, 0.0, 1.0);
    let bandc = p_pal(1u, fract(inb + best_row * 0.1));
    let sh = f_shadow() * exp(-max(best_s, 0.0) / (pitch * 0.45)) * step(0.0, best_s) * (1.0 - line);
    let fillc = bandc * (1.0 - 0.8 * sh);
    let fillcov = max(f_fill(), sh * 0.85);
    let col = mix(fillc, lc, line);
    let cov = max(line, fillcov);
    return vec4<f32>(col, clamp(cov, 0.0, 1.0));
}
