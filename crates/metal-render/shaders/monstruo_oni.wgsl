// monstruo — ONI (especie 3). A hannya / oni mask that is alive: lacquered
// skin with a clear coat, kintsugi neon cracks, gold-lacquered brows and
// horns, heavy knotted brows that furrow into deep wrinkles, a flared nose,
// high cheekbones, upturned tusks on the hinged jaw, a wild mane, gold eyes.
// Background: neon ukiyo-e — a rising-sun ray burst, seigaiha op-art waves, a
// rotating ring of Raijin's thunder drums, lightning on `pulso`, onibi aura.
//
// Helpers are prefixed `on_`. Materials from 10 up.

const M_HORN: f32 = 10.0;
const M_BROW: f32 = 11.0;
const M_TUSK: f32 = 12.0;
const M_HAIR: f32 = 13.0;
const ON_TAU: f32 = 6.2831853;

// Rig readings that do not depend on p, filled once per pixel in fs_main:
// (brow_furrow, brow_up, cheek_puff, nostril_flare).
var<private> on_k: vec4<f32>;
// (mane clock, trip, horn scale, tusk scale)
var<private> on_g: vec4<f32>;

// ---- reading the rig back -------------------------------------------------

fn on_read_rig() {
    // The raw controls straight from the packed block, not a reconstruction
    // from landmarks the driver has already moved.
    let furrow = c_brow_furrow();
    let bup = c_brow_up();
    let puff = c_cheek_puff();
    let flare = c_nostril_flare();
    on_k = vec4<f32>(furrow, bup, puff, flare);
    on_g = vec4<f32>(
        k_time() * (0.08 + 0.9 * k_flujo()),
        k_trip(),
        s_horns() / 0.7,
        s_fang_length() / 0.9
    );
}

