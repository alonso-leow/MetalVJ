// swarmfield: a dense particle cloud. Behaviour and controls follow the
// built-in "Particles 2.0" tool; this WGSL is original procedural scaffold code.
//
// The stateful 150k-dot simulation is replaced by a time-scrolled hashed site
// field: every cell of a jittered grid holds one point, and a 5x5 search gives
// the nearest one. Points are shaped on a ball (sphere, turbulent blob, twisted
// torus, spiked star) and morphed toward that target, then pushed around by a
// ring of drifting attractors. Several taps backwards along the flow make the
// motion trails; depth is derived from the ball shell plus a per-site hash and
// drives focus / depth-of-field (dots blur larger and dimmer out of focus).
// Chromatic fringes are drawn only near the attractors, as in the source.

struct SwSite {
    d: f32,
    p: vec2<f32>,
    cell: vec2<f32>,
    h: f32,
}

const SW_R: f32 = 0.45;

fn sw_spacing() -> f32 {
    let den = clamp(f_density(), 0.0, 100.0) / 100.0;
    return mix(0.105, 0.016, pow(den, 0.72));
}

fn sw_site_hash(cell: vec2<f32>) -> f32 {
    return p_hash(cell + vec2<f32>(f_seed() * 17.3, f_seed() * 9.1));
}

fn sw_shape_pos(base: vec2<f32>, t: f32) -> vec2<f32> {
    let mode = i32(round(clamp(f_shape(), 0.0, 3.0)));
    let morph = clamp(f_morph() / 100.0, 0.0, 1.0);
    let tw = clamp(f_twist() / 100.0, 0.0, 1.0);
    let spk = clamp(f_spikes() / 100.0, 0.0, 1.0);
    let tur = clamp(f_turbulence() / 100.0, 0.0, 1.0);
    let u = base / SW_R;
    let r = max(length(u), 1e-4);
    var shaped = base;

    if (mode == 1) {
        let o = f_seed() * TAU;
        let ce = 1.0
            + 0.22 * sin(4.0 * u.x * PI + o)
            + 0.18 * sin(4.0 * u.y * PI + o * 1.3)
            + 0.16 * sin(4.0 * (u.x + u.y) * PI + o * 0.7);
        let bulge = max(0.45, ce);
        let nd = vec2<f32>(
            sin(u.y * 7.0 + o + t * 0.31) * cos(u.x * 5.0),
            cos(u.x * 6.0 - o - t * 0.27) * sin(u.y * 4.0)
        );
        shaped = base * mix(1.0, bulge, tur) + nd * tur * SW_R * 0.22;
    } else if (mode == 2) {
        let aa = atan2(u.y, u.x + 1e-5) * 2.0 + r * 1.8;
        shaped = vec2<f32>(
            base.x * (1.0 + sin(aa) * tw * 0.65),
            base.y + cos(aa + u.x * 2.5) * length(base) * tw * 0.45
        );
    } else if (mode == 3) {
        let i2 = 1.3
            + sin(u.x * 4.0 + u.y * 3.0) * cos(u.y * 5.0 + u.x * 2.0) * 0.55
            + r * 0.45;
        shaped = base * mix(1.0, i2, spk);
    }

    var q = mix(base, shaped, morph);

    // Global twist: differential rotation, always active so the knob reads in
    // every shape.
    let ta = tw * (0.4 + 0.9 * r) * 1.6;
    q = p_rot(q, ta);

    // Global starburst, strongest away from the centre.
    let ang = atan2(q.y, q.x + 1e-5);
    let starburst = 1.0 + spk * 0.75 * sin(ang * 6.0 + t * 0.2)
        * smoothstep(0.15, 1.0, r);
    q = q * starburst;

    // Global turbulent displacement.
    let nd2 = vec2<f32>(
        sin(q.y * 13.0 + t * 0.5 + f_seed()),
        cos(q.x * 11.0 - t * 0.43 + f_seed() * 1.7)
    );
    q = q + nd2 * tur * SW_R * 0.16;
    return q;
}

fn sw_site(cell: vec2<f32>, t: f32, sp: f32) -> vec2<f32> {
    let key = cell + vec2<f32>(f_seed() * 3.7, f_seed() * 8.1);
    let j = p_hash2(key) - vec2<f32>(0.5);
    let jit = 0.15 + 0.75 * clamp(f_jitter() / 100.0, 0.0, 1.0);
    let scat = clamp(f_scatter() / 100.0, 0.0, 1.0);
    let base = (cell + vec2<f32>(0.5) + j * jit) * sp
        + (p_hash2(key + vec2<f32>(23.0, 41.0)) - vec2<f32>(0.5)) * sp * scat * 1.2;
    return sw_shape_pos(base, t);
}

