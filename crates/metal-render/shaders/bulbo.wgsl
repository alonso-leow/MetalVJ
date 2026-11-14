// Bulbo: ray-marched 3D fractals, written from the maths of eight formulas of
// the mandelbulber2 catalogue (escape-time and IFS distance estimators).
//
//   0 Mandelbulb        z -> z^p (spherical) + c            log DE
//   1 Bristorbrot       z -> (x2-y2-z2, y(2x-z), z(2x+y)) + c  log DE
//   2 Quaternion Julia  q -> q^2 + c, 4D slice w = const     log DE
//   3 Mandelbox         box fold, sphere fold, scale + c     linear DE
//   4 Amazing Box       xy box fold, radial scale + c        linear DE
//   5 Menger sponge     abs, sort, scale, shift              box DE / scale^n
//   6 Sierpinski tetra  tetra fold, scale, shift             tetra DE / scale^n
//   7 Kleinian          wrap, 180 turn, inversion            limit-set DE
//
// Every formula is evaluated in its own coordinates q = p * S_f, where S_f
// brings the object to roughly unit scale, and its distance is divided by S_f
// on the way out. `bu_map` returns (distance, orbit trap); with `mezcla_f > 0`
// it evaluates the next formula as well and crossfades both.
//
// The CPU mirrors of formulas 0 and 5 live in models/bulbo.rs (tested).

struct BuU {
    orb: ColorSlot,
    background: ColorSlot,
    // aspect, time, fondo_alpha, 0
    a: vec4<f32>,
    // formula A, formula B, mezcla_f, potencia
    f: vec4<f32>,
    // iteraciones, escala_f, pliegue, pasos
    g: vec4<f32>,
    // offset xyz, precision
    o: vec4<f32>,
    // julia xyz, julia_mix
    j: vec4<f32>,
    // per-iteration rotation, rows
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    // aoc, sombra, glow, niebla
    h: vec4<f32>,
    // trampa_modo, trampa_mezcla, trampa_ciclos, 0
    t: vec4<f32>,
    // light direction xyz, luz_dura
    light: vec4<f32>,
    // brillo, contraste, opacidad, fondo_nivel
    l: vec4<f32>,
    // tono, saturacion, tan(fov/2), pixel angle
    l2: vec4<f32>,
    eye: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    fwd: vec4<f32>,
    // The `body` part's picture and `mat_*`, one `MaterialU` — see
    // material.wgsl, prepended ahead of this file.
    mat_body: MaterialU,
};

@group(0) @binding(0) var<uniform> u: BuU;
@group(0) @binding(1) var surf_body_tex: texture_2d<f32>;
@group(0) @binding(2) var surf_body_samp: sampler;

const BU_R: f32 = 1.9;

fn bu_rot(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(u.r0.xyz, v), dot(u.r1.xyz, v), dot(u.r2.xyz, v));
}

// One orbit-trap sample of the current z; the minimum over the orbit is kept.
fn bu_trap(z: vec3<f32>) -> f32 {
    let m = i32(u.t.x + 0.5);
    if (m == 0) { return length(z); }
    if (m == 1) { return min(abs(z.x), min(abs(z.y), abs(z.z))); }
    if (m == 2) { return length(z.xy); }
    if (m == 3) { return abs(length(z) - 1.0); }
    return 1e3;
}

// Mode 4 colours by escape iteration instead of by a geometric trap.
fn bu_tfin(tr: f32, i: i32, n: i32) -> f32 {
    if (i32(u.t.x + 0.5) >= 4) { return 2.0 * f32(i) / f32(max(n, 1)); }
    return min(tr, 4.0);
}

fn bu_sdbox(z: vec3<f32>, h: f32) -> f32 {
    let q = abs(z) - vec3<f32>(h);
    return length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0);
}

// The pixel's own constant, or the Julia constant, by julia_mix.
fn bu_c(q: vec3<f32>) -> vec3<f32> {
    return mix(q, u.j.xyz, u.j.w) + u.o.xyz;
}