fn on_seg2(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn on_groove(d: f32, w: f32) -> f32 {
    return exp(-(d * d) / (w * w));
}

// ---- the skull ------------------------------------------------------------

// Forehead wrinkles: glabella furrows (brow_furrow), nose scrunch (furrow +
// flare), horizontal forehead lines (brow_up). A displacement, so + carves.
fn on_wrinkles(q: vec3<f32>) -> f32 {
    if (q.x > 0.5 || q.y < 0.12 || q.y > 1.0 || q.z < 0.45) {
        return 0.0;
    }
    let furrow = on_k.x;
    let bup = on_k.y;
    let b0 = brow_m(q, 0);
    let xy = q.xy;
    var w = 0.0;
    // Two deep vertical/diagonal furrows each side of the glabella.
    let g1 = on_seg2(xy, vec2<f32>(0.035, b0.y + 0.02), vec2<f32>(0.055, 0.80));
    let g2 = on_seg2(xy, vec2<f32>(0.10, b0.y + 0.05), vec2<f32>(0.19, 0.74));
    let g3 = on_seg2(xy, vec2<f32>(0.17, b0.y + 0.09), vec2<f32>(0.31, 0.70));
    w += (0.004 + 0.032 * furrow) * on_groove(g1, 0.024);
    w += (0.003 + 0.026 * furrow) * on_groove(g2, 0.026);
    w += (0.000 + 0.018 * furrow) * on_groove(g3, 0.026);
    // The knot between them bulges as the brows pull in.
    w -= 0.028 * furrow * on_groove(q.x, 0.03) * on_groove(q.y - b0.y - 0.12, 0.12);
    // Nose-bridge scrunch: short horizontal creases.
    let scr = 0.006 + 0.022 * furrow + 0.012 * on_k.w;
    for (var i = 0; i < 3; i++) {
        let y = 0.12 + 0.065 * f32(i) - 0.05 * q.x * q.x;
        w += scr * on_groove(q.y - y, 0.016) * smoothstep(0.17, 0.05, q.x);
    }
    // Forehead lines, curved like a frown's.
    let fl = 0.004 + 0.022 * bup + 0.006 * furrow;
    for (var i = 0; i < 3; i++) {
        let y = 0.66 + 0.075 * f32(i) + 0.22 * q.x * q.x - 0.06 * furrow * smoothstep(0.3, 0.0, q.x);
        w += fl * on_groove(q.y - y, 0.02) * smoothstep(0.46, 0.2, q.x) * smoothstep(0.62, 0.7, q.y);
    }
    return w;
}

// Horn on the creature's left (+x); called with |x|. Returns (d, along 0..1).
fn on_horn(q: vec3<f32>) -> vec2<f32> {
    let s = on_g.z;
    let h0 = vec3<f32>(0.30, 0.58, 0.50);
    let h1 = h0 + vec3<f32>(0.09, 0.30, 0.04) * s;
    let h2 = h1 + vec3<f32>(0.10, 0.25, -0.07) * s;
    let h3 = h2 + vec3<f32>(0.03, 0.21, -0.15) * s;
    let d0 = sd_round_cone(q, h0, h1, 0.125, 0.085);
    let d1 = sd_round_cone(q, h1, h2, 0.085, 0.052);
    let d2 = sd_round_cone(q, h2, h3, 0.052, 0.010);
    let d = min(d0, min(d1, d2));
    // Arc length from the root, roughly, for the rings.
    let along = clamp(length(q - h0) / max(length(h3 - h0), 1e-3), 0.0, 1.0);
    let ring = 0.007 * (1.0 - along) * sin(along * 46.0);
    return vec2<f32>(d + ring, along);
}

fn on_upper(p: vec3<f32>) -> vec2<f32> {
    let q = vec3<f32>(abs(p.x), p.y, p.z);
    let furrow = on_k.x;
    // Cranium, broad flat face, frontal bulge, muzzle.
    var d = sd_ellipsoid(p - vec3<f32>(0.0, 0.12, -0.08), vec3<f32>(0.9, 0.98, 0.92));
    d = smin(d, sd_ellipsoid(p - vec3<f32>(0.0, -0.12, 0.18), vec3<f32>(0.86, 0.8, 0.72)), 0.2);
    d = smin(d, sd_ellipsoid(p - vec3<f32>(0.0, 0.55, 0.42), vec3<f32>(0.62, 0.4, 0.44)), 0.15);
    d = smin(d, sd_ellipsoid(p - vec3<f32>(0.0, -0.40, 0.62), vec3<f32>(0.52, 0.24, 0.33)), 0.12);
    // High cheekbones and the puffing cheek.
    d = smin(d, sd_ellipsoid(q - vec3<f32>(0.5, 0.0, 0.6), vec3<f32>(0.25, 0.13, 0.22)), 0.12);
    let ch = cheek_m(p);
    d = smin(d, length(q - ch.xyz + vec3<f32>(0.0, 0.0, 0.12)) - ch.w, 0.16);
    // Grin creases: from the nose wing down past the mouth corner.
    let crease = on_seg2(q.xy, vec2<f32>(0.15, -0.2), vec2<f32>(0.46, -0.56));
    d += 0.022 * on_groove(crease, 0.03) * smoothstep(0.45, 0.75, q.z);
    // Eye sockets.
    let e = mirror_side(p, eye_centre(0), eye_centre(1));
    let er = eye_radius(side_of(p));
    d = smax(d, -(length(q - e - vec3<f32>(0.0, 0.0, 0.03)) - er * 1.2), 0.05);
    // Lower lid bag.
    d = smin(d, sd_capsule(q, e + vec3<f32>(-0.12, -0.17, 0.1), e + vec3<f32>(0.13, -0.15, 0.04), 0.035), 0.06);
    // Nose: a flat bridge, a broad tip and flaring wings, nostrils carved.
    let ns = nostril_m(p);
    let flare = on_k.w;
    var nose = sd_round_cone(q, vec3<f32>(0.0, 0.32, 0.86), vec3<f32>(0.0, -0.1, 1.0), 0.055, 0.07);
    nose = smin(nose, length(q - vec3<f32>(0.0, -0.14, 1.0)) - 0.08, 0.05);
    nose = smin(nose, sd_ellipsoid(q - ns.xyz - vec3<f32>(0.045, 0.012, -0.035), vec3<f32>(0.075, 0.06, 0.07) * (1.0 + 0.25 * flare)), 0.04);
    d = smin(d, nose, 0.08);
    d = smax(d, -sd_ellipsoid(q - ns.xyz - vec3<f32>(0.0, -0.03, 0.0), vec3<f32>(ns.w * 1.15, ns.w * 0.8, ns.w * 1.4)), 0.015);
    // Wrinkles.
    d += on_wrinkles(q);
    d += rig_creases(p);
    var res = vec2<f32>(d, M_SKIN);
    // Brows: knotted, slanting up and out, a tail to the horn root.
    let b0 = brow_m(p, 0) + vec3<f32>(0.0, -0.01, -0.03);
    let b1 = brow_m(p, 1) + vec3<f32>(0.0, 0.05, -0.02);
    let b2 = brow_m(p, 2) + vec3<f32>(0.04, 0.13, -0.06);
    let b3 = b2 + vec3<f32>(0.08, 0.12, -0.18);
    var db = sd_round_cone(q, b0, b1, 0.075, 0.068);
    db = smin(db, sd_round_cone(q, b1, b2, 0.068, 0.05), 0.03);
    db = smin(db, sd_round_cone(q, b2, b3, 0.05, 0.02), 0.03);
    db = smin(db, length(q - b0 - vec3<f32>(-0.015, 0.03, 0.0)) - (0.07 + 0.035 * furrow), 0.04);
    res = vec2<f32>(smin(res.x, db, 0.05), select(M_SKIN, M_BROW, db < res.x - 0.008));
    // Horns, bounded.
    let hb = length(q - vec3<f32>(0.45, 0.95, 0.45)) - 0.62 * max(on_g.z, 0.3);
    if (hb < res.x && s_horns() > 0.01) {
        let h = on_horn(q);
        if (h.x < res.x + 0.03) {
            let m = select(res.y, M_HORN, h.x < res.x);
            res = vec2<f32>(smin(res.x, h.x, 0.03), m);
        }
    }
    return res;
}

fn on_lower(p: vec3<f32>) -> vec2<f32> {
    let r = jaw_rest(p);
    let q = vec3<f32>(abs(r.x), r.y, r.z);
    var d = sd_ellipsoid(r - vec3<f32>(0.0, -0.48, 0.18), vec3<f32>(0.74, 0.36, 0.68));
    d = smin(d, sd_ellipsoid(r - vec3<f32>(0.0, -0.52, 0.6), vec3<f32>(0.5, 0.2, 0.33)), 0.1);
    d = smin(d, sd_ellipsoid(r - vec3<f32>(0.0, -0.74, 0.64), vec3<f32>(0.27, 0.16, 0.22)), 0.1);
    // Jowl knots at the jaw corners.
    d = smin(d, length(q - vec3<f32>(0.46, -0.62, 0.55)) - 0.12, 0.1);
    var res = vec2<f32>(d, M_SKIN);
    // Upturned tusks, rooted in the lower gum outside the lips.
    let tb = length(q - vec3<f32>(0.29, -0.44, 0.94)) - 0.32;
    if (tb < res.x) {
        let s = on_g.w;
        let t0 = vec3<f32>(0.27, -0.63, 0.83);
        let t1 = t0 + vec3<f32>(0.02, 0.17, 0.16) * s;
        let t2 = t1 + vec3<f32>(0.05, 0.21, -0.02) * s;
        let dt = min(sd_round_cone(q, t0, t1, 0.05, 0.034), sd_round_cone(q, t1, t2, 0.034, 0.004));
        if (dt < res.x + 0.02) {
            res = vec2<f32>(smin(res.x, dt, 0.02), select(res.y, M_TUSK, dt < res.x));
        }
    }
    return res;
}

// The mane: a shell behind the face with tufts that flow outward.
fn on_mane(p: vec3<f32>) -> f32 {
    let m = p - vec3<f32>(0.0, 0.1, -0.32);
    let ang = atan2(m.y, m.x);
    let rr = length(m.xy);
    let base = sd_ellipsoid(m, vec3<f32>(1.12, 1.12, 0.78));
    if (base > 0.4) {
        return base - 0.2;
    }
    let tc = on_g.x;
    let tr = on_g.y;
    let ph = ang * 13.0 + (1.2 + 1.5 * tr) * sin(ang * 5.0 + tc * 0.9) + m.z * 2.5;
    let tuft = 0.5 + 0.5 * cos(ph);
    let spikes = tuft * tuft * (0.2 + 0.06 * sin(ang * 3.0 - tc)) * clamp(rr, 0.0, 1.2);
    let fine = 0.025 * sin(ang * 41.0 + m.z * 11.0 + tc * 0.5);
    // Not under the chin.
    let keep = smoothstep(-1.0, -0.55, p.y);
    return (base - (spikes + fine) * keep) * 0.6;
}

fn on_skull(p: vec3<f32>) -> vec2<f32> {
    let up = on_upper(p);
    let lo = on_lower(p);
    let w = jaw_weight(p);
    var res = vec2<f32>(mix(up.x, lo.x, w), select(up.y, lo.y, w > 0.5));
    let dm = on_mane(p);
    if (dm < res.x + 0.04) {
        res = vec2<f32>(smin(res.x, dm, 0.04), select(res.y, M_HAIR, dm < res.x));
    }
    return res;
}

// The rig's face parts, composed like `rig_face` but bounded so the mouth and
// eyes cost nothing where they cannot be, and keeping the skull's materials.
fn on_face(p: vec3<f32>, sk: vec2<f32>) -> vec2<f32> {
    var res = sk;
    let mc = mouth_centre();
    let mb = length(p - vec3<f32>(0.0, mc.y - 0.05, mc.z - 0.2)) - 0.78;
    if (mb < 0.02) {
        let hole = min(sd_cavity(p), sd_mouth_hole(p));
        let d = smax(sk.x, -hole, 0.02);
        res = vec2<f32>(d, sk.y);
        if (-hole > sk.x - 0.005 && d < 0.02) {
            res.y = M_CAVITY;
        }
        let dl = sd_lips(p);
        if (dl < res.x + 0.02) {
            res = vec2<f32>(smin(res.x, dl, 0.02), select(res.y, M_LIP, dl < res.x));
        }
        let dt = sd_teeth(p);
        if (dt < res.x) { res = vec2<f32>(dt, M_TEETH); }
        let dg = sd_tongue(p);
        if (dg < res.x) { res = vec2<f32>(dg, M_TONGUE); }
    } else {
        res.x = min(res.x, mb);
    }
    for (var i = 0; i < 2; i++) {
        let eb = length(p - eye_centre(i)) - 0.3;
        if (eb < 0.02) {
            let de = sd_eyeball(p, i);
            if (de < res.x) { res = vec2<f32>(de, M_EYE); }
            let dd = sd_lids(p, i);
            if (dd < res.x + 0.01) {
                res = vec2<f32>(smin(res.x, dd, 0.01), select(res.y, M_LID, dd < res.x));
            }
        } else {
            res.x = min(res.x, eb);
        }
    }
    return res;
}

fn map_head(p: vec3<f32>) -> vec2<f32> {
    return on_face(p, on_skull(p));
}

// ---- camera and marching --------------------------------------------------

// The one shared camera; oni no longer carries its own.
fn on_camera(uv: vec2<f32>) -> array<vec3<f32>, 2> {
    return rig_camera(uv);
}

// Head-local point → screen p2 (x in ±aspect, y in ±1, y up), through the one
// shared camera.
fn on_project(w: vec3<f32>, ro: vec3<f32>) -> vec2<f32> {
    return (w.xy - ro.xy) / max(ro.z - w.z, 0.1) * cam_focal();
}

// Like rig_march, but also returns the closest approach (for the aura).
// (t or -1, material, closest distance)
fn on_march(ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    let b = dot(ro, rd);
    let c = dot(ro, ro) - 2.2 * 2.2;
    let h = b * b - c;
    let axis = sqrt(max(dot(ro, ro) - b * b, 0.0));
    if (h < 0.0) {
        return vec3<f32>(-1.0, 0.0, axis - 1.1);
    }
    var t = max(-b - sqrt(h), 0.0);
    let tmax = -b + sqrt(h);
    var mind = 1e9;
    let n = rig_steps();
    for (var i = 0; i < n; i++) {
        let r = rig_map(ro + rd * t);
        mind = min(mind, r.x);
        if (r.x < 0.0008 * t) {
            return vec3<f32>(t, r.y, 0.0);
        }
        t += r.x * 0.85;
        if (t > tmax) {
            break;
        }
    }
    return vec3<f32>(-1.0, 0.0, max(mind, 0.0));
}

fn on_shadow(ro: vec3<f32>, rd: vec3<f32>) -> f32 {
    var res = 1.0;
    var t = 0.03;
    let n = 10 + i32(22.0 * k_detalle());
    for (var i = 0; i < n; i++) {
        let h = rig_map(ro + rd * t).x;
        res = min(res, 10.0 * h / t);
        t += clamp(h, 0.025, 0.25);
        if (res < 0.002 || t > 2.5) {
            break;
        }
    }
    return clamp(res, 0.0, 1.0);
}

// ---- colour helpers -------------------------------------------------------

// Gold lacquer: the skin's highlight and a touch of neon, pushed to gold.
fn on_gold() -> vec3<f32> {
    let g = mix(grad4(u.skin, 1.0), grad4(u.neon, 0.67), 0.25);
    return max(g, vec3<f32>(0.08)) * vec3<f32>(1.0, 0.72, 0.32) * 1.35;
}

// A cheap studio environment for the clear coat: neon softboxes overhead,
// a warm floor bounce from the mouth colour.
fn on_env(r: vec3<f32>) -> vec3<f32> {
    let top = smoothstep(0.1, 0.8, r.y);
    var c = grad4(u.neon, 0.5 + 0.45 * r.x) * top * 0.7;
    c += grad4(u.neon, 1.0) * smoothstep(0.03, 0.0, abs(r.y - 0.55)) * 1.4;
    c += grad4(u.mouth, 0.66) * smoothstep(0.0, -0.8, r.y) * 0.15;
    c += grad4(u.background, 0.5) * 0.3;
    return c;
}

// iq's voronoi: (F1, border distance, cell id).
fn on_voronoi(x: vec2<f32>, tm: f32) -> vec3<f32> {
    let n = floor(x);
    let f = fract(x);
    var mr = vec2<f32>(0.0);
    var mg = vec2<f32>(0.0);
    var md = 8.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let g = vec2<f32>(f32(i), f32(j));
            let o = vec2<f32>(lattice(n + g), lattice(n + g + vec2<f32>(37.0, 17.0)));
            let oo = 0.5 + 0.45 * sin(tm + ON_TAU * o);
            let r = g + oo - f;
            let d = dot(r, r);
            if (d < md) {
                md = d;
                mr = r;
                mg = g;
            }
        }
    }
    var bd = 8.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let g = mg + vec2<f32>(f32(i), f32(j));
            let o = vec2<f32>(lattice(n + g), lattice(n + g + vec2<f32>(37.0, 17.0)));
            let oo = 0.5 + 0.45 * sin(tm + ON_TAU * o);
            let r = g + oo - f;
            if (dot(mr - r, mr - r) > 1e-5) {
                bd = min(bd, dot(0.5 * (mr + r), normalize(r - mr)));
            }
        }
    }
    return vec3<f32>(sqrt(md), bd, lattice(n + mg));
}

