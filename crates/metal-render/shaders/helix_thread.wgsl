// helix-thread: a double-helix filament. Behaviour follows the "DNA Filament"
// sketch tool: two counter-phased strands, periodic rungs, a twisting axis,
// weave, drift, field noise and a soft glow. The layout regime is chosen by
// `mode` (horizontal, vertical, diagonal, ring); everything here is original.
//
// Own floats are read through the generated `f_<name>()` accessors. Slots:
// 0 strands, 1 rungs, 2 glow/backdrop.

struct HtFrame {
    s: f32,
    a: f32,
    turn: f32,
    rung_f: f32,
    ring: f32,
}

// Map the picture into (along, across) helix coordinates.
fn ht_frame(p: vec2<f32>, t: f32) -> HtFrame {
    let m = u32(clamp(round(f_mode()), 0.0, 3.0));
    let twistn = clamp(f_twist() / 200.0, -1.0, 1.0);
    let rungs = max(round(f_rungs()), 1.0);

    var q = p;
    var turn = 1.0 + 9.0 * twistn;
    var rung_f = rungs * 0.65;
    var ring = 0.0;

    if (m == 1u) {
        q = vec2<f32>(p.y, p.x);
    } else if (m == 2u) {
        q = p_rot(p, PI * 0.25 + 0.12 * sin(t * 0.17));
    } else if (m == 3u) {
        let ang = atan2(p.y, p.x);
        let rad = length(p);
        q = vec2<f32>(ang, rad);
        ring = 1.0;
        let turns = max(round(2.0 + abs(twistn) * 5.0), 1.0);
        turn = select(-turns, turns, twistn >= 0.0);
        rung_f = rungs / TAU;
    }

    let ln = 0.55 + clamp(f_length(), 10.0, 240.0) / 240.0;
    // In ring mode make the phase and the rung count close over one turn, so
    // the atan2 seam does not show.
    if (ring > 0.5) {
        let cycles = max(round(abs(turn) * ln * 2.4 / TAU), 1.0);
        turn = select(-cycles, cycles, twistn >= 0.0) * TAU / (ln * 2.4);
        rung_f = max(round(rungs * ln), 1.0) / (TAU * ln);
    }
    var r: HtFrame;
    r.s = q.x * ln;
    r.a = q.y;
    r.turn = turn;
    r.rung_f = rung_f;
    r.ring = ring;
    return r;
}

