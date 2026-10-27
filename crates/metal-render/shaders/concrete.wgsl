// concrete: a brutalist facade built from the procedural scaffold. Everything here
// is original. The grid is the layout; every cell raises a rectangular slab
// or leaves a courtyard, and its face carries a field of window openings in
// one of the tool's motifs (square, disc, half/quarter disc, wedge, tall slit,
// four dots, arch). Fill/outline, spacing, stroke, per-cell sway and rotation
// are the tool's controls expressed as a continuous field instead of SVG
// instances.

fn con_box(p: vec2<f32>, ext: vec2<f32>, soft: f32) -> f32 {
    let d = abs(p) - ext;
    let outside = length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
    let w = max(soft, 0.0005);
    return 1.0 - smoothstep(-w, w, outside);
}

fn con_disc(p: vec2<f32>, r: f32) -> f32 {
    let rr = max(r, 0.001);
    return 1.0 - smoothstep(rr - 0.015, rr + 0.015, length(p));
}

fn con_ring(p: vec2<f32>, ext: vec2<f32>, soft: f32, w: f32) -> f32 {
    let outer = con_box(p, ext, soft);
    let inner_ext = max(ext - vec2<f32>(w), vec2<f32>(0.0));
    let inner = con_box(p, inner_ext, soft * 0.5);
    return clamp(outer - inner, 0.0, 1.0);
}