// ---- background -----------------------------------------------------------

// Seigaiha: overlapping wave scales. Returns the ring field 0..1 of the
// scale on top.
fn on_seigaiha(p: vec2<f32>) -> f32 {
    let row = floor(p.y * 2.0);
    var val = 0.0;
    var hit = false;
    for (var k = 2; k >= -1; k--) {
        let j = row + f32(k);
        let sh = 0.5 * (j - 2.0 * floor(j * 0.5));
        let cx = floor(p.x - sh + 0.5) + sh;
        let cy = j * 0.5;
        let d = length(p - vec2<f32>(cx, cy));
        if (d < 0.62 && p.y > cy - 0.05) {
            val = d / 0.62;
            hit = true;
        }
    }
    if (!hit) {
        return 0.0;
    }
    return 0.5 + 0.5 * cos(val * 5.0 * ON_TAU);
}

fn on_waves(p: vec2<f32>, tf: f32, tr: f32) -> f32 {
    var q = p * 4.2 + vec2<f32>(tf * 0.35, 0.12 * sin(p.x * 3.0 + tf));
    var v = on_seigaiha(q);
    if (tr > 0.35) {
        // Fractal: the scale pattern again inside itself.
        let v2 = on_seigaiha(q * 3.0 + vec2<f32>(-tf, 0.0));
        v = mix(v, v * v2 + 0.25 * v2, smoothstep(0.35, 1.0, tr));
    }
    return v;
}

