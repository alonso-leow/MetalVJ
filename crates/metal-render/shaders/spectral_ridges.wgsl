// spectral-ridges: a scrolling spectrogram as stacked ridge lines. Original
// WGSL; the parameter list follows the built-in Spectrogram tool's ridge field
// view (columns, rows, height, release, hill width, round-to-square, stroke,
// zoom / rotate / tilt / offsets, fade, colour-by). There is no audio texture:
// the band energies are a hashed field in column/frequency space with a
// temporal release tail, then each band's ridge rises inside its own row.
// Everything in the frame's outer edge is left transparent so the scaffold's
// `fondo` background (and background_level) shows through.

// Energy of one column/frequency band, with an exponential tail over the
// previous columns (release) and small per-band motion (drift, shimmer).
fn sr_energy(ci: f32, bi: f32, t: f32) -> f32 {
    let sd = f_seed() * 13.7;
    let dec = mix(3.2, 0.10, clamp(f_release(), 0.0, 1.0));

    var acc = 0.0;
    var wsum = 0.0;
    for (var k: i32 = 0; k < 6; k = k + 1) {
        let kf = f32(k);
        let h = p_hash(vec2<f32>(ci - kf, bi * 1.7 + sd + kf * 2.3));
        let wk = exp(-kf * dec);
        acc = acc + h * wk;
        wsum = wsum + wk;
    }
    var e = acc / max(wsum, 0.0001);

    // Neighbouring frequency bands share some energy, so a ridge has width.
    let n1 = p_hash(vec2<f32>(ci * 0.7, (bi + 1.0) * 1.7 + sd));
    let n2 = p_hash(vec2<f32>(ci * 0.7, (bi - 1.0) * 1.7 + sd));
    e = e * 0.68 + (n1 + n2) * 0.16;

    // Slow per-band drift and fast shimmer.
    let dr = sin(t * (0.6 + f_speed() * 0.35) + bi * 1.1 + ci * 0.13);
    e = e + dr * clamp(f_ridge_drift(), 0.0, 1.0) * 0.22;
    e = e + clamp(f_shimmer(), 0.0, 1.0) * 0.18
        * sin(t * 6.0 + bi * 2.1 + ci * 0.9);

    // Gain, noise floor and ceiling compression.
    e = e * (0.35 + 1.9 * clamp(f_gain(), 0.0, 2.0));
    e = mix(e, 1.0, clamp(f_floor(), 0.0, 1.0) * 0.30);
    let gam = 1.0 + clamp(f_ceiling(), 0.0, 1.0) * 2.4;
    e = pow(clamp(e, 0.0, 1.0), gam);
    return clamp(e, 0.0, 1.0);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let asp = max(p_aspect(), 0.01);
    let cols = max(floor(f_columns() + 0.5), 2.0);
    let rows = max(floor(f_rows() + 0.5), 2.0);

    // Spectrogram plane: zoom, rotate, tilt (compress + shear), then offset.
    var q = p / max(f_zoom_field(), 0.15);
    q = p_rot(q, f_yaw() * PI);
    q.y = q.y * (1.0 + f_tilt() * 0.9);
    q.x = q.x + q.y * f_tilt() * 0.5;
    q = q + vec2<f32>(f_offset_x(), f_offset_y());

    let sx = q.x / asp + 0.5;
    let sy = q.y + 0.5;

    // Continuous column coordinate, scrolled by time.
    let scroll = t * f_speed() * (2.0 + cols * 0.15);
    let cxf = sx * cols - scroll;
    let ci = floor(cxf);
    let fx = fract(cxf);

    let by = sy * rows;
    let bi = floor(by);
    let fy = fract(by);

    // Blend two columns so the ridges join horizontally.
    let e0 = sr_energy(ci, bi, t);
    let e1 = sr_energy(ci + 1.0, bi, t);
    let e = mix(e0, e1, smoothstep(0.0, 1.0, fx));

    // The ridge rises from the band's bottom (fy = 1) toward its top.
    let amp = clamp(f_height(), 0.0, 1.0) * e;
    let wob = clamp(f_ridge_drift(), 0.0, 1.0) * 0.16
        * sin(t * 0.9 + bi * 1.7 + ci * 0.21);
    let yline = 1.0 - amp + wob;
    let d = fy - yline;

    // Hill profile: rounded gaussian morphing to a square plateau.
    let hw = 0.012 + clamp(f_hill_width(), 0.0, 1.0) * 0.38;
    let roundp = exp(-(d * d) / (hw * hw));
    let squarep = 1.0 - smoothstep(hw * 0.85, hw * 1.05, abs(d));
    let prof = mix(roundp, squarep, clamp(f_squareness(), 0.0, 1.0));

    // Crest stroke: a thin bright line, thickness set by stroke.
    let sw = 0.040 + clamp(f_stroke(), 0.5, 8.0) * 0.030;
    let crest = exp(-(d * d) / (sw * sw));

    // Column seams and band baseline, so the grid reads as columns x rows.
    let seam = smoothstep(0.0, 0.16, min(fx, 1.0 - fx));
    let bd = (fy - 1.0) * 12.0;
    let baseline = exp(-bd * bd);

    // Colour: level (energy) or frequency (band position).
    let band_pos = clamp((bi + fy) / rows, 0.0, 1.0);
    let tc = clamp(mix(e, band_pos, clamp(f_color_by(), 0.0, 1.0)), 0.0, 1.0);
    let fillc = p_pal(0u, tc);
    let voidc = p_pal(1u, clamp(0.10 + 0.75 * band_pos, 0.0, 1.0));

    var col = voidc;
    col = mix(col, fillc, clamp(e * (0.16 + 0.9 * prof), 0.0, 1.0));
    col = col * mix(0.78, 1.0, seam);
    col = col * (1.0 - 0.22 * baseline);

    // Floor lifts a faint row baseline.
    col = col + p_pal(2u, 0.25) * baseline * clamp(f_floor(), 0.0, 1.0) * 0.45;

    // Glow halo and the sharp crest both use the crest slot.
    let halo = exp(-abs(d) / max(hw * 0.75, 0.001)) * e;
    let glowq = clamp(f_glow(), 0.0, 1.0);
    col = col + p_pal(2u, tc) * halo * glowq * 0.55;
    col = col + p_pal(2u, tc) * crest * (0.30 + glowq * 1.55);

    // Moving scan band across the screen, independent of the camera.
    let scan_x = fract(t * 0.11 + 0.5);
    let sd = uv.x - scan_x;
    let scan_band = exp(-(sd * sd) / 0.0025);
    col = col + p_pal(2u, 0.4 + 0.4 * sin(t * 1.7))
        * scan_band * clamp(f_scan(), 0.0, 1.0) * 0.9;

    // Old (left) columns fade toward the void.
    let fade = mix(1.0 - clamp(f_fade(), 0.0, 1.0), 1.0, smoothstep(0.0, 1.0, uv.x));
    col = col * fade;

    // Soft film grain keeps flat areas alive.
    col = col + vec3<f32>(p_grain(uv, t) * 0.035);

    // Deliberate transparent outer edge: background_level is visible there.
    let dx = abs(p.x) / max(asp * 0.5, 0.001);
    let dy = abs(p.y) / 0.5;
    let edge = max(dx, dy);
    let cov = 1.0 - smoothstep(0.82, 1.0, edge);

    return vec4<f32>(max(col, vec3<f32>(0.0)) * cov, cov);
}
