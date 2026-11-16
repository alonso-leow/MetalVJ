// monstruo — quetzalcóatl, the feathered serpent.
//
// A serpent head with a long snout, carved stone brow plates and venom fangs,
// emerging from a great radiating collar of quetzal plumes, as on the
// Teotihuacan temple heads. The skin is a turquoise/jade mosaic (Voronoi
// tesserae in gold grout) from `piel`, with stepped-fret (xicalcoliuhqui)
// neon inlays from `neon` that crawl with `flujo` and unfold over the whole
// skin as `trip` rises. The mouth is the rig's: gold labial plates round an
// obsidian throat lit by fire from inside, ivory fangs, a wet forked tongue.
// Eyes are obsidian with a gold iris and an emissive slit. Behind, a sun-stone
// calendar ring of abstract glyph bands and a solar ray burst, clicking a
// notch at a time, dissolving into a feathered fractal vortex with `trip`.
//
// Cost: the crest and the mouth are bounded (a cheap distance outside their
// volume), the march is the rig's budget, AO taps follow `detalle`, the soft
// shadow only runs at `detalle` > 0.7, the background is 2D.

const QU_FEATHER: f32 = 10.0;
const QU_PLATE: f32 = 11.0;
const QU_TAU: f32 = 6.2831853;
const QU_PI: f32 = 3.14159265;
// The crest's centre in the head's xy plane, and its radial reach.
const QU_CREST_Y: f32 = -0.08;
const QU_R0: f32 = 0.52;

// ---- small helpers --------------------------------------------------------

// A phase that runs with `flujo`, still slowly alive at 0.
fn qu_flow() -> f32 {
    return k_time() * (0.06 + 1.1 * k_flujo());
}

fn qu_gold() -> vec3<f32> {
    // Gold, shaped from the skin's highlight so a palette still reaches it.
    let h = grad4(u.skin, 1.0);
    return mix(h, vec3<f32>(1.0, 0.72, 0.28) * max(dot(h, vec3<f32>(0.333)), 0.5), 0.65);
}

fn qu_sd_seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

// The jaw line of the rest face: from the pivot forward to the mouth.
fn qu_mouth_rest() -> vec2<f32> {
    let s = s_snout();
    return vec2<f32>(-0.45 - 0.05 * s, 0.88 + 0.45 * s); // (y, z)
}

fn qu_jaw_line(z: f32) -> f32 {
    let pv = jaw_pivot();
    let m = qu_mouth_rest();
    let t = (z - pv.z) / (m.y - pv.z);
    return pv.y + (m.x - pv.y) * clamp(t, 0.0, 1.25);
}

// ---- the skull ------------------------------------------------------------

fn qu_upper(p: vec3<f32>) -> f32 {
    let m = qu_mouth_rest();
    let w = s_jaw_width();
    // Cranium, wide and flat-topped like a viper's.
    var d = sd_ellipsoid(p - vec3<f32>(0.0, 0.08, 0.02), vec3<f32>(0.74, 0.52, 0.82));
    // The face block carrying the brows and eyes.
    d = smin(d, sd_ellipsoid(p - vec3<f32>(0.0, 0.18, 0.42), vec3<f32>(0.6, 0.36, 0.46)), 0.12);
    // The long upper snout.
    let sn = sd_ellipsoid(p - vec3<f32>(0.0, m.x + 0.13, m.y - 0.5), vec3<f32>(0.32 * w, 0.21, 0.56));
    d = smin(d, sn, 0.2);
    // A dorsal ridge down the snout's midline.
    let ridge = sd_capsule(p, vec3<f32>(0.0, 0.42, 0.55), vec3<f32>(0.0, m.x + 0.31, m.y - 0.02), 0.035);
    d = smin(d, ridge, 0.08);
    // Eye bosses.
    for (var i = 0; i < 2; i++) {
        let e = eye_centre(i);
        let boss = sd_ellipsoid(p - (e + vec3<f32>(0.03 * sign(e.x), 0.0, -0.08)), vec3<f32>(0.23, 0.22, 0.2));
        d = smin(d, boss, 0.08);
    }
    // Cheeks: they puff with the rig.
    for (var i = 0; i < 2; i++) {
        let c = cheek(i);
        d = smin(d, length(p - c.xyz - vec3<f32>(0.0, 0.0, -0.1)) - c.w * 0.82, 0.14);
    }
    // Nostril bosses.
    for (var i = 0; i < 2; i++) {
        let n = nostril(i);
        d = smin(d, length((p - n.xyz - vec3<f32>(0.0, 0.0, -0.02)) * vec3<f32>(1.0, 1.3, 1.0)) - n.w * 1.7, 0.06);
    }
    // Everything below the jaw line belongs to the lower jaw.
    d = smax(d, qu_jaw_line(p.z) - p.y, 0.03);
    // Eye sockets.
    for (var i = 0; i < 2; i++) {
        d = smax(d, -(length(p - eye_centre(i)) - eye_radius(i) * 1.13), 0.025);
    }
    // Nostril holes; they open with the flare.
    for (var i = 0; i < 2; i++) {
        let n = nostril(i);
        let hole = length((p - n.xyz - vec3<f32>(0.0, 0.0, 0.02)) * vec3<f32>(1.25, 0.85, 0.6)) - n.w * 0.85;
        d = smax(d, -hole, 0.012);
    }
    return d + rig_creases(p);
}

