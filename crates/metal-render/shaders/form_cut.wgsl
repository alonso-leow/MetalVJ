// form-cut: a source figure is drawn twice, first in the base colour and then
// in the main colour, where the second copy is cut into fragments that are
// shifted and rotated to expose the base underneath. Behaviour follows the
// FormCutter tool: six source figures, six cut modes (horizontal bands,
// vertical bands, grid, radial grid, wedges, ghost copies), with the burst
// amount from `trigger` controlling how far the pieces travel.
//
// All original code written for MetalVJ.

fn fc_clamp01(x: f32) -> f32 {
    return clamp(x, 0.0, 1.0);
}

fn fc_ease(x: f32) -> f32 {
    let t = fc_clamp01(x);
    return 1.0 - pow(1.0 - t, 3.0);
}

fn fc_hash(x: f32) -> f32 {
    return p_hash(vec2<f32>(x, x * 0.731 + 11.0));
}

fn fc_sd_box(q: vec2<f32>, half: vec2<f32>) -> f32 {
    let d = abs(q) - half;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
}

// Coverage of one of the six source figures at q, in the centred picture
// coordinate (height = 1, y down, x spans +-aspect/2).
fn fc_shape(q: vec2<f32>, asp: f32) -> f32 {
    let src = floor(f_source() + 0.5);
    let size = fc_clamp01(f_scale());
    let stroke = fc_clamp01(f_stroke_width());
    let s = 0.18 + size * 0.78;
    let mw = asp * s;
    let mh = s;
    let r = 0.10 + size * 0.34;
    let sw = 0.004 + stroke * 0.14;
    let aa = 0.004 + fwidth(q.x) * 0.5 + fwidth(q.y) * 0.5;
    var m = 0.0;
    if (src < 0.5) {
        m = 1.0 - smoothstep(-aa, aa, length(q) - r);
    } else if (src < 1.5) {
        m = 1.0 - smoothstep(-aa, aa, fc_sd_box(q, vec2<f32>(mw * 0.5, mh * 0.5)));
    } else if (src < 2.5) {
        let sd = fc_sd_box(q, vec2<f32>(mw * 0.5, mh * 0.5));
        m = 1.0 - smoothstep(-aa, aa, abs(sd) - sw * 0.5);
    } else if (src < 3.5) {
        m = 1.0 - smoothstep(-aa, aa, abs(length(q) - r) - sw * 0.5);
    } else if (src < 4.5) {
        let n = 3.0 + floor(size * 9.0);
        let gap = mw / max(n, 1.0);
        let bw = min(gap * 0.95, sw);
        let x = q.x + mw * 0.5;
        let fx = x - floor(x / max(gap, 0.0001)) * gap;
        let sd = max(abs(fx - gap * 0.5) - bw * 0.5, abs(q.y) - mh * 0.5);
        m = 1.0 - smoothstep(-aa, aa, sd);
    } else {
        let n = 3.0 + floor(size * 9.0);
        let gap = mh / max(n, 1.0);
        let bh = min(gap * 0.95, sw);
        let y = q.y + mh * 0.5;
        let fy = y - floor(y / max(gap, 0.0001)) * gap;
        let sd = max(abs(fy - gap * 0.5) - bh * 0.5, abs(q.x) - mw * 0.5);
        m = 1.0 - smoothstep(-aa, aa, sd);
    }
    return clamp(m, 0.0, 1.0);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let asp = p_aspect();
    let t = p_time();
    let mode = floor(f_cut_mode() + 0.5);
    let src = floor(f_source() + 0.5);
    let trigger = fc_clamp01(f_trigger());
    let burst = fc_ease(trigger);
    let snap = burst * burst;

    let density = fc_clamp01(f_cut_density());
    let cut_size = fc_clamp01(f_cut_size());
    let disp = fc_clamp01(f_displacement());
    let rot_amt = fc_clamp01(f_rotation());

    let seed = f_seed() * 57.3 + t * f_speed() * 31.717 + mode * 903.13 + src * 201.4;
    let shift = disp * (0.005 + 0.60 * cut_size * snap);
    let rot_max = PI * 2.0 * rot_amt * cut_size * snap;

    let base_mask = fc_shape(p, asp);
    var main_mask = 0.0;

    if (burst <= 0.001) {
        main_mask = base_mask;
    } else if (mode < 0.5) {
        let bands = 3.0 + floor(density * 44.0);
        let bh = 1.0 / max(bands, 1.0);
        let idx = clamp(floor((p.y + 0.5) / bh), 0.0, bands - 1.0);
        let center = vec2<f32>(0.0, -0.5 + (idx + 0.5) * bh);
        let dir = select(-1.0, 1.0, fract(idx * 0.5) < 0.01);
        let dx = dir * shift * (0.20 + fc_hash(seed + idx * 1.9) * 1.25);
        let dy = (fc_hash(seed + idx * 3.7) - 0.5) * shift * 0.10;
        let rot = (fc_hash(seed + idx * 5.1) - 0.5) * rot_max;
        let q = center + p_rot(p - center - vec2<f32>(dx, dy), -rot);
        main_mask = fc_shape(q, asp);
    } else if (mode < 1.5) {
        let bands = 3.0 + floor(density * 44.0);
        let bw = asp / max(bands, 1.0);
        let idx = clamp(floor((p.x + asp * 0.5) / bw), 0.0, bands - 1.0);
        let center = vec2<f32>(-asp * 0.5 + (idx + 0.5) * bw, 0.0);
        let dir = select(-1.0, 1.0, fract(idx * 0.5) < 0.01);
        let dx = (fc_hash(seed + idx * 3.7) - 0.5) * shift * 0.10;
        let dy = dir * shift * (0.20 + fc_hash(seed + idx * 1.9) * 1.25);
        let rot = (fc_hash(seed + idx * 5.1) - 0.5) * rot_max;
        let q = center + p_rot(p - center - vec2<f32>(dx, dy), -rot);
        main_mask = fc_shape(q, asp);
    } else if (mode < 2.5) {
        let cols = 2.0 + floor(density * 12.0);
        let rows = 2.0 + floor(density * 12.0);
        let cw = asp / max(cols, 1.0);
        let ch = 1.0 / max(rows, 1.0);
        let cx = clamp(floor((p.x + asp * 0.5) / cw), 0.0, cols - 1.0);
        let cy = clamp(floor((p.y + 0.5) / ch), 0.0, rows - 1.0);
        let center = vec2<f32>(-asp * 0.5 + (cx + 0.5) * cw, -0.5 + (cy + 0.5) * ch);
        let id = cy * cols + cx + 1.0;
        let angle = fc_hash(seed + id * 4.7) * PI * 2.0;
        let mag = shift * (0.15 + fc_hash(seed + id * 8.9) * 1.15);
        let rot = (fc_hash(seed + id * 2.8) - 0.5) * rot_max * 1.35;
        let q = center + p_rot(p - center - vec2<f32>(cos(angle), sin(angle)) * mag, -rot);
        main_mask = fc_shape(q, asp);
    } else if (mode < 3.5) {
        let cols = 2.0 + floor(density * 10.0);
        let rows = 2.0 + floor(density * 10.0);
        let cw = asp / max(cols, 1.0);
        let ch = 1.0 / max(rows, 1.0);
        let cx = clamp(floor((p.x + asp * 0.5) / cw), 0.0, cols - 1.0);
        let cy = clamp(floor((p.y + 0.5) / ch), 0.0, rows - 1.0);
        let center = vec2<f32>(-asp * 0.5 + (cx + 0.5) * cw, -0.5 + (cy + 0.5) * ch);
        let id = cy * cols + cx + 1.0;
        let v = center - vec2<f32>(0.0, 0.0);
        let vn = v / max(length(v), 0.0001);
        let mag = shift * (0.30 + fc_hash(seed + id * 6.3) * 1.20);
        let rot = (fc_hash(seed + id * 9.1) - 0.5) * rot_max;
        let q = center + p_rot(p - center - vn * mag, -rot);
        main_mask = fc_shape(q, asp);
    } else if (mode < 4.5) {
        let wedges = 6.0 + floor(density * 28.0);
        let w = TAU / max(wedges, 1.0);
        let start = fc_hash(seed) * TAU;
        let ang = atan2(p.y, p.x);
        let rel = ang - start;
        let a = rel - floor(rel / TAU) * TAU;
        let idx = clamp(floor(a / w), 0.0, wedges - 1.0);
        let frac = (a - idx * w) / w;
        let a0 = start + idx * w;
        let mag = shift * (0.18 + fc_hash(seed + idx * 2.3) * 1.18);
        let dx = cos(a0) * mag;
        let dy = sin(a0) * mag;
        let rot = (fc_hash(seed + idx * 7.9) - 0.5) * rot_max * 1.2;
        let sc = 1.0 + cut_size * snap * 0.25 * fc_hash(seed + idx * 3.1);
        let q = p_rot((p - vec2<f32>(dx, dy)) / sc, -rot);
        main_mask = fc_shape(q, asp) * step(frac, 0.76);
    } else {
        let copies = floor(2.0 + floor(density * 10.0));
        var acc = base_mask;
        let n = i32(copies);
        for (var i = 1; i <= n; i = i + 1) {
            let k = f32(i) / copies;
            let ang = (k - 0.5) * rot_max * 2.0 + (fc_hash(seed + f32(i)) - 0.5) * snap * 0.18;
            let sc = 1.0 + k * cut_size * snap * 0.42;
            let off = vec2<f32>(
                (fc_hash(seed + f32(i) * 2.5) - 0.5) * shift * 0.35,
                (fc_hash(seed + f32(i) * 4.5) - 0.5) * shift * 0.35
            );
            let q = p_rot((p - off) / sc, -ang);
            let a = max(0.14, 0.85 - k * 0.55) * fc_shape(q, asp);
            acc = 1.0 - (1.0 - acc) * (1.0 - a);
        }
        main_mask = acc;
    }

    let base_col = p_pal(1u, clamp(0.45 + 0.25 * base_mask, 0.0, 1.0));
    let mott = clamp(0.5 + 0.35 * p_hash(floor(p * 24.0) + vec2<f32>(seed, 7.0)), 0.0, 1.0);
    let main_col = p_pal(0u, mott);
    let main_cov = clamp(main_mask, 0.0, 1.0);
    let col = mix(base_col, main_col, main_cov);
    let cov = max(base_mask, main_cov);
    return vec4<f32>(col * cov, cov);
}
