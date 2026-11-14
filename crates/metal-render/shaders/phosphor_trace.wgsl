// phosphor-trace: a CRT oscilloscope face. Behaviour and parameter list follow
// the scraped built-in oscilloscope tool; everything here is original WGSL.
//
// A trace is a sampled polyline. For every echo (the newest plus a few
// persistence ghosts) the shader walks a figure from 0 to 1, transforms the
// point, and measures the distance from the pixel to each segment; the nearest
// segment lights the beam. Figure mode blends analytic Lissajous/spiral/rose/
// polygon/figure-8/cube/procedural shapes with a synthesized stereo waveform.
// Dwell dims the start of the trace, decimation coarsens its sampling, and
// trail/persistence decide how many past traces survive. Grid, axis, scan and
// displacement are drawn separately. Coverage fades at the margin so the
// scaffold's background shows there.

const TRACE_MAX: i32 = 48;

fn tr_seg_dist(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-8), 0.0, 1.0);
    return length(pa - ba * h);
}

// A synthesized waveform sample: main tone, a harmonic/noise layer and a
// high-frequency jitter term. `s` runs 0..1 across the face.
fn tr_wave(s: f32, t: f32) -> f32 {
    let a = (s - 0.5) * f_frequency() * TAU
        + t * f_speed() * 2.0
        + f_phase() * TAU;
    var w = sin(a);
    // Sine toward a squarer edge.
    w = mix(w, clamp(w * 4.0, -1.0, 1.0), clamp(f_waveform(), 0.0, 1.0));
    w = w + f_noise() * (
        0.45 * sin(a * 2.7 + 1.3)
        + 0.25 * (p_noise(vec2<f32>(s * 22.0 + f_seed(), t * 1.7)) - 0.5) * 2.0
    );
    w = w + f_jitter() * 0.22 * sin(s * 180.0 + t * 17.0 + f_seed() * 2.0);
    return w;
}

// A wireframe cube traced along its twelve edges and projected by hand.
fn tr_cube(s: f32, t: f32) -> vec2<f32> {
    var v = array<vec3<f32>, 8>(
        vec3<f32>(-1.0, -1.0, -1.0),
        vec3<f32>( 1.0, -1.0, -1.0),
        vec3<f32>( 1.0,  1.0, -1.0),
        vec3<f32>(-1.0,  1.0, -1.0),
        vec3<f32>(-1.0, -1.0,  1.0),
        vec3<f32>( 1.0, -1.0,  1.0),
        vec3<f32>( 1.0,  1.0,  1.0),
        vec3<f32>(-1.0,  1.0,  1.0)
    );
    var e = array<vec2<u32>, 12>(
        vec2<u32>(0u, 1u), vec2<u32>(1u, 2u), vec2<u32>(2u, 3u),
        vec2<u32>(3u, 0u), vec2<u32>(4u, 5u), vec2<u32>(5u, 6u),
        vec2<u32>(6u, 7u), vec2<u32>(7u, 4u), vec2<u32>(0u, 4u),
        vec2<u32>(1u, 5u), vec2<u32>(2u, 6u), vec2<u32>(3u, 7u)
    );
    let seg = s * 12.0;
    let i = u32(clamp(floor(seg), 0.0, 11.0));
    let edge = e[i];
    var q = mix(v[edge.x], v[edge.y], fract(seg));

    let ang = t * f_speed() * 0.6 + f_phase() * TAU;
    let c = cos(ang);
    let sn = sin(ang);
    q = vec3<f32>(c * q.x + sn * q.z, q.y, -sn * q.x + c * q.z);

    let ang2 = t * f_speed() * 0.37 + f_rotation() * PI * 0.5 + f_spin() * t;
    let c2 = cos(ang2);
    let s2 = sin(ang2);
    q = vec3<f32>(q.x, c2 * q.y - s2 * q.z, s2 * q.y + c2 * q.z);

    let persp = 2.6 / (2.6 + q.z * 0.6);
    return q.xy * persp * 0.85;
}

