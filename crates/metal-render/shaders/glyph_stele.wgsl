// glyph-stele: a sung monolith of procedural letters. Behaviour and parameter
// list follow the sketch-tool "Monolito de Letras"; the code is original.
//
// A slab is covered by a monospace grid of cells. Each cell is a 3x5 bitmap
// of stroked blocks read from a hash (no font, no texture); some cells are
// spaces and show a dot. A playhead sweeps the cells in reading order: sung
// cells are solid, future cells are faint. Three pseudo-bands damage only the
// sung cells: graves punch holes and crack upward through the cells above,
// medios eat the glyph down to a contour, agudos displace and splinter it.
// The playhead cell gets an accent underline. Everything is a function of the
// seeded cell id, time and the three band levels, so it is deterministic.

fn g_box(d: vec2<f32>, b: vec2<f32>) -> f32 {
    let q = abs(d) - b;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
}

fn g_low(c: vec2<f32>, t: f32) -> f32 {
    let h = p_hash(c + vec2<f32>(f_seed() * 13.7, f_seed() * 5.3));
    let h2 = p_hash(c + vec2<f32>(31.0, 17.0));
    return f_graves() * clamp(0.5 + 0.5 * sin(t * 1.1 + h * TAU + h2 * 2.0), 0.0, 1.0);
}

fn g_mid(c: vec2<f32>, t: f32) -> f32 {
    let h = p_hash(c + vec2<f32>(7.0, f_seed() * 9.1));
    let h2 = p_hash(c + vec2<f32>(67.0, 43.0));
    return f_medios() * clamp(0.5 + 0.5 * sin(t * 0.73 + h * TAU * 1.7 + h2 * 3.0), 0.0, 1.0);
}

