// deep-tunnel: a 2D projection of a receding polygonal tunnel. The screen
// radius is read as depth, the angle as the position around the wall, so rings
// bunch toward the opening and rays fan out from it. The upstream tool builds
// a 3D mesh; this is a separate flat-field construction with the same controls.
//
// Every style shares one wall/ring/ray field. `style` changes the balance of
// the treatments (wireframe, gradient, spiral, star streaks, grid, alien
// corridor), but each treatment stays faintly present, so its controls always
// alter the frame. Coverage fades at the far end, at the wall boundary and at
// the screen edge, leaving the backdrop visible.

fn dt_shape(shape: f32, ang: f32) -> vec2<f32> {
    var rr = 1.0;
    if ((shape > 0.5 && shape < 3.5) || shape > 4.5) {
        var sides = 4.0;
        var off = PI * 0.25;
        if (shape > 1.5 && shape < 2.5) {
            sides = 3.0;
            off = PI * 0.5;
        }
        if (shape > 2.5 && shape < 3.5) {
            sides = 6.0;
            off = PI / 6.0;
        }
        let aa = ang + off;
        let pp = TAU / sides;
        let mm = aa - pp * floor(aa / pp);
        rr = cos(PI / sides) / cos(mm - pp * 0.5);
    }
    var v = rr * vec2<f32>(cos(ang), sin(ang));
    if (shape > 3.5) {
        v = v * vec2<f32>(1.4, 0.7);
    }
    return v;
}

fn dt_radius(shape: f32, ang: f32) -> f32 {
    return max(length(dt_shape(shape, ang)), 0.2);
}

fn dt_line_cov(coord: f32, wpx: f32) -> f32 {
    let w = max(fwidth(coord), 1e-6);
    let d = abs(fract(coord + 0.5) - 0.5) / w;
    let aa = clamp(wpx * 0.5 - d + 0.5, 0.0, 1.0);
    return mix(aa, min(1.0, wpx * w), smoothstep(0.3, 0.7, w));
}

fn dt_line_dist(coord: f32) -> f32 {
    return abs(fract(coord + 0.5) - 0.5) / max(fwidth(coord), 1e-6);
}