// The lower jaw at rest; evaluated at jaw_rest(p).
fn qu_lower(q: vec3<f32>) -> f32 {
    let m = qu_mouth_rest();
    let w = s_jaw_width();
    var d = sd_ellipsoid(q - vec3<f32>(0.0, m.x - 0.1, m.y - 0.52), vec3<f32>(0.3 * w, 0.13, 0.56));
    d = smin(d, sd_ellipsoid(q - vec3<f32>(0.0, -0.5, 0.22), vec3<f32>(0.5 * w, 0.2, 0.5)), 0.16);
    d = smin(d, sd_ellipsoid(q - vec3<f32>(0.0, -0.52, -0.12), vec3<f32>(0.52, 0.28, 0.42)), 0.18);
    // Labial ridge along the jaw's edge.
    d = smax(d, q.y - qu_jaw_line(q.z), 0.03);
    return d;
}

// The carved stone brow plates, from the rig's brow points: a heavy lower
// plate and a stepped upper tier.
fn qu_plates(p: vec3<f32>) -> f32 {
    var d = 1e9;
    for (var s = 0; s < 2; s++) {
        let a = brow(s, 0);
        let b = brow(s, 1);
        let c = brow(s, 2);
        let my = b.y;
        let q = vec3<f32>(p.x, my + (p.y - my) * 1.7, p.z);
        let t1 = min(sd_round_cone(q, a, b, 0.085, 0.075), sd_round_cone(q, b, c, 0.075, 0.05)) / 1.7;
        let up = vec3<f32>(0.0, 0.075, -0.05);
        let q2 = vec3<f32>(p.x, my + up.y + (p.y - my - up.y) * 1.7, p.z);
        let t2 = min(sd_round_cone(q2, a + up, b + up, 0.06, 0.05), sd_round_cone(q2, b + up, c + up * 0.6, 0.05, 0.03)) / 1.7;
        d = min(d, min(t1, t2));
    }
    return d;
}

fn qu_skull(p: vec3<f32>) -> f32 {
    let up = qu_upper(p);
    let lo = qu_lower(jaw_rest(p));
    var d = smin(up, lo, 0.035);
    // The gape web: a shell round the cavity seals the open jaw's sides.
    let web = sd_ellipsoid(p - cavity_centre(), cavity_radii() + vec3<f32>(0.07, 0.06, -0.02));
    d = min(d, web);
    return d;
}

// ---- the crest of quetzal plumes ------------------------------------------

fn qu_plume_n() -> f32 {
    return floor(16.0 + 12.0 * clamp(k_detalle(), 0.0, 1.0));
}

fn qu_r1max() -> f32 {
    return 1.62 + 0.24 * k_pulso();
}

// One plume k of a layer: (distance, along 0..1, across −1..1, id).
fn qu_plume(p: vec3<f32>, k: f32, n: f32, layer: f32) -> vec4<f32> {
    let step = QU_TAU / n;
    let a0 = (k + 0.5 * layer) * step;
    let fl = qu_flow();
    let id = k + 41.0 * layer;
    let h = fract(sin(id * 12.9898 + 4.1) * 43758.5453);
    let pul = k_pulso();
    let r1 = mix(1.6, 1.22, layer) * (0.96 + 0.08 * h) + 0.24 * pul;
    let q = p.xy - vec2<f32>(0.0, QU_CREST_Y);
    let r = length(q);
    let sc = clamp((r - QU_R0) / (r1 - QU_R0), 0.0, 1.0);
    let sway = (0.04 + 0.08 * k_trip()) * sc * sc * sin(fl * 2.1 + h * QU_TAU + sc * 3.0);
    var da = atan2(q.y, q.x) - a0 - sway;
    da = da - QU_TAU * round(da / QU_TAU);
    let v = da * r;
    // A quetzal plume: narrow quill, broad vane, rounded tip.
    let leaf = sqrt(max(sin(QU_PI * pow(sc, 1.25)), 0.0));
    let wmax = 0.5 * step * r1 * (1.12 + 0.3 * pul) * mix(1.0, 0.85, layer);
    let w = wmax * (0.12 + 0.88 * leaf);
    // Plumes sweep back, ripple with the flow, curl forward on a hit.
    var zc = -0.3 - 0.22 * sc + 0.16 * layer;
    zc += (0.03 + 0.05 * k_flujo()) * sc * sin(sc * 7.0 - fl * 4.0 + h * QU_TAU);
    zc += 0.3 * pul * sc * sc;
    let vn = v / max(w, 1e-3);
    let thick = 0.012 + 0.018 * max(1.0 - abs(vn), 0.0) * (1.0 - sc);
    let dz = abs(p.z - zc) - thick;
    let dv = abs(v) - w;
    let dr = max(QU_R0 - r, r - r1);
    let d2 = max(dv, dr);
    let d = length(max(vec2<f32>(d2, dz), vec2<f32>(0.0))) + min(max(d2, dz), 0.0);
    return vec4<f32>(d * 0.8, sc, vn, id);
}

fn qu_crest_bound(p: vec3<f32>) -> f32 {
    let r = length(p.xy - vec2<f32>(0.0, QU_CREST_Y));
    return max(max(QU_R0 - 0.05 - r, r - qu_r1max() - 0.05), max(-0.82 - p.z, p.z - 0.3));
}