fn g_high(c: vec2<f32>, t: f32) -> f32 {
    let h = p_hash(c + vec2<f32>(f_seed() * 3.3, 71.0));
    let h2 = p_hash(c + vec2<f32>(5.0, 101.0));
    return f_agudos() * clamp(0.5 + 0.5 * sin(t * 1.9 + h * TAU * 2.3 + h2 * 5.0), 0.0, 1.0);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let asp = p_aspect();
    let m = clamp(f_margin(), 0.0, 0.25);

    // The slab, centred in the usable area.
    let avail = vec2<f32>(max(asp * (1.0 - 2.0 * m), 0.02), max(1.0 - 2.0 * m, 0.02));
    let sw = avail.x * mix(0.35, 1.0, clamp(f_monolith(), 0.0, 1.0));
    let sh = avail.y;
    let org = vec2<f32>((asp - sw) * 0.5, (1.0 - sh) * 0.5);
    let pp = vec2<f32>(p.x + asp * 0.5, p.y + 0.5);
    let rel = pp - org;
    let in_slab = step(0.0, rel.x) * step(rel.x, sw) * step(0.0, rel.y) * step(rel.y, sh);

    let cw = max(f_cell_size() * 0.62, 0.002);
    let chh = max(f_cell_size(), 0.002);
    let cols = max(floor(sw / cw), 1.0);
    let rows = max(floor(sh / chh), 1.0);
    let q = rel / vec2<f32>(cw, chh);
    let cell = floor(q);
    let g = q - cell;
    let inside = in_slab * step(0.0, cell.x) * step(cell.x, cols - 1.0)
        * step(0.0, cell.y) * step(cell.y, rows - 1.0);

    let id = cell + vec2<f32>(f_seed() * 13.7, f_seed() * 5.3);
    let h1 = p_hash(id);
    let h2 = p_hash(id + vec2<f32>(37.0, 91.0));
    let h3 = p_hash(id + vec2<f32>(11.0, 5.0));
    let h4 = p_hash(id + vec2<f32>(53.0, 29.0));

    // Playhead in reading order.
    let total = max(cols * rows, 1.0);
    let idx = cell.y * cols + cell.x;
    let head_pos = (0.12 + fract(t * f_speed() * 0.045)) * total;
    let sung = step(idx, head_pos);

    let u = clamp(f_threshold(), 0.0, 0.95);
    let u_m = u + (1.0 - u) * 0.4;
    let u_h = u * 0.45;
    let d_low = g_low(cell, t);
    let d_mid = g_mid(cell, t);
    let d_high = g_high(cell, t);

    // Graves: a damaged glyph opens and the crack climbs the same column.
    var hole = 0.0;
    if (sung > 0.5) {
        if (d_low > u) {
            hole = 1.0;
        }
        let len = floor(clamp(f_crack_len(), 0.0, 12.0) + 0.5);
        for (var k = 1; k <= 12; k = k + 1) {
            if (f32(k) > len) { break; }
            let oc = cell + vec2<f32>(0.0, f32(k));
            if (oc.y > rows - 1.0) { break; }
            let oi = oc.y * cols + oc.x;
            let os = step(oi, head_pos);
            if (os > 0.5 && g_low(oc, t) > u) {
                hole = 1.0;
            }
        }
    }

    let ms = clamp((d_mid - u_m) / max(1.0 - u_m, 0.05), 0.0, 1.0);
    let hs = clamp((d_high - u_h) / max(1.0 - u_h, 0.05), 0.0, 1.0);
    let is_space = step(f_density(), h2);

    // Stone base, with a vertical shade and grain.
    var col = p_pal(2u, clamp(rel.y / max(sh, 1e-3) * 0.85 + (h2 - 0.5) * 0.08, 0.0, 1.0));
    col = col * (0.85 + 0.15 * h3);
    col = col + vec3<f32>(p_grain(uv * 1.3, t)) * f_grain() * 0.5;
    var cov = in_slab;

    let aa = 0.008 + clamp(f_soft(), 0.0, 1.0) * 0.05;

    // Cell-local glyph: size variation, splinter displacement, chip cut.
    var gl = (g - vec2<f32>(0.5)) / max(1.0 + (h3 - 0.5) * f_size_var() * 0.8, 0.3) + vec2<f32>(0.5);
    let dsp = clamp(f_displace(), 0.0, 1.0) * hs;
    gl = gl + vec2<f32>((h1 * 2.0 - 1.0) * dsp * 0.9, (h4 * 2.0 - 1.0) * dsp * 0.45);

    var chip = false;
    if (hs > 0.12 && is_space < 0.5) {
        let cut = 0.15 + 0.55 * hs * h3;
        let side = i32(floor(h4 * 4.0)) % 4;
        if (side == 0) { chip = gl.y > cut; }
        else if (side == 1) { chip = gl.y < 1.0 - cut; }
        else if (side == 2) { chip = gl.x > cut; }
        else { chip = gl.x < 1.0 - cut; }
    }

    if (inside > 0.5 && hole < 0.5 && ms <= 0.85) {
        if (is_space > 0.5) {
            // Free cells carry a faint dot lattice, brighter once sung.
            let dsd = length(g - vec2<f32>(0.5)) - (0.08 + 0.05 * h3);
            let dot = 1.0 - smoothstep(-aa, aa, dsd);
            let a = dot * clamp(f_dots(), 0.0, 1.0) * mix(f_future_alpha() * 0.5, 0.35, sung);
            col = mix(col, p_pal(0u, clamp(0.25 + h2 * 0.6, 0.0, 1.0)), clamp(a, 0.0, 1.0));
        } else {
            // 3x5 bitmap from the cell hash; at least one block so cells read.
            let mask = i32(floor(h1 * 32767.0)) | 1;
            let gw = 0.8;
            let hw = clamp(f_stroke(), 0.05, 0.95) * 1.5;
            var sd = 2.0;
            for (var gy = 0; gy < 5; gy = gy + 1) {
                for (var gx = 0; gx < 3; gx = gx + 1) {
                    let bit = u32(gy * 3 + gx);
                    if (((mask >> bit) & 1) == 1) {
                        let ctr = vec2<f32>(
                            0.1 + (f32(gx) + 0.5) / 3.0 * gw,
                            0.1 + (f32(gy) + 0.5) / 5.0 * gw
                        );
                        let half = vec2<f32>(gw / 3.0 * 0.5, gw / 5.0 * 0.5)
                            * clamp(hw, 0.15, 0.95);
                        sd = min(sd, g_box(gl - ctr, half));
                    }
                }
            }

            let fill = 1.0 - smoothstep(-aa, aa, sd);
            let ow = max(0.008, clamp(f_stroke(), 0.05, 0.95) * 0.07 * (1.0 - ms * 0.8));
            let outline = 1.0 - smoothstep(ow, ow + aa, abs(sd));
            var shape = mix(fill, outline, step(0.02, ms));
            shape = shape * mix(f_future_alpha(), 1.0, sung);
            if (chip) { shape = 0.0; }

            var gcol = p_pal(0u, clamp(0.2 + h2 * 0.7, 0.0, 1.0));
            gcol = mix(gcol, p_pal(1u, clamp(0.25 + h4 * 0.75, 0.0, 1.0)),
                clamp(f_accent() * hs * 1.1, 0.0, 1.0));
            gcol = gcol * (1.0 - clamp(f_flicker(), 0.0, 1.0) * 0.5
                * (0.5 + 0.5 * sin(t * 5.0 + h3 * TAU)));
            col = mix(col, gcol, clamp(shape, 0.0, 1.0));
        }
    }

    // Head: underline the cell under the playhead.
    if (f_head() > 0.001 && hole < 0.5 && abs(idx - floor(head_pos)) < 0.5) {
        let by = 0.88;
        let frac = clamp(head_pos - floor(head_pos), 0.25, 1.0);
        let bar = step(by, g.y) * step(g.y, by + 0.08) * step(g.x, frac);
        col = mix(col, p_pal(1u, 0.6), bar * clamp(f_head(), 0.0, 1.0) * 0.9);
    }

    return vec4<f32>(max(col, vec3<f32>(0.0)) * cov * (1.0 + 0.06 * clamp(f_head(), 0.0, 1.0)), cov);
}