fn sw_sample(pp: vec2<f32>, sp: f32, t: f32) -> SwSite {
    let gq = pp / max(sp, 1e-4);
    let ip = floor(gq);
    var r: SwSite;
    r.d = 1e9;
    r.p = vec2<f32>(0.0);
    r.cell = ip;
    r.h = 0.0;
    for (var j = -2; j <= 2; j = j + 1) {
        for (var i = -2; i <= 2; i = i + 1) {
            let cell = ip + vec2<f32>(f32(i), f32(j));
            let site = sw_site(cell, t, sp);
            let d = length(site - pp);
            if (d < r.d) {
                r.d = d;
                r.p = site;
                r.cell = cell;
                r.h = sw_site_hash(cell);
            }
        }
    }
    return r;
}

fn sw_depth(pos: vec2<f32>, h: f32) -> f32 {
    let rr = clamp(length(pos) / SW_R, 0.0, 1.0);
    let shell = sqrt(max(1.0 - rr * rr, 0.0));
    return clamp(0.5 * shell + 0.5 * h, 0.0, 1.0);
}

fn sw_attractor(k: f32, t: f32) -> vec2<f32> {
    let ph = t * (0.15 + 0.08 * k) + k * 2.39996 + f_seed() * 0.4;
    let rad = 0.30 + 0.14 * sin(k * 1.7 + f_seed());
    return vec2<f32>(
        cos(ph) * rad * (1.0 + 0.25 * sin(k * 1.3)),
        sin(ph * 0.83 + k) * rad * 0.8
    );
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let zoom = max(f_swarm_zoom(), 0.05);
    let sp = sw_spacing();
    let basep = p / zoom;

    // Camera rotation from rotation speed and direction.
    let speed = 0.15 + clamp(f_speed() / 100.0, 0.0, 1.0) * 1.4;
    let spin = t * speed * (0.2 + clamp(f_rotation_speed() / 100.0, 0.0, 1.0) * 2.2);
    let dir = i32(round(clamp(f_direction(), 0.0, 3.0)));
    var sgn = 1.0;
    var ctr = vec2<f32>(0.0);
    if (dir == 0) {
        sgn = -1.0;
        ctr = vec2<f32>(-0.12, 0.0);
    } else if (dir == 1) {
        sgn = 1.0;
        ctr = vec2<f32>(0.12, 0.0);
    } else if (dir == 2) {
        sgn = 1.0;
        ctr = vec2<f32>(0.0, 0.12);
    } else {
        sgn = -1.0;
        ctr = vec2<f32>(0.0, -0.12);
    }
    var pp = p_rot(basep - ctr, sgn * spin) + ctr;
    if (dir >= 2) {
        pp = vec2<f32>(pp.x + sin(spin) * basep.y * 0.28, pp.y);
    }

    // Attractors: a ring of drifting wells that compress the field and mark the
    // spots where the chromatic fringe lives.
    let natt_a = i32(round(clamp(f_attractors(), 0.0, 8.0)));
    // Attractors scale with p_detail, never below a quarter (and 0 stays 0).
    let natt = min(natt_a, i32(ceil(f32(natt_a) * p_detail())));
    let ar = clamp(f_attractor_radius(), 0.02, 0.6);
    let grav = clamp(f_gravity() / 100.0, 0.0, 1.0);
    var pull = vec2<f32>(0.0);
    var near_d = 1e9;
    for (var k: i32 = 0; k < 8; k = k + 1) {
        if (k >= natt) {
            break;
        }
        let a = sw_attractor(f32(k), t);
        let dv = a - pp;
        let dd = length(dv);
        let w = exp(-dd * dd / max(ar * ar, 1e-5));
        pull = pull + dv * w;
        near_d = min(near_d, dd);
    }
    let pullc = pull * grav * 0.8 / (1.0 + length(pull));
    pp = pp + pullc;

    let spot = 1.0 - smoothstep(0.0, ar * 1.4, near_d);
    let ab = clamp(f_aberration() / 100.0, 0.0, 1.0);
    let glow = clamp(f_glow(), 0.0, 200.0) / 100.0;
    let flick = clamp(f_flicker() / 100.0, 0.0, 1.0);
    let szv = clamp(f_size_variation() / 100.0, 0.0, 1.0);
    let size01 = clamp(f_size() / 100.0, 0.0, 1.0);
    let focus = clamp(f_focus() / 100.0, 0.0, 1.0);
    let dof = max(clamp(f_dof() / 100.0, 0.0, 1.0), 0.02);
    let trail = clamp(f_trail() / 100.0, 0.0, 1.0);
    let swirl = clamp(f_swirl() / 100.0, 0.0, 1.0);

    // Flow direction for the motion smear: tangential swirl plus camera spin.
    let tang = vec2<f32>(-basep.y, basep.x);
    let flow = normalize(
        tang * (0.4 + swirl)
        + vec2<f32>(cos(sgn * spin), sin(sgn * spin)) * 0.5
        + vec2<f32>(1e-5, 0.0)
    );
    let tr = trail * sp * 3.0;
    let base_r = 0.0015 + pow(size01, 1.3) * 0.018;

    var ink = vec3<f32>(0.0);
    var alpha = 0.0;
    for (var k: i32 = 0; k < 3; k = k + 1) {
        let off = flow * tr * f32(k);
        let sm = sw_sample(pp - off, sp, t);

        let depth = sw_depth(sm.p, sm.h);
        let blur = pow(clamp(abs(depth - focus) / dof, 0.0, 1.0), 0.7);
        let hvar = 0.6 + 0.9 * sm.h;
        let radius = max(base_r * mix(1.0, hvar, szv) * (1.0 + blur * 3.2), 0.0005);
        let aa = radius * 0.35 + 0.0005;
        let env = 1.0 - smoothstep(0.90, 1.08, length(sm.p) / SW_R);
        let cov = (1.0 - smoothstep(radius - aa, radius + aa, sm.d))
            * env * mix(1.0, 0.14, blur);

        let h2 = p_hash(sm.cell + vec2<f32>(17.0, 5.0));
        let bright = 1.0
            - flick * 0.5 * (0.5 + 0.5 * sin(t * (2.0 + 4.0 * h2) + sm.h * TAU));
        var tint = p_pal(0u, clamp(0.2 + 0.7 * sm.h, 0.0, 1.0)) * bright;

        // Halo around each dot.
        tint = tint + p_pal(2u, 0.6) * glow * exp(-sm.d * 45.0) * 0.55;

        // Chromatic fringe: warm on one side, cool on the other, only in the
        // attractor spots (as the source's spot mask does).
        let dvp = normalize(pp - sm.p + vec2<f32>(1e-5, 0.0));
        let side = dvp.x;
        let rim = smoothstep(radius * 0.5, radius * 1.1, sm.d)
            * (1.0 - smoothstep(radius * 1.1, radius * 2.4, sm.d));
        tint = tint + p_pal(1u, 0.5) * max(side, 0.0) * rim * ab * spot * 1.8;
        tint = tint + p_pal(2u, 0.5) * max(-side, 0.0) * rim * ab * spot * 1.8;

        let w = 1.0 / (1.0 + f32(k) * 0.7);
        ink = ink + tint * cov * w;
        alpha = alpha + cov * w * (1.0 - alpha);

        // The nearest tap draws the attractor bloom once; the others only add
        // their dots.
        if (k == 0) {
            let halo = (1.0 - smoothstep(0.0, ar * 2.0, near_d)) * glow * 0.5;
            ink = ink + p_pal(2u, 0.5) * halo;
            ink = ink + p_pal(1u, 0.5) * halo * ab * 0.6;
        }
    }

    // Soft plate: gives the empty region a tint while leaving the backdrop
    // visible away from the cloud.
    let fill = clamp(f_fill() / 100.0, 0.0, 1.0);
    let plate_env = exp(-dot(basep, basep) * 2.2);
    let plate_a = fill * 0.35 * plate_env;
    ink = ink + p_pal(0u, 0.35) * plate_a;
    alpha = clamp(alpha + plate_a, 0.0, 1.0);
    ink = ink + p_grain(uv, t) * 0.004;

    return vec4<f32>(max(ink, vec3<f32>(0.0)) * (1.0 + 0.06 * clamp(f_motion() / 100.0, 0.0, 1.0)), alpha);
}