fn bu_bulb(q: vec3<f32>) -> vec2<f32> {
    let n = i32(u.g.x);
    let pw = u.f.w;
    let c = bu_c(q) * u.g.y;
    let aoff = (u.g.z - 1.0) * 0.6;
    var z = q;
    var dr = 1.0;
    var r = length(z);
    var tr = 1e3;
    var i = 0;
    for (; i < n; i = i + 1) {
        r = length(z);
        if (r > 4.0) { break; }
        tr = min(tr, bu_trap(z));
        let th = acos(clamp(z.z / max(r, 1e-6), -1.0, 1.0)) * pw;
        let ph = (atan2(z.y, z.x) + aoff) * pw;
        let rp = pow(r, pw - 1.0);
        dr = rp * pw * dr + 1.0;
        z = rp * r * vec3<f32>(sin(th) * cos(ph), sin(th) * sin(ph), cos(th));
        z = bu_rot(z) + c;
    }
    r = length(z);
    let d = 0.5 * log(max(r, 1e-6)) * r / dr;
    return vec2<f32>(d, bu_tfin(tr, i, n));
}

fn bu_bristor(q: vec3<f32>) -> vec2<f32> {
    let n = i32(u.g.x);
    let c = bu_c(q) * u.g.y;
    var z = q;
    var dr = 1.0;
    var r = length(z);
    var tr = 1e3;
    var i = 0;
    for (; i < n; i = i + 1) {
        r = length(z);
        if (r > 4.0) { break; }
        tr = min(tr, bu_trap(z));
        dr = max(dr * 2.0 * r, 1e-4);
        z = vec3<f32>(z.x * z.x - z.y * z.y - z.z * z.z, z.y * (2.0 * z.x - z.z), z.z * (2.0 * z.x + z.y));
        z = bu_rot(z) + c;
    }
    r = length(z);
    let d = 0.5 * log(max(r, 1e-6)) * r / dr;
    return vec2<f32>(d, bu_tfin(tr, i, n));
}

fn bu_quat(q: vec3<f32>) -> vec2<f32> {
    let n = i32(u.g.x);
    // Natural constant is the Julia one; julia_mix slides it to the pixel.
    let c = (mix(u.j.xyz, q, u.j.w) + u.o.xyz) * u.g.y;
    var w = vec4<f32>(q, (u.g.z - 1.0) * 0.5);
    var dr = 1.0;
    var r = length(w);
    var tr = 1e3;
    var i = 0;
    for (; i < n; i = i + 1) {
        r = length(w);
        if (r > 4.0) { break; }
        tr = min(tr, bu_trap(w.xyz));
        dr = max(dr * 2.0 * r, 1e-4);
        w = vec4<f32>(w.x * w.x - w.y * w.y - w.z * w.z - w.w * w.w, 2.0 * w.x * w.y, 2.0 * w.x * w.z, 2.0 * w.x * w.w);
        w = vec4<f32>(bu_rot(w.xyz) + c, w.w);
    }
    r = length(w);
    let d = 0.5 * log(max(r, 1e-6)) * r / dr;
    return vec2<f32>(d, bu_tfin(tr, i, n));
}

fn bu_mbox(q: vec3<f32>) -> vec2<f32> {
    let n = i32(u.g.x);
    let s = clamp(2.0 * u.g.y, 1.6, 4.0);
    let lim = clamp(u.g.z, 0.3, 2.0);
    let c = bu_c(q);
    var z = q;
    var dr = 1.0;
    var tr = 1e3;
    var i = 0;
    for (; i < n; i = i + 1) {
        if (length(z) > 20.0) { break; }
        z = clamp(z, vec3<f32>(-lim), vec3<f32>(lim)) * 2.0 - z;
        let r2 = dot(z, z);
        tr = min(tr, bu_trap(z));
        if (r2 < 0.25) {
            z = z * 4.0;
            dr = dr * 4.0;
        } else if (r2 < 1.0) {
            z = z / r2;
            dr = dr / r2;
        }
        z = bu_rot(z) * s + c;
        dr = dr * s + 1.0;
    }
    let d = 0.5 * length(z) / abs(dr);
    return vec2<f32>(d, bu_tfin(tr, i, n));
}

fn bu_abox(q: vec3<f32>) -> vec2<f32> {
    let n = i32(u.g.x);
    let s = 1.5 * u.g.y;
    let a = clamp(u.g.z, 0.3, 2.0);
    let c = bu_c(q);
    var z = q;
    var dr = 1.0;
    var tr = 1e3;
    var i = 0;
    for (; i < n; i = i + 1) {
        if (length(z) > 20.0) { break; }
        z.x = abs(z.x + a) - abs(z.x - a) - z.x;
        z.y = abs(z.y + a) - abs(z.y - a) - z.y;
        let rr = dot(z, z);
        tr = min(tr, bu_trap(z));
        let div = select(min(rr, 1.0), 0.70710678, rr < 0.70710678);
        let m = s / div;
        z = z * m;
        dr = dr * abs(m) + 1.0;
        z = bu_rot(z) + vec3<f32>(c.y, c.x, c.z);
    }
    let d = 0.5 * length(z) / abs(dr);
    return vec2<f32>(d, bu_tfin(tr, i, n));
}