fn qu_crest(p: vec3<f32>) -> vec4<f32> {
    let n = qu_plume_n();
    let step = QU_TAU / n;
    let q = p.xy - vec2<f32>(0.0, QU_CREST_Y);
    let ang = atan2(q.y, q.x);
    var best = vec4<f32>(1e9, 0.0, 0.0, 0.0);
    for (var layer = 0; layer < 2; layer++) {
        let lf = f32(layer);
        let x = ang / step - 0.5 * lf;
        let k0 = round(x);
        let k1 = k0 + select(-1.0, 1.0, x > k0);
        let a = qu_plume(p, k0, n, lf);
        if (a.x < best.x) { best = a; }
        let b = qu_plume(p, k1, n, lf);
        if (b.x < best.x) { best = b; }
    }
    return best;
}

// ---- the head -------------------------------------------------------------

fn qu_fang(p: vec3<f32>, i: i32) -> f32 {
    let root = u.face.upper_teeth[2 * i];
    let tip = u.face.upper_teeth[2 * i + 1];
    let dir = tip.xyz - root.xyz;
    let end = tip.xyz + dir * 0.9 + vec3<f32>(0.0, 0.0, -0.025);
    return sd_round_cone(p, root.xyz + dir * 0.3, end, root.w * 0.95, 0.006);
}

// Everything the mouth can reach: the cavity back to the lips, the lower lip
// wherever the jaw has taken it, and the tongue's tip.
fn qu_mouth_bound(p: vec3<f32>) -> f32 {
    let back = cavity_centre() - vec3<f32>(0.0, 0.0, cavity_radii().z);
    let lo = lip(3 * LIP_RING / 4);
    let b0 = sd_capsule(p, back, mouth_centre(), 0.42 + 0.5 * mouth_gap());
    let b1 = sd_capsule(p, back, lo, 0.3);
    let tip = u.face.tongue[7].xyz;
    let b2 = sd_capsule(p, mouth_centre(), tip, 0.2);
    return min(b0, min(b1, b2));
}

fn map_head(p: vec3<f32>) -> vec2<f32> {
    var res = vec2<f32>(1e9, M_SKIN);
    // The crest, bounded.
    let cb = qu_crest_bound(p);
    if (cb > 0.03) {
        res = vec2<f32>(cb, QU_FEATHER);
    } else {
        res = vec2<f32>(qu_crest(p).x, QU_FEATHER);
    }
    let skull = qu_skull(p);
    let mb = qu_mouth_bound(p);
    var face = vec2<f32>(skull, M_SKIN);
    if (mb > 0.03) {
        face.x = min(skull, mb);
        // Eyes and lids only.
        for (var i = 0; i < 2; i++) {
            let de = sd_eyeball(p, i);
            if (de < face.x) { face = vec2<f32>(de, M_EYE); }
            let dd = sd_lids(p, i);
            if (dd < face.x + 0.01) {
                face = vec2<f32>(smin(face.x, dd, 0.01), select(face.y, M_LID, dd < face.x));
            }
        }
    } else {
        face = rig_face(p, skull);
        let df = min(qu_fang(p, 1), qu_fang(p, TEETH - 2));
        if (df < face.x) { face = vec2<f32>(df, M_TEETH); }
    }
    let pl = qu_plates(p);
    if (pl < face.x + 0.02) {
        face = vec2<f32>(smin(face.x, pl, 0.02), select(face.y, QU_PLATE, pl < face.x));
    }
    if (face.x < res.x) { res = face; }
    return res;
}

// ---- march with a glow ----------------------------------------------------

// rig_march, also returning how close the ray came to the head (for the
// halo): (t, material, steps, min distance).
fn qu_march(ro: vec3<f32>, rd: vec3<f32>) -> vec4<f32> {
    let n = rig_steps();
    let b = dot(ro, rd);
    let c = dot(ro, ro) - 2.2 * 2.2;
    let h = b * b - c;
    if (h < 0.0) {
        return vec4<f32>(-1.0, 0.0, 0.0, 9.0);
    }
    var t = max(-b - sqrt(h), 0.0);
    let tmax = -b + sqrt(h);
    var md = 9.0;
    for (var i = 0; i < n; i++) {
        let r = rig_map(ro + rd * t);
        md = min(md, r.x);
        if (r.x < 0.0008 * t) {
            return vec4<f32>(t, r.y, f32(i) / f32(n), 0.0);
        }
        t += r.x * 0.85;
        if (t > tmax) {
            break;
        }
    }
    return vec4<f32>(-1.0, 0.0, 1.0, md);
}

// ---- patterns -------------------------------------------------------------

// 3D Voronoi: (F1, F2, cell hash, unused) and the direction from the nearest
// to the second-nearest site.
struct QuCell {
    f: vec4<f32>,
    dir: vec3<f32>,
};

fn qu_voronoi(p: vec3<f32>, jit: f32) -> QuCell {
    let ip = floor(p);
    let fp = p - ip;
    var f1 = 9.0;
    var f2 = 9.0;
    var id = 0.0;
    var s1 = vec3<f32>(0.0);
    var s2 = vec3<f32>(0.0);
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let g = vec3<f32>(f32(x), f32(y), f32(z));
                let c = ip + g;
                let o = vec3<f32>(lattice3(c), lattice3(c + vec3<f32>(17.0, 3.0, 9.0)), lattice3(c + vec3<f32>(-5.0, 29.0, 13.0)));
                let site = g + 0.5 + (o - 0.5) * jit;
                let dd = length(site - fp);
                if (dd < f1) {
                    f2 = f1; s2 = s1;
                    f1 = dd; s1 = site; id = o.x;
                } else if (dd < f2) {
                    f2 = dd; s2 = site;
                }
            }
        }
    }
    var cellr: QuCell;
    cellr.f = vec4<f32>(f1, f2, id, 0.0);
    cellr.dir = normalize(s2 - s1 + vec3<f32>(1e-5));
    return cellr;
}

