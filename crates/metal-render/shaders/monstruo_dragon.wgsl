// monstruo — DRAGON (species 0).
//
// A long-muzzled dragon head: heavy brow ridge, swept-back horns ringed with
// emissive bands, a crest of fins down the skull and snout, cheek frills that
// flare when the mouth opens, whisker barbels, an opal thin-film scaled skin
// whose dark gaps glow neon, and a mouth that is a furnace. Behind it a
// domain-warped fire tunnel that folds into a kaleidoscope as `trip` rises.
//
// The skull is two solids cut along the jaw seam: the upper head (evaluated at
// p) and the mandible (evaluated at `jaw_rest(p)`), so `jaw_open` opens the
// whole jaw and the gape runs back along the muzzle to the hinge, its cut
// faces shaded as gum. Everything else (lips, teeth, tongue, eyes, lids) is
// the rig's, hand-composed from the rig parts with bounding volumes.
//
// Helpers are prefixed `dr_`. Materials from 10 up.

const M_JAW: f32 = 10.0;
const M_HORN: f32 = 11.0;
const M_SPINE: f32 = 12.0;
const M_FRILL: f32 = 13.0;
const M_BARBEL: f32 = 14.0;
const M_GUM: f32 = 15.0;

const DR_TAU: f32 = 6.2831853;

// ---- small helpers ----------------------------------------------------------

fn dr_rbox2(q: vec2<f32>, h: vec2<f32>, r: f32) -> f32 {
    let d = abs(q) - h + vec2<f32>(r);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

fn dr_noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let w = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = mix(lattice3(i), lattice3(i + vec3<f32>(1.0, 0.0, 0.0)), w.x);
    let b = mix(lattice3(i + vec3<f32>(0.0, 1.0, 0.0)), lattice3(i + vec3<f32>(1.0, 1.0, 0.0)), w.x);
    let c = mix(lattice3(i + vec3<f32>(0.0, 0.0, 1.0)), lattice3(i + vec3<f32>(1.0, 0.0, 1.0)), w.x);
    let d = mix(lattice3(i + vec3<f32>(0.0, 1.0, 1.0)), lattice3(i + vec3<f32>(1.0, 1.0, 1.0)), w.x);
    return mix(mix(a, b, w.y), mix(c, d, w.y), w.z);
}

fn dr_fbm3(p: vec3<f32>, oct: i32) -> f32 {
    var t = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 5; i++) {
        if (i >= oct) { break; }
        t += a * dr_noise3(q);
        q = q * 2.03 + vec3<f32>(1.7, 9.2, 3.1);
        a *= 0.5;
    }
    return t;
}

fn dr_octaves() -> i32 {
    return 2 + i32(3.0 * clamp(k_detalle(), 0.0, 1.0));
}

// The flow clock: never frozen, faster with flujo.
fn dr_clock() -> f32 {
    return k_time() * (0.12 + 1.3 * k_flujo());
}

// A furnace ramp from the mouth slot to the neon white-hot end.
fn dr_heat(t: f32) -> vec3<f32> {
    let a = grad4(u.mouth, 0.33) * 1.4;
    let b = grad4(u.mouth, 0.5) * 1.9;
    let c = mix(grad4(u.mouth, 0.66), grad4(u.neon, 1.0), 0.5) * 1.7;
    let x = clamp(t, 0.0, 1.0);
    return mix(mix(a, b, smoothstep(0.0, 0.6, x)), c, smoothstep(0.6, 1.0, x));
}

// ---- the rest mouth and the jaw seam ---------------------------------------

// The upper teeth never move, so their roots give the rest mouth exactly.
fn dr_mouth_rest() -> vec3<f32> {
    let r = u.face.upper_teeth[4].xyz;
    return vec3<f32>(0.0, r.y - 0.06, r.z + 0.0616);
}

// Signed distance to the jaw seam (pivot → rest mouth line), + above.
fn dr_seam(q: vec3<f32>) -> f32 {
    let m = dr_mouth_rest();
    let pv = jaw_pivot();
    let d = normalize(vec2<f32>(m.z - pv.z, m.y - pv.y));
    let r = vec2<f32>(q.z - pv.z, q.y - pv.y);
    return d.x * r.y - d.y * r.x;
}

// ---- the upper head ---------------------------------------------------------

fn dr_muzzle(p: vec3<f32>) -> f32 {
    let m = dr_mouth_rest();
    let w = s_jaw_width();
    let z0 = 0.2;
    let zf = m.z + 0.1;
    let t = clamp((p.z - z0) / (zf - z0), 0.0, 1.0);
    let cy = mix(0.12, m.y + 0.215, t);
    let hx = mix(0.47, 0.34, t) * (0.55 + 0.375 * w);
    let hy = mix(0.34, 0.215, t);
    let d2 = dr_rbox2(vec2<f32>(p.x, p.y - cy), vec2<f32>(hx, hy), 0.13);
    let dy = p.y - (cy + 0.02);
    let front = (p.z - (zf - 3.8 * dy * dy - 0.8 * p.x * p.x)) * 0.55;
    return smax(d2, front, 0.1) * 0.85;
}

fn dr_upper(p: vec3<f32>, side: i32, s: f32) -> f32 {
    let ridge = s_brow_ridge();
    var d = sd_ellipsoid(p - vec3<f32>(0.0, 0.16, -0.12), vec3<f32>(0.74, 0.72, 0.82));
    d = smin(d, dr_muzzle(p), 0.2);
    // Jowl mass down to the hinge.
    d = smin(d, sd_ellipsoid(p - vec3<f32>(s * 0.6, -0.14, 0.05), vec3<f32>(0.26, 0.3, 0.42)), 0.15);
    // Cheek, from the rig (puffs).
    let ch = cheek(side);
    d = smin(d, length(p - ch.xyz) - ch.w * 0.8, 0.14);
    // Eye mound, then the socket.
    let e = eye_centre(side);
    let er = eye_radius(side);
    d = smin(d, length(p - e + vec3<f32>(0.0, 0.0, 0.06)) - er * 1.25, 0.08);
    d = smax(d, -(length(p - e) - er * 1.1), 0.03);
    // Brow ridge, from the rig's brow points, swept back toward the horns.
    let b0 = brow(side, 0);
    let b1 = brow(side, 1);
    let b2 = brow(side, 2);
    let rr = 0.06 + 0.035 * ridge;
    // The inner end never crosses toward the midline: two ridges knotting
    // into a furrow, not one forehead lump.
    let bin = vec3<f32>(s * max(abs(b0.x) - 0.05, 0.1), b0.y - 0.02, b0.z - 0.02);
    var br = sd_round_cone(p, bin, b1, rr * 0.8, rr);
    br = min(br, sd_round_cone(p, b1, b2, rr, rr * 0.8));
    br = min(br, sd_round_cone(p, b2, b2 + vec3<f32>(s * 0.1, 0.06, -0.4), rr * 0.8, rr * 0.5));
    let bd = c_brow_down(side);
    d = smin(d, br, mix(0.07, 0.035, bd));
    // AU4: a deep vertical furrow between the knotted ridges.
    let fy = smoothstep(0.2, 0.0, abs(p.y - b0.y - 0.02));
    d += 0.05 * bd * exp(-(p.x * p.x) / (0.035 * 0.035)) * fy * step(0.4, p.z);
    // Nostrils: a raised rim and a hole.
    let n = nostril(side);
    d = smin(d, length((p - n.xyz - vec3<f32>(s * 0.01, 0.015, -0.02)) * vec3<f32>(1.0, 1.3, 1.0)) - n.w * 1.9, 0.05);
    d = smax(d, -(length((p - n.xyz - vec3<f32>(0.0, 0.0, 0.02)) * vec3<f32>(1.0, 1.5, 0.8)) - n.w), 0.012);
    return d + rig_creases(p);
}

