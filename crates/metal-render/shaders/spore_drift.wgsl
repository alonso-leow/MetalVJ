// spore-drift: a drifting field of coloured spores. Behaviour and parameter
// list follow the built-in "Spores" tool; this WGSL is original, and the
// stateful particle simulation is approximated by a time-scrolled hashed site
// field (jittered cells, one drifting site each, nearest two sites kept).
//
// Each site has a random radius variation, a colour band (patches, ranked by a
// low-frequency noise, or random), and a drift orbit whose amplitude follows
// the density spacing. The bass/treble proxy swells and sparkles the dots, the
// optional ring ripples from a shaped origin, and `connections` draws a thread
// from each site to its nearest neighbour (mesh) or from the core (core mode).

struct SporeSite {
    d1: f32,
    d2: f32,
    p1: vec2<f32>,
    p2: vec2<f32>,
    id: vec2<f32>,
    h: f32,
}

fn sp_spacing() -> f32 {
    let den = clamp(f_density(), 0.0, 100.0);
    if (den < 20.0) {
        return 0.6 * pow(0.085 / 0.6, den / 20.0);
    }
    return 0.085 * pow(0.014 / 0.085, (den - 20.0) / 80.0);
}

fn sp_hash(p: vec2<f32>) -> f32 {
    return p_hash(p + vec2<f32>(f_seed() * 13.1, f_seed() * 7.7));
}

fn sp_hash2(p: vec2<f32>) -> vec2<f32> {
    return p_hash2(p + vec2<f32>(f_seed() * 5.3, f_seed() * 11.9));
}

fn sp_site(cell: vec2<f32>, t: f32, sp: f32) -> vec2<f32> {
    let key = cell + vec2<f32>(f_seed() * 3.1, f_seed() * 5.7);
    let j = sp_hash2(key);
    let base = cell + vec2<f32>(0.5) + (j - vec2<f32>(0.5)) * 0.70;
    let h = sp_hash(key + vec2<f32>(23.0, 11.0));
    let h2 = sp_hash(key + vec2<f32>(41.0, 5.0));
    let amp = clamp(f_mote_drift(), 0.0, 100.0) / 100.0 * min(sp, 0.085) * 0.9 * (0.4 + 0.6 * h2);
    let spd = 0.35 + clamp(f_speed(), 0.0, 100.0) / 100.0 * 1.6;
    let rs = 0.7 + clamp(f_rotation_speed(), 0.0, 100.0) / 100.0 * 0.6;
    let ph = t * spd * rs * (0.4 + 0.8 * h) + h * TAU;
    let wob = vec2<f32>(sin(ph), sin(ph * 0.83 + h2 * TAU));
    return base + wob * min(amp / max(sp, 1e-4), 0.5);
}

fn sp_eval(q: vec2<f32>, sp: f32, t: f32) -> SporeSite {
    let ip = floor(q);
    let fp = q - ip;
    var r: SporeSite;
    r.d1 = 99.0;
    r.d2 = 99.0;
    r.p1 = vec2<f32>(0.0);
    r.p2 = vec2<f32>(0.0);
    r.id = ip;
    r.h = 0.0;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let cell = ip + vec2<f32>(f32(i), f32(j));
            let site = sp_site(cell, t, sp);
            let d = length(site - q);
            if (d < r.d1) {
                r.d2 = r.d1;
                r.p2 = r.p1;
                r.d1 = d;
                r.p1 = site;
                r.id = cell;
                r.h = sp_hash(cell + vec2<f32>(7.0, 3.0));
            } else if (d < r.d2) {
                r.d2 = d;
                r.p2 = site;
            }
        }
    }
    return r;
}

fn sp_weight(pos: vec2<f32>, h: f32, t: f32, asp: f32) -> f32 {
    let px = clamp(pos.x / max(asp, 0.01), 0.0, 1.0);
    let py = clamp(pos.y, 0.0, 1.0);
    let dc = vec2<f32>(px - 0.5, py - 0.5);
    let rad = clamp(length(dc) / 0.7071, 0.0, 1.0);
    let lay = i32(round(clamp(f_layout_mode(), 0.0, 6.0)));
    if (lay == 0) {
        let jump = floor(t * (0.2 + clamp(f_rotation_speed(), 0.0, 100.0) / 100.0 * 8.0));
        let ang = h * TAU + jump * 2.39996 + (sp_hash(vec2<f32>(f_seed() * 13.0, jump)) - 0.5) * 0.8;
        let v = 0.5 + 0.5 * ((dc.x * cos(ang) + dc.y * sin(ang)) / 0.7071);
        return clamp(0.5 + (v - 0.5) * 1.25, 0.0, 1.0);
    }
    if (lay == 1) {
        return 1.0 - rad;
    }
    if (lay == 2) {
        return rad;
    }
    if (lay == 3) {
        return py;
    }
    if (lay == 4) {
        return 1.0 - py;
    }
    if (lay == 5) {
        return px;
    }
    if (lay == 6) {
        return 1.0 - px;
    }
    return rad;
}