// The stepped fret (xicalcoliuhqui) in one cell of a band: x in 0..2, y in
// 0..1. Distance to the stroke.
fn qu_fret_cell(c: vec2<f32>) -> f32 {
    var pts = array<vec2<f32>, 14>(
        vec2<f32>(0.0, 0.12), vec2<f32>(0.25, 0.12), vec2<f32>(0.25, 0.32), vec2<f32>(0.5, 0.32),
        vec2<f32>(0.5, 0.52), vec2<f32>(0.75, 0.52), vec2<f32>(0.75, 0.86), vec2<f32>(1.86, 0.86),
        vec2<f32>(1.86, 0.14), vec2<f32>(1.14, 0.14), vec2<f32>(1.14, 0.62), vec2<f32>(1.6, 0.62),
        vec2<f32>(1.6, 0.38), vec2<f32>(1.38, 0.38)
    );
    var d = 9.0;
    for (var i = 0; i < 13; i++) {
        d = min(d, qu_sd_seg(c, pts[i], pts[i + 1]));
    }
    return d;
}

// A fret band coordinate (x along, y across in −0.5..0.5) → stroke distance,
// alternating cells turned half a turn so the hooks interlock.
fn qu_fret(x: f32, y: f32) -> f32 {
    let cell = floor(x / 2.0);
    var c = vec2<f32>(x - cell * 2.0, y + 0.5);
    if (fract(cell * 0.5) > 0.25) {
        c = vec2<f32>(2.0, 1.0) - c;
    }
    let border = min(abs(y + 0.5), abs(y - 0.5));
    return min(qu_fret_cell(c), border);
}

// ---- the background: sun stone, rays, the feathered vortex ----------------

fn qu_glyph(c: vec2<f32>, kind: f32) -> f32 {
    // c in −1..1 inside a glyph cell. Returns coverage 0..1. Abstract marks:
    // nested squares, a quincunx, a stepped chevron, a ringed bar.
    let a = abs(c);
    var d = 9.0;
    if (kind < 0.25) {
        d = min(abs(max(a.x, a.y) - 0.7), abs(max(a.x, a.y) - 0.3));
    } else if (kind < 0.5) {
        d = length(a - vec2<f32>(0.55)) - 0.18;
        d = min(d, length(c) - 0.22);
    } else if (kind < 0.75) {
        let st = floor((c.x + 1.0) * 2.0) * 0.4 - 0.8;
        d = abs(c.y - st) - 0.04;
        d = max(d, a.x - 0.85);
        d = min(d, abs(c.y + 0.8));
    } else {
        d = min(abs(length(c) - 0.6), max(a.x - 0.75, a.y - 0.08));
    }
    return 1.0 - smoothstep(0.06, 0.14, d);
}

