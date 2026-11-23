// morphform: one paper-cut form, displaced into copies when the trigger
// envelope opens. Behaviour follows the community "FormShift" tool (six FX
// families: bands, quadrants, wedges, slots, echoes, arc fan); every line here
// is original.
//
// The form is a signed-distance shape (disk, double ring, bar, dot grid,
// nested squares, cross) in a local space rotated and anisotropically scaled by
// the knobs. The trigger drives an ease-out burst; each FX family warps the
// sample coordinate per screen region (band, quadrant, angular wedge, slot,
// radial copy) and stacks the resulting coverage. No offscreen canvas exists,
// so clip-and-blit is expressed as a per-pixel coordinate warp.

fn mh(n: f32) -> f32 {
    return p_hash(vec2<f32>(n, n * 0.37 + 11.0));
}

fn m_box(q: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = abs(q) - b;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
}

// Coverage of the selected shape, in local units where the shape radius is 1.
fn m_shape(q0: vec2<f32>, aa: f32) -> f32 {
    let sh = i32(clamp(floor(f_shape() + 0.5), 0.0, 5.0));
    let sc = max(0.18 + f_scale() * 0.0122, 0.12);
    let h = clamp(0.11 / (sc * sc), 0.02, 0.7);
    var cov = 0.0;
    if (sh == 0) {
        let d = length(q0) - 1.0;
        cov = 1.0 - smoothstep(-aa, aa, d);
    } else if (sh == 1) {
        let d = min(abs(length(q0) - 0.90), abs(length(q0) - 0.38));
        cov = 1.0 - smoothstep(h - aa, h + aa, d);
    } else if (sh == 2) {
        let d = m_box(q0, vec2<f32>(1.45, 0.22));
        cov = 1.0 - smoothstep(-aa, aa, d);
    } else if (sh == 3) {
        let step = 0.34;
        let dot = 0.066;
        for (var j = -3; j <= 3; j = j + 1) {
            for (var i = -3; i <= 3; i = i + 1) {
                let k = 0.96 + 0.08 * mh(f32(i * 31 + j * 17) + f_seed() * 7.3);
                let c = vec2<f32>(f32(i), f32(j)) * step;
                let rr = dot * k;
                let d = length(q0 - c) - rr;
                cov = max(cov, 1.0 - smoothstep(-aa, aa, d));
            }
        }
    } else if (sh == 4) {
        let d = min(abs(m_box(q0, vec2<f32>(1.10))), abs(m_box(q0, vec2<f32>(0.55))));
        cov = 1.0 - smoothstep(h - aa, h + aa, d);
    } else {
        let d = min(m_box(q0, vec2<f32>(1.18, 0.13)), m_box(q0, vec2<f32>(0.13, 1.18)));
        cov = 1.0 - smoothstep(-aa, aa, d);
    }
    return clamp(cov, 0.0, 1.0);
}

// Shape transform: rotate, then the horizontal/vertical stretch pair.
// `aa` comes from the continuous picture coordinate, never from a warped one,
// so band / wedge / slot seams do not blow the antialias width up.
fn m_form(q: vec2<f32>, aa: f32) -> f32 {
    let sc = 0.18 + f_scale() * 0.0122;
    let stx = f_stretch_x() * 0.01;
    let sty = f_stretch_y() * 0.01;
    let sx = max(sc * (1.0 + stx * 1.75), 0.02);
    let sy = max(sc * (1.0 - stx * 0.46) * (1.0 + sty * 1.75), 0.02);
    let rot = (f_rotation() - 50.0) * 0.01 * PI * 0.55;
    let l = p_rot(q, rot) / vec2<f32>(sx * 0.25, sy * 0.25);
    return m_shape(l, aa);
}

// Extra rigid transform used by the shatter families.
fn m_xform(q: vec2<f32>, rot: f32, scl: f32, off: vec2<f32>) -> vec2<f32> {
    return p_rot(q - off, rot) / max(scl, 0.05);
}

