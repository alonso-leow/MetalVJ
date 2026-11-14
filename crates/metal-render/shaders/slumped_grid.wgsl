// slumped-grid: a lattice of randomly occupied squares that sag, drip and
// fuse together. Behaviour and parameter list follow the "Melted Squares" app
// tool (rows/cols, random fill, corner radius, ink colour, diagonal melts);
// the field and shading here are original.
//
// Each occupied cell contributes a rounded box whose bottom edge stretches
// and sinks with a slow fbm (`sag`, `drip`), whose body wobbles and jitters in
// x/y, and whose width pulses with `breath`. The boxes are united with a
// polynomial smooth minimum (`melt`), so neighbours and diagonal corners pull
// into each other like warm plastic. `mode` 1 additionally drops a little
// blob at every occupied diagonal corner (the tool's melted bridge motif);
// mode 2 doubles gravity on sag/drip. Colours come from per-cell hashes mixed
// between slots 0 and 1; slot 2 carries the rim, glint and sparkle.

fn sg_smin(a: f32, b: f32, k: f32) -> f32 {
    if (k <= 1e-5) { return min(a, b); }
    let m = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, m) - k * m * (1.0 - m);
}

fn sg_box(q: vec2<f32>, hh: vec2<f32>, r: f32) -> f32 {
    let rr = clamp(r, 0.0, min(hh.x, hh.y));
    let d = abs(q) - hh + vec2<f32>(rr);
    return min(max(d.x, d.y), 0.0) + length(max(d, vec2<f32>(0.0))) - rr;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let rows = max(f_rows(), 2.0);
    let cols = max(f_cols(), 2.0);
    let pitch = 0.92 / max(rows, cols);
    let drift = vec2<f32>(f_drift_x(), f_drift_y()) * t * 0.35;
    let g = p / pitch + vec2<f32>(cols * 0.5, rows * 0.5) - drift;
    let seedv = vec2<f32>(f_seed() * 13.7, f_seed() * 5.3);
    let mode = i32(floor(f_mode() + 0.5));
    let grav = select(1.0, 2.1, mode == 2);
    let origin = floor(g);
    let fillp = clamp(f_fill_prob(), 0.0, 1.0);

    var d = 4.0;
    var best = 9.0;
    var hc = vec2<f32>(0.5);

    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let cellv = origin + vec2<f32>(f32(i), f32(j));
            let h0 = p_hash2(cellv + seedv);
            if (h0.x > fillp) { continue; }
            let h1 = p_hash2(cellv + seedv + vec2<f32>(71.0, 29.0));

            // Slow, continuous melt field: both per-cell identity and a
            // spatial warp, advected by noise_speed and speed.
            let nq = cellv * (0.22 * f_noise_scale()) + seedv * 0.11;
            let ph = t * (f_noise_speed() * 0.55 + f_speed() * 0.18);
            let n1 = p_fbm(nq + vec2<f32>(0.0, ph), 3);
            let n2 = p_fbm(nq + vec2<f32>(5.3, 1.7 - ph * 0.83), 3);

            let hs = 0.5 * (1.0 - clamp(f_gap(), 0.0, 0.45) * 0.9);
            var cen = cellv + vec2<f32>(0.5);
            cen.x = cen.x + (h0.y - 0.5) * f_jitter() * 0.5 + (n2 - 0.5) * f_wobble() * 0.28;
            cen.y = cen.y + (h1.x - 0.5) * f_jitter() * 0.3;

            // Drip: the bottom edge stretches down while the top stays put.
            let dripn = clamp(n1 * 1.15 - 0.1, 0.0, 1.0);
            let dy = f_drip() * grav * 0.75 * dripn;
            let hy = hs * (1.0 + dy);
            cen.y = cen.y + (hy - hs) + f_sag() * grav * 0.35 * (n1 - 0.35);

            let breath = 1.0 + f_breath() * 0.16 * sin(t * 2.7 + h0.x * TAU);
            let hx = hs * (1.0 + (h1.y - 0.5) * f_stretch() * 0.6) * breath;
            let hyy = hy * breath;
            let ang = f_rot_jitter() * (h1.x - 0.5) * 0.9;
            let q = p_rot(g - cen, ang);
            let rad = f_corner_radius() * min(hx, hyy) * 0.95;
            let sd = sg_box(q, vec2<f32>(hx, hyy), rad);

            d = sg_smin(d, sd, f_melt() * 0.32 * grav);

            // The tool's signature melted bridge at a shared diagonal corner.
            if (mode == 1) {
                let diag = p_hash(cellv + vec2<f32>(1.0, 1.0) + seedv);
                if (diag <= fillp) {
                    let br = (0.07 + 0.10 * f_melt()) * (0.7 + 0.6 * h1.y);
                    let blob = length(g - (cellv + vec2<f32>(1.0, 1.0))) - br;
                    d = sg_smin(d, blob, f_melt() * 0.18);
                }
            }

            let dist = length(g - cen);
            if (dist < best) {
                best = dist;
                hc = vec2<f32>(h0.y, h1.x);
            }
        }
    }

    let aa = max(fwidth(d), 1e-4) + f_edge_soft() * 0.03;
    let shape = 1.0 - smoothstep(-aa, aa, d);
    let depth = clamp(-d * 2.5, 0.0, 1.0);
    let hue = fract(f_seed() * 0.137 + hc.x * 0.63 + t * f_hue_speed() * 0.17);
    let ink = mix(p_pal(0u, hue), p_pal(1u, fract(hue + 0.35)), f_color_mix() * 0.65);
    let glint = p_pal(2u, clamp(hue + 0.2, 0.0, 1.0));

    var rgb = ink * (0.72 + 0.42 * depth);
    // Contour: darken the inside rim, from slot 1 mixed toward the ink.
    let rim = 1.0 - smoothstep(0.0, 0.14, -d);
    rgb = rgb * mix(1.0, 0.28, f_contour() * rim);
    // Shadow pools toward the bottom of each cell.
    let bottom = clamp(g.y - origin.y, 0.0, 1.0);
    rgb = rgb * (1.0 - 0.35 * f_shadow() * bottom);
    // Glossy highlight following the inside depth.
    rgb = rgb + glint * f_highlight() * 0.65 * pow(depth, 1.5) * (0.4 + 0.6 * hc.y);
    // Sparse animated sparkle in the thick parts.
    let sp_h = p_hash(floor(g * 3.0) + vec2<f32>(floor(t * 5.0), seedv.x));
    let sp = step(0.86, sp_h) * smoothstep(0.15, 0.5, depth);
    rgb = rgb + vec3<f32>(1.0) * sp * f_sparkle() * 1.2;

    var outw = rgb * shape;
    var cov = shape;

    // Soft shadow and rim glow live outside the silhouette.
    let outd = max(d, 0.0);
    let glow_am = clamp(f_glow() * exp(-outd * 5.5), 0.0, 1.0);
    let shadow_am = clamp(f_shadow() * exp(-outd * 4.0), 0.0, 1.0);
    if (shape < 0.999) {
        let halo = p_pal(1u, hue) * 0.35;
        let outside = glint * (glow_am * 0.9) + halo * shadow_am;
        let oa = clamp(glow_am * 0.8 + shadow_am * 0.45, 0.0, 1.0) * (1.0 - shape);
        outw = outw + outside * (1.0 - shape);
        cov = max(cov, oa);
    }

    let a = clamp(cov, 0.0, 1.0);
    return vec4<f32>(outw / max(a, 1e-4), a);
}