fn sp_color(idx: f32) -> vec3<f32> {
    let k = clamp(idx, 0.0, 4.0);
    if (k < 1.5) {
        return p_pal(0u, k / 3.0);
    }
    if (k < 3.5) {
        return p_pal(0u, (k - 1.0) / 2.0);
    }
    return p_pal(1u, (k - 3.0) / 1.0);
}

fn sp_seg_dist(pos: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = pos - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-8), 0.0, 1.0);
    return length(pa - ba * h);
}

fn sp_shape(d: vec2<f32>, shape: i32) -> f32 {
    let a = abs(d);
    if (shape == 1) {
        return max(a.x * 0.866 + a.y * 0.5, a.y);
    }
    if (shape == 2) {
        return max(a.x, a.y);
    }
    if (shape == 3) {
        let ang = atan2(d.y, d.x);
        return length(d) * (1.0 + 0.28 * sin(ang * 1.5 + length(d) * 9.0));
    }
    return length(d);
}

fn sp_swell(weight: f32, bass_sig: f32, hit: f32, treb_sig: f32, bass: f32, treb: f32) -> f32 {
    return weight * bass_sig * bass * 0.9 + hit * bass * 0.55 + treb_sig * treb * 0.4;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let asp = p_aspect();
    let c = vec2<f32>(p.x + asp * 0.5, p.y + 0.5);
    let sp = sp_spacing();
    let inv = 1.0 / max(sp, 1e-4);
    let q = vec2<f32>(c.x * inv, c.y * inv);

    let v = sp_eval(q, sp, t);
    let pos = q;
    let h = v.h;
    let weight = sp_weight(c, h, t, asp);

    // Audio proxy: a slow bass swell and a faster treble shimmer stand in for
    // the spectrum the tool reads.
    let bass_sig = 0.5 + 0.5 * sin(t * 1.6 + 0.4);
    let treb_sig = 0.5 + 0.5 * sin(t * 6.1 + h * TAU);
    let hit_exp = 1.0 + clamp(f_hit_contrast(), 0.0, 100.0) / 100.0 * 5.0;
    let hit = pow(clamp((bass_sig - 0.32) / 0.68, 0.0, 1.0), hit_exp);
    let bass = clamp(f_bass(), 0.0, 100.0) / 100.0;
    let treb = clamp(f_treble(), 0.0, 100.0) / 100.0;

    let rnd = pow(sp_hash(v.id + vec2<f32>(13.0, 29.0)), 1.7);
    let vary = clamp(f_size_variation(), 0.0, 100.0) / 100.0;
    let size_mul = 1.0 + (-0.6 + 1.2 * rnd) * vary;
    let base_r = 0.0012 + pow(clamp(f_dot_size(), 0.0, 100.0) / 100.0, 1.3) * 0.0145;
    let swell = 1.0 + sp_swell(weight, bass_sig, hit, treb_sig, bass, treb);
    let radius = max(base_r * size_mul * swell, 0.0004);

    let dist = v.d1 * sp;
    let aa = 0.0007 + radius * 0.18;
    let cov = 1.0 - smoothstep(radius - aa, radius + aa, dist);

    // Colour band: patches rank a low-frequency noise; random uses the site id.
    let patch_v = p_noise(c * 1.7 + vec2<f32>(f_seed() * 3.0, 0.0)) * 0.65
        + p_noise(c * 3.9 + vec2<f32>(0.0, f_seed() * 5.0)) * 0.35;
    let rank = floor(clamp(patch_v, 0.0, 0.999) * 5.0);
    let pick = floor(clamp(sp_hash(v.id + vec2<f32>(31.0, 17.0)), 0.0, 0.999) * 5.0);
    let idx = select(rank, pick, f_color_mode() > 0.5);
    var spore = sp_color(idx);
    spore = spore * (0.62 + 0.55 * weight + 0.45 * hit);
    spore = spore * (0.85 + 0.3 * treb_sig * treb);

    // Optional ring: a shaped ripple travelling from a direction.
    var ring = 0.0;
    let ring_amt = clamp(f_ring_intensity(), 0.0, 100.0) / 100.0;
    if (ring_amt > 0.001) {
        let shp = i32(round(clamp(f_ring_shape(), 0.0, 3.0)));
        let dir = i32(round(clamp(f_ring_direction(), 0.0, 5.0)));
        var origin = vec2<f32>(asp * 0.5, 0.5);
        if (dir == 2) { origin = vec2<f32>(asp * 0.5, 0.0); }
        if (dir == 3) { origin = vec2<f32>(asp * 0.5, 1.0); }
        if (dir == 4) { origin = vec2<f32>(0.0, 0.5); }
        if (dir == 5) { origin = vec2<f32>(asp, 0.5); }
        let reach = max(length(vec2<f32>(max(origin.x, asp - origin.x), max(origin.y, 1.0 - origin.y))), 0.4);
        let ph = fract(t * (0.18 + 0.5 * bass) + f_seed() * 0.13);
        var rad = ph * reach;
        if (dir == 1) { rad = (1.0 - ph) * reach; }
        let sd = sp_shape(c - origin, shp);
        let width = 0.012 + 0.05 * bass;
        ring = exp(-pow((sd - rad) / width, 2.0)) * ring_amt * (0.25 + 0.75 * bass_sig);
    }

    // Threads: nearest-neighbour mesh, or core spokes.
    var thread = 0.0;
    var thread_col = p_pal(2u, clamp(0.35 + 0.5 * h, 0.0, 1.0));
    let conn = clamp(f_connections(), 0.0, 100.0) / 100.0;
    if (conn > 0.001 && v.d2 < 90.0) {
        let core_mode = i32(round(clamp(f_connection_mode(), 0.0, 1.0))) == 1;
        var a = v.p1;
        var b = v.p2;
        if (core_mode) {
            let core = vec2<f32>(asp * 0.5, 0.5) + (sp_hash2(vec2<f32>(f_seed() * 17.0, 3.0)) - vec2<f32>(0.5)) * 0.35;
            a = core * inv;
            b = v.p1;
        }
        let idh = sp_hash(v.id + vec2<f32>(9.0, 23.0));
        let gate = step(idh, conn);
        let seg = b - a;
        let len = max(length(seg), 1e-5);
        let nrm = vec2<f32>(-seg.y, seg.x) / len;
        let wob = sin(t * (0.4 + clamp(f_thread_speed(), 0.0, 100.0) / 100.0 * 3.0) + idh * TAU);
        let loose = clamp(f_looseness(), 0.0, 100.0) / 100.0;
        let bend = (idh * 2.0 - 1.0) * loose * 0.8 + wob * loose * 0.35;
        let mid = (a + b) * 0.5 + nrm * bend * len;
        let dl = min(sp_seg_dist(pos, a, mid), sp_seg_dist(pos, mid, b));
        let lw_raw = clamp(f_stroke(), 0.2, 4.0) * 0.0009 / max(sp, 1e-4);
        let lw = max(lw_raw, 0.004);
        let laa = lw * 0.7 + 0.006;
        let ab = clamp(f_aberration(), 0.0, 100.0) / 100.0 * 0.09;
        let lr = 1.0 - smoothstep(lw, lw + laa, dl - ab);
        let lb = 1.0 - smoothstep(lw, lw + laa, dl + ab);
        let line_cov = max(lr, lb) * gate * clamp(f_line_opacity(), 0.0, 100.0) / 100.0;
        thread_col = thread_col + vec3<f32>(1.0, 0.15, 0.2) * lr * ab * 2.5
            + vec3<f32>(0.2, 0.4, 1.0) * lb * ab * 2.5;
        thread = clamp(line_cov, 0.0, 1.0);
    }

    // A dark plate so the field always reads as a picture, not a cutout.
    let plate = mix(p_pal(1u, 0.15), p_pal(1u, 0.65), clamp(uv.y * 0.8 + c.y * 0.2, 0.0, 1.0));
    var ink = plate * 0.11;
    ink = ink + p_pal(2u, 0.55) * (0.10 + 0.55 * cov) * exp(-dist * 55.0);
    ink = mix(ink, spore, cov);
    ink = mix(ink, thread_col, thread);
    ink = ink + p_pal(2u, 0.7) * ring * 0.9;
    ink = ink + p_grain(uv, t) * 0.004;

    let edge_a = 1.0 - smoothstep(0.47, 0.5, max(abs(uv.x - 0.5), abs(uv.y - 0.5)));
    return vec4<f32>(max(ink, vec3<f32>(0.0)), edge_a);
}