fn m_col(p: vec2<f32>, uv: vec2<f32>, t: f32, hot: f32) -> vec3<f32> {
    let g = clamp(0.5 + 0.9 * p.y + 0.12 * sin(p.x * 4.0 + t * 0.7), 0.0, 1.0);
    let base = p_pal(0u, g);
    let acc = p_pal(1u, clamp(0.5 + 0.5 * sin(p.x * 5.0 - p.y * 3.0 + t * 0.9), 0.0, 1.0));
    let col = mix(base, acc, clamp(hot, 0.0, 1.0) * 0.5);
    return col * (1.0 + 0.08 * p_grain(uv, t));
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let trig = clamp(f_trigger() * 0.01, 0.0, 1.0);
    let burst = 1.0 - pow(1.0 - trig, 3.0);
    let snap = burst * burst;
    let scn = i32(clamp(floor(f_scene() + 0.5), 0.0, 5.0));
    let impact = clamp(f_impact() * 0.01, 0.0, 1.0);
    let split = clamp(f_split() * 0.01, 0.0, 1.0);
    let shutter = clamp(f_shutter() * 0.01, 0.0, 1.0);
    let rotamt = (f_rotation() - 50.0) * 0.02;
    let ms = min(p_aspect(), 1.0);

    // Antialias width in shape-local units, from the smooth picture coordinate.
    let sc0 = 0.18 + f_scale() * 0.0122;
    let stx0 = f_stretch_x() * 0.01;
    let sty0 = f_stretch_y() * 0.01;
    let sx0 = max(sc0 * (1.0 + stx0 * 1.75), 0.02);
    let sy0 = max(sc0 * (1.0 - stx0 * 0.46) * (1.0 + sty0 * 1.75), 0.02);
    let pa = max(fwidth(p.x), fwidth(p.y)) * 1.2 + 0.0015;
    let aa = clamp(pa / (0.25 * min(sx0, sy0)), 0.002, 0.35);

    // A hit counter the audio trigger would have advanced; time-quantised here.
    let cut = floor(abs(t) * (0.5 + 2.0 * abs(f_speed()))) + floor(f_seed());
    let seed = cut * 19.19;

    if (burst <= 0.0001) {
        let cov = m_form(p, aa);
        let col = m_col(p, uv, t, 0.0);
        return vec4<f32>(col * cov, cov);
    }

    let shift = ms * impact * split * (0.03 + 0.56 * snap);
    let micro = ms * impact * snap * 0.012;

    var cov = 0.0;
    var col = vec3<f32>(0.0);

    if (scn == 0) {
        // Bands: alternating horizontal strips slide sideways.
        let bands = max(floor(4.0 + shutter * 42.0), 1.0);
        let bh = 1.0 / bands;
        let bi = floor(clamp(uv.y, 0.0, 0.99999) / bh);
        let sgn = select(-1.0, 1.0, (bi % 2.0) < 1.0);
        let r = mh(seed + bi * 2.7) - 0.5;
        let dx = sgn * shift * (0.22 + 1.14 * mh(seed + bi * 4.1)) + r * micro * 8.0;
        let dy = (mh(seed + bi * 6.3) - 0.5) * micro * 2.0;
        cov = m_form(p - vec2<f32>(dx, dy), aa);
        col = m_col(p, uv, t, 0.0);
        if (snap > 0.04) {
            let ly = abs(fract(uv.y / bh + 0.5) - 0.5) * bh;
            let lw = max(0.0015, 0.002 * shutter);
            let line = 1.0 - smoothstep(lw, lw * 2.0 + 0.0015, ly);
            let even = select(0.0, 1.0, (floor(uv.y / bh) % 2.0) < 1.0);
            let add = line * even * 0.18 * snap;
            col = mix(col, p_pal(1u, 0.6), clamp(add, 0.0, 1.0));
            cov = max(cov, clamp(add, 0.0, 1.0));
        }
    } else if (scn == 1) {
        // Quadrants fly apart, then two faint horizontal ghosts.
        let qx = shift * (0.80 + shutter * 0.95);
        let qy = shift * (0.55 + shutter * 0.62);
        let sx = select(-1.0, 1.0, p.x >= 0.0);
        let sy = select(-1.0, 1.0, p.y >= 0.0);
        cov = m_form(p - vec2<f32>(sx * qx, sy * qy), aa);
        col = m_col(p, uv, t, 0.1);
        if (snap > 0.08) {
            let g1 = m_form(p - vec2<f32>(-qx * 0.30, 0.0), aa) * 0.18 * snap;
            let g2 = m_form(p - vec2<f32>(qx * 0.30, 0.0), aa) * 0.18 * snap;
            let g = max(g1, g2);
            col = mix(col, p_pal(1u, 0.4), clamp(g * 0.6, 0.0, 0.7));
            cov = max(cov, g);
        }
    } else if (scn == 2) {
        // Angular wedges: each slice shifts along its own bisector.
        let ang = atan2(p.y, p.x);
        let wedges = max(floor(8.0 + shutter * 28.0), 1.0);
        let wi = clamp(floor((ang + PI) / TAU * wedges), 0.0, wedges - 1.0);
        let a0 = (wi / wedges) * TAU + mh(seed) * PI * 0.22;
        let amt = shift * (0.18 + mh(seed + wi * 3.1) * 1.05);
        let off = vec2<f32>(cos(a0), sin(a0)) * amt;
        let rot = (mh(seed + wi * 8.2) - 0.5) * split * snap * 0.52;
        let scl = 1.0 + split * snap * 0.26 * mh(seed + wi * 6.4);
        cov = m_form(m_xform(p, rot, scl, off), aa / max(scl, 0.5));
        col = m_col(p, uv, t, 0.12);
    } else if (scn == 3) {
        // Slots: alternate columns (or rows) slide up/down, parity by cut.
        let slots = max(floor(4.0 + shutter * 36.0), 1.0);
        let vertical = (cut % 2.0) < 1.0;
        var q = p;
        if (vertical) {
            let i = floor(clamp(uv.x, 0.0, 0.99999) * slots);
            let amp = shift * (0.36 + mh(seed + i * 14.2));
            let dir = select(-1.0, 1.0, (i % 2.0) < 1.0);
            q = p - vec2<f32>(0.0, dir * amp);
        } else {
            let i = floor(clamp(uv.y, 0.0, 0.99999) * slots);
            let amp = shift * (0.36 + mh(seed + i * 14.2));
            let dir = select(-1.0, 1.0, (i % 2.0) < 1.0);
            q = p - vec2<f32>(dir * amp, 0.0);
        }
        cov = m_form(q, aa);
        col = m_col(p, uv, t, 0.05);
    } else if (scn == 4) {
        // Echoes: scaled, rotated, drifting copies stacked over the form.
        let copies = clamp(floor(1.0 + shutter * 10.0), 1.0, 10.0);
        cov = m_form(p, aa);
        col = m_col(p, uv, t, 0.0);
        for (var ii = 1.0; ii <= copies; ii = ii + 1.0) {
            let k = ii / copies;
            let rot = (k - 0.5) * rotamt * PI * 0.9 * snap + (mh(seed + ii) - 0.5) * 0.18 * snap;
            let scl = 1.0 + k * split * 0.62 * snap;
            let off = vec2<f32>(
                (mh(seed + ii * 2.5) - 0.5) * shift * 0.32,
                (mh(seed + ii * 5.5) - 0.5) * shift * 0.32
            );
            let a = m_form(m_xform(p, rot, scl, off), aa / max(scl, 0.5)) * (0.08 + (1.0 - k) * 0.35) * snap;
            if (a > 0.001) {
                col = mix(col, p_pal(1u, clamp(k, 0.0, 1.0)), clamp(a * 0.6, 0.0, 0.7));
                cov = cov + a * (1.0 - cov);
            }
        }
    } else {
        // Arc fan: copies travel along an arc, scaled by a sine bell.
        let copies = clamp(floor(4.0 + shutter * 18.0), 2.0, 20.0);
        let arc = PI * (0.20 + 1.70 * split * snap);
        var wsum = 0.0;
        for (var ii = 0.0; ii < copies; ii = ii + 1.0) {
            let k = ii / (copies - 1.0);
            let a = -arc * 0.5 + arc * k + rotamt * PI * 0.45;
            let rad = ms * split * snap * (0.04 + 0.48 * shutter);
            let ctr = vec2<f32>(cos(a - PI * 0.5), sin(a - PI * 0.5)) * rad;
            let rot = a * 0.55 * snap;
            let scl = 1.0 - 0.16 * snap + 0.36 * snap * sin(k * PI);
            let m = m_form(m_xform(p, rot, scl, ctr), aa / max(scl, 0.5)) * (0.18 + 0.82 * sin(k * PI));
            col = col + m_col(p, uv, t, k * 0.25) * m;
            wsum = wsum + m;
            cov = cov + m * (1.0 - cov);
        }
        if (wsum > 0.0001) {
            col = col / wsum;
        } else {
            col = m_col(p, uv, t, 0.0);
        }
    }

    cov = clamp(cov, 0.0, 1.0);
    return vec4<f32>(col * cov, cov);
}