// One point of the traced path at parameter s, after gain, size, displacement,
// noise, warp, stereo rotation and drift.
fn tr_point(s: f32, t: f32) -> vec2<f32> {
    let k = i32(clamp(floor(f_figure() + 0.5), 0.0, 7.0));
    let a = s * TAU + f_phase() * TAU + t * f_speed() * 0.35;
    var g = vec2<f32>(0.0);

    if (k == 0) {
        g = vec2<f32>(cos(a), sin(a));
    } else if (k == 1) {
        g = vec2<f32>(
            sin(f_ratio_a() * a + PI * 0.5),
            sin(f_ratio_b() * a)
        );
    } else if (k == 2) {
        let r = 0.12 + 0.88 * s;
        let aa = a * (1.0 + f_ratio_a());
        g = r * vec2<f32>(cos(aa), sin(aa));
    } else if (k == 3) {
        let n = max(3.0, floor(f_ratio_a() + 0.5) + 2.0);
        let step = TAU / n;
        let turn = floor(a / max(step, 1e-4)) * step;
        let r = cos(PI / n) / max(cos(turn - step * 0.5), 0.08);
        g = r * vec2<f32>(cos(a), sin(a));
    } else if (k == 4) {
        let kk = max(1.0, f_ratio_a()) / max(1.0, f_ratio_b());
        let r = cos(kk * a * 2.0);
        g = r * vec2<f32>(cos(a * 2.0), sin(a * 2.0));
    } else if (k == 5) {
        g = vec2<f32>(sin(a), sin(a * 2.0) * 0.75);
    } else if (k == 6) {
        g = tr_cube(s, t);
    } else {
        // "custom": a procedural closed scribble keyed by seed.
        let h = p_hash(vec2<f32>(floor(s * 14.0) + f_seed(), 3.0));
        let r = 0.55 + 0.28 * sin(s * TAU * 2.0 + f_seed()) + 0.12 * h;
        g = r * vec2<f32>(cos(a * 2.0), sin(a * 3.0 + 0.5));
    }

    // The signal-looking trace.
    let scope = vec2<f32>((s - 0.5) * 1.8, tr_wave(s, t) * 0.6);
    let md = i32(clamp(floor(f_mode() + 0.5), 0.0, 2.0));
    var pt = g;
    if (md == 1) {
        pt = scope;
    } else if (md == 2) {
        pt = mix(g, scope, clamp(f_blend(), 0.0, 1.0));
    }

    // Displacement wriggles the path around its nominal figure.
    pt = pt + f_displacement() * 0.16 * vec2<f32>(
        sin(s * TAU * 3.0 + t * 2.1 + f_seed()),
        cos(s * TAU * 5.0 - t * 1.3)
    );
    // Gain and size scale the trace before the face is composed.
    let amp = (0.55 + 0.45 * clamp(f_size(), 0.0, 1.0))
        * (0.25 + 0.55 * clamp(f_gain(), 0.0, 2.0));
    pt = pt * amp;

    // Roughness and high-frequency jitter live on the path itself.
    pt = pt + f_noise() * 0.09 * vec2<f32>(
        p_fbm(vec2<f32>(s * 17.0 + f_seed(), t * 1.3), 2) - 0.5,
        p_fbm(vec2<f32>(s * 17.0 + 31.0 + f_seed(), -t * 1.1), 2) - 0.5
    );
    pt = pt + f_jitter() * 0.025 * vec2<f32>(
        sin(s * 160.0 + t * 21.0),
        cos(s * 145.0 - t * 19.0)
    );

    // Mid-band domain warp.
    if (f_warp() > 0.001) {
        let n1 = p_fbm(vec2<f32>(pt.x * 1.7 + f_seed(), pt.y * 1.7 - t * 0.2), 2) - 0.5;
        let n2 = p_fbm(vec2<f32>(pt.x * 1.7 + 7.0 - t * 0.17, pt.y * 1.7 + f_seed()), 2) - 0.5;
        pt = pt + vec2<f32>(n1, n2) * f_warp() * 0.65;
    }

    // Stereo rotation, slow spin and drift.
    pt = p_rot(pt, f_rotation() * PI * 0.25 + f_spin() * t * 1.4);
    pt = pt + f_trace_drift() * 0.07 * vec2<f32>(sin(t * 0.7), cos(t * 0.53));
    return pt;
}