// ---- the lower jaw, at rest ------------------------------------------------

fn dr_lower(q: vec3<f32>) -> f32 {
    let m = dr_mouth_rest();
    let w = s_jaw_width();
    let z0 = -0.05;
    let zf = m.z - 0.02;
    let t = clamp((q.z - z0) / (zf - z0), 0.0, 1.0);
    let cy = mix(-0.40, m.y - 0.07, t);
    let hx = mix(0.60, 0.32, t) * (0.55 + 0.375 * w);
    let hy = mix(0.30, 0.14, t);
    let d2 = dr_rbox2(vec2<f32>(q.x, q.y - cy), vec2<f32>(hx, hy), 0.12);
    let dy = q.y - (m.y - 0.05);
    let front = (q.z - (zf - 2.5 * dy * dy - 0.8 * q.x * q.x)) * 0.6;
    var d = smax(d2, front, 0.08) * 0.85;
    d = smin(d, sd_ellipsoid(q - vec3<f32>(0.0, m.y - 0.19, m.z - 0.14), vec3<f32>(0.17, 0.08, 0.13)), 0.08);
    return d;
}

// Chin barbs and mandible spikes, in the jaw's rest frame (mirrored).
fn dr_jaw_spikes(q: vec3<f32>) -> f32 {
    let m = dr_mouth_rest();
    let qm = vec3<f32>(abs(q.x), q.y, q.z);
    let bnd = min(length(qm - vec3<f32>(0.1, m.y - 0.35, m.z - 0.1)) - 0.35, length(qm - vec3<f32>(0.6, -0.72, 0.2)) - 0.45);
    if (bnd > 0.1) { return bnd; }
    var d = sd_round_cone(qm, vec3<f32>(0.0, m.y - 0.2, m.z - 0.12), vec3<f32>(0.0, m.y - 0.46, m.z - 0.02), 0.05, 0.008);
    d = min(d, sd_round_cone(qm, vec3<f32>(0.13, m.y - 0.17, m.z - 0.2), vec3<f32>(0.2, m.y - 0.38, m.z - 0.16), 0.04, 0.006));
    d = min(d, sd_round_cone(qm, vec3<f32>(0.5, -0.6, 0.3), vec3<f32>(0.82, -0.84, 0.02), 0.06, 0.008));
    d = min(d, sd_round_cone(qm, vec3<f32>(0.42, -0.66, 0.6), vec3<f32>(0.66, -0.9, 0.42), 0.045, 0.006));
    return d;
}

// ---- horns, crest, frills, barbels -----------------------------------------

// (distance, along-horn 0..1) for the mirrored side.
fn dr_horns(pm: vec3<f32>) -> vec2<f32> {
    let hs = 0.45 + 0.7 * s_horns();
    let wob = 0.04 * k_trip() * sin(k_time() * 1.3 + pm.z * 4.0);
    let a = vec3<f32>(0.40, 0.60, 0.12);
    let b = a + vec3<f32>(0.36, 0.30, -0.34) * hs;
    let c = a + vec3<f32>(0.76, 0.40, -0.70) * hs + vec3<f32>(wob, 0.0, 0.0);
    let e = a + vec3<f32>(1.02, 0.70, -0.92) * hs + vec3<f32>(wob, wob, 0.0);
    let d1 = sd_round_cone(pm, a, b, 0.13, 0.095);
    let d2 = sd_round_cone(pm, b, c, 0.095, 0.058);
    let d3 = sd_round_cone(pm, c, e, 0.058, 0.012);
    var d = min(d1, min(d2, d3));
    // Which segment, and how far along it: h.
    var h = 0.0;
    if (d1 <= d2 && d1 <= d3) {
        h = clamp(dot(pm - a, b - a) / dot(b - a, b - a), 0.0, 1.0) / 3.0;
    } else if (d2 <= d3) {
        h = (1.0 + clamp(dot(pm - b, c - b) / dot(c - b, c - b), 0.0, 1.0)) / 3.0;
    } else {
        h = (2.0 + clamp(dot(pm - c, e - c) / dot(e - c, e - c), 0.0, 1.0)) / 3.0;
    }
    // Ridged rings.
    d += 0.006 * sin(h * 70.0) * (1.0 - h);
    // The lesser horn behind the brow.
    let f = vec3<f32>(0.58, 0.40, 0.42);
    let g = f + vec3<f32>(0.34, 0.08, -0.38) * hs;
    let k = f + vec3<f32>(0.58, 0.02, -0.72) * hs;
    let ds = min(sd_round_cone(pm, f, g, 0.07, 0.04), sd_round_cone(pm, g, k, 0.04, 0.008));
    if (ds < d) {
        h = 0.5 + 0.5 * clamp(length(pm - f) / length(k - f), 0.0, 1.0);
        d = ds;
    }
    return vec2<f32>(d, h);
}

