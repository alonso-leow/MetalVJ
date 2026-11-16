// halftonedots: a picture rendered as a halftone dot screen. Behaviour
//; the code
// is original.
//
// The frame is cut into a lattice (square or hex-staggered) of pitch `tam`,
// rotated by `angulo` and optionally scrolling. Each lattice point reads the
// picture once (at its centre), turns it into a level (contrast, invert,
// threshold) and grows a dot whose area follows that level. Four dot types:
// classic (separate discs), gooey (neighbouring dots fuse as a summed field),
// holes (a solid sheet punched by discs) and soft (gaussian blobs). Each pixel
// looks at the 3x3 lattice points around it. Dot colour is the `punto` slot or
// the picture colour (`colores_orig`).

fn hd_lum(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let c = max(f_size(), 0.006);
    let hexm = clamp(f_grid(), 0.0, 1.0);
    let hexr = select(0.0, 1.0, f_grid() > 0.5);
    let kind = clamp(i32(floor(f_type() + 0.5)), 0, 3);
    let ang = f_angle() * TAU;

    // Lens: bulge the sampling lattice around the centre.
    var pp = p;
    let rl = length(pp);
    pp = pp * (1.0 + f_lens() * (0.5 - rl) * 0.8 * select(0.0, 1.0, rl < 0.5));
    let q0 = p_rot(pp, -ang);
    let scroll = vec2<f32>(f_speed_x(), f_speed_y()) * t * c;
    let q = q0 + scroll;

    let cy = c * mix(1.0, 0.8660254, hexr);
    let ri = floor(q.y / cy);
    let breath = 1.0 + f_breathe() * 0.5 * sin(t * f_breathe_speed() * 3.0);
    let gsz = max(f_grain_size(), 0.05);
    let seed = f_seed() * 1.37;

    var cov = 0.0;
    var field = 0.0;
    var colw = vec3<f32>(0.0);
    var colsum = 0.0;
    var alpha = 0.0;
    for (var oy = -1; oy <= 1; oy = oy + 1) {
        let row = ri + f32(oy);
        let odd = abs(row - 2.0 * floor(row * 0.5));
        let stag = odd * 0.5 * hexr;
        let xi0 = floor(q.x / c - stag);
        for (var ox = -1; ox <= 1; ox = ox + 1) {
            let xi = xi0 + f32(ox);
            let id = vec2<f32>(xi, row);
            var ce = vec2<f32>((xi + 0.5 + stag) * c, (row + 0.5) * cy);
            let h = p_hash(id + vec2<f32>(seed, 3.0));
            let h2 = p_hash(id + vec2<f32>(11.0, seed + 7.0));
            // Position jitter, plus grain-driven shimmer.
            ce = ce + (vec2<f32>(h, h2) - 0.5) * c * f_jitter_pos();
            let gh = p_hash(floor(ce / (c * gsz)) + vec2<f32>(floor(t * 12.0), 5.0));
            ce = ce + (vec2<f32>(gh, p_hash(vec2<f32>(gh, 2.0))) - 0.5) * c * f_grain_mix() * 0.5;

            let world = p_rot(ce - scroll, ang);
            let s = p_src(world);
            var lv = hd_lum(s.rgb);
            lv = mix(lv, 1.0 - lv, f_invert());
            lv = clamp((lv - 0.5) * (1.0 + f_contrast_dot() * 3.0) + 0.5, 0.0, 1.0);
            // Travelling size wave along the lattice.
            let wv = 0.5 + 0.5 * sin(dot(ce / c, vec2<f32>(cos(f_wave_direction() * TAU), sin(f_wave_direction() * TAU))) * f_wave_freq() * 0.6 - t * f_wave_speed() * 2.0);
            lv = lv * (1.0 - f_wave() * wv * 0.8);
            lv = lv * (1.0 - f_jitter_size() * h2);
            lv = max(lv - f_threshold(), 0.0) / max(1.0 - f_threshold(), 0.05);
            if (kind == 2) { lv = 1.0 - lv; }
            // Dot radius from area: r ~ sqrt(level).
            let rr = c * 0.5 * f_radius() * sqrt(clamp(lv, 0.0, 1.0)) * breath * s.a;
            let d = length(q - ce);
            let soft = c * (0.02 + (1.0 - f_sharpness()) * 0.45);
            var w = 0.0;
            if (kind == 0 || kind == 2) {
                w = 1.0 - smoothstep(rr - soft, rr + soft * 0.5 + 1e-4, d);
                cov = max(cov, w);
            } else if (kind == 1) {
                w = (rr * rr) / (d * d + c * c * 0.004);
                field = field + w;
            } else {
                w = exp(-pow(d / max(rr * 0.8, 1e-4), 2.0) * mix(2.5, 0.8, 1.0 - f_sharpness()));
                cov = max(cov, w);
            }
            let wc = select(w, min(w, 1.0), kind != 1) + 1e-4;
            let pc = mix(p_pal(0u, mix(lv, h, f_tint())), s.rgb, f_colors_origin());
            colw = colw + pc * wc;
            colsum = colsum + wc;
            alpha = max(alpha, s.a);
        }
    }
    if (kind == 1) {
        let e = 0.15 + (1.0 - f_sharpness()) * 0.6;
        cov = smoothstep(1.0 - e, 1.0 + e * 0.3, field);
    }
    if (kind == 2) { cov = 1.0 - cov; }
    var col = colw / max(colsum, 1e-4);
    if (kind == 2) { col = mix(col, p_pal(0u, 0.5), 1.0 - f_colors_origin()); }
    col = col + vec3<f32>(p_grain(uv * gsz, t)) * f_grain_surface() * 0.6;
    return vec4<f32>(max(col, vec3<f32>(0.0)), clamp(cov, 0.0, 1.0));
}