fn dt_mode(style: f32, want: f32) -> f32 {
    return 1.0 - step(0.5, abs(style - want));
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let style = f_style();
    let shape = f_shape();
    let seed = f_seed();

    let lines_mode = dt_mode(style, 0.0);
    let spiral_mode = dt_mode(style, 2.0);
    let warp_mode = dt_mode(style, 3.0);
    let grid_mode = dt_mode(style, 4.0);
    let alien_mode = dt_mode(style, 5.0);

    let sp = clamp(f_speed(), 0.0, 100.0) / 100.0;
    let rings = clamp(f_rings(), 0.0, 100.0) / 100.0;
    let zstep = 0.25 + (1.0 - rings) * 2.25;
    let zmax = 4.0 + clamp(f_amplitude(), 0.0, 100.0) / 100.0 * 31.0;
    let ray_n = max(1.0, floor(1.0 + clamp(f_rays(), 0.0, 100.0) / 100.0 * 7.0)) * 12.0;

    // First pass at depth drives the sideways/vertical bend of the far end.
    let r0 = max(length(p), 1e-4);
    let nz0 = clamp((1.0 / (r0 + 0.05)) / zmax, 0.0, 1.0);
    let bend = vec2<f32>(clamp(f_bend_x(), -100.0, 100.0), clamp(f_bend_y(), -100.0, 100.0)) / 100.0;
    let q = p - bend * 0.55 * nz0 * nz0;

    let r = max(length(q), 1e-4);
    let ang = atan2(q.y, q.x);
    let sr = dt_radius(shape, ang) * 0.52;
    let rn = r / sr;
    let z = 1.0 / max(rn + 0.04, 1e-3);
    let nz = clamp(z / zmax, 0.0, 1.0);
    let zlo = min(zmax * 0.5, 1.3);
    let depth = clamp(log(max(z, zlo) / zlo) / max(log(zmax / zlo), 1e-4), 0.0, 1.0);

    // Travelling ring index and around-the-wall index.
    let travel = t * sp * 3.0 / max(zstep, 0.05);
    let ring_c = z / zstep + travel + seed * 0.37;
    let ray_c = (ang / TAU + 0.5) * ray_n + seed * 0.11;
    let k = floor(ring_c);
    let fr = fract(ring_c);
    let k_h = k - 256.0 * floor(k / 256.0);
    let j = floor(ray_c);
    let fx = fract(ray_c);
    let j_h = j - ray_n * floor(j / ray_n);

    // Wall lighting: shape normal against a fixed key, scaled by shading.
    let eps = 0.002;
    let sp0 = dt_shape(shape, ang - eps);
    let sp1 = dt_shape(shape, ang + eps);
    let tanv = sp1 - sp0;
    var nrm = vec2<f32>(tanv.y, -tanv.x);
    if (dot(nrm, sp0) < 0.0) {
        nrm = -nrm;
    }
    let lit = dot(normalize(nrm), normalize(vec2<f32>(0.3, 1.0))) * 0.5 + 0.5;
    let shade = 1.0 - clamp(f_shading(), 0.0, 100.0) / 100.0 * 0.6 * (1.0 - lit);

    // Gradient treatment: depth ramp, optionally repeating and stepped.
    let grepeat = clamp(f_grad_repeat(), 0.0, 10.0);
    var gpos = depth;
    if (grepeat > 0.05) {
        let period = max((zmax - 0.1) / grepeat, 0.01);
        gpos = fract((z - 0.1) / period + 0.2 + seed * 0.013);
    }
    let gsteps = max(1.0, floor(clamp(f_grad_steps(), 0.0, 100.0) / 100.0 * 16.0 + 1.5));
    let gq = mix(gpos, floor(gpos * gsteps) / gsteps, clamp(f_grad_steps(), 0.0, 100.0) / 100.0);
    let odd = step(0.5, k - 2.0 * floor(k / 2.0));
    let stripe = 1.0 - 0.7 * clamp(f_grad_stripes(), 0.0, 100.0) / 100.0 * odd;
    var col = p_pal(0u, clamp(gq, 0.0, 1.0)) * (0.55 + 0.45 * shade) * stripe;

    // Spiral treatment: arms winding down the depth, with optional band gaps.
    let arms = max(1.0, floor(f_spiral_arms() + 0.5));
    let twist = clamp(f_spiral_twist(), -100.0, 100.0) / 100.0 * 2.0;
    let spin = clamp(f_spiral_spin(), -100.0, 100.0) / 100.0 * 0.5;
    let turb = clamp(f_spiral_turb(), 0.0, 100.0) / 100.0;
    let zz = max(z - 0.1, 0.0);
    var phase = (ang / TAU + 0.5) * arms + twist * zz + arms * spin * t * 2.0 + seed * 0.17;
    if (turb > 0.001) {
        let nn = sin((ang / TAU + 0.5) * TAU * 2.0 + zz * 1.7 + t * 0.9) * 0.4
            + sin((ang / TAU + 0.5) * TAU * 5.0 - zz * 3.1 + t * 1.3) * 0.25
            + sin((ang / TAU + 0.5) * TAU * 9.0 + zz * 6.3 - t * 1.9) * 0.15
            + sin(zz * 4.1 - t * 0.7) * 0.2;
        phase = phase + turb * nn * 0.45;
    }
    let band_i = floor(phase);
    let bf = fract(phase);
    let band = clamp(f_spiral_band(), 5.0, 100.0) / 100.0;
    let bsoft = 0.02 + clamp(f_spiral_soft(), 0.0, 100.0) / 100.0 * band * 0.4;
    var bcov = 1.0;
    if (band < 0.999) {
        bcov = smoothstep(0.0, bsoft, bf) * (1.0 - smoothstep(band - bsoft, band, bf));
    }
    let flat_col = p_pal(0u, fract(band_i / arms + seed * 0.03));
    let sweep_col = p_pal(0u, clamp(bf / band, 0.0, 1.0));
    let spiral_col = mix(flat_col, sweep_col, clamp(f_spiral_blend(), 0.0, 100.0) / 100.0);
    col = mix(col, spiral_col, clamp(bcov, 0.0, 1.0) * (0.18 + 0.55 * spiral_mode));

    // Star streaks: cell in (around, depth), elongated along the travel.
    let star_density = clamp(f_stars(), 0.0, 100.0) / 100.0;
    let streak_len = clamp(f_streak_len(), 0.0, 100.0) / 100.0;
    let streak_thick = clamp(f_streak_thick(), 0.0, 100.0) / 100.0;
    let spread = clamp(f_spread(), 0.0, 100.0) / 100.0;
    let split = clamp(f_color_split(), 0.0, 100.0) / 100.0 * 0.05;
    let sc2 = vec2<f32>((ang / TAU + 0.5) * ray_n * 0.55, ring_c * 0.65) + vec2<f32>(seed * 3.1, seed * 1.7);
    let s_cell = floor(sc2);
    let s_f = fract(sc2);
    let s_h = p_hash2(s_cell);
    let s_h2 = p_hash2(s_cell + vec2<f32>(71.0, 19.0));
    let s_pos = vec2<f32>(0.2 + 0.6 * s_h.x, 0.15 + 0.7 * s_h.y);
    let s_d = s_f - s_pos;
    let sx = s_d.x * (8.0 + spread * 22.0);
    let sy = s_d.y / (0.55 + streak_len * 5.5);
    let srad = 0.015 + streak_thick * 0.11;
    let mr = 1.0 - smoothstep(srad * 0.15, srad, length(vec2<f32>(sx + split * 8.0, sy)));
    let mg = 1.0 - smoothstep(srad * 0.15, srad, length(vec2<f32>(sx, sy)));
    let mb = 1.0 - smoothstep(srad * 0.15, srad, length(vec2<f32>(sx - split * 8.0, sy)));
    let s_on = step(s_h2.y, star_density);
    let star_col = p_pal(1u, s_h.x);
    let star_glow = clamp(f_glow(), 0.0, 100.0) / 100.0 * 0.7;
    let s_halo = star_glow * exp(-length(vec2<f32>(sx, sy)) / max(srad * 5.0, 1e-4));
    col = col + vec3<f32>(mr, mg, mb) * (0.35 + 0.65 * star_col) * s_on;
    col = col + star_col * (s_halo + 0.25 * ((mr + mg + mb) / 3.0)) * s_on * (0.5 + warp_mode);

    // Grid overlay: square tiles in the ring/ray lattice.
    let gap = clamp(f_grid_gap(), 0.0, 90.0) / 100.0 * 0.5;
    let gfill = clamp(f_grid_fill(), 0.0, 100.0) / 100.0;
    let gdens = clamp(f_grid_density(), 0.0, 100.0) / 100.0;
    let gtw = clamp(f_grid_twinkle(), 0.0, 100.0) / 100.0;
    let grand = clamp(f_grid_random(), 0.0, 100.0) / 100.0;
    let wx = max(fwidth(ray_c), 1e-6);
    let wy = max(fwidth(ring_c), 1e-6);
    let gdx = max(gap - fx, fx - (1.0 - gap)) / wx;
    let gdy = max(gap - fr, fr - (1.0 - gap)) / wy;
    var gsd = max(gdx, gdy);
    if (gdx > 0.0 && gdy > 0.0) {
        gsd = length(vec2<f32>(gdx, gdy));
    }
    let lw = max(f_line_width(), 0.5);
    let solid = clamp(0.5 - gsd, 0.0, 1.0);
    let outline = clamp(lw * 0.5 - abs(gsd + lw * 0.5) + 0.5, 0.0, 1.0);
    let gcov = max(outline, solid * gfill);
    let tile_h = p_hash(vec2<f32>(k_h, j_h) + vec2<f32>(seed * 5.3, seed * 2.1));
    let g_lit = step(1.0 - gdens, tile_h);
    let tw = 0.5 + 0.5 * sin(t * (1.2 + 3.0 * tile_h) + tile_h * 50.0);
    let g_on = mix(1.0, smoothstep(0.35, 0.65, tw), gtw);
    let rand_col = p_pal(1u, tile_h);
    let depth_col = p_pal(1u, depth);
    let grid_col = mix(depth_col, rand_col, grand);
    col = mix(col, grid_col, clamp(gcov * g_lit * g_on, 0.0, 1.0) * (0.22 + 0.55 * grid_mode));

    // Alien corridor: panel brightness, frames, neon strips, windows and orbs.
    let walls = clamp(f_walls(), 0.0, 100.0) / 100.0;
    let windows = clamp(f_windows(), 0.0, 100.0) / 100.0;
    let orbs = clamp(f_orbs(), 0.0, 100.0) / 100.0;
    let strips = max(1.0, floor(f_strips() + 0.5));
    let strip_w = clamp(f_strip_width(), 0.5, 12.0) / 100.0;
    let glow = clamp(f_glow(), 0.0, 100.0) / 100.0;
    col = col * (0.78 + 0.45 * walls) + p_pal(2u, depth) * walls * 0.13;
    let frame_w = 0.035 + 0.03 * walls;
    let fd = min(fr, 1.0 - fr);
    let frame = 1.0 - smoothstep(0.0, frame_w, fd);
    col = mix(col, p_pal(2u, 0.15) * (0.25 + 0.45 * shade), frame * 0.45);
    let tube_w = 0.012 + strip_w * 0.045;
    var tube = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        if (f32(i) < strips) {
            let rr = frame_w * 0.45 + f32(i) * frame_w * 0.9;
            let dd = abs(fd - rr);
            tube = tube + exp(-dd * dd / max(tube_w * tube_w, 1e-6));
        }
    }
    col = col + p_pal(2u, fract(0.1 + depth * 0.7)) * tube * frame * (0.28 + 0.35 * glow);
    let panel = step(0.12, fx) * step(fx, 0.88) * step(0.18, fr) * step(fr, 0.82);
    let w_h = p_hash(vec2<f32>(j_h, k_h) + vec2<f32>(seed * 7.1, 13.0));
    let w_h2 = p_hash(vec2<f32>(k_h, j_h) + vec2<f32>(19.0, seed * 3.7));
    let window = panel * step(w_h, windows * 0.5);
    col = mix(col, p_pal(2u, w_h2) * 1.25, window * (0.35 + 0.4 * alien_mode));

    let oc = vec2<f32>((ang / TAU + 0.5) * ray_n * 0.9, ring_c * 0.42) + vec2<f32>(seed * 1.9, seed * 4.3);
    let o_cell = floor(oc);
    let o_f = fract(oc);
    let o_h = p_hash2(o_cell + vec2<f32>(37.0, 91.0));
    let o_pos = vec2<f32>(0.2 + 0.6 * o_h.x, 0.2 + 0.6 * o_h.y);
    let o_d = length((o_f - o_pos) * vec2<f32>(9.0, 5.0));
    let orb = (1.0 - smoothstep(0.02, 0.22, o_d)) * step(o_h.y, orbs * 0.28);
    col = col + p_pal(2u, fract(o_h.x * 3.0)) * orb * (0.4 + 1.3 * glow);

    // Wireframe: rings and rays, their overlap, and a soft halo.
    let ring_a = dt_line_cov(ring_c, lw) * clamp(f_ring_opacity(), 0.0, 100.0) / 100.0;
    let ray_a = dt_line_cov(ray_c, lw) * clamp(f_ray_opacity(), 0.0, 100.0) / 100.0;
    let line_a = 1.0 - (1.0 - ring_a) * (1.0 - ray_a);
    let line_col = p_pal(1u, mix(0.35, 0.9, depth));
    col = mix(col, line_col, line_a * (0.55 + 0.4 * lines_mode));
    let halo = glow * 0.75 * exp(-max(dt_line_dist(ring_c), dt_line_dist(ray_c)) * 0.18);
    col = col + line_col * halo * (0.25 + 0.5 * max(ring_a, ray_a));

    // Far fade, wall boundary and a thin screen border stay transparent so the
    // backdrop level always has pixels to act on.
    var fog = max(0.0, 1.0 - nz * nz * clamp(f_diffuse(), 0.0, 100.0) / 100.0 * 1.5);
    if (nz > 0.95) {
        fog = fog * max(0.0, 1.0 - (nz - 0.95) * 20.0);
    }
    let edge = 1.0 - smoothstep(0.92, 1.12, rn);
    let ex = smoothstep(0.0, 0.07, uv.x) * (1.0 - smoothstep(0.93, 1.0, uv.x));
    let ey = smoothstep(0.0, 0.07, uv.y) * (1.0 - smoothstep(0.93, 1.0, uv.y));
    let screen_edge = ex * ey;
    let solid_style = 1.0 - lines_mode;
    let base_cov = mix(0.34, 1.0, solid_style);
    var cov = clamp(base_cov * fog * edge, 0.0, 1.0) * screen_edge;
    cov = max(cov, clamp(line_a * fog * edge, 0.0, 1.0) * screen_edge * 0.9);
    return vec4<f32>(col * (1.0 + 0.06 * gfill) * (1.0 + 0.06 * clamp(f_orbs() / 100.0, 0.0, 1.0)), cov);
}