// The crest of fins down the skull's midline and the snout's bridge.
fn dr_crest(p: vec3<f32>) -> f32 {
    let ax = abs(p.x);
    if (ax > 0.14) { return ax - 0.1; }
    let x = p.x * 2.4;
    // Skull: fins arranged about the cranium centre.
    let c = vec2<f32>(0.16, -0.12);
    let q = vec2<f32>(p.y, p.z) - c;
    let th = atan2(q.y, q.x);
    let sp = 0.3;
    let idx = clamp(round((th + 1.3) / sp), 0.0, 6.0);
    let tk = -1.3 + idx * sp;
    let dir = vec2<f32>(cos(tk), sin(tk));
    let tan = vec2<f32>(-dir.y, dir.x);
    let rad = dot(q, dir);
    let tg = dot(q, tan);
    let rs = 1.0 / sqrt(pow(dir.x / 0.72, 2.0) + pow(dir.y / 0.82, 2.0));
    let sz = 0.6 + 0.5 * sin((idx + 0.5) / 7.0 * 3.14159);
    let fin = sd_round_cone(vec3<f32>(x, tg, rad), vec3<f32>(0.0, 0.0, rs - 0.06), vec3<f32>(0.0, -0.12 * sz, rs + 0.24 * sz), 0.07 * sz, 0.005) / 2.4;
    // Snout bridge: small fins and the nose horn.
    let m = dr_mouth_rest();
    let pp = vec3<f32>(x, p.y, p.z);
    var dn = 1e3;
    for (var i = 0; i < 3; i++) {
        let fi = f32(i);
        let z = mix(0.66, m.z - 0.08, fi / 2.0);
        let t = clamp((z - 0.2) / (m.z + 0.1 - 0.2), 0.0, 1.0);
        let ytop = mix(0.12, m.y + 0.215, t) + mix(0.34, 0.215, t) - 0.04;
        let hgt = mix(0.09, 0.16, fi / 2.0);
        dn = min(dn, sd_round_cone(pp, vec3<f32>(0.0, ytop, z), vec3<f32>(0.0, ytop + hgt, z - 0.06 + 0.05 * fi), 0.05 + 0.01 * fi, 0.005));
    }
    return min(fin, dn / 2.4);
}

fn dr_frill_spread() -> f32 {
    return 0.5 + 0.45 * smoothstep(0.02, 0.3, mouth_gap()) + 0.25 * k_pulso();
}

// The cheek frill of the mirrored side: (distance, material).
fn dr_frill(pm: vec3<f32>) -> vec2<f32> {
    let o = vec3<f32>(0.62, -0.18, 0.22);
    let outd = normalize(vec3<f32>(0.85, -0.1, -0.5));
    let bnd = length(pm - o - outd * 0.3) - 0.62;
    if (bnd > 0.1) { return vec2<f32>(bnd, M_FRILL); }
    let up = normalize(cross(cross(outd, vec3<f32>(0.0, 1.0, 0.0)), outd));
    let nrm = normalize(cross(outd, up));
    let spread = dr_frill_spread();
    let tm = k_time();
    var ds = 1e3;
    for (var k = 0; k < 5; k++) {
        let fk = f32(k) / 4.0 * 2.0 - 1.0;
        let a = fk * spread + 0.02 * sin(tm * 2.0 + f32(k));
        let len = 0.62 - 0.12 * abs(fk);
        let tip = o + (outd * cos(a) + up * sin(a)) * len;
        ds = min(ds, sd_round_cone(pm, o, tip, 0.035, 0.005));
    }
    let r = pm - o;
    let la = dot(r, outd);
    let lb = dot(r, up);
    let lh = dot(r, nrm);
    let ang = atan2(lb, la);
    let rad = length(vec2<f32>(la, lb));
    let segs = 4.0 / (2.0 * spread);
    let scallop = 0.5 + 0.08 * cos(ang * segs * 3.14159 * 2.0);
    let edge = max(rad - scallop * (0.62 - 0.12 * abs(ang / spread)), (abs(ang) - spread) * rad);
    let dm = max(edge, abs(lh) - 0.01);
    if (ds < dm) { return vec2<f32>(ds, M_SPINE); }
    return vec2<f32>(dm, M_FRILL);
}

// One whisker barbel, upper frame, mirrored. (distance, along 0..1)
fn dr_barbel(pm: vec3<f32>, s: f32) -> vec2<f32> {
    let m = dr_mouth_rest();
    let b0 = vec3<f32>(0.28, m.y + 0.13, m.z - 0.12);
    let bnd = sd_capsule(pm, b0, b0 + vec3<f32>(0.42, -0.62, -0.05), 0.3);
    if (bnd > 0.1) { return vec2<f32>(bnd, 0.0); }
    let ph = k_time() * (0.7 + 2.0 * k_flujo()) + s * 0.9;
    let lift = 0.12 * smoothstep(0.02, 0.3, mouth_gap());
    let s1 = vec3<f32>(sin(ph), 0.0, cos(ph * 0.7)) * 0.025;
    let s2 = vec3<f32>(sin(ph - 0.8), 0.3 * sin(ph * 1.3), cos(ph * 0.7 - 0.9)) * 0.06;
    let s3 = vec3<f32>(sin(ph - 1.6), 0.3 * sin(ph * 1.3 - 1.0), cos(ph * 0.7 - 1.8)) * 0.1;
    let b1 = b0 + vec3<f32>(0.2, -0.04 + lift, 0.06) + s1;
    let b2 = b1 + vec3<f32>(0.18, -0.2 + lift, -0.02) + s2;
    let b3 = b2 + vec3<f32>(0.06, -0.3, -0.06) + s3;
    let d1 = sd_round_cone(pm, b0, b1, 0.032, 0.022);
    let d2 = sd_round_cone(pm, b1, b2, 0.022, 0.014);
    let d3 = sd_round_cone(pm, b2, b3, 0.014, 0.004);
    let d = min(d1, min(d2, d3));
    var h = 0.2;
    if (d2 <= d1 && d2 <= d3) { h = 0.55; }
    if (d3 < d1 && d3 < d2) { h = 0.75 + 0.25 * clamp(dot(pm - b2, b3 - b2) / dot(b3 - b2, b3 - b2), 0.0, 1.0); }
    return vec2<f32>(d, h);
}

// ---- the skull --------------------------------------------------------------

fn dr_skull(p: vec3<f32>) -> vec2<f32> {
    let right = p.x < 0.0;
    let s = select(1.0, -1.0, right);
    let side = select(0, 1, right);
    let pv = jaw_pivot();
    let up = dr_upper(p, side, s);
    let keep_u = max(dr_seam(p), pv.z - p.z);
    let du = smax(up, -keep_u, 0.015);
    let q = jaw_rest(p);
    let lo = dr_lower(q);
    let keep_l = min(-dr_seam(q), q.z - pv.z);
    let dl = smax(lo, -keep_l, 0.015);
    // The neck holds the head up; it does not move with the jaw.
    let neck = sd_round_cone(p, vec3<f32>(0.0, -0.2, -0.5), vec3<f32>(0.0, -1.4, -1.05), 0.5, 0.42);
    var res = vec2<f32>(smin(du, neck, 0.18), M_SKIN);
    if (-keep_u > up - 0.004 && du < 0.02 && p.z > pv.z + 0.1) { res.y = M_GUM; }
    // The neck seen through the gape is the throat: the furnace, not skin.
    if (neck < du - 0.01 && p.z > pv.z) { res.y = M_CAVITY; }
    var jaw_m = M_JAW;
    if (-keep_l > lo - 0.004 && dl < 0.02) { jaw_m = M_GUM; }
    if (dl < res.x) { res.y = jaw_m; }
    res.x = smin(res.x, dl, 0.012);
    // Cheek-to-lip-corner web: the lips sit forward on the muzzle while the
    // cheeks are further back, so opening the jaw would otherwise expose a
    // dark gap at each mouth corner, between the two. A skin web — a cone
    // from the lip corner out to the cheek, thickening as the jaw opens —
    // bridges it, so only the central mouth stays dark when the jaw opens.
    let gap = mouth_gap();
    let wr = 0.018 + 0.09 * gap;
    let web_l = sd_round_cone(p, lip(0), cheek(0).xyz, wr, cheek(0).w * 0.6);
    let web_r = sd_round_cone(p, lip(LIP_RING / 2), cheek(1).xyz, wr, cheek(1).w * 0.6);
    let web = min(web_l, web_r);
    if (web < res.x) { res.y = M_SKIN; }
    res.x = smin(res.x, web, 0.06);
    return res;
}