// Wandering axis of the helix, so the filament is not a rigid sine.
fn ht_axis(s: f32, t: f32, ring: f32) -> f32 {
    let curl = clamp(f_curl() / 200.0, -1.0, 1.0);
    let driftn = clamp(f_drift_v() / 200.0, 0.0, 1.0);
    let fieldn = clamp(f_field() / 200.0, 0.0, 1.0);
    let ph = t * (0.35 + 2.2 * driftn);
    var w = sin(s * (1.8 + 2.4 * fieldn) + ph) * (0.045 + 0.08 * fieldn) * curl;
    w = w + sin(s * 5.1 - ph * 0.7 + f_seed()) * 0.018 * fieldn;
    if (ring > 0.5) {
        w = w * (1.0 + 0.8 * fieldn);
    }
    return w;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t0 = p_time();
    let t = t0 * (clamp(f_speed(), 0.0, 260.0) / 60.0);
    let fr = ht_frame(p, t);
    let s = fr.s;
    let a = fr.a;

    let spreadn = clamp(f_spread() / 200.0, 0.0, 1.0);
    let bundlen = clamp(f_bundle() / 200.0, 0.0, 1.0);
    let fieldn = clamp(f_field() / 200.0, 0.0, 1.0);
    let weaven = clamp(f_weave() / 200.0, -1.0, 1.0);
    let taper = clamp(f_taper(), 0.0, 1.0);

    let ax = ht_axis(s, t + f_seed() * 0.017, fr.ring);
    var amp = (0.03 + 0.28 * spreadn) * (0.62 + 0.45 * bundlen);
    amp = amp * (1.0 - 0.55 * taper);

    // Phase: signed twist, time (bpm-scaled), seed and the axis wobble.
    let ph = s * fr.turn * 2.4 + t * 1.35 + f_seed() * 0.7 + ax * 2.0;
    let wob = weaven * 0.055 * sin(ph * 1.7 + t * 0.6);
    let amp2 = amp * (1.0 + 0.18 * sin(ph * 0.47 + 1.3));

    // Two counter-phased strands.
    let o1 = ax + (amp2 + wob) * sin(ph);
    let o2 = ax - (amp2 + wob) * sin(ph);

    let d1 = abs(a - o1);
    let d2 = abs(a - o2);
    let dst = min(d1, d2);

    let w = clamp(f_width(), 0.5, 20.0) * 0.00115;
    let aa = 0.0009 + w * 0.8;
    let strand = 1.0 - p_edge(dst, w, aa);

    // Rungs: line segments between the two strands at a regular spacing.
    let rs = s * fr.rung_f;
    let rr = fract(rs);
    let ds = min(rr, 1.0 - rr) / max(fr.rung_f, 1e-3);
    let lo = min(o1, o2);
    let hi = max(o1, o2);
    let da = max(max(lo - a, a - hi), 0.0);
    let dr = length(vec2<f32>(ds, da));
    let rung = (1.0 - p_edge(dr, w * 0.85, aa)) * step(0.002, hi - lo);

    // Taper the filament toward the frame edge (or the outer ring).
    var env = 1.0;
    if (fr.ring > 0.5) {
        env = 1.0 - taper * smoothstep(0.52, 0.95, a);
    } else {
        env = 1.0 - taper * smoothstep(0.35, 0.85, abs(s));
    }

    let glown = clamp(f_glow(), 0.0, 1.0);
    let gstrand = exp(-dst / (0.004 + w * 4.0)) * glown;
    let grung = exp(-dr / (0.005 + w * 3.0)) * glown;

    // "Lines": a fine filament striation, denser as the count rises.
    let ln = clamp(f_lines(), 4.0, 180.0);
    let fib = 0.5 + 0.5 * sin(a * (6.0 + ln * 0.55) + s * 2.3 + t * 0.5);
    let stri = mix(1.0, 0.55 + 0.65 * fib, 0.28 + 0.22 * bundlen);

    // Backdrop haze so the frame is never empty.
    let neb = p_fbm(p * (1.3 + 1.8 * fieldn) + vec2<f32>(t * 0.05, -t * 0.04), 3);
    var col = p_pal(0u, 0.02) * 0.22;
    col = col + p_pal(2u, 0.06) * (0.05 + 0.12 * fieldn);
    col = col + p_pal(2u, clamp(0.15 + 0.5 * neb, 0.0, 1.0)) * (0.04 + 0.10 * fieldn) * neb;

    let strand_t = clamp(0.25 + 0.62 * (0.5 + 0.5 * sin(ph * 0.7 + s * 1.3)), 0.0, 1.0);
    let scol = p_pal(0u, strand_t);
    let rcol = p_pal(1u, clamp(0.2 + 0.7 * fract(rs * 0.37 + 0.11), 0.0, 1.0));
    let gcol = p_pal(2u, clamp(0.5 + 0.4 * sin(ph * 0.5 + 2.0), 0.0, 1.0));

    col = col + scol * strand * (0.85 + 0.35 * fib) * env;
    col = col + rcol * rung * env;
    col = col + gcol * (gstrand * 0.75 + grung * 0.45) * (0.7 + 0.6 * fib);
    col = col * stri;

    // Keep the camera-independent part (grain, vignette) on screen uv.
    let grain = p_grain(uv, t0) * 0.035;
    let vd = uv - vec2<f32>(0.5);
    col = col * (1.0 - 0.35 * dot(vd, vd)) + vec3<f32>(grain);

    let cov = clamp(0.80 + 0.20 * neb, 0.0, 1.0);
    return vec4<f32>(max(col, vec3<f32>(0.0)), cov);
}