// The eight motifs of the source tool, at a window-sized extent.
fn con_motif(kind: f32, p: vec2<f32>, ext: vec2<f32>, soft: f32) -> f32 {
    let k = i32(round(kind));
    let r = max(max(ext.x, ext.y), 0.02);
    var m = con_box(p, ext, soft);
    if (k == 1) {
        m = con_disc(p, r * 0.94);
    }
    if (k == 2) {
        m = con_disc(p, r * 0.94) * step(p.y, 0.0);
    }
    if (k == 3) {
        m = con_disc(p, r * 0.94) * step(p.x, 0.0) * step(p.y, 0.0);
    }
    if (k == 4) {
        m = con_box(p, ext, soft) * step(p.x + p.y * 0.8, ext.x * 1.1);
    }
    if (k == 5) {
        m = con_box(p, vec2<f32>(ext.x * 0.36, ext.y), soft);
    }
    if (k == 6) {
        let d = vec2<f32>(ext.x * 0.48, ext.y * 0.48);
        let rr = min(ext.x, ext.y) * 0.46;
        let a = con_disc(p - vec2<f32>(d.x, d.y), rr);
        let b = con_disc(p - vec2<f32>(-d.x, d.y), rr);
        let c = con_disc(p - vec2<f32>(d.x, -d.y), rr);
        let e = con_disc(p - vec2<f32>(-d.x, -d.y), rr);
        m = max(max(a, b), max(c, e));
    }
    if (k == 7) {
        let body = con_box(p - vec2<f32>(0.0, ext.y * 0.38), vec2<f32>(ext.x, ext.y * 0.62), soft);
        let top = con_disc(p - vec2<f32>(0.0, -ext.y * 0.36), max(ext.x, ext.y * 0.34));
        m = max(body, top);
    }
    return clamp(m, 0.0, 1.0);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let asp = p_aspect();
    let spd = f_speed();
    let seed = f_seed();
    let cols = max(round(f_columns()), 2.0);
    let rows = max(round(f_rows()), 2.0);
    let gap = clamp(f_spacing(), 0.0, 0.65);
    let fill_amt = clamp(f_fill(), 0.0, 1.0);
    let stroke = clamp(f_stroke(), 0.005, 0.6);

    // Picture coordinate -> cell grid. p is centred, one unit tall.
    let q = vec2<f32>((p.x / asp + 0.5) * cols, (p.y + 0.5) * rows);
    let cell = floor(q);
    let fp = q - cell;
    let h = p_hash(cell + vec2<f32>(seed * 13.1, seed * 7.7));
    let h2 = p_hash2(cell + vec2<f32>(31.0, 73.0) + vec2<f32>(seed));
    let h3 = p_hash(cell + vec2<f32>(101.0, 17.0) + vec2<f32>(seed * 2.3));

    // Tile-local coordinates, with a slow per-cell sway.
    var local = (fp - vec2<f32>(0.5)) * 2.0;
    let sway_ph = t * spd * 0.9 + h * TAU;
    local = local + vec2<f32>(sin(sway_ph), cos(sway_ph * 0.73)) * clamp(f_sway(), 0.0, 1.0) * 0.07;

    // Coarse slabs: every two rows share one mass, behind the cell blocks.
    let qs = vec2<f32>(q.x, q.y * 0.5);
    let scell = floor(qs);
    let sfp = qs - scell;
    let hs = p_hash(scell + vec2<f32>(seed * 3.7, 9.0));
    let slab_mask = con_box(
        (sfp - vec2<f32>(0.5)) * 2.0,
        vec2<f32>(1.0 - gap * 0.6, 1.0 - gap * 0.15),
        0.02
    ) * step(hs, clamp(f_slab(), 0.0, 1.0));
    let slab_rgb = p_pal(0, clamp(0.10 + 0.62 * hs + 0.30 * f_palette(), 0.0, 1.0));

    // Cell block, anchored to the cell floor.
    let hx = max(mix(0.12, 1.0 - gap * 0.5, clamp(f_block_w(), 0.0, 1.0)) * (0.70 + 0.30 * h2.x), 0.02);
    let hy = max(mix(0.05, 1.0 - gap * 0.5, clamp(f_block_h(), 0.0, 1.0)) * (0.75 + 0.25 * h2.y), 0.02);
    let bc = local - vec2<f32>(0.0, 1.0 - hy);
    let soft = 0.01 + clamp(f_bevel(), 0.0, 1.0) * 0.06;
    let block_mask = con_box(bc, vec2<f32>(hx, hy), soft);
    let open_m = step(h3, clamp(f_open(), 0.0, 1.0));
    let solid_mask = block_mask * (1.0 - open_m);

    // Concrete face: per-cell tone, directional light, edge wear, dirt.
    var face = p_pal(0, clamp(0.12 + 0.72 * h2.y + 0.25 * f_palette(), 0.0, 1.0));
    let lx = mix(-1.0, 1.0, clamp(f_light(), 0.0, 1.0));
    let lgrad = clamp((bc.x / hx) * lx + (bc.y / hy) * 0.30, -1.0, 1.0);
    face = face * (0.66 + 0.34 * lgrad);

    let ac = step(1.0 - clamp(f_accent(), 0.0, 1.0), h3);
    let acc_rgb = p_pal(2, clamp(0.20 + 0.70 * h2.x, 0.0, 1.0));
    face = mix(face, acc_rgb, ac * 0.85);

    let near_x = smoothstep(0.0, 0.38, (hx - abs(bc.x)) / hx);
    let near_y = smoothstep(0.0, 0.38, (hy - abs(bc.y)) / hy);
    face = face * (1.0 - clamp(f_relief(), 0.0, 1.0) * 0.36 * (1.0 - min(near_x, near_y)));
    face = face * (1.0 + clamp(f_bevel(), 0.0, 1.0) * 0.28 * (1.0 - near_x));

    let grime = p_fbm(q * 1.7 + vec2<f32>(seed * 2.1, seed * 1.3), 3);
    face = face * (1.0 - clamp(f_grime(), 0.0, 1.0) * 0.55 * grime);

    // Window field on the block face.
    let wc = max(round(mix(2.0, 6.0, clamp(f_window_cols(), 0.0, 1.0))), 1.0);
    let wr = max(round(mix(2.0, 8.0, clamp(f_window_rows(), 0.0, 1.0))), 1.0);
    let bn = vec2<f32>((bc.x / hx) * 0.5 + 0.5, (bc.y + hy) / (2.0 * hy));
    let wp = bn * vec2<f32>(wc, wr);
    let wcell = floor(wp);
    let wf0 = (wp - wcell - vec2<f32>(0.5)) * 2.0;
    let rot = (h - 0.5) * f_rotation_speed() * PI
        + t * spd * f_rotation_speed() * 0.45 * (h2.x - 0.5);
    let wf = p_rot(wf0, rot);

    let mix_kind = f_shape() > 7.5;
    let base_kind = select(floor(f_shape()), floor(h3 * 7.99), mix_kind);
    let wkind = select(base_kind, floor(p_hash(wcell + cell + vec2<f32>(seed)) * 7.99), mix_kind);
    let wext = vec2<f32>(
        mix(0.14, 0.92, clamp(f_window_w(), 0.0, 1.0)),
        mix(0.14, 0.92, clamp(f_window_h(), 0.0, 1.0))
    );
    let win_shape = con_motif(wkind, wf, wext, 0.015);
    let present = step(
        p_hash(wcell + cell + vec2<f32>(seed * 1.9, 3.0)),
        clamp(f_windows(), 0.0, 1.0)
    );
    let win_mask = win_shape * present * solid_mask * fill_amt;

    let lit_hash = p_hash(
        wcell + cell * 1.7 + vec2<f32>(seed * 4.1, floor(t * spd * 1.7 + h * 5.0))
    );
    let lit = step(0.45, lit_hash);
    let wt = clamp(0.08 + 0.88 * lit * (0.55 + 0.45 * f_palette()), 0.0, 1.0);
    let win_rgb = p_pal(1, wt);

    // Compose slabs, blocks, windows.
    var rgb = slab_rgb;
    var cov = slab_mask;
    let ring = con_ring(bc, vec2<f32>(hx, hy), soft, 0.025 + stroke * 0.18);
    let block_cov = mix(ring, solid_mask, fill_amt);
    let block_rgb = mix(p_pal(2, clamp(0.35 + 0.50 * h2.x, 0.0, 1.0)), face, fill_amt);
    rgb = mix(rgb, block_rgb, block_cov);
    cov = max(cov, block_cov);

    rgb = mix(rgb, win_rgb, win_mask);
    cov = max(cov, win_mask);
    rgb = rgb + win_rgb * clamp(f_glow(), 0.0, 1.0) * win_mask * lit * 0.65;

    // Scaffold grid, on top of walls and gaps.
    let ex = min(fract(q.x), 1.0 - fract(q.x));
    let ey = min(fract(q.y), 1.0 - fract(q.y));
    let gw = 0.010 + clamp(f_grid_width(), 0.0, 1.0) * 0.16;
    let gline = 1.0 - smoothstep(0.0, gw, min(ex, ey));
    let grid_amt = clamp(f_grid(), 0.0, 1.0) * gline;
    let grid_rgb = p_pal(2, clamp(0.55 + 0.40 * h, 0.0, 1.0));
    rgb = mix(rgb, grid_rgb, grid_amt);
    cov = max(cov, grid_amt);

    // Dust and film grain over the whole facade.
    rgb = rgb * (1.0 - clamp(f_grime(), 0.0, 1.0) * 0.30 * grime);
    let granular = p_grain(uv, t) * clamp(f_granular(), 0.0, 1.0);
    rgb = rgb + vec3<f32>(granular * 0.35);

    return vec4<f32>(max(rgb * (1.0 + 0.06 * clamp(f_stroke(), 0.0, 1.0)), vec3<f32>(0.0)), clamp(cov, 0.0, 1.0));
}