// ---- the face ---------------------------------------------------------------

fn dr_mouth(p: vec3<f32>, sk: vec2<f32>) -> vec2<f32> {
    let cc = cavity_centre();
    let cr = cavity_radii();
    let mc = mouth_centre();
    let tip = u.face.tongue[7].xyz;
    let bnd = min(sd_capsule(p, cc - vec3<f32>(0.0, 0.0, cr.z * 0.6), mc + vec3<f32>(0.0, 0.0, 0.12), 0.62),
                  sd_capsule(p, mc, tip, 0.2));
    if (bnd > sk.x) { return sk; }
    let hole = min(sd_cavity(p), sd_mouth_hole(p));
    let d = smax(sk.x, -hole, 0.02);
    var res = vec2<f32>(d, sk.y);
    if (-hole > sk.x - 0.005 && d < 0.02) { res.y = M_CAVITY; }
    let dlp = sd_lips(p);
    if (dlp < res.x + 0.02) {
        res = vec2<f32>(smin(res.x, dlp, 0.02), select(res.y, M_LIP, dlp < res.x));
    }
    let dt = sd_teeth(p);
    if (dt < res.x) { res = vec2<f32>(dt, M_TEETH); }
    let dg = sd_tongue(p);
    if (dg < res.x) { res = vec2<f32>(dg, M_TONGUE); }
    return res;
}

fn map_head(p: vec3<f32>) -> vec2<f32> {
    let right = p.x < 0.0;
    let s = select(1.0, -1.0, right);
    let side = select(0, 1, right);
    let pm = vec3<f32>(abs(p.x), p.y, p.z);
    var res = dr_mouth(p, dr_skull(p));
    // Spikes riding the jaw.
    let js = dr_jaw_spikes(jaw_rest(p));
    if (js < res.x) { res = vec2<f32>(js, M_SPINE); }
    // Horns (bounded).
    if (length(pm - vec3<f32>(0.85, 0.9, -0.45)) - 1.0 < res.x) {
        let h = dr_horns(pm);
        if (h.x < res.x) { res = vec2<f32>(h.x, M_HORN); }
    }
    let cr = dr_crest(p);
    if (cr < res.x) { res = vec2<f32>(smin(res.x, cr, 0.02), M_SPINE); }
    let fr = dr_frill(pm);
    if (fr.x < res.x) { res = fr; }
    let bb = dr_barbel(pm, s);
    if (bb.x < res.x) { res = vec2<f32>(bb.x, M_BARBEL); }
    // Eyes and lids (only this side's: the other is further).
    let e = eye_centre(side);
    if (length(p - e) - eye_radius(side) * 1.3 < res.x) {
        let de = sd_eyeball(p, side);
        if (de < res.x) { res = vec2<f32>(de, M_EYE); }
        let dd = sd_lids(p, side);
        if (dd < res.x + 0.01) {
            res = vec2<f32>(smin(res.x, dd, 0.01), select(res.y, M_LID, dd < res.x));
        }
    }
    return res;
}

// ---- marching ---------------------------------------------------------------

// (t, material, closest approach, steps used / budget). t < 0 is a miss.
fn dr_march(ro: vec3<f32>, rd: vec3<f32>) -> vec4<f32> {
    let b = dot(ro, rd);
    let c = dot(ro, ro) - 2.2 * 2.2;
    let h = b * b - c;
    if (h < 0.0) { return vec4<f32>(-1.0, 0.0, 9.0, 0.0); }
    var t = max(-b - sqrt(h), 0.0);
    let tmax = -b + sqrt(h);
    let n = rig_steps();
    var md = 9.0;
    for (var i = 0; i < n; i++) {
        let r = rig_map(ro + rd * t);
        md = min(md, r.x);
        if (r.x < 0.0008 * t) {
            return vec4<f32>(t, r.y, 0.0, f32(i) / f32(n));
        }
        t += r.x * 0.85;
        if (t > tmax) { break; }
    }
    return vec4<f32>(-1.0, 0.0, md, 1.0);
}

fn dr_shadow(ro: vec3<f32>, rd: vec3<f32>) -> f32 {
    var res = 1.0;
    var t = 0.03;
    let n = 10 + i32(14.0 * k_detalle());
    for (var i = 0; i < n; i++) {
        let hh = rig_map(ro + rd * t).x;
        res = min(res, 10.0 * hh / t);
        t += clamp(hh, 0.03, 0.25);
        if (res < 0.01 || t > 2.5) { break; }
    }
    return clamp(res, 0.0, 1.0);
}

// Head-local point → screen (x in ±aspect, y in ±1, y up), through the one
// shared camera in `monstruo_rig.wgsl`.
fn dr_project(pl: vec3<f32>) -> vec2<f32> {
    return cam_project(pl);
}

// ---- scales -----------------------------------------------------------------

struct DrScale {
    edge: f32,       // distance to the cell border, cell units
    id: f32,
    centre: vec3<f32>,
    grad: vec3<f32>, // gradient of the height, head units
    h: f32,
};

