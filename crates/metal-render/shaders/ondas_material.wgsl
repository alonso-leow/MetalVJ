// Ondas, the material map shared by the step and the display pass: a signed
// distance (in grid cells, negative inside) to the dielectric shape, or to the
// wall for the slit mask. Computed analytically from the floats, so the field
// needs no texture for it.
//
// m0 = (epsilon, forma, centre x in cells, centre y in cells)
// m1 = (rotation radians, size in grid heights, slit count, slit width in cells)

fn on_rot(q: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * q.x - s * q.y, s * q.x + c * q.y);
}

fn on_sd(c: vec2<f32>, m0: vec4<f32>, m1: vec4<f32>, gh: f32) -> f32 {
    let q = on_rot(c - vec2<f32>(m0.z, m0.w), -m1.x);
    let t = max(m1.y * gh, 1.0);
    let k = i32(floor(m0.y + 0.5));
    if (k == 0) {
        let d = abs(q) - vec2<f32>(t, t * 0.6);
        return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
    }
    if (k == 1) {
        let r = 1.6 * t;
        return max(length(q - vec2<f32>(t, 0.0)) - r, length(q + vec2<f32>(t, 0.0)) - r);
    }
    if (k == 2) {
        let kk = 1.7320508;
        var p = vec2<f32>(abs(q.x / t) - 1.0, q.y / t + 1.0 / kk);
        if (p.x + kk * p.y > 0.0) {
            p = vec2<f32>(p.x - kk * p.y, -kk * p.x - p.y) * 0.5;
        }
        p.x = p.x - clamp(p.x, -2.0, 0.0);
        return -length(p) * sign(p.y) * t;
    }
    // A wall three cells thick with `n` openings, evenly spread.
    let n = max(m1.z, 1.0);
    let wd = max(m1.w, 1.0);
    let spacing = max(0.7 * gh / n, wd + 3.0);
    let idx = clamp(floor(q.y / spacing + (n - 1.0) * 0.5 + 0.5), 0.0, n - 1.0);
    let cy = (idx - (n - 1.0) * 0.5) * spacing;
    let dy = abs(q.y - cy) - wd * 0.5;
    return max(abs(q.x) - 1.5, -dy);
}

fn on_eps(sd: f32, m0: vec4<f32>) -> f32 {
    if (i32(floor(m0.y + 0.5)) == 3) { return 1.0; }
    return select(1.0, max(m0.x, 1.0), sd < 0.0);
}

fn on_is_wall(sd: f32, m0: vec4<f32>) -> bool {
    return i32(floor(m0.y + 0.5)) == 3 && sd < 0.0;
}

// The source waveform. `f` cycles per time unit, `ph` radians: a phase is a
// time shift so every type honours it. Pulsed types repeat so the picture
// does not go dark after one shot.
fn on_wave(tipo: i32, t0: f32, f: f32, ph: f32) -> f32 {
    let tw = 6.2831853 * f;
    let t = t0 - ph / tw;
    if (tipo == 0) {
        return sin(tw * t) * smoothstep(0.0, 1.0, t * f * 0.5);
    }
    if (tipo == 1) {
        let per = 14.0 / f;
        let tt = t - floor(t / per) * per;
        let w = 1.5 / f;
        let d = tt - 4.0 * w;
        return exp(-d * d / (2.0 * w * w)) * sin(tw * tt);
    }
    let per = 8.0 / f;
    let tt = t - floor(t / per) * per;
    let a = 3.1415927 * f * (tt - 1.5 / f);
    return (1.0 - 2.0 * a * a) * exp(-a * a);
}

fn on_ricker(t: f32, f: f32) -> f32 {
    let a = 3.1415927 * f * (t - 1.5 / f);
    return (1.0 - 2.0 * a * a) * exp(-a * a);
}