fn bu_menger(q: vec3<f32>) -> vec2<f32> {
    let n = i32(u.g.x);
    let s = clamp(3.0 * u.g.y, 2.0, 4.5);
    let off = clamp(u.g.z, 0.5, 1.5) * (s - 1.0);
    let hb = max(s - off, 0.3);
    let add = u.j.xyz * u.j.w * 0.3;
    var z = q + u.o.xyz * 0.5;
    var dr = 1.0;
    var tr = 1e3;
    var i = 0;
    for (; i < n; i = i + 1) {
        tr = min(tr, bu_trap(z));
        z = abs(z);
        if (z.x < z.y) { z = z.yxz; }
        if (z.x < z.z) { z = z.zyx; }
        if (z.y < z.z) { z = z.xzy; }
        z = z * s;
        z.x = z.x - off;
        z.y = z.y - off;
        if (z.z > 0.5 * off) { z.z = z.z - off; }
        dr = dr * s;
        z = bu_rot(z) + add;
    }
    let d = bu_sdbox(z, hb) / dr;
    return vec2<f32>(d, bu_tfin(tr, i, n));
}

fn bu_tetra(q: vec3<f32>) -> vec2<f32> {
    let n = i32(u.g.x);
    let s = clamp(2.0 * u.g.y, 1.5, 3.0);
    let off = clamp(u.g.z, 0.4, 1.6) * (s - 1.0);
    let add = u.j.xyz * u.j.w * 0.3;
    var z = q + u.o.xyz * 0.5;
    var dr = 1.0;
    var tr = 1e3;
    var i = 0;
    for (; i < n; i = i + 1) {
        tr = min(tr, bu_trap(z));
        if (z.x + z.y < 0.0) { z = vec3<f32>(-z.y, -z.x, z.z); }
        if (z.x + z.z < 0.0) { z = vec3<f32>(-z.z, z.y, -z.x); }
        if (z.y + z.z < 0.0) { z = vec3<f32>(z.x, -z.z, -z.y); }
        z = z * s - vec3<f32>(off);
        dr = dr * s;
        z = bu_rot(z) + add;
    }
    // Distance to the base tetrahedron (vertices (1,1,1), (1,-1,-1), ...).
    let m = max(max(-z.x - z.y - z.z, -z.x + z.y + z.z), max(z.x - z.y + z.z, z.x + z.y - z.z));
    let d = (m - 1.0) * 0.57735027 / dr;
    return vec2<f32>(d, bu_tfin(tr, i, n));
}

fn bu_klein(q: vec3<f32>) -> vec2<f32> {
    let n = i32(u.g.x);
    let a = clamp(1.8462756 + (u.g.z - 1.0) * 0.12, 1.5, 2.1);
    let b = 0.0112785 + (u.g.y - 1.0) * 0.08;
    let bs = vec3<f32>(0.92436, 0.90756, 0.92436);
    let box1 = vec3<f32>(2.0 * bs.x, a, 2.0 * bs.z);
    let box2 = vec3<f32>(-bs.x, 1.0 - bs.y, -bs.z);
    var z = q + vec3<f32>(0.0, a * 0.5, 0.0) + u.o.xyz * 0.5 + u.j.xyz * u.j.w * 0.3;
    var dr = 1.0;
    var tr = 1e3;
    var i = 0;
    for (; i < n; i = i + 1) {
        z = bu_rot(z - vec3<f32>(0.0, a * 0.5, 0.0)) + vec3<f32>(0.0, a * 0.5, 0.0);
        z = z - box1 * floor((z - box2) / box1);
        if (z.y >= a * (0.5 + 0.2 * sin(3.14159265 * (z.x + b * 0.5) / bs.x))) {
            z = vec3<f32>(-b, a, 0.0) - z;
        }
        let rr = max(dot(z, z), 1e-6);
        tr = min(tr, bu_trap(z));
        let ir = 1.0 / rr;
        z = z * -ir;
        z.x = -b - z.x;
        z.y = a + z.y;
        dr = dr * ir;
        if (dr > 1e6) { break; }
    }
    let zy = min(z.y, a - z.y);
    let d = min(zy, 1.0) / max(abs(dr), 1.0);
    return vec2<f32>(max(d, 0.0), bu_tfin(tr, i, n));
}