fn dr_scales(pl: vec3<f32>, freq: f32) -> DrScale {
    let p = pl * freq;
    let ip = floor(p);
    let fp = p - ip;
    var f1 = 9.0;
    var f2 = 9.0;
    var c1 = vec3<f32>(0.0);
    var c2 = vec3<f32>(0.0);
    var id = 0.0;
    for (var k = 0; k < 27; k++) {
        let o = vec3<f32>(f32(k % 3) - 1.0, f32((k / 3) % 3) - 1.0, f32(k / 9) - 1.0);
        let cell = ip + o;
        let r = o + 0.15 + 0.7 * vec3<f32>(lattice3(cell), lattice3(cell + vec3<f32>(17.0, 0.0, 0.0)), lattice3(cell + vec3<f32>(0.0, 31.0, 0.0)));
        let d = length(r - fp);
        if (d < f1) {
            f2 = f1; c2 = c1; f1 = d; c1 = r;
            id = lattice3(cell + vec3<f32>(5.0, 7.0, 11.0));
        } else if (d < f2) {
            f2 = d; c2 = r;
        }
    }
    let nd = normalize(c2 - c1 + vec3<f32>(1e-5));
    let edge = dot(0.5 * (c1 + c2) - fp, nd);
    // Shingles: each scale rises toward its back edge (the tail, −z, and down).
    let back = normalize(vec3<f32>(0.0, -0.35, -1.0));
    let tl = clamp(dot(fp - c1, back) * 1.4 + 0.45, 0.0, 1.0);
    let dome = smoothstep(0.0, 0.2, edge);
    let ddome = select(0.0, 6.0 * (edge / 0.2) * (1.0 - edge / 0.2) / 0.2, edge > 0.0 && edge < 0.2);
    let tilt = 0.55 + 0.45 * tl;
    var gt = vec3<f32>(0.0);
    if (tl > 0.0 && tl < 1.0) { gt = back * 1.4 * 0.45; }
    let g = (-nd * ddome * tilt + dome * gt) * freq;
    var out: DrScale;
    out.edge = edge;
    out.id = id;
    out.centre = (ip + c1) / freq;
    out.grad = g;
    out.h = dome * tilt;
    return out;
}

// ---- shading ----------------------------------------------------------------

struct DrLight {
    key: vec3<f32>,
    key_col: vec3<f32>,
    rim_a: vec3<f32>,
    rim_b: vec3<f32>,
    mouth_pos: vec3<f32>,
    mouth_col: vec3<f32>,
};

fn dr_lights(open: f32) -> DrLight {
    var l: DrLight;
    l.key = normalize(vec3<f32>(-0.55, 0.75, 0.6));
    l.key_col = mix(vec3<f32>(0.9, 0.92, 1.0), grad4(u.skin, 1.0), 0.25) * 1.25;
    l.rim_a = grad4(u.neon, 0.0) * (0.6 + 1.6 * k_brillo());
    l.rim_b = grad4(u.neon, 0.34) * (0.6 + 1.6 * k_brillo());
    l.mouth_pos = head_matrix() * (mouth_centre() - vec3<f32>(0.0, 0.0, 0.22));
    let flick = 0.85 + 0.15 * sin(k_time() * 23.0) * sin(k_time() * 7.3);
    l.mouth_col = dr_heat(0.7) * open * 1.4 * flick;
    return l;
}

fn dr_mouth_light(p: vec3<f32>, n: vec3<f32>, lt: DrLight) -> vec3<f32> {
    let v = lt.mouth_pos - p;
    let d2 = dot(v, v);
    let nl = max(dot(n, v * inverseSqrt(d2)), 0.0);
    return lt.mouth_col * nl / (1.0 + 10.0 * d2);
}

// Skin: opal thin film over shingled scales, gaps that glow.
fn dr_skin(p: vec3<f32>, pl_in: vec3<f32>, n0: vec3<f32>, v: vec3<f32>, jaw: bool, lt: DrLight, occ: f32, sh: f32, ft: f32) -> vec3<f32> {
    let trip = k_trip();
    let shine = k_brillo();
    let pulse = k_pulso();
    var pl = pl_in;
    if (jaw) { pl = jaw_rest(pl_in); }
    let freq = mix(6.0, 16.0, s_scales());
    let sc = dr_scales(pl, freq);
    let hm = head_matrix();
    // Bump: the height gradient's tangential part tilts the normal.
    let g = hm * sc.grad;
    let gt = g - n0 * dot(g, n0);
    let n = normalize(n0 - 0.008 * gt);
    // Underside plates on throat and jaw: broad bands, not cells.
    let belly = smoothstep(-0.2, -0.75, dot(n0, hm * vec3<f32>(0.0, 1.0, 0.0))) * smoothstep(-0.4, -0.7, pl.y);
    let ndv = max(dot(n, v), 0.0);
    // Albedo walks the skin slot; each scale its own shade.
    var alb = grad4(u.skin, 0.05 + 0.3 * sc.h + 0.2 * sc.id) * (0.45 + 0.55 * sc.h);
    alb = mix(alb, grad4(u.skin, 0.7) * 0.8, belly * 0.6);
    let gap = 1.0 - smoothstep(0.0, 0.05, sc.edge);
    alb *= mix(1.0, 0.12, gap);
    alb = surf_skin_mix(alb, pl, transpose(hm) * n0);
    // Thin film: hue walks with the view angle and the film thickness.
    let film_t = fract(1.6 * (1.0 - ndv) + 0.25 * sc.id + 0.3 * sc.h + 0.15 * sin(ft * 0.7) + trip * 0.4 * sin(dot(sc.centre, vec3<f32>(3.0, 2.0, 1.0)) + ft));
    let film = mix(grad4(u.neon, film_t), grad4(u.skin, fract(film_t + 0.5)), 0.35);
    // Light.
    let dif = max(dot(n, lt.key), 0.0) * sh;
    let hv = normalize(lt.key + v);
    let spec = pow(max(dot(n, hv), 0.0), 40.0) * sh;
    let fres = pow(1.0 - ndv, 3.0);
    let rim_a = pow(clamp(dot(n, normalize(vec3<f32>(-1.0, 0.3, -0.5))) * 0.6 + 0.4 * (1.0 - ndv), 0.0, 1.0), 2.0);
    let rim_b = pow(clamp(dot(n, normalize(vec3<f32>(1.0, 0.25, -0.5))) * 0.6 + 0.4 * (1.0 - ndv), 0.0, 1.0), 2.0);
    var col = alb * (0.05 + lt.key_col * dif) * occ;
    col += alb * (lt.rim_a * rim_a + lt.rim_b * rim_b) * 0.6 * occ;
    col += film * (spec * 1.5 + fres * (0.15 + 0.5 * shine)) * (1.0 - gap) * occ;
    col += (alb + 0.3 * film) * dr_mouth_light(p, n, lt) * 1.4;
    // Flow: waves of whole scales lighting up, crawling backward.
    let c = sc.centre;
    let wave = sin(dot(c, vec3<f32>(0.0, 1.2, 4.0)) * (2.0 + 4.0 * trip) + ft * 3.0 + 3.0 * dr_noise3(c * 2.0 + vec3<f32>(0.0, 0.0, ft * 0.3)));
    let lit = smoothstep(0.55 - 0.35 * trip, 1.0, wave);
    let gap_glow = gap * (0.05 + 1.2 * shine) * (0.12 + 0.88 * lit + 2.0 * pulse);
    let nc = grad4(u.neon, fract(0.2 * sc.id + 0.15 * c.z + ft * 0.05 * (1.0 + trip)));
    col += nc * gap_glow * 0.9;
    col += nc * lit * (0.1 + 0.6 * trip) * shine * (1.0 - gap) * sc.h * 0.5;
    // Pulso: the whole hide flares.
    col += film * pulse * (0.15 + 0.7 * fres) * (1.0 - gap);
    // Op-art: at full trip, interference bands sweep across the scales.
    let bands = 0.5 + 0.5 * sin(dot(pl, vec3<f32>(9.0, 13.0, 5.0)) + 7.0 * sc.h - ft * 2.0);
    col += grad4(u.neon, fract(bands * 0.5 + ft * 0.1)) * smoothstep(0.5, 1.0, trip) * pow(bands, 6.0) * 0.6 * shine;
    return col;
}

