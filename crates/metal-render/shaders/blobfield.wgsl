// blobfield: a regular grid of pulsating molecular blobs. Behaviour and
// controls follow the built-in "Molecules" tool; this WGSL is original.
//
// For every cell a seeded phase drives two sine terms that modulate the blob
// radius around `blob_size`. Each blob contributes a wide gaussian halo plus a
// narrower core gaussian weighted by `core_glow`; the sum is normalised so one
// isolated blob sits high in the palette and overlaps clip at the hot end,
// which is what makes neighbouring cells look merged. A six-stop palette is
// rebuilt from the two colour slots, and a transparent margin at the frame
// edge lets the backdrop show through.

fn bf_phase(seed: f32) -> f32 {
    return p_hash(vec2<f32>(seed * 0.017, seed * 0.031)) * TAU;
}

fn bf_palette_shift(k: f32) -> f32 {
    if (k < 0.5) {
        return 0.0;
    } else if (k < 1.5) {
        return 0.52;
    } else if (k < 2.5) {
        return -0.02;
    } else if (k < 3.5) {
        return 0.30;
    }
    return 0.78;
}

fn bf_lut(t: f32, shift: f32) -> vec3<f32> {
    let c0 = kit_hue(p_pal(0u, 0.0), shift);
    let c1 = kit_hue(p_pal(0u, 0.333333), shift);
    let c2 = kit_hue(p_pal(0u, 0.666667), shift);
    let c3 = kit_hue(p_pal(0u, 1.0), shift);
    let c4 = kit_hue(p_pal(1u, 0.0), shift);
    let c5 = kit_hue(p_pal(1u, 1.0), shift);
    var col: vec3<f32>;
    if (t < 0.12) {
        col = mix(c0, c1, t / 0.12);
    } else if (t < 0.36) {
        col = mix(c1, c2, (t - 0.12) / 0.24);
    } else if (t < 0.55) {
        col = mix(c2, c3, (t - 0.36) / 0.19);
    } else if (t < 0.78) {
        col = mix(c3, c4, (t - 0.55) / 0.23);
    } else {
        col = mix(c4, c5, (t - 0.78) / 0.22);
    }
    return col;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let aspect = p_aspect();
    let cols = max(floor(f_columns() + 0.5), 1.0);
    let rows = max(floor(f_rows() + 0.5), 1.0);
    let cell_h = 1.0 / rows;
    let sym = f_symmetry();
    let base_r = f_blob_size() * cell_h * 0.5;
    let soft = max(f_softness(), 0.05);
    let core = max(f_core_size(), 0.02);
    let glow = f_core_glow();

    var field = 0.0;
    for (var jj: i32 = 0; jj < 16; jj = jj + 1) {
        let jf = f32(jj);
        if (jf >= rows) {
            break;
        }
        for (var ii: i32 = 0; ii < 16; ii = ii + 1) {
            let iff = f32(ii);
            if (iff >= cols) {
                break;
            }
            // Mirror the seed index, not the geometry: paired cells animate
            // identically while each keeps its true position.
            var mi = iff;
            var mj = jf;
            if (sym < 0.5) {
                mi = min(iff, cols - 1.0 - iff);
                mj = min(jf, rows - 1.0 - jf);
            } else if (sym < 1.5) {
                mi = min(iff, cols - 1.0 - iff);
            } else if (sym < 2.5) {
                mj = min(jf, rows - 1.0 - jf);
            }

            let seed = mi * 53.0 + mj * 17.0 + f_seed();
            let phase = bf_phase(seed);
            var rr = base_r
                + f_variance() * base_r * 0.5
                    * (sin(t * f_speed() + phase)
                        + 0.5 * sin(t * f_speed() * 1.7 + phase * 1.3));
            rr = max(rr, 0.0001);

            let cx = ((iff + 0.5) / cols - 0.5) * aspect;
            let cy = (jf + 0.5) / rows - 0.5;
            let dx = p.x - cx;
            let dy = p.y - cy;
            let d2 = dx * dx + dy * dy;

            let sig = max(rr * soft, 0.0005);
            field = field + rr * rr * exp(-d2 / (2.0 * sig * sig));
            let csig = max(sig * core, 0.0003);
            field = field + glow * rr * rr * exp(-d2 / (2.0 * csig * csig));
        }
    }

    let norm = max(base_r * base_r * 1.3 * (1.0 + glow), 0.00001);
    let raw = field * f_gain() / norm;
    let bloom = max(raw - f_bloom_threshold(), 0.0) * f_bloom_amount() * 0.65;
    var tt = clamp(raw + bloom, 0.0, 1.0);
    tt = pow(tt, max(f_snap(), 0.05));

    var col = bf_lut(tt, bf_palette_shift(f_palette()));
    col = col * (1.0 - f_bg_dark() * 0.9 * (1.0 - smoothstep(0.0, 0.4, tt)));

    // Keep a soft transparent frame margin so the backdrop level can move.
    let ex = smoothstep(0.0, 0.11, aspect * 0.5 - abs(p.x));
    let ey = smoothstep(0.0, 0.11, 0.5 - abs(p.y));
    let cov = clamp(ex * ey * (0.88 + 0.12 * smoothstep(0.0, 0.35, tt)), 0.0, 1.0);
    return vec4<f32>(col * cov, cov);
}