fn on_bolt(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, seed: f32) -> f32 {
    let ab = b - a;
    let l = length(ab);
    let dir = ab / l;
    let nrm = vec2<f32>(-dir.y, dir.x);
    let s = dot(p - a, dir);
    let tt = s / l;
    if (tt < -0.02 || tt > 1.02) {
        return 0.0;
    }
    let env = sin(clamp(tt, 0.0, 1.0) * RIG_PI);
    let jag = (quintic_noise(vec2<f32>(s * 5.0, seed)) - 0.5) * 0.35
        + (quintic_noise(vec2<f32>(s * 17.0, seed + 9.0)) - 0.5) * 0.12
        + (quintic_noise(vec2<f32>(s * 55.0, seed + 3.0)) - 0.5) * 0.04;
    let d = abs(dot(p - a, nrm) - jag * env * l);
    return 0.0018 / (d * d + 0.0004) * 0.04 + smoothstep(0.006, 0.0, d);
}

fn on_background(uv: vec2<f32>, p2: vec2<f32>, ms: vec2<f32>, ctr: vec2<f32>, mind: f32) -> vec3<f32> {
    let aspect = k_aspect();
    let time = k_time();
    let tr = k_trip();
    let au = k_aura();
    let br = k_brillo();
    let pu = k_pulso();
    let tf = time * (0.06 + k_flujo());
    var col = backdrop_from_uv(u.background, uv, aspect);
    let q0 = p2 - ctr;
    var q = q0;
    if (tr > 0.01) {
        q += tr * tr * 0.3 * vec2<f32>(sin(q.y * 3.0 + tf * 1.3), cos(q.x * 2.6 - tf));
    }
    let r = length(q);
    var a = atan2(q.y, q.x);
    if (tr > 0.45) {
        let n = 6.0 + floor(tr * 8.0);
        let sec = ON_TAU / n;
        let folded = abs(fract(a / sec + 0.5) - 0.5) * sec;
        a = mix(a, folded, smoothstep(0.45, 0.8, tr));
    }
    let sun = grad4(u.neon, 0.0);
    let ink = grad4(u.skin, 0.15);
    // Rising-sun ray burst.
    let nr = 16.0;
    let ray = smoothstep(-0.1, 0.1, sin(a * nr + tf * 0.5 + tr * tr * 1.5 * sin(r * 5.0 - tf * 2.0)));
    let fall = exp(-r * 1.1);
    col += mix(ink * 0.05, sun * 0.3, ray) * fall * (0.15 + 0.6 * au);
    // The sun disc behind the head, banded at the bottom like a print.
    let disc = smoothstep(0.98, 0.96, r);
    let band = step(0.5, fract((q.y + tf * 0.05) * 9.0)) * smoothstep(0.0, -0.6, q.y);
    let sun_c = sun * (0.35 + 0.35 * au) * (1.0 - 0.7 * band) * (0.6 + 0.5 * smoothstep(-0.9, 0.9, q.y));
    col = mix(col, sun_c, disc * (0.5 + 0.4 * au));
    // Seigaiha sea, chromatic with trip.
    let horizon = -0.42 + 0.05 * sin(p2.x * 2.3 + tf) + 0.03 * sin(p2.x * 5.1 - tf * 1.7);
    let sea = smoothstep(horizon + 0.01, horizon - 0.02, p2.y);
    if (sea > 0.0) {
        let ch = 0.012 * tr;
        let wr = on_waves(p2 + vec2<f32>(ch, 0.0), tf, tr);
        let wg = on_waves(p2, tf, tr);
        let wb = on_waves(p2 - vec2<f32>(ch, 0.0), tf, tr);
        let lines = grad4(u.neon, 0.33 + 0.2 * tr * sin(p2.x + tf));
        let w = vec3<f32>(wr, wg, wb);
        let deep = grad4(u.skin, 0.1) * 0.12 + grad4(u.background, 0.0);
        let fade = 0.35 + 0.65 * smoothstep(-1.0, horizon, p2.y);
        col = mix(col, deep + lines * w * (0.12 + 0.35 * br) * fade, sea);
        // Foam crest with claws.
        let claw = 0.5 + 0.5 * sin(p2.x * 22.0 + tf * 2.0);
        let crest = smoothstep(0.025, 0.0, abs(p2.y - horizon - 0.012 * claw * claw));
        col += grad4(u.neon, 1.0) * crest * (0.25 + 0.5 * br);
    }
    // Raijin's ring of thunder drums.
    let rot = tf * 0.25 + pu * 0.25;
    let ringr = 0.98 * (1.0 + 0.14 * pu) / k_distancia();
    let nd = 9.0;
    let aa = atan2(q0.y, q0.x) - rot;
    let cell = round(aa / (ON_TAU / nd));
    let ca = cell * ON_TAU / nd + rot;
    let dc = q0 - vec2<f32>(cos(ca), sin(ca)) * ringr;
    let drum_r = 0.13 * (1.0 + 0.2 * pu) / k_distancia();
    let ld = length(dc);
    let gold = on_gold();
    let kick = 0.35 + 0.5 * au + 1.5 * pu;
    let rope = smoothstep(0.008, 0.0, abs(length(q0) - ringr) - 0.004) * smoothstep(drum_r * 0.9, drum_r * 1.1, ld);
    col += gold * rope * 0.5 * kick;
    if (ld < drum_r * 1.15) {
        let rho = ld / drum_r;
        let phi = atan2(dc.y, dc.x) + tf * 0.8 + cell;
        let rim = smoothstep(0.06, 0.0, abs(rho - 0.93) - 0.05);
        let tomoe = smoothstep(0.15, -0.15, sin(3.0 * phi + 7.0 * rho)) * smoothstep(0.86, 0.8, rho) * smoothstep(0.1, 0.25, rho);
        let skin = smoothstep(1.0, 0.96, rho);
        col = mix(col, grad4(u.skin, 0.1) * 0.6, skin * 0.85);
        col += gold * rim * kick;
        col += grad4(u.neon, fract(0.67 + cell * 0.13 + tr * 0.3)) * tomoe * kick;
    }
    // Sound rings out of the open mouth.
    let gap = mouth_gap();
    if (gap > 0.002) {
        let rm = length((p2 - ms) * vec2<f32>(1.0, 1.3));
        let rings = pow(0.5 + 0.5 * sin(rm * 22.0 - time * 7.0), 12.0);
        col += grad4(u.neon, fract(0.2 + rm * 0.3)) * rings * smoothstep(0.0, 0.2, gap) * exp(-rm * 2.6) * (0.2 + 0.5 * br);
    }
    // Onibi aura: fire hugging the silhouette.
    let flame = quintic_noise(vec2<f32>(atan2(q0.y, q0.x) * 4.0 + 20.0, mind * 6.0 - tf * 3.0));
    let glow = exp(-mind * (7.0 - 3.0 * tr)) * (0.4 + 0.9 * flame) * au * (0.4 + br) * smoothstep(-0.9, 0.1, q0.y);
    col += grad4(u.neon, fract(atan2(q0.y, q0.x) / ON_TAU + tf * 0.07 + tr * mind)) * glow;
    // Lightning on the hit.
    if (pu > 0.01) {
        let seed = floor(time * 3.0) * 7.31;
        let side = select(-1.0, 1.0, fract(seed * 0.13) > 0.5);
        var bolt = on_bolt(p2, vec2<f32>(side * 1.1, 1.15), vec2<f32>(side * 0.85, -0.55), seed);
        bolt += 0.6 * on_bolt(p2, vec2<f32>(side * 1.02, 0.5), vec2<f32>(side * 1.45, -0.2), seed + 3.0);
        bolt += 0.8 * on_bolt(p2, vec2<f32>(-side * 1.3, 1.15), vec2<f32>(-side * 1.15, 0.1), seed + 5.0);
        col += (grad4(u.neon, 1.0) + vec3<f32>(0.3)) * bolt * pu * 1.6;
        col += grad4(u.neon, 0.33) * pu * 0.18 * smoothstep(-0.6, 1.0, p2.y);
    }
    return col;
}