fn dr_shade(ro: vec3<f32>, rd: vec3<f32>, hit: vec4<f32>, ft: f32, open: f32) -> vec3<f32> {
    let p = ro + rd * hit.x;
    let n = rig_normal(p);
    let v = -rd;
    let pl = to_head(p);
    let hm = head_matrix();
    let lt = dr_lights(open);
    let occ = rig_ao(p, n);
    var sh = 1.0;
    if (k_detalle() > 0.45) { sh = mix(0.25, 1.0, dr_shadow(p + n * 0.01, lt.key)); }
    let mat = hit.y;
    let ndv = max(dot(n, v), 0.0);
    let fres = pow(1.0 - ndv, 4.0);
    let shine = k_brillo();
    let dif = max(dot(n, lt.key), 0.0) * sh;
    let hv = normalize(lt.key + v);
    let ml = dr_mouth_light(p, n, lt);
    let rim = lt.rim_a * pow(max(dot(n, normalize(vec3<f32>(-1.0, 0.3, -0.5))), 0.0), 2.0)
            + lt.rim_b * pow(max(dot(n, normalize(vec3<f32>(1.0, 0.25, -0.5))), 0.0), 2.0);
    let side = select(0, 1, pl.x < 0.0);

    if (mat == M_SKIN || mat == M_JAW) {
        var col = dr_skin(p, pl, n, v, mat == M_JAW, lt, occ, sh, ft);
        // Nostrils breathe fire-light.
        let nn = nostril(side);
        let dn = length(pl - nn.xyz) / nn.w;
        col += dr_heat(0.6) * (1.0 - smoothstep(0.8, 1.5, dn)) * (0.3 + 0.7 * open + k_pulso()) * 0.8;
        return col;
    }
    if (mat == M_LID) {
        var col = dr_skin(p, pl, n, v, false, lt, occ, sh, ft) * 0.85;
        // The lid's wet edge.
        let q = pl - eye_centre(side);
        let phi = atan2(q.y, max(q.z, 1e-4));
        let lids = eye_lids(side);
        let edge = min(abs(phi - lids.x), abs(phi - lids.y));
        col += grad4(u.neon, 0.0) * (1.0 - smoothstep(0.0, 0.1, edge)) * 0.4 * shine * step(0.05, lids.x - lids.y);
        return col;
    }
    if (mat == M_EYE) {
        // In the gaze frame (gaze, scan, turn, iris size).
        let q = eye_dir(pl, side);
        let qp = pupil_dir(pl, side);
        let r = length(q.xy);
        let ang = atan2(q.y, q.x);
        let open_e = eye_open(side);
        // Slit pupil: narrows as the eye squints, opens a little on a hit.
        let sw = 0.05 + 0.06 * clamp(open_e - 0.6, 0.0, 1.0) + 0.08 * k_pulso();
        let slit = pow(qp.x / sw, 2.0) + pow(qp.y / 0.62, 2.0);
        let pupil = 1.0 - smoothstep(0.7, 1.15, slit);
        let fl = dr_fbm3(vec3<f32>(cos(ang) * 2.0, sin(ang) * 2.0, r * 6.0 - k_time() * (0.4 + 1.2 * k_flujo())), 3);
        let heat = clamp(1.15 - r * 1.3 + 0.55 * (fl - 0.5), 0.0, 1.0);
        var iris = mix(dr_heat(0.15) * 0.6, dr_heat(0.62), heat * heat);
        iris = mix(grad4(u.neon, 0.0) * 0.25, iris, smoothstep(0.95, 0.55, r));
        iris *= 0.1 + 1.1 * k_ojos();
        iris *= eye_iris_tint(r);
        iris = surf_eyes_mix(iris, pl, normalize(pl - eye_centre(side)));
        var col = iris * (1.0 - pupil) + eye_pupil_color(sqrt(slit)) * 0.4 * pupil;
        col *= 1.0 - 0.7 * smoothstep(0.85, 1.0, r);
        // Cornea: a sharp key highlight and a fresnel sheen.
        col += lt.key_col * pow(max(dot(n, hv), 0.0), 220.0) * 4.0 * sh;
        col += (rim + grad4(u.neon, 0.34) * 0.3) * fres * 0.6;
        col += eye_catchlight(pl, side);
        return col;
    }
    if (mat == M_TEETH) {
        let ivory = grad4(u.mouth, 1.0);
        let spec = pow(max(dot(n, hv), 0.0), 60.0) * sh;
        var col = ivory * (0.08 + lt.key_col * dif * 0.9) * occ;
        col += vec3<f32>(1.0) * spec * 0.9;
        col += ivory * rim * 0.9 + ivory * fres * (0.3 + shine);
        // Back-lit by the furnace: translucent edges. The back teeth sit in
        // the throat's shadow.
        let tdepth = clamp((mouth_centre().z - pl.z) / 0.5, 0.0, 1.0);
        col += dr_heat(0.6) * open * fres * 0.4;
        col += ivory * ml * 0.6;
        col *= mix(1.0, 0.35, tdepth);
        return col;
    }
    if (mat == M_TONGUE) {
        let base = mouth_tongue_base();
        let spec = pow(max(dot(n, hv), 0.0), 90.0);
        var col = base * (0.08 + lt.key_col * dif * 0.8) * occ;
        col += vec3<f32>(1.0) * spec * 1.3 * sh;
        col += base * rim * 0.6;
        col += base * ml * 0.8 + dr_heat(0.5) * open * 0.08;
        // Subsurface warmth, and a wet ridge down the middle so it reads as
        // a tongue and not as more glow.
        col += base * 0.12 * (0.4 + open);
        col *= 0.75 + 0.35 * smoothstep(0.05, 0.0, abs(pl.x - mouth_centre().x)) ;
        return col;
    }
    if (mat == M_LIP) {
        // The same scaled hide as the head, just shaded a touch darker
        // toward the inner edge — no separate glossy material.
        var col = dr_skin(p, pl, n, v, false, lt, occ, sh, ft);
        let inner = smoothstep(0.03, -0.02, abs(pl.y - mouth_centre().y) - 0.5 * mouth_gap() - 0.02);
        col *= mix(1.0, 0.55, inner);
        col += col * ml * 0.6;
        return col;
    }
    if (mat == M_CAVITY || mat == M_GUM) {
        // Dark shaded skin, not a furnace: the cavity is the head's own
        // palette driven near-black, with only a faint ember deep in the
        // throat (the tongue stays the one warm accent in this mouth).
        let depth = clamp((mouth_centre().z - pl.z) / 0.7, 0.0, 1.0);
        let sw = dr_fbm3(vec3<f32>(pl.xy * 5.0, pl.z * 3.0 - k_time() * (0.8 + 2.0 * k_flujo())), 3);
        var col = mouth_cavity_base() * (0.08 + lt.key_col * dif * 0.5) * occ;
        let veins = smoothstep(0.55, 0.78, sw);
        let core = exp(-dot(pl.xy - mouth_centre().xy, pl.xy - mouth_centre().xy) * 18.0) * depth;
        // The throat falls off to black with depth.
        let throat = mix(1.0, 0.1, smoothstep(0.1, 0.8, depth));
        col *= throat;
        // A faint, toned-down ember: thin veins and one small coal far back.
        col += dr_heat(0.75) * veins * 0.06 * (0.1 + 0.3 * open) * mix(1.0, 0.3, depth);
        col += dr_heat(0.85) * pow(core, 3.0) * open * 0.1;
        if (mat == M_GUM) {
            col = mouth_gum_base() * (0.15 + lt.key_col * dif * 0.6) * occ + mouth_gum_base() * ml * 0.3;
            col *= throat;
        }
        return col;
    }
    if (mat == M_HORN) {
        let side_s = select(1.0, -1.0, pl.x < 0.0);
        let hh = dr_horns(vec3<f32>(abs(pl.x), pl.y, pl.z)).y;
        let base = mix(grad4(u.skin, 0.05) * 0.6, grad4(u.mouth, 1.0) * 0.8, smoothstep(0.1, 1.0, hh));
        let spec = pow(max(dot(n, hv), 0.0), 50.0) * sh;
        var col = base * (0.05 + lt.key_col * dif) * occ + vec3<f32>(spec * 0.5) + base * rim * 0.8;
        // Emissive bands travelling to the tips.
        let band = smoothstep(0.75, 1.0, sin(hh * 28.0 - k_time() * (1.0 + 4.0 * k_flujo()) + side_s));
        let bc = grad4(u.neon, fract(hh * 0.8 + k_trip() * 0.3 * sin(k_time() * 0.3)));
        col += bc * band * (0.2 + 1.8 * shine) * (1.0 + 2.5 * k_pulso());
        col += bc * fres * 0.4 * shine;
        return col;
    }
    if (mat == M_SPINE) {
        let base = mix(grad4(u.mouth, 1.0) * 0.6, grad4(u.skin, 0.3), 0.4);
        let spec = pow(max(dot(n, hv), 0.0), 40.0) * sh;
        var col = base * (0.05 + lt.key_col * dif) * occ + vec3<f32>(spec * 0.4) + base * rim;
        col += grad4(u.neon, 0.34) * fres * (0.3 + 1.2 * shine) + base * ml;
        col += grad4(u.neon, 0.0) * k_pulso() * 0.8;
        return col;
    }
    if (mat == M_FRILL) {
        // A translucent membrane: back-lit neon with op-art veins.
        let pm = vec3<f32>(abs(pl.x), pl.y, pl.z);
        let r = length(pm - vec3<f32>(0.62, -0.18, 0.22));
        let ang = atan2(pm.y + 0.18, pm.x - 0.62);
        let stripes = 0.5 + 0.5 * sin(r * (30.0 + 40.0 * k_trip()) - k_time() * (1.0 + 5.0 * k_flujo()) + ang * 3.0 * k_trip());
        let mc = grad4(u.neon, fract(r * 1.2 + 0.1 * dr_clock()));
        var col = mc * (0.15 + 0.9 * shine) * (0.35 + 0.65 * stripes) * (0.5 + 0.5 * smoothstep(0.1, 0.5, r));
        col += grad4(u.skin, 0.4) * (0.05 + dif * 0.5) * occ + mc * k_pulso();
        return col;
    }
    // Barbels: bioluminescent whiskers, brightest at the tips.
    let pm = vec3<f32>(abs(pl.x), pl.y, pl.z);
    let hb = dr_barbel(pm, select(1.0, -1.0, pl.x < 0.0)).y;
    let bc = grad4(u.neon, fract(0.34 + 0.4 * hb + 0.05 * dr_clock()));
    var col = grad4(u.skin, 0.3) * (0.05 + lt.key_col * dif) * occ + rim * 0.5;
    col += bc * (0.1 + 1.5 * shine) * hb * hb + bc * k_pulso();
    return col;
}