// Core + wide halo for one trace age.
fn tr_layer(p: vec2<f32>, t: f32) -> vec2<f32> {
    let n = max(4.0, ceil(floor(48.0 / max(f_decimate(), 1.0)) * p_detail()));
    let wid = max(f_beam_width() * (0.55 + 0.9 * f_softness()), 0.0012);
    let upper = wid * (1.15 + 2.4 * f_softness());
    var core = 0.0;
    var halo = 0.0;
    var prev = tr_point(0.0, t);

    for (var i = 1; i < TRACE_MAX + 1; i = i + 1) {
        let fi = f32(i);
        if (fi > n) {
            break;
        }
        let s = fi / n;
        let cur = tr_point(s, t);
        let d = tr_seg_dist(p, prev, cur);
        // Dwell: the pen is dimmer where it entered the face.
        let dwell = mix(1.0 - f_dwell() * 0.85, 1.0, s);
        let c = 1.0 - smoothstep(wid, upper, d);
        let hw = wid * 4.5;
        let h = exp(-d * d / (2.0 * hw * hw));
        core = max(core, c * dwell);
        halo = max(halo, h * dwell);
        prev = cur;
    }
    return vec2<f32>(core, halo);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let hh = max(0.5 - f_margin(), 0.08);
    let ww = max(0.5 * p_aspect() - f_margin(), 0.08);
    let edge = 0.035;

    // Soft rectangular face: the margin is transparent coverage.
    let mx = 1.0 - smoothstep(ww - edge, ww, abs(p.x));
    let my = 1.0 - smoothstep(hh - edge, hh, abs(p.y));
    let face = mx * my;

    // Graticule.
    let div = max(floor(f_divisions() + 0.5), 2.0);
    let cx = 2.0 * ww / div;
    let cy = 2.0 * hh / div;
    let dx = abs(fract(p.x / cx + 0.5) - 0.5) * cx;
    let dy = abs(fract(p.y / cy + 0.5) - 0.5) * cy;
    let gw = 0.006;
    let inner = max(
        1.0 - smoothstep(0.0, gw, dx),
        1.0 - smoothstep(0.0, gw, dy)
    );
    let border = max(
        1.0 - smoothstep(0.0, 0.006, abs(abs(p.x) - ww)),
        1.0 - smoothstep(0.0, 0.006, abs(abs(p.y) - hh))
    );
    let graticule = max(inner, border);
    var col = p_pal(1u, 0.55) * graticule * (0.18 + 0.55 * clamp(f_grid(), 0.0, 1.0));

    // Centre axis and its tick rhythm.
    let aw = 0.012;
    let ax = 1.0 - smoothstep(0.0, aw, abs(p.y));
    let ay = 1.0 - smoothstep(0.0, aw, abs(p.x));
    let tick_x = 1.0 - smoothstep(0.0, aw, abs(p.x - round(p.x / (cx * 0.5)) * (cx * 0.5)));
    let tick_y = 1.0 - smoothstep(0.0, aw, abs(p.y - round(p.y / (cy * 0.5)) * (cy * 0.5)));
    let axis_line = max(ax, ay);
    let ticks = max(tick_x * ax, tick_y * ay) * 0.5;
    col = col + p_pal(1u, 0.9) * (axis_line + ticks) * clamp(f_axis(), 0.0, 1.0) * 0.55;

    // Persistence: newest trace plus decaying ghosts.
    var core = 0.0;
    var halo = 0.0;
    var ghost = 0.0;
    let layers = 1.0 + floor(clamp(f_trail(), 0.0, 1.0) * 3.0);
    for (var e = 0; e < 4; e = e + 1) {
        let fe = f32(e);
        if (fe >= layers) {
            break;
        }
        let tt = t - fe * (0.02 + f_trail() * 0.13);
        let r = tr_layer(p, tt);
        let att = select(1.0, pow(clamp(f_persistence(), 0.0, 0.99), fe), e > 0);
        let cw = select(1.0, 0.35, e > 0);
        core = max(core, r.x * att * cw);
        halo = max(halo, r.y * att);
        if (e > 0) {
            ghost = max(ghost, r.x * att);
        }
    }

    let bh = clamp(0.35 + 0.4 * sin(t * 0.7 + p.y * 2.0), 0.0, 1.0);
    let beam = p_pal(0u, bh);
    let after = p_pal(2u, clamp(0.4 + 0.4 * p.y, 0.0, 1.0));
    col = col + beam * core * 1.25;
    col = col + after * ghost * 0.65;
    col = col + beam * max(halo - core, 0.0) * clamp(f_glow(), 0.0, 1.0) * 1.6;
    col = col + beam * halo * clamp(f_bloom(), 0.0, 1.0) * 0.35;
    col = col + vec3<f32>(1.0) * core * core * 0.22;

    // CRT scan modulation, strongest at the centre of the face.
    let scan = 0.5 + 0.5 * sin(uv.y * 900.0 + t * 20.0);
    col = col * (1.0 - clamp(f_scan(), 0.0, 1.0) * 0.22 * scan);

    // Slight face shading so the picture is not flat.
    col = col * mix(0.82, 1.0, face);
    col = col * (0.9 + 0.1 * clamp(1.0 - length(p) * 1.2, 0.0, 1.0));

    let cov = clamp(face, 0.0, 1.0);
    return vec4<f32>(col, cov);
}