fn qu_background(uv: vec2<f32>, ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    let trip = k_trip();
    let fl = qu_flow();
    let pul = k_pulso();
    let aura = k_aura();
    let bri = k_brillo();
    var col = backdrop_from_uv(u.background, uv, k_aspect()) * 0.45;
    // The calendar plane, behind the crest, in world space.
    let tp = (-1.4 - ro.z) / rd.z;
    let wp = ro + rd * tp;
    var q = wp.xy - vec2<f32>(0.0, QU_CREST_Y);
    var r = length(q);
    var a = atan2(q.y, q.x);
    // Trip twists the plane into a vortex.
    let vort = smoothstep(0.55, 1.0, trip);
    a += trip * (0.9 / (r + 0.3)) * sin(fl * 0.3) + vort * (1.2 / (r + 0.2) + fl * 0.4);
    let gold = qu_gold();

    // Sun stone: the ring clicks a notch at a time; a hit advances it.
    let notch = QU_TAU / 20.0;
    let tick = k_time() * (0.15 + 0.6 * k_flujo());
    let rot = (floor(tick) + smoothstep(0.82, 1.0, fract(tick)) + 0.5 * pul) * notch;
    let ar = a - rot;
    let stone = 1.0 - vort * 0.85;
    let R = 2.05;
    var ring = 0.0;
    var glyph = 0.0;
    // Thin bounding rings.
    for (var i = 0; i < 4; i++) {
        let rr = R * (1.0 + 0.09 * f32(i) + select(0.0, 0.25, i == 3));
        ring = max(ring, 1.0 - smoothstep(0.008, 0.022, abs(r - rr)));
    }
    // The glyph band between the first two rings.
    let gb = (r - R * 1.09) / (R * 0.16);
    if (gb > 0.0 && gb < 1.0) {
        let cell = floor(ar / notch);
        let fx = fract(ar / notch) * 2.0 - 1.0;
        let kind = lattice(vec2<f32>(cell + 40.0, 3.0));
        glyph = qu_glyph(vec2<f32>(fx * 1.15, gb * 2.0 - 1.0), kind);
    }
    // A band of dots.
    let db = (r - R * 1.27) / (R * 0.06);
    var dots = 0.0;
    if (db > -1.0 && db < 1.0) {
        let fx = (fract(ar / (notch * 0.5)) * 2.0 - 1.0);
        dots = 1.0 - smoothstep(0.35, 0.55, length(vec2<f32>(fx, db)));
    }
    // Solar rays: eight great, eight small, pointed outward.
    let ray8 = abs(fract(ar / (QU_TAU / 8.0)) - 0.5) * 2.0; // 0 at a ray's centre
    let ray16 = abs(fract(ar / (QU_TAU / 8.0) + 0.5) - 0.5) * 2.0;
    let rb = r - R * 1.36;
    let big = (1.0 - smoothstep(0.0, 0.03, ray8 * 0.55 - (1.0 - rb / (R * (0.75 + 0.3 * pul))) * 0.5)) * step(0.0, rb);
    let small = (1.0 - smoothstep(0.0, 0.03, ray16 * 0.55 - (1.0 - rb / (R * 0.4)) * 0.4)) * step(0.0, rb);
    let rays = max(big, small * 0.7) * exp(-max(rb, 0.0) * 0.9);
    let carve = max(max(ring, glyph * 0.9), max(dots * 0.8, rays * 0.6));
    let stone_col = mix(grad4(u.background, 0.6) * 1.6, gold, 0.55);
    col = mix(col, stone_col * (0.35 + 0.65 * bri), carve * stone * (0.45 + 0.5 * aura));
    // Neon in the glyph cuts, a slow wave round the ring.
    let wave = 0.5 + 0.5 * sin(ar * 3.0 - fl * 2.0);
    col += grad4(u.neon, fract(ar / QU_TAU + 0.3)) * glyph * wave * bri * 0.6 * stone;

    // The feathered fractal vortex: log-polar plumes at three scales.
    if (vort > 0.001) {
        var v = vec3<f32>(0.0);
        let lr = log(max(r, 1e-3));
        for (var o = 0; o < 3; o++) {
            let fo = f32(o);
            let n = 9.0 + 6.0 * fo;
            let lp = vec2<f32>(a * n / QU_TAU + fo * 0.37, (lr - fl * (0.35 + 0.1 * fo)) * n / QU_TAU * 1.6);
            let cid = floor(lp);
            var f = fract(lp) - 0.5;
            f.x += 0.15 * sin(cid.y * 1.7 + fl);
            let along = f.y + 0.5;
            let wv = 0.42 * sqrt(max(sin(QU_PI * along), 0.0));
            let vane = 1.0 - smoothstep(wv - 0.06, wv, abs(f.x));
            let eye = length(vec2<f32>(f.x, f.y - 0.18) * vec2<f32>(1.0, 1.3));
            let ocell = (1.0 - smoothstep(0.1, 0.13, eye)) + 0.6 * (1.0 - smoothstep(0.02, 0.04, abs(eye - 0.19)));
            let barbs = 0.6 + 0.4 * sin(abs(f.x) * 60.0 - along * 12.0);
            let tint = grad4(u.skin, fract(0.45 + 0.25 * f.y + 0.2 * fo + 0.1 * sin(cid.x)));
            v += (tint * vane * barbs * (0.5 + 0.5 * along) + grad4(u.neon, fract(cid.x * 0.13 + fo * 0.3)) * ocell * 1.3) * (0.75 - 0.2 * fo);
        }
        col = mix(col, v * (0.3 + 0.5 * bri), vort * smoothstep(0.15, 0.9, r));
    }

    // Aura: domain-warped energy round the head, from the neon slot.
    let oct = 2 + i32(3.0 * k_detalle());
    let wq = wp.xy * 1.2;
    let warp = vec2<f32>(lattice_fbm(wq + vec2<f32>(fl * 0.3, 0.0), oct), lattice_fbm(wq + vec2<f32>(5.2, -fl * 0.25), oct));
    let e = lattice_fbm(wq * 1.5 + 2.5 * warp * (0.6 + trip), oct);
    let halo = exp(-pow(max(r - 1.3, 0.0) * 1.1, 2.0)) * (0.35 + 0.65 * e);
    col += grad4(u.neon, fract(e * 0.8 + 0.1 * fl + 0.2 * trip)) * halo * e * e * aura * (0.2 + 0.35 * bri);

    // The solar flare on a hit: a ring blown out to the rays and a flash.
    let fr = R * (0.9 + 0.5 * pul);
    col += (grad4(u.neon, 0.4) * 0.6 + gold * 0.5) * pul * (exp(-abs(r - fr) * 10.0) * 1.4 + 0.04);
    // Sound rings out of the mouth when it sings.
    let mw = head_matrix() * mouth_centre();
    let md = mw - ro;
    let ms = md.xy / -md.z;
    let ps = rd.xy / -rd.z;
    let sr = length((ps - ms) * vec2<f32>(1.0, 1.0)) * 8.0;
    let gap = clamp(mouth_gap() * 4.0, 0.0, 1.0);
    let rings = pow(0.5 + 0.5 * sin(sr * 6.0 - k_time() * 9.0), 12.0) * exp(-sr * 0.35) * smoothstep(0.6, 1.4, sr);
    col += grad4(u.neon, 0.5) * rings * gap * (0.4 + 0.6 * aura) * bri;
    return col;
}