// ---- background and overlays --------------------------------------------------

fn dr_background(sp: vec2<f32>, c: vec2<f32>, ft: f32) -> vec3<f32> {
    let trip = k_trip();
    let aura = k_aura();
    let q = sp - c;
    let r = length(q) + 1e-4;
    let a = atan2(q.y, q.x);
    // Kaleidoscope folds in with trip.
    let seg = DR_TAU / 6.0;
    let af = abs(fract((a + ft * 0.05) / seg + 0.5) - 0.5) * seg;
    let kal = smoothstep(0.35, 0.95, trip);
    var dir = normalize(mix(q / r, vec2<f32>(cos(af), sin(af)), kal) + vec2<f32>(1e-4));
    // A tunnel: depth grows toward the centre and flows inward with time.
    let depth = 0.6 / (r + 0.1) + ft * 0.8;
    let tw = 0.25 * depth;
    dir = vec2<f32>(dir.x * cos(tw) - dir.y * sin(tw), dir.x * sin(tw) + dir.y * cos(tw));
    let oct = dr_octaves();
    let pp = vec3<f32>(dir * 1.7, depth * 0.9);
    let w1 = dr_fbm3(pp * 1.1 + vec3<f32>(0.0, 0.0, ft * 0.25), oct);
    let w2 = dr_fbm3(pp * 1.1 + vec3<f32>(3.1, 1.7, -ft * 0.2), oct);
    let warp = vec3<f32>(w1 - 0.5, w2 - 0.5, 0.0) * (1.4 + 3.5 * trip);
    let f = dr_fbm3(pp * 1.6 + warp + vec3<f32>(0.0, 0.0, -ft * 0.5), oct);
    let flame = pow(smoothstep(0.42, 0.85, f), 2.0);
    let liquid = pow(0.5 + 0.5 * sin(f * (8.0 + 30.0 * trip) - ft * 2.0), 6.0) * smoothstep(0.2, 0.9, trip);
    let fall = smoothstep(0.5, 1.5, r) * (0.6 + 0.4 * smoothstep(2.2, 0.6, r));
    var col = grad4(u.background, clamp(0.5 - sp.y * 0.35, 0.0, 1.0)) * (0.5 + 0.9 * f);
    let hot = grad4(u.neon, fract(f * 1.3 + depth * 0.04 + 0.15 * ft * trip));
    col += hot * (flame * flame * (0.04 + 0.6 * aura) + liquid * 0.25 * aura) * fall;
    col += grad4(u.mouth, 0.33) * smoothstep(0.3, 0.7, f) * 0.12 * aura * fall;
    return col;
}