// The escape-time formulas have their pole on z: stand them up (y is up in the
// world). The IFS solids get a fixed turn so the default view is not face-on.
fn bu_up(p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(p.x, -p.z, p.y);
}
fn bu_tilt(p: vec3<f32>) -> vec3<f32> {
    let c1 = cos(0.7); let s1 = sin(0.7);
    let c2 = cos(0.5); let s2 = sin(0.5);
    let a = vec3<f32>(c1 * p.x + s1 * p.z, p.y, -s1 * p.x + c1 * p.z);
    return vec3<f32>(a.x, c2 * a.y - s2 * a.z, s2 * a.y + c2 * a.z);
}

// Formula `id` in world space: scale to its own coordinates and back.
fn bu_one(p: vec3<f32>, id: i32) -> vec2<f32> {
    var r = vec2<f32>(1.0, 1.0);
    var sc = 1.0;
    switch (id) {
        case 0: { sc = 0.92; r = bu_bulb(bu_up(p) * sc); }
        case 1: { sc = 0.85; r = bu_bristor(bu_up(p) * sc); }
        case 2: { sc = 0.85; r = bu_quat(bu_up(p) * sc); }
        case 3: { sc = 7.0; r = bu_mbox(bu_tilt(p) * sc); }
        case 4: { sc = 5.0; r = bu_abox(bu_tilt(p) * sc); }
        case 5: { sc = 1.0; r = bu_menger(bu_tilt(p) * sc); }
        case 6: { sc = 1.0; r = bu_tetra(bu_tilt(bu_tilt(p)) * sc); }
        default: { sc = 2.4; r = bu_klein(p * sc); }
    }
    return vec2<f32>(r.x / sc, r.y);
}

fn bu_map(p: vec3<f32>) -> vec2<f32> {
    var r = bu_one(p, i32(u.f.x));
    if (u.f.z > 0.001) {
        r = mix(r, bu_one(p, i32(u.f.y)), u.f.z);
    }
    // Keep the shape inside the bounding ball without cutting open shells.
    r.x = max(r.x, length(p) - BU_R);
    return r;
}

fn bu_normal(p: vec3<f32>, h: f32) -> vec3<f32> {
    let k = vec2<f32>(1.0, -1.0);
    let n = k.xyy * bu_map(p + k.xyy * h).x + k.yyx * bu_map(p + k.yyx * h).x
          + k.yxy * bu_map(p + k.yxy * h).x + k.xxx * bu_map(p + k.xxx * h).x;
    let l = length(n);
    if (l < 1e-12) { return vec3<f32>(0.0, 1.0, 0.0); }
    return n / l;
}

