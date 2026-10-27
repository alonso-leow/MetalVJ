// Ondas, the step pass: one leapfrog of the 2D TMz Yee scheme (Ez, Hx, Hy),
// pull-based. State per cell, RGBA16F: r = Ez, g = Hx (at j+1/2), b = Hy (at
// i+1/2), a = running mean of the energy density (for the flux monitor).
// The H half-step is recomputed inline for the neighbours E needs, so the
// whole leapfrog is one pass over the previous state. Out of the grid the
// field is zero (a perfect conductor); a conductivity ramp along the border,
// the same for E and H so the layer is impedance matched, absorbs outgoing
// waves; a tiny bulk loss keeps a closed cavity from growing without bound.
//
// dims = (w, h, courant S, sim time); src = (x, y, freq, width cells)
// src2 = (type, count, phase per source, conductivity)
// pulse = (start time or -1, amplitude, clear flag, 0)

struct StepU {
    dims: vec4<f32>,
    src: vec4<f32>,
    src2: vec4<f32>,
    pulse: vec4<f32>,
    m0: vec4<f32>,
    m1: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: StepU;
@group(0) @binding(1) var prev: texture_2d<f32>;
@group(0) @binding(2) var prev_smp: sampler;

const BORDER: f32 = 14.0;
const BULK: f32 = 0.0008;
const LIMIT: f32 = 200.0;

fn ld(p: vec2<i32>) -> vec4<f32> {
    let sz = vec2<i32>(i32(u.dims.x), i32(u.dims.y));
    if (p.x < 0 || p.y < 0 || p.x >= sz.x || p.y >= sz.y) { return vec4<f32>(0.0); }
    return textureLoad(prev, p, 0);
}

fn sigma(c: vec2<f32>) -> f32 {
    let d = min(min(c.x, u.dims.x - c.x), min(c.y, u.dims.y - c.y));
    let r = clamp((BORDER - d) / BORDER, 0.0, 1.0);
    return BULK + u.src2.w * 0.6 * r * r;
}

// (Hx, Hy) after the half step, at cell p.
fn hnew(p: vec2<i32>) -> vec2<f32> {
    let sz = vec2<i32>(i32(u.dims.x), i32(u.dims.y));
    if (p.x < 0 || p.y < 0 || p.x >= sz.x || p.y >= sz.y) { return vec2<f32>(0.0); }
    let s = u.dims.z;
    let c = ld(p);
    let k = sigma(vec2<f32>(p) + 0.5) * s * 0.5;
    let da = (1.0 - k) / (1.0 + k);
    let db = s / (1.0 + k);
    let ex = ld(p + vec2<i32>(0, 1)).x;
    let ey = ld(p + vec2<i32>(1, 0)).x;
    return vec2<f32>(da * c.y - db * (ex - c.x), da * c.z + db * (ey - c.x));
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    if (u.pulse.z > 0.5) { return vec4<f32>(0.0); }
    let p = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
    let cc = vec2<f32>(p) + 0.5;
    let s = u.dims.z;
    let me = ld(p);
    let h0 = hnew(p);
    let hxm = hnew(p + vec2<i32>(0, -1)).x;
    let hym = hnew(p + vec2<i32>(-1, 0)).y;
    let curl = (h0.y - hym) - (h0.x - hxm);

    let sd = on_sd(cc, u.m0, u.m1, u.dims.y);
    let eps = on_eps(sd, u.m0);
    let k = sigma(cc) * s * 0.5;
    var ez = ((1.0 - k) / (1.0 + k)) * me.x + (s / (eps * (1.0 + k))) * curl;

    // Soft sources: a gaussian blob of current, `count` of them in a column.
    let f = u.src.z;
    let w = max(u.src.w, 0.3);
    let n = i32(max(u.src2.y, 1.0));
    let tipo = i32(floor(u.src2.x + 0.5));
    let spacing = 0.1 * u.dims.y;
    var inj = 0.0;
    for (var i = 0; i < 6; i = i + 1) {
        if (i < n) {
            let sp = vec2<f32>(u.src.x, u.src.y + (f32(i) - f32(n - 1) * 0.5) * spacing);
            let d = cc - sp;
            let blob = exp(-dot(d, d) / (2.0 * w * w));
            inj = inj + blob * on_wave(tipo, u.dims.w, f, u.src2.z * f32(i + 1));
        }
    }
    if (u.pulse.x >= 0.0) {
        let d = cc - vec2<f32>(u.src.x, u.src.y);
        let wp = 2.0 * w;
        inj = inj + u.pulse.y * exp(-dot(d, d) / (2.0 * wp * wp)) * on_ricker(u.dims.w - u.pulse.x, f);
    }
    ez = ez + (s / eps) * 2.0 * inj;

    if (on_is_wall(sd, u.m0)) { ez = 0.0; }
    ez = clamp(ez, -LIMIT, LIMIT);
    let hx = clamp(h0.x, -LIMIT, LIMIT);
    let hy = clamp(h0.y, -LIMIT, LIMIT);
    let en = 0.5 * (eps * ez * ez + hx * hx + hy * hy);
    let avg = me.w + (min(en, 1000.0) - me.w) * 0.03;
    return vec4<f32>(ez, hx, hy, avg);
}