// ---- shading --------------------------------------------------------------

fn qu_spec(n: vec3<f32>, l: vec3<f32>, v: vec3<f32>, pw: f32) -> f32 {
    return pow(max(dot(n, normalize(l + v)), 0.0), pw);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    surf_uv = in.uv;
    let cam = rig_camera(in.uv);
    let ro = cam[0];
    let rd = cam[1];
    let trip = k_trip();
    let fl = qu_flow();
    let pul = k_pulso();
    let bri = k_brillo();
    let det = clamp(k_detalle(), 0.0, 1.0);
    let bg = qu_background(in.uv, ro, rd);
    let hit = qu_march(ro, rd);
    var col = bg;
    if (hit.x < 0.0) {
        // Rim halo of the silhouette.
        let g = exp(-hit.w * 18.0) * (0.25 + 0.5 * k_aura());
        col += grad4(u.neon, 0.35 + 0.2 * sin(fl)) * g * bri;
    } else {
        let p = ro + rd * hit.x;
        var n = rig_normal(p);
        let v = -rd;
        let hp = to_head(p);
        var hn = to_head(n);
        let mat = hit.y;
        let gold = qu_gold();
        // Lights, in world space: warm key from above-left, cool fill,
        // two neon rims from behind, fire from the mouth.
        let key = normalize(vec3<f32>(-0.55, 0.65, 0.6));
        let fill = normalize(vec3<f32>(0.6, -0.2, 0.7));
        let rimc = grad4(u.neon, 0.35);
        let mw = head_matrix() * mouth_centre();
        let gap = clamp(mouth_gap() * 3.5, 0.0, 1.0);
        let fire = grad4(u.mouth, 0.72) * vec3<f32>(1.0, 0.75, 0.5) + vec3<f32>(0.2, 0.05, 0.0);

        var base = grad4(u.skin, 0.5);
        var emit = vec3<f32>(0.0);
        var spw = 24.0;
        var spk = 0.3;
        var sss = 0.0;

        if (mat == QU_FEATHER) {
            let f = qu_crest(hp);
            let sc = f.y;
            let vn = f.z;
            let h = fract(sin(f.w * 7.13) * 4375.85);
            let ndv = abs(dot(n, v));
            // Iridescence: walk the skin slot with the view angle.
            let it = clamp(0.22 + 0.4 * (1.0 - ndv) + 0.14 * sc + 0.1 * (h - 0.5) + 0.12 * trip * sin(fl + sc * 6.0), 0.0, 1.0);
            base = grad4(u.skin, it) * 0.85;
            let barbs = 0.72 + 0.28 * sin(abs(vn) * 26.0 - sc * 70.0 * (0.8 + 0.4 * h));
            base *= barbs * (0.55 + 0.45 * smoothstep(0.0, 0.3, sc));
            // The quill.
            let quill = 1.0 - smoothstep(0.03, 0.08, abs(vn));
            base = mix(base, gold * 0.9, quill * 0.8);
            // The eye-spot near the tip: dark core, neon iris, gold ring.
            let ex = (sc - 0.8) * 4.2;
            let ed = length(vec2<f32>(ex, vn * 1.25));
            let core = 1.0 - smoothstep(0.24, 0.3, ed);
            let iris = (1.0 - smoothstep(0.42, 0.48, ed)) - core;
            let ringg = (1.0 - smoothstep(0.6, 0.68, ed)) - core - iris;
            base = mix(base, gold, max(ringg, 0.0));
            base = mix(base, grad4(u.neon, fract(0.15 + 0.3 * h + 0.2 * trip)) * 0.6, max(iris, 0.0));
            base = mix(base, grad4(u.skin, 0.0) * 0.6, core);
            emit += grad4(u.neon, fract(0.15 + 0.3 * h + 0.2 * trip)) * max(iris, 0.0) * bri * (0.45 + 0.9 * pul);
            emit += gold * core * (0.5 + 0.5 * sin(fl * 3.0 + h * 6.0)) * bri * 0.25;
            // A shimmer running out along the plume.
            let wave = pow(0.5 + 0.5 * sin(sc * 10.0 - fl * 5.0 + h * 6.0), 6.0);
            emit += grad4(u.neon, fract(sc + 0.5 * h)) * wave * bri * (0.15 + 0.6 * trip) * smoothstep(0.1, 0.4, sc);
            emit += grad4(u.neon, 0.4) * pul * smoothstep(0.3, 1.0, sc) * 0.5;
            spw = 40.0;
            spk = 0.5;
            if (dot(n, v) < 0.0) { n = -n; hn = -hn; }
        } else if (mat == M_SKIN || mat == M_LID || mat == QU_PLATE) {
            // Turquoise/jade mosaic, gold grout.
            let melt = trip * 0.18;
            let wp = hp + melt * vec3<f32>(sin(hp.y * 5.0 + fl), sin(hp.z * 5.0 - fl), sin(hp.x * 5.0 + fl * 0.7));
            let scale = 7.0 + 5.0 * s_scales();
            let cell = qu_voronoi(wp * scale, 0.85);
            let edge = cell.f.y - cell.f.x;
            let grout = 1.0 - smoothstep(0.03, 0.09, edge);
            let tid = cell.f.z;
            let tt = clamp(0.22 + 0.4 * tid + 0.08 * sin(tid * 40.0 + trip * fl * 2.0) + trip * 0.25 * sin(tid * 6.0 + fl), 0.0, 1.0);
            base = grad4(u.skin, tt);
            // Tile bevel: the normal tilts away from the grout.
            let bev = 1.0 - smoothstep(0.0, 0.18, edge);
            let tdir = cell.dir - hn * dot(cell.dir, hn);
            hn = normalize(hn - tdir * bev * 0.6);
            base = mix(base * (0.75 + 0.25 * tid), gold * 0.7, grout);
            spk = mix(0.35, 0.9, grout);
            spw = mix(30.0, 60.0, grout);
            // Under the jaw: paler belly scutes.
            let belly = smoothstep(-0.45, -0.75, hp.y) * smoothstep(0.35, 0.0, abs(hp.x));
            base = mix(base, grad4(u.skin, 0.85) * 0.8, belly * 0.6);

            // Stepped-fret inlays: a band across the forehead over the brow
            // plates and one along each side of the jaw; trip unfolds them
            // into bands over the whole skin.
            let unfold = smoothstep(0.35, 0.95, trip);
            let fu = hp.x * 7.0 * sign(hp.x + 1e-4) + fl * 0.9;
            let fy = (hp.y - 0.66) * 7.0;
            let bandi = round(fy / 2.6);
            let isjaw = select(0.0, 1.0, abs(hp.x) > 0.22 && hp.y < -0.4 && hp.y > -0.62);
            let bandon = max(select(0.0, 1.0, bandi == 0.0), isjaw) + unfold * select(0.0, 1.0, bandi != 0.0);
            var ly = fy - bandi * 2.6;
            var fx = fu + bandi * 0.7 + unfold * sin(fl * 0.5 + bandi);
            if (isjaw > 0.5) {
                ly = (hp.y + 0.51) * 9.0;
                fx = hp.z * 9.0 - fl * 0.9;
            }
            if (abs(ly) < 0.5 && bandon > 0.0 && hp.z > 0.0) {
                let fd = qu_fret(fx, ly);
                let stroke = 1.0 - smoothstep(0.05, 0.1, fd);
                let glow = exp(-fd * 9.0);
                let nc = grad4(u.neon, fract(fx * 0.05 + bandi * 0.21 + 0.1 * fl));
                base = mix(base, base * 0.35, (1.0 - stroke) * 0.4 * min(bandon, 1.0));
                base = mix(base, nc * 0.4, stroke * min(bandon, 1.0));
                emit += nc * (stroke * 1.2 + glow * 0.3) * bri * min(bandon, 1.0) * (0.6 + 0.4 * sin(fx * 0.7 - fl * 3.0));
            }
            // Pulso: the tesserae flare.
            emit += grad4(u.neon, 0.3 + 0.4 * tid) * pul * (1.0 - grout) * 0.5 * step(0.6, tid);
            if (mat == QU_PLATE) {
                // Carved stone: desaturated jade, incised steps, gold edge.
                let l = dot(base, vec3<f32>(0.333));
                base = mix(vec3<f32>(l), grad4(u.skin, 0.55), 0.35) * 1.1 + 0.05;
                let inc = abs(fract(hp.y * 22.0 + 0.3 * hp.x * 10.0) - 0.5);
                base *= 0.75 + 0.25 * smoothstep(0.1, 0.3, inc);
                let edge2 = pow(1.0 - abs(dot(n, v)), 3.0);
                base = mix(base, gold, edge2 * 0.6);
                spk = 0.25;
                spw = 18.0;
                emit += grad4(u.neon, 0.15) * (1.0 - smoothstep(0.03, 0.06, inc)) * bri * (0.15 + 0.6 * trip);
            }
            if (mat == M_LID) {
                base = mix(base, gold * 0.6, 0.25);
            }
        } else if (mat == M_LIP) {
            // Dark jade labial plates with a gold edge: the performer's
            // outline, dark against the mosaic, bright where it turns away.
            let seg = abs(fract(atan2(hp.y - mouth_centre().y, hp.x) * 3.8) - 0.5);
            let edgeg = pow(1.0 - abs(dot(n, v)), 2.0);
            base = mix(grad4(u.skin, 0.12) * 0.7, gold, 0.25 + 0.6 * edgeg);
            base *= 0.6 + 0.4 * smoothstep(0.02, 0.12, seg);
            spk = 1.2;
            spw = 60.0;
            emit += gold * edgeg * bri * 0.35;
        } else if (mat == M_TEETH) {
            base = grad4(u.mouth, 1.0);
            let tipg = smoothstep(mouth_centre().y + 0.02, mouth_centre().y - 0.15, hp.y);
            base = mix(base, gold, tipg * 0.25);
            spk = 1.4;
            spw = 70.0;
            sss = 0.2;
        } else if (mat == M_TONGUE) {
            base = mouth_tongue_base();
            let groove = 1.0 - smoothstep(0.0, 0.025, abs(hp.x - mouth_centre().x * 0.0));
            base *= 1.0 - 0.3 * groove;
            spk = 1.6;
            spw = 80.0;
            sss = 0.9;
            // The fork's tips glow.
            let tip = u.face.tongue[7].xyz;
            emit += grad4(u.neon, 0.05) * exp(-length(hp - tip) * 9.0) * bri * 0.8;
        } else if (mat == M_CAVITY) {
            // Obsidian throat with fire behind it.
            base = mouth_cavity_base();
            spk = 1.0;
            spw = 90.0;
            let depth = clamp((mouth_centre().z - hp.z) / 0.55, 0.0, 1.0);
            let fl2 = lattice_fbm(vec2<f32>(hp.x * 8.0, hp.y * 6.0 - k_time() * 3.0), 3);
            let tongues = smoothstep(0.45, 0.85, fl2 + 0.35 * depth - 0.25 * abs(hp.x - mouth_centre().x) * 4.0);
            let heat = pow(depth, 2.5) * (0.12 + 0.9 * tongues);
            emit += fire * heat * (0.25 + 0.8 * gap) * (0.4 + 0.6 * bri);
        } else if (mat == M_EYE) {
            // Obsidian eyeball, gold iris, emissive slit.
            var ei = 0;
            if (length(hp - eye_centre(1)) < length(hp - eye_centre(0))) { ei = 1; }
            let iu = iris_uv(hp, ei);
            let rr = iu.x;
            let ang = iu.y;
            let nn = pupil_dir(hp, ei);
            base = grad4(u.mouth, 0.0) * 0.3 + vec3<f32>(0.01);
            let irism = 1.0 - smoothstep(0.62, 0.68, rr);
            let stri = 0.6 + 0.4 * sin(ang * 26.0 + rr * 20.0 - fl * 2.0);
            let tint = eye_iris_tint(rr / 0.65);
            base = mix(base, gold * stri * 0.8 * tint, irism);
            let slit_w = 0.07 + 0.08 * eye_open(ei) * 0.5;
            let slit = (1.0 - smoothstep(slit_w, slit_w + 0.03, abs(nn.x))) * (1.0 - smoothstep(0.42, 0.5, abs(nn.y)));
            base = mix(base, eye_pupil_color(abs(nn.x) / slit_w) * 0.4, slit);
            let og = k_ojos() * clamp(eye_open(ei), 0.0, 1.0);
            emit += grad4(u.neon, fract(0.6 + 0.1 * sin(fl))) * slit * og * (1.5 + 2.0 * pul) * (0.5 + bri);
            emit += gold * tint * irism * og * 0.5 * stri;
            emit += eye_catchlight(hp, ei);
            spk = 2.0;
            spw = 120.0;
        }

        if (mat == M_SKIN || mat == M_LID) { base = surf_skin_mix(base, hp, hn); }
        if (mat == M_EYE) { base = surf_eyes_mix(base, hp, hn); }

        // World-space normal back from the bevelled head-local one.
        let nw = normalize(head_matrix() * hn);
        let ao = rig_ao(p, n);
        var sh = 1.0;
        if (det > 0.7 && mat != QU_FEATHER) {
            sh = mix(1.0, rig_shadow(p + n * 0.01, key, 10.0), smoothstep(0.7, 0.9, det));
        }
        let dk = max(dot(nw, key), 0.0) * sh;
        let df = max(dot(nw, fill), 0.0);
        let keyc = vec3<f32>(1.0, 0.86, 0.68) * 1.25;
        let fillc = grad4(u.background, 0.8) * 0.25 + vec3<f32>(0.02, 0.05, 0.08);
        var lit = base * (keyc * dk + fillc * df + vec3<f32>(0.04) + grad4(u.background, 0.5) * 0.15) * ao;
        lit += keyc * qu_spec(nw, key, v, spw) * spk * sh * ao;
        // Rims.
        let fres = pow(1.0 - max(dot(nw, v), 0.0), 3.0);
        let rimL = max(dot(nw, normalize(vec3<f32>(-0.8, 0.3, -0.5))), 0.0);
        let rimR = max(dot(nw, normalize(vec3<f32>(0.8, 0.3, -0.5))), 0.0);
        lit += rimc * fres * (rimL + rimR + 0.15) * (0.25 + 0.7 * bri);
        lit += grad4(u.neon, 0.4) * fres * pul * 0.7;
        // Fire light out of the mouth, onto the snout, chin and tongue.
        let ml = mw - p;
        let mdist = length(ml);
        let mlit = max(dot(nw, ml / mdist), 0.0) * gap / (1.0 + 12.0 * mdist * mdist);
        lit += base * fire * mlit * 2.5;
        // Subsurface warmth in the wet parts.
        lit += base * grad4(u.mouth, 0.55) * sss * (0.25 + 0.6 * gap) * (0.5 + 0.5 * (1.0 - abs(dot(nw, v))));
        if (mat == M_CAVITY) {
            // The throat is obsidian: no fill, no rim, only a glossy glint and
            // the fire inside.
            lit = base * 0.2 + keyc * qu_spec(nw, key, v, spw) * spk * ao * 0.5;
        }
        col = lit + emit;
        // Fog the far crest a little into the stone.
        if (mat == QU_FEATHER) {
            col = mix(col, bg, 0.08);
        }
    }

    // Trip: hue drift by radius, a chromatic shimmer in the colours.
    let sp = (in.uv - 0.5) * vec2<f32>(k_aspect(), 1.0);
    let drift = trip * 0.12 * sin(length(sp) * 6.0 - fl * 0.8);
    col = hue_rotate(col, k_matiz() + drift);
    // Vignette.
    col *= 1.0 - 0.35 * pow(length(sp) * 0.9, 2.5);
    return vec4<f32>(tonemap(col * 1.05), 1.0);
}
