// Brasa, the step: one generation of the fire automaton.
//
// Heat lives in .r, sparks in .g, smoke in .b. A cell takes the heat of the
// cell one row towards the source, shifted sideways by the wind, and loses a
// random amount of it. The source row is rewritten from the ignition profile.

struct StepU {
    // step index, fresh (1 = clear the grid), rise (1 = source at the bottom), mode
    s: vec4<u32>,
    // res_x, res_y, decay, decay_noise
    a: vec4<f32>,
    // viento, viento_vel, turbulencia, source time
    b: vec4<f32>,
    // ritmo, ancho, forma, emisores_n
    c: vec4<f32>,
    // brasas, humo, 0, 0
    d: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: StepU;
@group(0) @binding(1) var prev: texture_2d<f32>;
@group(0) @binding(2) var prev_s: sampler;

// Integer hash of (x, y, step, salt) -> 0..1. Seeded by the step index only,
// never by a wall clock, so a replay of the same steps is the same fire.
fn bh(x: i32, y: i32, n: u32, salt: u32) -> f32 {
    var h = u32(x) * 747796405u + u32(y) * 2891336453u + n * 277803737u + salt * 1664525u + 1013904223u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    return f32(h & 0xffffffu) / 16777216.0;
}

fn env_at(t: f32, rhythm: f32, phase: f32) -> f32 {
    if (rhythm <= 0.0) { return 1.0; }
    return 0.35 + 0.65 * exp(-3.0 * fract(t * rhythm + phase));
}

fn window(xn: f32, width: f32) -> f32 {
    let d = abs(xn - 0.5);
    return 1.0 - smoothstep(width * 0.5 - 0.05, width * 0.5 + 0.02, d);
}

fn ignition(x: i32, n: u32) -> f32 {
    let w = u.a.x;
    let xn = (f32(x) + 0.5) / w;
    let t = u.b.w;
    let rhythm = u.c.x;
    let width = clamp(u.c.y, 0.05, 1.0);
    let forma = i32(round(u.c.z));
    let mode = u.s.w;
    var shape = window(xn, width);
    if (forma == 1) { shape = shape * xn; }
    else if (forma == 2) {
        let g = (xn - 0.5) / max(width * 0.35, 0.02);
        shape = exp(-g * g);
    } else if (forma == 3) {
        shape = shape * step(fract(xn * 8.0), 0.5);
    }
    var heat = 0.0;
    if (mode == 0u) {
        heat = shape * env_at(t, rhythm, 0.0);
    } else if (mode == 1u) {
        // A stand-in spectrum: eight bands, each an envelope of its own speed.
        // The lowest bands are the strongest, like a real one.
        let f = xn * 7.0;
        let k0 = floor(f);
        let k1 = min(k0 + 1.0, 7.0);
        let sp = 1.5 + rhythm;
        let a0 = (0.5 + 0.5 * sin(t * sp * (0.6 + hash11(k0 + 3.0)) + 6.2832 * hash11(k0)))
            * (1.0 - 0.45 * k0 / 7.0);
        let a1 = (0.5 + 0.5 * sin(t * sp * (0.6 + hash11(k1 + 3.0)) + 6.2832 * hash11(k1)))
            * (1.0 - 0.45 * k1 / 7.0);
        let amp = mix(a0, a1, smoothstep(0.0, 1.0, f - k0));
        heat = shape * (0.15 + 0.85 * amp) * (0.88 + 0.12 * bh(x, 0, n, 9u));
    } else {
        let cnt = i32(clamp(round(u.c.w), 1.0, 12.0));
        for (var i = 0; i < 12; i = i + 1) {
            if (i >= cnt) { break; }
            let fi = f32(i);
            let pos = fract(hash11(fi + 1.0) + 0.07 * sin(t * 0.3 + fi * 1.7));
            let g = (xn - pos) / max(width * 0.08, 0.01);
            heat = max(heat, exp(-g * g) * env_at(t, rhythm, hash11(fi + 10.0)));
        }
    }
    return clamp(heat, 0.0, 1.0);
}

fn lw(p: vec2<i32>) -> vec2<i32> {
    let w = i32(u.a.x);
    let h = i32(u.a.y);
    return vec2<i32>(((p.x % w) + w) % w, clamp(p.y, 0, h - 1));
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    if (u.s.y == 1u) { return vec4<f32>(0.0); }
    let p = vec2<i32>(floor(in.pos.xy));
    let w = i32(u.a.x);
    let h = i32(u.a.y);
    let n = u.s.x;
    let rise = u.s.z == 1u;
    var r = p.y;
    var dy = -1;
    if (rise) { r = h - 1 - p.y; dy = 1; }
    if (r == 0) {
        return vec4<f32>(ignition(p.x, n), 0.0, 0.0, 1.0);
    }
    // Wind: a mean push, a per-cell jitter, and a slow swell that varies
    // with the row and the step. Stochastic rounding keeps fractions alive.
    let swell = 0.5 * (sin(f32(r) * 0.23 + f32(n) * 0.11 + 1.3) + sin(f32(r) * 0.071 - f32(n) * 0.07));
    let wind = u.b.x * 1.2 + (bh(p.x, p.y, n, 1u) - 0.5) * 2.0 * u.b.y * 1.5 + u.b.z * 2.5 * swell;
    let dx = i32(floor(wind + bh(p.x, p.y, n, 2u)));
    let sp = lw(vec2<i32>(p.x - dx, p.y + dy));
    let src = textureLoad(prev, sp, 0);
    let dec = u.a.z * (1.0 - u.a.w + u.a.w * 2.0 * bh(p.x, p.y, n, 3u));
    let heat = max(src.r - dec, 0.0);

    // Sparks climb two rows a step and die slowly.
    let sp2 = lw(vec2<i32>(p.x - dx, p.y + 2 * dy));
    var spark = max(textureLoad(prev, sp2, 0).g - 0.03, 0.0);
    if (heat > 0.4 && bh(p.x, p.y, n, 4u) < u.d.x * 0.004) { spark = 1.0; }

    // Smoke is born where the flame dies and drifts with it.
    var smoke = src.b * 0.985;
    if (src.r > 0.04 && heat < 0.12) { smoke = max(smoke, u.d.y * clamp(src.r * 6.0, 0.0, 1.0)); }
    return vec4<f32>(heat, spark, smoke, 1.0);
}