// A glowing ring of radius rr about c, chromatically split.
fn dr_ring(q: vec2<f32>, rr: f32, w: f32) -> vec3<f32> {
    let r = length(q);
    let s = 0.012 + 0.03 * k_trip();
    return vec3<f32>(exp(-pow((r - rr - s) / w, 2.0)), exp(-pow((r - rr) / w, 2.0)), exp(-pow((r - rr + s) / w, 2.0)));
}

fn dr_overlays(sp: vec2<f32>, hc: vec2<f32>, mc: vec2<f32>, open: f32) -> vec3<f32> {
    let pulse = k_pulso();
    let tm = k_time();
    var col = vec3<f32>(0.0);
    // Shockwave: tight at the hit, expanding as pulso falls away.
    if (pulse > 0.01) {
        let rr = 0.75 + (1.0 - pulse) * 1.6;
        col += dr_ring(sp - hc, rr, 0.03 + 0.08 * (1.0 - pulse)) * grad4(u.neon, 0.34 + 0.3 * (1.0 - pulse)) * pulse * 1.6;
    }
    // Sacred-geometry halo: a slow hexagram ring behind the head at high trip.
    let trip = k_trip();
    if (trip > 0.3) {
        let q = sp - hc;
        let a = atan2(q.y, q.x) + tm * 0.1;
        let hex = cos(3.14159 / 6.0) / cos(((a % (DR_TAU / 6.0)) + DR_TAU / 6.0) % (DR_TAU / 6.0) - 3.14159 / 6.0);
        let rh = length(q) / (1.25 * hex);
        let line = exp(-pow((rh - 1.0) / 0.006, 2.0)) + 0.6 * exp(-pow((length(q) - 1.32) / 0.006, 2.0));
        col += grad4(u.neon, fract(a / DR_TAU + tm * 0.05)) * line * smoothstep(0.3, 0.8, trip) * k_aura() * 0.8;
    }
    // Embers: sparks pouring out of an open mouth and bursting on a hit.
    let nsp = 10 + i32(22.0 * clamp(k_detalle(), 0.0, 1.0));
    let pr = 0.35 * open + 0.25 * pulse;
    if (pr > 0.01) {
        for (var i = 0; i < 32; i++) {
            if (i >= nsp) { break; }
            let fi = f32(i);
            let h1 = hash11(fi * 1.31 + 0.7);
            let h2 = hash11(fi * 2.17 + 3.1);
            let h3 = hash11(fi * 0.73 + 9.4);
            let age = fract(tm * (0.35 + 0.5 * h1) * (0.5 + k_flujo()) + h2);
            let sx = (h3 - 0.5) * 2.0;
            var pos = mc + vec2<f32>(sx * 0.18, 0.0);
            pos += vec2<f32>(sx * (0.5 + 0.8 * h1) * age, (0.15 + 0.6 * h2) * age + 0.25 * age * age);
            pos.x += 0.04 * sin(tm * 6.0 + fi);
            // Pulso throws a radial burst.
            let ba = fi * 2.399;
            pos = mix(pos, mc + vec2<f32>(cos(ba), sin(ba)) * (0.1 + (1.0 - pulse) * (0.6 + 0.6 * h1)), clamp(pulse * 1.2, 0.0, 1.0) * step(0.5, h2 + pulse));
            let d = length(sp - pos);
            let sz = 0.006 + 0.006 * h3;
            let life = (1.0 - age) * smoothstep(0.0, 0.08, age);
            col += dr_heat(0.6 + 0.4 * (1.0 - age)) * exp(-d * d / (sz * sz)) * life * pr * 1.6;
        }
    }
    return col;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    surf_uv = in.uv;
    let cam = rig_camera(in.uv);
    let ro = cam[0];
    let rd = cam[1];
    let sp = (in.uv - vec2<f32>(0.5)) * vec2<f32>(2.0 * k_aspect(), -2.0);
    let ft = dr_clock();
    let open = smoothstep(0.0, 0.22, mouth_gap());
    let hc = dr_project(vec3<f32>(0.0, 0.1, 0.0));
    let mc = dr_project(mouth_centre() + vec3<f32>(0.0, 0.0, 0.08));
    let hit = dr_march(ro, rd);
    var col: vec3<f32>;
    if (hit.x < 0.0) {
        // Heat shimmer bends the background above an open mouth.
        let dm = sp - mc;
        let plume = open * exp(-dm.x * dm.x * 6.0) * smoothstep(-0.1, 0.3, dm.y) * exp(-max(dm.y, 0.0) * 1.2);
        let shim = vec2<f32>(dr_noise3(vec3<f32>(sp * 18.0, k_time() * 6.0)), dr_noise3(vec3<f32>(sp * 18.0 + 7.0, k_time() * 6.0))) - 0.5;
        col = dr_background(sp + shim * 0.05 * plume, hc, ft);
        col += dr_heat(0.5) * plume * 0.12;
        // Aura: a halo hugging the silhouette, from the march's closest pass.
        let q = sp - hc;
        let wav = 0.5 + 0.5 * sin(atan2(q.y, q.x) * 7.0 + ft * 3.0 + k_trip() * 6.0 * dr_noise3(vec3<f32>(q * 3.0, ft)));
        let halo = exp(-hit.z * (16.0 - 6.0 * k_trip()));
        col += grad4(u.neon, fract(0.5 * wav + 0.1 * ft)) * halo * (0.05 + 0.4 * k_aura()) * (0.5 + 0.5 * wav) * (1.0 + 2.0 * k_pulso());
        col *= 1.0 + 0.8 * k_pulso() * smoothstep(1.6, 0.2, length(q));
    } else {
        col = dr_shade(ro, rd, hit, ft, open);
    }
    col += dr_overlays(sp, hc, mc, open);
    col = hue_rotate(col, k_matiz());
    let vig = 1.0 - 0.35 * dot(in.uv - 0.5, in.uv - 0.5) * 2.0;
    return vec4<f32>(tonemap(col * vig), 1.0);
}