// ---- shading --------------------------------------------------------------

fn on_shade(ro: vec3<f32>, rd: vec3<f32>, t: f32, mat: f32, mcw: vec3<f32>) -> vec3<f32> {
    let time = k_time();
    let tr = k_trip();
    let br = k_brillo();
    let pu = k_pulso();
    let tf = time * (0.06 + k_flujo());
    let pw = ro + rd * t;
    let n = rig_normal(pw);
    let pl = to_head(pw);
    let nl = to_head(n);
    let v = -rd;
    let ndv = clamp(dot(n, v), 0.0, 1.0);
    let ao = rig_ao(pw, n);
    let kl = normalize(vec3<f32>(-0.5, 0.75, 0.6));
    var sh = 1.0;
    if (k_detalle() > 0.3) {
        sh = on_shadow(pw + n * 0.01, kl);
    }
    let dif = max(dot(n, kl), 0.0) * sh;
    let hv = normalize(kl + v);
    let ndh = max(dot(n, hv), 0.0);
    let fres = 0.04 + 0.96 * pow(1.0 - ndv, 5.0);
    let refl = reflect(rd, n);
    let gold = on_gold();
    let gap = mouth_gap();

    var alb = grad4(u.skin, 0.4);
    var emis = vec3<f32>(0.0);
    var spec_c = vec3<f32>(1.0);
    var spec_p = 90.0;
    var spec_k = 1.0;
    var coat = 1.0;
    var metal = 0.0;
    var sss = 0.0;
    var rimk = 1.0;

    if (mat == M_SKIN || mat == M_LID) {
        // Lacquer: deep in the crevices, light on the planes, an
        // iridescent walk at grazing angles.
        let irid = (1.0 - ndv) * (0.3 + 0.5 * tr);
        let tt = 0.25 + 0.3 * ao + 0.12 * nl.y + irid * 0.5 + 0.35 * tr * tr * sin(pl.y * 7.0 + pl.x * 3.0 + tf);
        alb = grad4(u.skin, clamp(tt, 0.0, 1.0)) * 0.85;
        // Kintsugi cracks.
        let cuv = vec2<f32>(atan2(pl.x, pl.z) * 1.1, pl.y) * 4.4 + tr * 0.2 * vec2<f32>(sin(pl.y * 4.0 + tf), cos(pl.x * 4.0 - tf));
        let vo = on_voronoi(cuv, tf * 0.2 * tr);
        let spread = 0.12 + 0.8 * tr;
        let reach = smoothstep(spread, spread - 0.25, lattice_fbm(cuv * 0.7 + 3.0, 2 + i32(2.0 * k_detalle())));
        let width = 0.015 + 0.035 * tr + 0.02 * pu;
        let crack = smoothstep(width, width * 0.3, vo.y) * reach;
        let beat = 0.55 + 0.45 * sin(time * (1.5 + 3.0 * k_flujo()) + vo.z * ON_TAU);
        let crack_c = mix(gold, grad4(u.neon, fract(vo.z + tf * 0.05 + tr * pl.y)), smoothstep(0.1, 0.7, tr));
        alb = mix(alb, gold * 0.6, crack);
        emis += crack_c * crack * br * beat * (0.6 + 2.0 * pu) * 1.4;
        metal = crack * 0.6;
        // Op-art contour lines melting over the skin at high trip.
        if (tr > 0.3) {
            let warp = lattice_fbm(pl.xy * 2.0 + vec2<f32>(tf * 0.2, 0.0), 2);
            let lines = pow(0.5 + 0.5 * sin((pl.y + 0.4 * warp) * 40.0 - tf * 2.0 + length(pl.xy) * 10.0), 12.0);
            emis += grad4(u.neon, fract(warp + tf * 0.1 + pl.y)) * lines * smoothstep(0.3, 1.0, tr) * br * 1.6;
        }
        // Jaw seam: a kintsugi line where the hinged jaw parts.
        let wj = jaw_weight(pl);
        let seam = smoothstep(0.5, 0.0, abs(wj - 0.5)) * smoothstep(0.0, 0.15, jaw_angle());
        emis += gold * seam * br * 2.0;
        if (mat == M_LID) {
            alb *= 0.6;
            coat = 0.25;
            spec_k = 0.3;
            // Gold liner along the lid edges.
            var ei = 0;
            if (pl.x < 0.0) { ei = 1; }
            let qe = pl - eye_centre(ei);
            let phi = atan2(qe.y, max(qe.z, 1e-4));
            let lids = eye_lids(ei);
            let edge = min(abs(phi - lids.x), abs(phi - lids.y));
            let liner = smoothstep(0.12, 0.0, edge);
            alb = mix(alb, gold, liner);
            metal = max(metal, liner);
        }
    } else if (mat == M_BROW) {
        // Carved hair strokes in black lacquer, the crest edged in gold.
        let stroke = 0.5 + 0.5 * sin(abs(pl.x) * 70.0 - pl.y * 55.0);
        let edge = smoothstep(0.35, 0.75, nl.y) + smoothstep(0.75, 0.95, stroke) * 0.6;
        alb = mix(grad4(u.skin, 0.0) * 0.4, gold, clamp(edge, 0.0, 1.0));
        metal = clamp(edge, 0.0, 1.0);
        spec_p = 60.0;
        emis += gold * clamp(edge, 0.0, 1.0) * br * 0.25 * (1.0 + 3.0 * pu);
    } else if (mat == M_HORN) {
        let q = vec3<f32>(abs(pl.x), pl.y, pl.z);
        let along = on_horn(q).y;
        let ringv = 0.5 + 0.5 * sin(along * 46.0);
        alb = mix(grad4(u.skin, 0.05) * 0.6, gold, smoothstep(0.35, 0.05, along) * 0.0 + ringv * 0.35 + smoothstep(0.55, 0.9, along));
        metal = 0.4 + 0.5 * smoothstep(0.55, 0.9, along);
        emis += grad4(u.neon, fract(along + tf * 0.1)) * pow(ringv, 6.0) * br * (0.3 + 0.6 * tr + 2.0 * pu);
        emis += grad4(u.neon, 1.0) * smoothstep(0.85, 1.0, along) * br * (1.0 + 3.0 * pu);
    } else if (mat == M_HAIR) {
        let m = pl - vec3<f32>(0.0, 0.1, -0.32);
        let ang = atan2(m.y, m.x);
        let strand = 0.5 + 0.5 * sin(ang * 140.0 + 6.0 * lattice_fbm(vec2<f32>(ang * 8.0, m.z * 3.0), 2) + m.z * 20.0);
        alb = grad4(u.skin, 0.0) * (0.15 + 0.35 * strand);
        spec_p = 30.0;
        spec_k = 0.4 * strand;
        coat = 0.2;
        let tip = smoothstep(1.0, 1.35, length(m.xy));
        emis += grad4(u.neon, fract(0.33 + ang * 0.2 + tf * 0.1)) * tip * strand * br * (0.5 + tr + 2.0 * pu);
        rimk = 1.6;
    } else if (mat == M_LIP) {
        // Same lacquered skin tone as the face, darker toward the inner
        // edge of the lip, with far less coat than the face's lacquer.
        let tt = 0.25 + 0.12 * nl.y;
        let inner = smoothstep(0.03, -0.02, abs(pl.y - mouth_centre().y) - 0.5 * gap - 0.02);
        alb = grad4(u.skin, mix(clamp(tt, 0.0, 1.0), 0.06, inner)) * 0.85;
        spec_p = 100.0;
        spec_k = 0.6;
        sss = 0.15;
        coat = 0.15;
    } else if (mat == M_TEETH || mat == M_TUSK) {
        alb = grad4(u.mouth, 1.0);
        spec_p = 120.0;
        spec_k = 1.4;
        sss = 0.35;
        coat = 0.5;
        rimk = 0.5;
        emis += alb * 0.12 * (0.5 + br);
        if (mat == M_TUSK) {
            // Gold-capped tips.
            let cap = smoothstep(-0.33, -0.29, jaw_rest(pl).y);
            alb = mix(alb, gold, cap);
            metal = cap;
            emis += gold * cap * br * 0.3 * (1.0 + 3.0 * pu);
        }
    } else if (mat == M_TONGUE) {
        alb = mouth_tongue_base();
        spec_p = 260.0;
        spec_k = 3.0;
        sss = 0.8;
        coat = 1.2;
    } else if (mat == M_CAVITY) {
        // Gums near the teeth roots, throat deeper in.
        let dt = sd_teeth(pl);
        let gum = smoothstep(0.08, 0.01, dt);
        let depth = smoothstep(0.15, 0.5, cavity_centre().z + 0.25 - pl.z);
        alb = mix(mouth_cavity_base(), mouth_gum_base(), gum);
        spec_p = 80.0;
        spec_k = 0.6 * gum;
        sss = 0.3 * gum;
        rimk = 0.0;
        coat = 0.1 * gum;
        emis += mix(grad4(u.neon, 0.0), grad4(u.mouth, 0.66), 0.6) * (1.0 - gum) * depth * depth * smoothstep(0.0, 0.2, gap) * br * 0.04;
    } else if (mat == M_EYE) {
        var ei = 0;
        if (pl.x < 0.0) { ei = 1; }
        let iu = iris_uv(pl, ei);
        let pq = pupil_dir(pl, ei);
        let iris = smoothstep(0.62, 0.56, iu.x);
        let fib = 0.5 + 0.5 * sin(iu.y * 28.0 + iu.x * 18.0 + tf);
        let slit = smoothstep(0.05, 0.02, abs(pq.x)) * smoothstep(0.5, 0.42, length(pq.xy));
        let open = clamp(eye_open(ei), 0.0, 1.0);
        alb = mix(gold * 0.8, gold * (0.8 + 0.6 * fib), iris);
        alb = mix(alb, grad4(u.neon, fract(0.1 + tr * iu.x + tf * 0.05)), iris * smoothstep(0.3, 0.55, iu.x) * 0.6);
        alb *= eye_iris_tint(iu.x / 0.6);
        alb = surf_eyes_mix(alb, pl, nl);
        alb = mix(alb, eye_pupil_color(abs(pq.x) / 0.05) * 0.4, slit);
        metal = 0.7 * (1.0 - slit);
        spec_p = 220.0;
        spec_k = 2.0;
        emis += alb * k_ojos() * (0.6 + 1.6 * iris) * (0.4 + 0.6 * open) * (1.0 + 1.5 * pu);
        emis += grad4(u.neon, 1.0) * smoothstep(0.62, 0.58, iu.x) * smoothstep(0.52, 0.58, iu.x) * k_ojos() * 2.0;
        emis *= 1.0 - slit;
        emis += eye_catchlight(pl, ei);
    }

    if (mat == M_SKIN || mat == M_LID) { alb = surf_skin_mix(alb, pl, nl); }

    // Light.
    let key_c = vec3<f32>(1.0, 0.94, 0.86) * (1.1 + 0.8 * pu);
    let amb = mix(grad4(u.background, 0.0) + grad4(u.skin, 0.1) * 0.25, grad4(u.neon, 0.5) * 0.18, 0.5 + 0.5 * n.y) * 0.5;
    let diff_c = alb * (1.0 - 0.75 * metal);
    var col = diff_c * (key_c * dif + amb) * ao;
    // Subsurface warmth (mouth parts): wrap light and back light.
    col += alb * grad4(u.mouth, 0.66) * sss * (0.25 * max(dot(n, kl) + 0.5, 0.0) * sh + 0.15) * ao;
    // Specular: the clear coat, tinted for metals.
    let sc = mix(spec_c, alb, metal);
    col += sc * key_c * pow(ndh, spec_p) * spec_k * dif * (2.0 + spec_p * 0.01);
    col += mix(vec3<f32>(fres), alb * (0.5 + 0.5 * fres), metal) * on_env(refl) * coat * ao;
    // Neon rims, two colours from behind.
    let ra = normalize(vec3<f32>(-0.9, 0.35, -0.55));
    let rb = normalize(vec3<f32>(0.9, 0.2, -0.55));
    let rim = pow(1.0 - ndv, 2.5) * (0.3 + 1.4 * br) * rimk;
    col += grad4(u.neon, 0.33) * rim * max(dot(n, ra) + 0.45, 0.0);
    col += grad4(u.neon, 0.0) * rim * max(dot(n, rb) + 0.45, 0.0);
    // Light from inside the open mouth.
    let lm = mcw - pw;
    let dm = length(lm);
    let mouth_l = smoothstep(0.0, 0.2, gap) * max(dot(n, lm / dm) + 0.3, 0.0) / (1.0 + dm * dm * 30.0);
    col += alb * mix(grad4(u.neon, 0.0), grad4(u.mouth, 0.66), 0.6) * mouth_l * (0.8 + 1.5 * br) * select(1.0, 0.3, mat == M_CAVITY);
    // The flash.
    col += grad4(u.neon, 1.0) * pu * 0.5 * pow(1.0 - ndv, 2.0);
    return col + emis;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    surf_uv = in.uv;
    on_read_rig();
    let aspect = k_aspect();
    let cam = on_camera(in.uv);
    let ro = cam[0];
    let rd = cam[1];
    let p2 = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    // The mouth, in world space and on screen.
    let mcw = head_matrix() * (mouth_centre() + vec3<f32>(0.0, 0.0, -0.1));
    let ms = on_project(mcw, ro);
    let ctr = on_project(vec3<f32>(0.0), ro);
    let hit = on_march(ro, rd);
    var col: vec3<f32>;
    if (hit.x < 0.0) {
        col = on_background(in.uv, p2, ms, ctr, hit.z);
    } else {
        col = on_shade(ro, rd, hit.x, hit.y, mcw);
    }
    // Breath glow in front of an open mouth, over everything.
    let gap = mouth_gap();
    let md = length((p2 - ms) * vec2<f32>(0.8, 1.4));
    col += mix(grad4(u.neon, 0.0), grad4(u.mouth, 0.66), 0.5) * exp(-md * 14.0) * smoothstep(0.05, 0.3, gap) * k_brillo() * 0.25;
    col = hue_rotate(col, k_matiz());
    // Vignette and a little grain.
    let vig = 1.0 - 0.35 * dot(p2 / vec2<f32>(aspect, 1.0), p2 / vec2<f32>(aspect, 1.0)) * 0.5;
    col *= vig;
    col += (hash21(in.uv * 977.0 + fract(k_time())) - 0.5) * 0.015;
    return vec4<f32>(tonemap(col), 1.0);
}