fn bu_shade(p: vec3<f32>, rd: vec3<f32>, t: f32, steps: f32, eps: f32, suv: vec2<f32>) -> vec3<f32> {
    let n0 = bu_normal(p, max(eps * 1.5, 1e-4));
    let tr = bu_map(p).y;
    let trap = (tr / (1.0 + tr)) * u.t.z;
    let tri = 1.0 - abs(fract(trap * 0.5) * 2.0 - 1.0);
    let geo = clamp(0.5 + 0.5 * n0.y, 0.0, 1.0);
    let tt = clamp(mix(geo, tri, u.t.y), 0.0, 1.0);
    let base0 = grad4(u.orb, tt);

    // The shared material, off at mix 0 (a migrated row): the orbit colour is
    // replaced by the kind or a `tex.body` picture, and its bump/glitch
    // perturb the normal the whole shade below reads.
    let V = -rd;
    let shaded = material_apply(
        u.mat_body, surf_body_tex, surf_body_samp, base0, p, n0, suv, V, u.a.y,
    );
    let n = shaded.normal;
    let base = shaded.albedo;

    let L = normalize(u.light.xyz);
    let dif = dot(n, L);
    let dura = clamp(u.light.w, 0.0, 1.0);
    let lam = mix(clamp(dif * 0.5 + 0.5, 0.0, 1.0), smoothstep(0.0, 0.1, dif), dura);

    // Soft shadow: sphere-trace towards the light, keep the narrowest cone.
    var sh = 1.0;
    if (u.h.y > 0.001 && dif > 0.0) {
        var ts = eps * 6.0;
        for (var k = 0; k < 14; k = k + 1) {
            let d = bu_map(p + n * eps * 3.0 + L * ts).x;
            sh = min(sh, 8.0 * d / ts);
            ts = ts + clamp(d, 0.02, 0.4);
            if (sh < 0.02 || ts > 3.0) { break; }
        }
        sh = clamp(sh, 0.0, 1.0);
    }
    sh = mix(1.0, sh, u.h.y);

    // Cheap AO: three taps along the normal plus the step count.
    var occ = 0.0;
    var sca = 1.0;
    for (var k = 1; k < 4; k = k + 1) {
        let hh = 0.035 * f32(k) * f32(k);
        let d = bu_map(p + n * hh).x;
        occ = occ + (hh - d) * sca;
        sca = sca * 0.65;
    }
    let ao = clamp(1.0 - u.h.x * (2.2 * occ + 0.6 * steps), 0.0, 1.0);

    let H = normalize(L + V);
    var spec_exp = mix(12.0, 120.0, dura);
    var spec_amt = 0.5;
    if (u.mat_body.b.x > 0.0) {
        spec_exp = mix(4.0, 160.0, 1.0 - clamp(shaded.roughness, 0.0, 1.0));
        spec_amt = 0.5 * mix(1.0, 2.0, clamp(shaded.metalness, 0.0, 1.0));
    }
    let spec = pow(max(dot(n, H), 0.0), spec_exp) * spec_amt * sh * step(0.0, dif);
    var col = base * (0.18 + 0.82 * lam * sh) * ao;
    col = col + vec3<f32>(spec) * ao;
    let rim = pow(1.0 - clamp(dot(n, V), 0.0, 1.0), 3.0) * 0.25;
    col = col + grad4(u.orb, min(tt + 0.3, 1.0)) * rim * ao;
    return col;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let ndc = vec2<f32>((in.uv.x * 2.0 - 1.0) * aspect, 1.0 - in.uv.y * 2.0);
    let rd = normalize(u.fwd.xyz + u.right.xyz * ndc.x * u.l2.z + u.up.xyz * ndc.y * u.l2.z);
    let ro = u.eye.xyz;
    let bgc = backdrop_from_uv(u.background, in.uv, aspect) * u.l.w;
    var col = bgc;
    var cover = 0.0;
    var glow = 0.0;

    let b = dot(ro, rd);
    let c = dot(ro, ro) - BU_R * BU_R;
    let h2 = b * b - c;
    if (h2 > 0.0) {
        let sq = sqrt(h2);
        let tend = -b + sq;
        let t0 = max(-b - sq, 0.0);
        var t = t0;
        var dmin = 1e9;
        var tm = t;
        var hit = false;
        var used = 0;
        let px = max(u.l2.w, 1e-5);
        let prec = max(u.o.w, 0.05);
        let maxs = i32(u.g.w);
        if (tend > 0.0) {
            for (var i = 0; i < 112; i = i + 1) {
                if (i >= maxs) { break; }
                used = i + 1;
                let d = bu_map(ro + rd * t).x;
                let eps = max(t * px * 0.6 / prec, 2e-5);
                let rr = d / (max(t, 0.1) * px);
                if (rr < dmin) { dmin = rr; tm = t; }
                if (d < eps) { hit = true; break; }
                t = t + max(d * 0.85, eps * 0.5);
                if (t > tend) { break; }
            }
            let sf = f32(used) / f32(max(maxs, 1));
            glow = u.h.z * sf * sf;
            if (hit || dmin < 1.5) {
                let ts = select(tm, t, hit);
                let eps = max(ts * px * 0.6 / prec, 2e-5);
                let sc = bu_shade(ro + rd * ts, rd, ts, sf, eps, in.uv);
                cover = select(0.5 * (1.0 - smoothstep(0.0, 1.5, dmin)), 1.0, hit);
                let fg = clamp(1.0 - exp(-u.h.w * (ts - t0) * 0.7), 0.0, 1.0);
                col = mix(bgc, mix(sc, bgc, fg), cover);
            }
        }
    }
    col = col + grad4(u.orb, 0.9) * glow * 0.6;
    cover = max(cover, clamp(glow, 0.0, 1.0));
    col = kit_look(col, u.l.x, u.l.y, u.l2.x, u.l2.y);
    var alpha = u.l.z;
    if (u.a.z > 0.5) { alpha = alpha * max(cover, u.l.w); }
    return vec4<f32>(col, alpha);
}
