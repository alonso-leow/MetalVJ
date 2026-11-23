// Nubes, the step pass: one explicit MUSCL-Hancock step of the 2D compressible
// Euler equations on a periodic grid, written from the equations (the scheme
// of Mocz's finite-volume tutorial: primitive slopes with an optional limiter,
// a half-step predictor, face extrapolation, local Lax-Friedrichs / Rusanov
// fluxes, a conservative update). Pull-based: a cell needs the fluxes on its
// four faces, so it reconstructs itself and its four neighbours (taps at
// distance 2 along each axis) from the previous state.
//
// State per cell, RGBA16F: r = rho, g = rho*u, b = rho*v, a = E - Eref (the
// total energy density minus the energy of the ambient pressure, so the
// float16 mantissa is spent on the deviations). Cell size is 1 in the update;
// `r = dt/dx` is the only time-step number.
//
// Safety: every primitive read is clamped (density floor, speed cap, pressure
// between a floor and rho*cmax^2/gamma), and the state written is rebuilt from
// clamped primitives, so no input can make the field NaN or grow without bound.

struct StepU {
    // w, h, r = dt/dx, gamma
    dims: vec4<f32>,
    // limiter, viscosity coefficient, gravity*dt, mean density
    lim: vec4<f32>,
    // Eref, rho floor, pressure floor, speed cap
    cap: vec4<f32>,
    // sound speed cap, init flag, ambient pressure, 0
    cap2: vec4<f32>,
    // rho_hi, rho_lo, shear, layer width
    ic0: vec4<f32>,
    // perturbation amplitude, temperature gradient, 0, 0
    ic1: vec4<f32>,
    cyc: vec4<f32>,
    phs: vec4<f32>,
    amp: vec4<f32>,
    // injection: x, y (cells), amount this step (0 = off), radius (cells)
    inj: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: StepU;
@group(0) @binding(1) var prev: texture_2d<f32>;
@group(0) @binding(2) var prev_smp: sampler;

const TAU: f32 = 6.2831853;
const RHO_MAX: f32 = 60.0;

// Round to the nearest float16 value *here*, in f32 arithmetic, so what the
// render target stores is exact whatever rounding mode the hardware converts
// with (some truncate, which bleeds mass out of a conservative scheme: the
// mean density fell by two thirds over twenty seconds before this).
fn q16(v: vec4<f32>) -> vec4<f32> {
    let a = max(abs(v), vec4<f32>(1.0e-30));
    let ulp = max(exp2(floor(log2(a)) - 10.0), vec4<f32>(5.9604645e-8));
    return round(v / ulp) * ulp;
}

fn ld(p: vec2<i32>) -> vec4<f32> {
    let sz = vec2<i32>(i32(u.dims.x), i32(u.dims.y));
    let q = (p + sz * 4) % sz;
    return textureLoad(prev, q, 0);
}

// Primitive (rho, vx, vy, P) from a stored state, clamped.
fn prim(s: vec4<f32>) -> vec4<f32> {
    let g = u.dims.w;
    let rho = clamp(s.x, u.cap.y, RHO_MAX);
    let v = clamp(s.yz / rho, vec2<f32>(-u.cap.w), vec2<f32>(u.cap.w));
    let e = s.w + u.cap.x;
    let p = (e - 0.5 * rho * dot(v, v)) * (g - 1.0);
    let pmax = max(rho * u.cap2.x * u.cap2.x / g, u.cap.z);
    return vec4<f32>(rho, v, clamp(p, u.cap.z, pmax));
}

// Stored state from a primitive.
fn cons(q: vec4<f32>) -> vec4<f32> {
    let e = q.w / (u.dims.w - 1.0) + 0.5 * q.x * dot(q.yz, q.yz);
    return vec4<f32>(q.x, q.x * q.yz, e - u.cap.x);
}

// Clamp a face/predicted primitive so the flux never sees a vacuum.
fn safe(q: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(max(q.x, u.cap.y), clamp(q.yz, vec2<f32>(-u.cap.w), vec2<f32>(u.cap.w)), max(q.w, u.cap.z));
}

struct Recon {
    xm: vec4<f32>,
    xp: vec4<f32>,
    ym: vec4<f32>,
    yp: vec4<f32>,
};

// Slope limiter of the source: each slope is scaled by min(1, ratio) against
// the one-sided differences, clipped at 0; blended in by `limiter`.
fn limit(f: vec4<f32>, fl: vec4<f32>, fr: vec4<f32>, g: vec4<f32>) -> vec4<f32> {
    let z = vec4<f32>(0.0);
    let one = vec4<f32>(1.0);
    let eps = vec4<f32>(1.0e-8);
    var a = g + select(z, eps, g == z);
    let g1 = max(z, min(one, (f - fl) / a)) * g;
    a = g1 + select(z, eps, g1 == z);
    let g2 = max(z, min(one, (fr - f) / a)) * g1;
    return mix(g, g2, u.lim.x);
}

fn recon(cp: vec2<i32>) -> Recon {
    let c = prim(ld(cp));
    let xl = prim(ld(cp + vec2<i32>(-1, 0)));
    let xr = prim(ld(cp + vec2<i32>(1, 0)));
    let yl = prim(ld(cp + vec2<i32>(0, -1)));
    let yr = prim(ld(cp + vec2<i32>(0, 1)));
    let gx = limit(c, xl, xr, 0.5 * (xr - xl));
    let gy = limit(c, yl, yr, 0.5 * (yr - yl));
    // gx = (d rho, d vx, d vy, d P) along x; gy likewise along y.
    let r = u.dims.z;
    let g = u.dims.w;
    let rho = c.x;
    let vx = c.y;
    let vy = c.z;
    let p = c.w;
    var h = c;
    h.x = rho - 0.5 * r * (vx * gx.x + rho * gx.y + vy * gy.x + rho * gy.z);
    h.y = vx - 0.5 * r * (vx * gx.y + vy * gy.y + gx.w / rho);
    h.z = vy - 0.5 * r * (vx * gx.z + vy * gy.z + gy.w / rho);
    h.w = p - 0.5 * r * (g * p * (gx.y + gy.z) + vx * gx.w + vy * gy.w);
    var o: Recon;
    o.xm = safe(h - 0.5 * gx);
    o.xp = safe(h + 0.5 * gx);
    o.ym = safe(h - 0.5 * gy);
    o.yp = safe(h + 0.5 * gy);
    return o;
}

// Rusanov flux across a face. `ax` 0: the face normal is x; 1: it is y.
// States are primitives (rho, vx, vy, P); the result is the flux of
// (rho, rho*vx, rho*vy, E).
fn flux(l: vec4<f32>, rr: vec4<f32>, ax: i32) -> vec4<f32> {
    let g = u.dims.w;
    var vnl = l.y; var vtl = l.z; var vnr = rr.y; var vtr = rr.z;
    if (ax == 1) { vnl = l.z; vtl = l.y; vnr = rr.z; vtr = rr.y; }
    let enl = l.w / (g - 1.0) + 0.5 * l.x * (vnl * vnl + vtl * vtl);
    let enr = rr.w / (g - 1.0) + 0.5 * rr.x * (vnr * vnr + vtr * vtr);
    let rs = 0.5 * (l.x + rr.x);
    let mn = 0.5 * (l.x * vnl + rr.x * vnr);
    let mt = 0.5 * (l.x * vtl + rr.x * vtr);
    let es = 0.5 * (enl + enr);
    let ps = max((g - 1.0) * (es - 0.5 * (mn * mn + mt * mt) / rs), 0.0);
    var fm = mn;
    var fn_ = mn * mn / rs + ps;
    var ft = mn * mt / rs;
    var fe = (es + ps) * mn / rs;
    let cl = sqrt(g * l.w / l.x) + abs(vnl);
    let cr = sqrt(g * rr.w / rr.x) + abs(vnr);
    let c = max(cl, cr);
    fm = fm - c * 0.5 * (rr.x - l.x);
    fn_ = fn_ - c * 0.5 * (rr.x * vnr - l.x * vnl);
    ft = ft - c * 0.5 * (rr.x * vtr - l.x * vtl);
    fe = fe - c * 0.5 * (enr - enl);
    if (ax == 0) { return vec4<f32>(fm, fn_, ft, fe); }
    return vec4<f32>(fm, ft, fn_, fe);
}

fn initial(cc: vec2<f32>) -> vec4<f32> {
    let h = u.dims.y;
    let w = u.dims.x;
    let x = cc.x / h;
    let y = cc.y / h;
    let wid = max(u.ic0.w, 0.002);
    // 1 inside the central band, 0 outside; symmetric about y = 0.5 so it is
    // periodic in y.
    let s = 0.5 * (1.0 - tanh((abs(y - 0.5) - 0.25) / wid));
    var rho = u.ic0.y + (u.ic0.x - u.ic0.y) * s;
    rho = rho * (1.0 - 0.35 * u.ic1.y * sin(TAU * y));
    let vx = u.ic0.z * (2.0 * s - 1.0);
    let sig = 0.0354 + wid;
    let env = exp(-pow(y - 0.25, 2.0) / (2.0 * sig * sig)) + exp(-pow(y - 0.75, 2.0) / (2.0 * sig * sig));
    let kx = cc.x / w * TAU;
    let wave = u.amp.x * sin(u.cyc.x * kx + u.phs.x) + u.amp.y * sin(u.cyc.y * kx + u.phs.y)
        + u.amp.z * sin(u.cyc.z * kx + u.phs.z) + u.amp.w * sin(u.cyc.w * kx + u.phs.w);
    let vy = u.ic1.x * wave * env;
    return q16(cons(vec4<f32>(rho, vx, vy, u.cap2.z)));
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
    let cc = vec2<f32>(p) + 0.5;
    if (u.cap2.y > 0.5) { return initial(cc); }

    let r = u.dims.z;
    let me = recon(p);
    let rl = recon(p + vec2<i32>(-1, 0));
    let rr = recon(p + vec2<i32>(1, 0));
    let rd = recon(p + vec2<i32>(0, -1));
    let ru = recon(p + vec2<i32>(0, 1));
    let fxm = flux(rl.xp, me.xm, 0);
    let fxp = flux(me.xp, rr.xm, 0);
    let fym = flux(rd.yp, me.ym, 1);
    let fyp = flux(me.yp, ru.ym, 1);

    let s0 = ld(p);
    var s = s0 - r * ((fxp - fxm) + (fyp - fym));

    // Viscosity: diffusion of the momentum (conserves its sum).
    if (u.lim.y > 0.0) {
        let lap = ld(p + vec2<i32>(-1, 0)).yz + ld(p + vec2<i32>(1, 0)).yz
            + ld(p + vec2<i32>(0, -1)).yz + ld(p + vec2<i32>(0, 1)).yz - 4.0 * s0.yz;
        s = vec4<f32>(s.x, s.yz + u.lim.y * lap, s.w);
    }

    // Buoyancy: gravity on the density anomaly, so the net momentum stays zero.
    if (u.lim.z != 0.0) {
        let an = s0.x - u.lim.w;
        let vy = s0.z / max(s0.x, u.cap.y);
        s.z = s.z + u.lim.z * an;
        s.w = s.w + u.lim.z * an * vy;
    }

    // A puff on the beat: heat, a little cloud and an outward push.
    if (u.inj.z > 0.0) {
        let sz = vec2<f32>(u.dims.x, u.dims.y);
        var d = cc - u.inj.xy;
        d = d - sz * round(d / sz);
        let rad = max(u.inj.w, 1.0);
        let wgt = exp(-dot(d, d) / (2.0 * rad * rad));
        let dir = d / max(length(d), 0.5);
        let a = u.inj.z * wgt;
        s.x = s.x + 0.6 * a;
        s.y = s.y + 1.2 * a * dir.x;
        s.z = s.z + 1.2 * a * dir.y;
        s.w = s.w + 5.0 * a;
    }

    return q16(cons(prim(s)));
}
