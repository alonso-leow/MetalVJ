// chargefield: electrostatic field lines. Behaviour and parameter list follow
// the community tool "Electrostatic Field v1.0"; everything here is original.
//
// For 2D point charges the electrostatic field lines are the level sets of the
// stream function psi = sum q_i * atan2(y-y_i, x-x_i). Each charge is weighted
// by an integer polarity (+1/-1), so a branch-cut wrap moves psi by a whole
// multiple of 2*pi and, with an integer line spacing, the contour pattern stays
// continuous. A second potential sum supplies a coordinate *along* the lines,
// which gives the moving dashes (path_length) and makes flow legible.
//
// The charge cloud, its scatter and its scatter radius follow the source tool's
// four systems (concentric, parallel, converging, resonance). Charge
// generation, the domain warp and the line phase are all driven by time/speed.

struct Ch {
    p: vec2<f32>,
    q: f32,
    inf: f32,
};

fn cf_mode() -> u32 {
    return u32(clamp(floor(f_system_shape() + 0.5), 0.0, 3.0));
}

fn cf_charge(i: u32, count: u32, radius: f32, phase: f32, mode: u32) -> Ch {
    let fi = f32(i);
    let cf = max(f32(count), 1.0);
    let countT = clamp((cf - 3.0) / 93.0, 0.0, 1.0);
    let sd = f_seed();
    let baseInf = 0.17 - countT * 0.10;
    let scatter = (0.10 + countT * 0.52) * (0.55 + 0.9 * f_pole_force());

    let h1 = p_hash(vec2<f32>(fi * 3.17 + 1.2 + sd * 4.7, sd * 2.3 + 0.7));
    let h2 = p_hash(vec2<f32>(fi * 5.71 + 3.6 + sd * 1.9, sd * 5.1 + 2.1));
    let h3 = p_hash(vec2<f32>(fi * 7.13 + 8.1 + sd * 3.3, sd * 6.7 + 4.4));

    var c = Ch(vec2<f32>(0.0), 0.0, 0.22);
    c.inf = clamp(baseInf * (0.82 + h3 * 0.30), 0.045, 1.0);
    c.q = select(-1.0, 1.0, (i & 1u) == 0u);

    if (mode == 1u) {
        // Parallel: staggered bands across the width.
        var rows = 1.0;
        if (cf > 12.0) { rows = 2.0; }
        if (cf > 36.0) { rows = 3.0; }
        if (cf > 72.0) { rows = 4.0; }
        let t = select(0.5, fi / max(cf - 1.0, 1.0), cf > 1.0);
        let row = fi - floor(fi / rows) * rows;
        let xBase = (t - 0.5) * radius * 1.56;
        let rowY = select(0.0, (row - (rows - 1.0) * 0.5) * radius * 0.28, rows > 1.0);
        let randX = (h1 - 0.5) * radius * 0.58 * scatter;
        let randY = (h2 - 0.5) * radius * 0.78 * scatter;
        let waveY = sin(phase * (0.18 + fi * 0.003) + fi * 0.63) * radius * 0.12;
        c.p = vec2<f32>(xBase + randX, rowY + randY + waveY);
    } else if (mode == 2u) {
        // Converging: rings of opposing poles.
        var rings = 2.0;
        if (cf > 15.0) { rings = 3.0; }
        if (cf > 36.0) { rings = 4.0; }
        if (cf > 72.0) { rings = 5.0; }
        let ri = fi - floor(fi / rings) * rings;
        let layer = floor(fi / rings);
        let per = ceil(cf / rings);
        let ringT = select(0.0, ri / max(rings - 1.0, 1.0), rings > 1.0);
        let ringRadius = radius * (0.08 + ringT * 0.46);
        let a = (layer / max(per, 1.0)) * TAU + ri * 0.29 + fi * 0.017;
        let randR = radius * scatter * (h1 - 0.5) * 0.44;
        let randA = (h2 - 0.5) * 0.9;
        let rr = max(radius * 0.04, ringRadius + randR);
        let aa = a + randA;
        c.p = vec2<f32>(
            cos(aa + phase * (0.04 + fi * 0.0012)),
            sin(aa - phase * (0.04 + fi * 0.0010))
        ) * rr;
    } else if (mode == 3u) {
        // Resonance: broken lissajous structure.
        let t = fi / cf;
        let a = t * TAU;
        let xBase = sin(a * 3.0 + phase * 0.03) * radius * 0.36
                  + sin(a * 7.0 + fi * 0.13) * radius * 0.08;
        let yBase = sin(a * 2.0 - phase * 0.025) * radius * 0.30
                  + cos(a * 5.0 + fi * 0.09) * radius * 0.10;
        let randX = (h1 - 0.5) * radius * 0.74 * scatter;
        let randY = (h2 - 0.5) * radius * 0.74 * scatter;
        c.p = vec2<f32>(xBase + randX, yBase + randY);
    } else {
        // Concentric / orbital: radial cloud in bands.
        let bands = max(3.0, round(sqrt(cf) * 0.9));
        let band = fi - floor(fi / bands) * bands;
        let layer = floor(fi / bands);
        let layers = ceil(cf / bands);
        let aBase = h1 * TAU;
        let aWave = sin(aBase * 2.0 + phase * 0.16 + layer * 0.41) * 0.28;
        let a = aBase + aWave;
        let t = select(0.0, layer / max(layers - 1.0, 1.0), layers > 1.0);
        let radialJitter = (h2 - 0.5) * 2.0;
        let cyclical = sin(a * 3.0 + phase * 0.31 + band * 0.8) * 0.18;
        let rBase = radius * (0.10 + 0.90 * t);
        let r = rBase * (0.72 + 0.28 * f_spread())
              + radius * radialJitter * (0.30 * f_spread() + 0.14 * countT)
              + radius * cyclical * (0.18 * f_spread() + 0.10 * countT);
        c.p = vec2<f32>(cos(a), sin(a)) * r;
    }

    // Spread disperses every system, including the default resonance cloud.
    c.p = c.p + vec2<f32>(h2 - 0.5, h1 - 0.5) * radius * f_spread() * 0.18;
    c.p = c.p * (1.0 + (f_pole_force() - 0.5) * 0.12);
    return c;
}

// x: stream function psi, y: potential (for dashes), z: nearest distance,
// w: sign of the nearest charge.
fn cf_sample(p: vec2<f32>, radius: f32, phase: f32, mode: u32, count: u32) -> vec4<f32> {
    var psi = 0.0;
    var pot = 0.0;
    var nd = 1e6;
    var nq = 0.0;
    for (var i: u32 = 0u; i < 36u; i = i + 1u) {
        if (i >= count) { break; }
        let c = cf_charge(i, count, radius, phase, mode);
        let d = p - c.p;
        let d2 = dot(d, d) + 1e-4;
        let dd = sqrt(d2);
        psi = psi + c.q * atan2(d.y, d.x);
        pot = pot + c.q * log(d2);
        if (dd < nd) {
            nd = dd;
            nq = c.q;
        }
    }
    return vec4<f32>(psi, pot, nd, nq);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let mode = cf_mode();
    let countF = clamp(round(f_pole_count()), 3.0, 96.0);
    let count = u32(countF);
    let radius = 0.37 * f_scale() / 100.0;

    let phase = t * (f_motion_bpm() / 60.0) * PI * 0.44 * f_speed() + f_seed() * 0.37;

    // Drift turns the whole system; flow adds a breathing swirl.
    var q = p_rot(p, t * f_drift_v() * 0.16);
    let swirl = f_flow() * (0.35 + 0.55 * sin(length(q) * 2.3 - t * 0.45));
    q = p_rot(q, swirl);

    // Domain warp, the field-line analogue of the source's warp.
    let wn = vec2<f32>(
        p_noise(q * 2.7 + vec2<f32>(t * 0.07, 1.3)),
        p_noise(q * 2.7 + vec2<f32>(-2.1, -t * 0.05))
    );
    q = q + (wn - vec2<f32>(0.5)) * f_warp() * 0.9;

    let sm = cf_sample(q, radius, phase, mode, count);

    // Integer lines per charge keeps branch cuts continuous and scales the
    // total line count with density.
    let dens = max(round(f_density()), 1.0);
    let k = max(round(dens / max(countF, 1.0)), 1.0);
    let spacing = TAU / k;
    let s = sm.x / spacing;
    let dline = abs(fract(s + 0.5) - 0.5);

    let aa = max(fwidth(dline), 1e-4);
    let lw = clamp(f_line_width() * 0.35, 0.10, 4.0);
    var line = 1.0 - smoothstep(lw * aa, (lw + 0.8) * aa, dline);

    // Dashes run along the field: short path_length chops the lines, long
    // path_length lets them run.
    let dashFreq = mix(0.55, 0.04, clamp(f_path_length(), 0.0, 1.0));
    let dashCoord = (sm.y / max(countF, 1.0)) * dashFreq + t * 0.14 * f_flow();
    let dash = abs(fract(dashCoord) - 0.5) * 2.0;
    let dashMask = 1.0 - 0.78 * smoothstep(0.55, 0.95, dash);
    line = line * dashMask;

    let nd = sm.z;
    let nq = sm.w;
    let neg = clamp(-nq, 0.0, 1.0);

    let ah = p_hash(vec2<f32>(floor(s) + 0.5, f_seed() * 7.7 + 3.0));
    let accentOn = step(1.0 - clamp(f_accent(), 0.0, 1.0), ah);

    var col = mix(
        p_pal(0u, clamp(0.22 + 0.55 * ah + 0.18 * sin(t * 0.3 + nd * 3.0), 0.0, 1.0)),
        p_pal(1u, clamp(0.30 + 0.60 * ah, 0.0, 1.0)),
        accentOn * 0.92
    );
    col = mix(col, p_pal(1u, 0.55), neg * 0.20 * (1.0 - accentOn));

    let core = exp(-nd * nd * 26.0 / (radius * radius + 1e-4))
             * (0.30 + 0.70 * f_pole_force());
    col = col + p_pal(1u, 0.72) * core * (0.45 + 0.55 * neg) * 0.9;

    let bright = 0.55 + 0.75 * exp(-nd / (radius * 0.8 + 1e-4));
    col = col * bright * (0.88 + 0.24 * clamp(dens / 120.0, 0.0, 1.0));

    let cov = clamp(line, 0.0, 1.0);
    let halo = clamp(core * 0.6, 0.0, 1.0);
    let a = clamp(cov + halo * (1.0 - cov), 0.0, 1.0);
    return vec4<f32>(col * a, a);
}
