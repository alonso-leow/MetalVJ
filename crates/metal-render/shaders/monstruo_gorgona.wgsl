// monstruo — GORGONA (species 2). Concatenated after `monstruo_rig.wgsl`.
//
// Medusa: a near-human face, beautiful and terrible, in jade-marble skin that
// cracks open into neon veins; fine serpent scales at the temples, jaw and
// neck; a crown of living snakes for hair; full sculpted lips (she is the
// singer of the five); and the petrifying gaze — op-art irises that throw
// beams when `ojos` is high. Behind her, a mirror-symmetric mandala of eyes
// and coiling serpents that unfolds into endless recursion with `trip`.
//
// Cost: the face is evaluated twice (upper head at p, lower jaw at
// jaw_rest(p)) only where the jaw seam needs both; the mouth parts, the eyes
// and every snake sit behind bounding spheres; snakes and their segments
// scale with `detalle` (8×4 at 0, 14×6 at 1); fbm never runs inside the SDF.
// Everything psychedelic is 2D, in the shading or the background.

const M_SNAKE: f32 = 10.0;
const M_SNAKE_EYE: f32 = 11.0;
const M_SNAKE_MOUTH: f32 = 12.0;

const GO_MAX_SNAKES: i32 = 14;
const GO_MAX_SEG: i32 = 6;
const GO_TAU: f32 = 6.2831853;

// ---- clocks and colour ----------------------------------------------------

// Flow time: never frozen, `flujo` sets the speed.
fn go_t() -> f32 {
    return k_time() * (0.1 + 1.25 * k_flujo());
}

fn go_det() -> f32 {
    return clamp(k_detalle(), 0.0, 1.0);
}

fn go_c(slot: ColorSlot, t: f32) -> vec3<f32> {
    return max(hue_rotate(grad4(slot, t), k_matiz()), vec3<f32>(0.0));
}

// Neon with a slow hue drift that grows with trip.
fn go_neon(t: f32) -> vec3<f32> {
    let drift = k_trip() * 0.18 * sin(go_t() * 0.45 + t * 3.0);
    return max(hue_rotate(grad4(u.neon, fract(t)), k_matiz() + drift), vec3<f32>(0.0));
}

// ---- the head -------------------------------------------------------------

// The whole face as modelled at rest (cranium, face, nose, cheeks, brows,
// sockets, chin, mandible). Called at p for the upper head and at
// jaw_rest(p) for the lower jaw.
fn go_skin(p: vec3<f32>, upper: bool) -> f32 {
    let jw = s_jaw_width();
    // Cranium, a little long front to back, and the face mask in front.
    var d = sd_ellipsoid(p - vec3<f32>(0.0, 0.12, -0.08), vec3<f32>(0.78, 0.92, 0.88));
    let face = sd_ellipsoid(p - vec3<f32>(0.0, -0.02, 0.34), vec3<f32>(0.58, 0.80, 0.58));
    d = smin(d, face, 0.16);
    // Cheekbones and the apples of the cheeks, from the rig (both halves,
    // so the jaw seam never steps across them).
    for (var i = 0; i < 2; i++) {
        let c = cheek(i);
        let sx = sign(c.x);
        let apple = length(p - (c.xyz + vec3<f32>(-0.02 * sx, 0.0, -0.11))) - c.w * 0.92;
        d = smin(d, apple, 0.14);
        let bone = sd_capsule(p, c.xyz + vec3<f32>(-0.06 * sx, 0.2, -0.1), c.xyz + vec3<f32>(0.12 * sx, 0.24, -0.26), 0.07);
        d = smin(d, bone, 0.12);
    }
    if (upper && p.y > -0.4) {
        // Orbits: the flesh around each eye, then the socket carved into it.
        for (var i = 0; i < 2; i++) {
            let ec = eye_centre(i);
            let orb = length((p - ec + vec3<f32>(0.0, 0.0, 0.07)) * vec3<f32>(0.9, 1.05, 1.0)) - eye_radius(i) * 1.3;
            d = smin(d, orb, 0.1);
        }
        // Brow ridge, along the rig's brows, set a touch behind them.
        for (var s = 0; s < 2; s++) {
            let back = vec3<f32>(0.0, -0.03, -0.05);
            let b0 = brow(s, 0) + back;
            let b1 = brow(s, 1) + back;
            let b2 = brow(s, 2) + back;
            d = smin(d, sd_round_cone(p, b0, b1, 0.05, 0.045), 0.08);
            d = smin(d, sd_round_cone(p, b1, b2, 0.045, 0.03), 0.08);
        }
        // The nose: bridge, tip, wings, nostrils.
        let n0 = nostril(0);
        let n1 = nostril(1);
        let tipc = vec3<f32>(0.0, 0.5 * (n0.y + n1.y) + 0.06, 0.5 * (n0.z + n1.z) - 0.025);
        let bridge = sd_round_cone(p, vec3<f32>(0.0, 0.32, 0.84), tipc + vec3<f32>(0.0, 0.03, -0.01), 0.032, 0.045);
        d = smin(d, bridge, 0.07);
        d = smin(d, length(p - tipc) - 0.058, 0.04);
        for (var i = 0; i < 2; i++) {
            let n = nostril(i);
            let sx = sign(n.x);
            let wing = length((p - (n.xyz + vec3<f32>(0.028 * sx, 0.03, -0.045))) * vec3<f32>(1.0, 1.2, 1.0)) - n.w * 1.25;
            d = smin(d, wing, 0.035);
        }
        for (var i = 0; i < 2; i++) {
            let n = nostril(i);
            let hole = sd_ellipsoid(p - (n.xyz + vec3<f32>(0.0, 0.012, -0.012)), vec3<f32>(n.w * 0.75, n.w * 0.5, n.w * 0.85));
            d = smax(d, -hole, 0.012);
        }
        // Sockets.
        for (var i = 0; i < 2; i++) {
            let sock = length(p - eye_centre(i)) - eye_radius(i) * 1.13;
            d = smax(d, -sock, 0.03);
        }
    }
    // The muzzle the mouth sits on: upper (maxilla) and lower.
    let mz = lip(UPPER_MID_GO()).z;
    let muz = sd_ellipsoid(p - vec3<f32>(0.0, -0.38, mz - 0.4), vec3<f32>(0.34 * jw, 0.26, 0.42));
    d = smin(d, muz, 0.14);
    // Chin and mandible (the lower jaw's own mass).
    let chin = sd_ellipsoid(p - vec3<f32>(0.0, -0.73, mz - 0.43), vec3<f32>(0.2 * jw + 0.04, 0.19, 0.36));
    d = smin(d, chin, 0.12);
    let lowm = sd_ellipsoid(p - vec3<f32>(0.0, -0.58, mz - 0.42), vec3<f32>(0.3 * jw, 0.17, 0.42));
    d = smin(d, lowm, 0.1);
    for (var s = 0; s < 2; s++) {
        let sx = f32(1 - 2 * s);
        let jl = sd_capsule(p, vec3<f32>(sx * 0.56 * jw, -0.3, -0.04), vec3<f32>(sx * 0.18, -0.76, mz - 0.38), 0.1);
        d = smin(d, jl, 0.14);
    }
    return d;
}

fn UPPER_MID_GO() -> i32 { return LIP_RING / 4; }

// ---- snakes ---------------------------------------------------------------

struct GoSnake {
    root: vec3<f32>,
    o: vec3<f32>,
    b1: vec3<f32>,
    b2: vec3<f32>,
    len: f32,
    amp: f32,
    ph: f32,
};

fn go_snake_count() -> i32 {
    return 8 + i32(round(6.0 * go_det()));
}

fn go_seg_count() -> i32 {
    return 4 + i32(round(2.0 * go_det()));
}

fn go_snake(i: i32, n: i32) -> GoSnake {
    let fi = f32(i);
    let h = hash11(fi * 3.17 + 1.3);
    let ang = mix(-2.25, 2.25, (fi + 0.5) / f32(n)) + (h - 0.5) * 0.18;
    let back = select(-0.02, -0.38, (i & 1) == 1);
    let d = normalize(vec3<f32>(sin(ang) * 0.9, cos(ang), back));
    var sn: GoSnake;
    sn.root = vec3<f32>(d.x * 0.66, d.y * 0.78 + 0.12, d.z * 0.75 - 0.1);
    let rise = 0.35 - 0.2 * abs(sin(ang));
    sn.o = normalize(vec3<f32>(sin(ang) * 1.2, cos(ang) + rise, 0.25 + 0.4 * back));
    sn.b1 = normalize(cross(sn.o, vec3<f32>(0.0, 0.0, 1.0)));
    sn.b2 = cross(sn.b1, sn.o);
    let strike = k_pulso();
    sn.len = (0.8 + 0.35 * h) * (1.0 + 0.4 * strike) * select(1.0, 0.85, (i & 1) == 1);
    sn.amp = (0.15 + 0.07 * hash11(fi + 7.7)) * (1.0 - 0.75 * strike);
    sn.ph = h * GO_TAU;
    return sn;
}

fn go_curve(sn: GoSnake, s: f32) -> vec3<f32> {
    let t = go_t() * 2.2;
    let env = smoothstep(0.0, 0.35, s);
    let lat = sin(7.0 * s - t + sn.ph) * sn.amp * env;
    let dep = cos(5.0 * s - 0.8 * t + 1.3 * sn.ph) * sn.amp * 0.7 * env;
    // The head rears up and turns toward the viewer; on a strike it lunges.
    let strike = k_pulso();
    let rear = s * s * (0.28 + 0.3 * strike);
    return sn.root + sn.o * sn.len * s + sn.b1 * lat + sn.b2 * dep + vec3<f32>(0.0, 0.0, rear);
}

fn go_snake_r(s: f32) -> f32 {
    return 0.085 - 0.042 * s;
}

// (distance, material, s along the body) for one snake.
fn go_snake_sdf(p: vec3<f32>, sn: GoSnake, segs: i32) -> vec3<f32> {
    var best = vec3<f32>(1e9, M_SNAKE, 0.0);
    var a = go_curve(sn, 0.0);
    let fs = f32(segs);
    for (var k = 0; k < GO_MAX_SEG; k++) {
        if (k >= segs) { break; }
        let s0 = f32(k) / fs;
        let s1 = f32(k + 1) / fs;
        let b = go_curve(sn, s1);
        let d = sd_round_cone(p, a, b, go_snake_r(s0), go_snake_r(s1));
        if (d < best.x) {
            let ab = b - a;
            let hh = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
            best = vec3<f32>(d, M_SNAKE, mix(s0, s1, hh));
        }
        a = b;
    }
    // The head: a flattened wedge on the end, looking where the body points.
    let tip = go_curve(sn, 1.0);
    let fwd = normalize(tip - go_curve(sn, 1.0 - 1.0 / fs));
    let side = normalize(cross(fwd, sn.b2) + vec3<f32>(1e-4, 0.0, 0.0));
    let up = cross(side, fwd);
    let q = p - (tip + fwd * 0.05);
    let lp = vec3<f32>(dot(q, side), dot(q, up), dot(q, fwd));
    // Jaws gape on a strike.
    let gape = 0.15 + 0.5 * k_pulso();
    var head = sd_ellipsoid(lp - vec3<f32>(0.0, 0.006, 0.0), vec3<f32>(0.072, 0.048, 0.12));
    let lower = sd_ellipsoid(rot_x(lp - vec3<f32>(0.0, -0.01, -0.05), -gape) - vec3<f32>(0.0, -0.012, 0.05), vec3<f32>(0.045, 0.022, 0.085));
    head = smin(head, lower, 0.015);
    // The mouth line.
    let mouth = max(abs(lp.y + 0.012 - 0.25 * gape * max(lp.z + 0.02, 0.0)) - 0.004 - 0.2 * gape * max(lp.z, 0.0), -lp.z - 0.01);
    head = smax(head, -mouth, 0.006);
    var mtl = M_SNAKE;
    if (-mouth > head - 0.004 && head < 0.01) { mtl = M_SNAKE_MOUTH; }
    if (head < best.x) {
        best = vec3<f32>(smin(best.x, head, 0.025), mtl, 1.0);
    } else {
        best.x = smin(best.x, head, 0.025);
    }
    // Eyes on the sides of the head.
    let ex = vec3<f32>(abs(lp.x), lp.y, lp.z) - vec3<f32>(0.052, 0.024, 0.035);
    let de = length(ex) - 0.02;
    if (de < best.x) { best = vec3<f32>(de, M_SNAKE_EYE, 1.0); }
    return best;
}

// The crown: (distance, material, s). Skips every snake whose bound is
// further than the closest thing so far.
fn go_snakes(p: vec3<f32>, far: f32) -> vec3<f32> {
    var best = vec3<f32>(far, M_SKIN, 0.0);
    // The crown lives outside the face; nothing to do deep in front or below.
    if (p.y < -1.2 && length(p.xz) < 0.9) { return best; }
    let n = go_snake_count();
    let segs = go_seg_count();
    for (var i = 0; i < GO_MAX_SNAKES; i++) {
        if (i >= n) { break; }
        let sn = go_snake(i, n);
        let mid = sn.root + sn.o * (0.5 * sn.len) + vec3<f32>(0.0, 0.0, 0.12);
        let bound = length(p - mid) - (0.5 * sn.len + sn.amp + 0.2);
        if (bound > best.x) { continue; }
        let r = go_snake_sdf(p, sn, segs);
        if (r.x < best.x) {
            best = vec3<f32>(r.x, r.y, r.z + f32(i) * 2.0);
        }
    }
    return best;
}

// ---- the face composition (the rig's parts, bounded) ----------------------

// The shared `sd_mouth_hole`, but with its corners rounded (gorgona only):
// a hard max() between the lip-line, side and back bounds reads as a dark
// box inside the mouth on her smooth, pale face more than it does on the
// other four. A smooth max rounds that away cheaply, no new geometry.
fn go_mouth_hole_round(p: vec3<f32>) -> f32 {
    let hw = 0.5 * abs(lip(0).x - lip(LIP_RING / 2).x);
    let xc = 0.5 * (lip(0).x + lip(LIP_RING / 2).x);
    let x = clamp(p.x, xc - hw, xc + hw);
    let yu = lip_line(x, true);
    let yl = lip_line(x, false);
    let dy = max(p.y - yu, yl - p.y);
    let dx = abs(p.x - xc) - hw;
    let dz = cavity_centre().z - p.z;
    return smax(smax(dy, dx, 0.04), dz, 0.04);
}

fn go_face(p: vec3<f32>, skull: f32) -> vec2<f32> {
    var res = vec2<f32>(skull, M_SKIN);
    // The mouth: everything the rig moves lives within this ball.
    let mc = mouth_centre();
    let dm = length(p - (mc + vec3<f32>(0.0, -0.05, -0.12))) - 0.78;
    if (dm > 0.02) {
        res.x = min(res.x, dm);
    } else {
        // The rig's slot between the lips runs straight back to the cavity
        // at full mouth width, which slices a groove into the sides of a
        // muzzle wider than the mouth (her full cheeks). Keep it to an oval
        // round the lips; the cavity does the rest.
        let hw = 0.5 * abs(lip(0).x - lip(LIP_RING / 2).x) + 0.04;
        let oval = (length(vec2<f32>((p.x - mc.x) / hw, (p.z - mc.z) / 0.3)) - 1.0) * min(hw, 0.3);
        // smin, not min: a hard min leaves the box-cornered mouth_hole wall
        // visible right where it meets the round cavity, reading as a dark
        // rectangle inside the mouth. Blending the two rounds that away.
        let hole = smin(sd_cavity(p), smax(go_mouth_hole_round(p), oval, 0.04), 0.07);
        let d = smax(skull, -hole, 0.02);
        res = vec2<f32>(d, M_SKIN);
        if (-hole > skull - 0.005 && d < 0.02) {
            res.y = M_CAVITY;
        }
        let dl = sd_lips(p);
        if (dl < res.x + 0.025) {
            res = vec2<f32>(smin(res.x, dl, 0.025), select(res.y, M_LIP, dl < res.x));
        }
        let dt = sd_teeth(p);
        if (dt < res.x) { res = vec2<f32>(dt, M_TEETH); }
        let dg = sd_tongue(p);
        if (dg < res.x) { res = vec2<f32>(dg, M_TONGUE); }
    }
    for (var i = 0; i < 2; i++) {
        let db = length(p - eye_centre(i)) - eye_radius(i) * 1.35;
        if (db > res.x) { continue; }
        let de = sd_eyeball(p, i);
        if (de < res.x) { res = vec2<f32>(de, M_EYE); }
        let dd = sd_lids(p, i);
        if (dd < res.x + 0.012) {
            res = vec2<f32>(smin(res.x, dd, 0.012), select(res.y, M_LID, dd < res.x));
        }
    }
    return res;
}

fn go_skull(p: vec3<f32>) -> f32 {
    let w = jaw_weight(p);
    // smoothstep gives exactly 0 and 1 outside the seam band, so each half is
    // evaluated only where it has weight, and never mixed with a sentinel.
    var d: f32;
    if (w <= 0.0) {
        d = go_skin(p, true);
    } else if (w >= 1.0) {
        d = go_skin(jaw_rest(p), true);
    } else {
        d = mix(go_skin(p, true), go_skin(jaw_rest(p), true), w);
    }
    d += rig_creases(p);
    // Neck, under the skull and behind the jaw.
    let neck = sd_round_cone(p, vec3<f32>(0.0, -0.45, -0.22), vec3<f32>(0.0, -2.1, -0.4), 0.46, 0.62);
    d = smin(d, neck, 0.22);
    return d;
}

fn map_head(p: vec3<f32>) -> vec2<f32> {
    let skull = go_skull(p);
    var res = go_face(p, skull);
    let sn = go_snakes(p, res.x);
    if (sn.x < res.x) {
        // Roots melt into the scalp.
        res = vec2<f32>(smin(sn.x, skull, 0.05), sn.y);
    }
    return res;
}

// The snake under a surface point (for shading): (index, s, ring angle).
fn go_snake_at(p: vec3<f32>) -> vec3<f32> {
    let r = go_snakes(p, 1e3);
    let id = floor(r.z * 0.5);
    let s = r.z - id * 2.0;
    return vec3<f32>(id, s, 0.0);
}

// ---- 2D patterns ----------------------------------------------------------

// Cellular: (distance to the nearest cell edge, cell hash).
fn go_cells(x: vec2<f32>) -> vec2<f32> {
    let ip = floor(x);
    let fp = fract(x);
    var d1 = 8.0;
    var d2 = 8.0;
    var id = 0.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let g = vec2<f32>(f32(i), f32(j));
            let h = hash21(ip + g);
            let o = vec2<f32>(h, hash21(ip + g + 17.3));
            let r = g + o - fp;
            let dd = dot(r, r);
            if (dd < d1) {
                d2 = d1;
                d1 = dd;
                id = h;
            } else if (dd < d2) {
                d2 = dd;
            }
        }
    }
    return vec2<f32>(sqrt(d2) - sqrt(d1), id);
}

// Serpent scales: overlapping rounded rows. (height 0..1, cell id).
fn go_scales(x: vec2<f32>) -> vec2<f32> {
    let row = floor(x.y);
    let xx = x.x + 0.5 * (row - 2.0 * floor(row * 0.5));
    let cell = vec2<f32>(fract(xx) - 0.5, fract(x.y));
    // Each scale is a disc hanging from its row, overlapping the next.
    let d = length(vec2<f32>(cell.x, (cell.y - 0.15) * 0.8));
    let hgt = clamp(1.0 - d * 1.9, 0.0, 1.0);
    return vec2<f32>(hgt, hash21(vec2<f32>(floor(xx), row)));
}

// ---- screen helpers -------------------------------------------------------

fn go_screen(uv: vec2<f32>) -> vec2<f32> {
    return (uv - vec2<f32>(0.5)) * vec2<f32>(2.0 * k_aspect(), -2.0);
}

fn go_proj(w: vec3<f32>) -> vec2<f32> {
    let rel = w - cam_origin();
    return rel.xy / max(-rel.z, 0.05) * cam_focal();
}

fn go_scale() -> f32 {
    return cam_focal() / cam_origin().z;
}

// ---- background: the mandala ----------------------------------------------

// An almond eye glyph in local coords (x along, y across), returns
// (coverage, iris, pupil).
fn go_glyph_eye(q: vec2<f32>, open: f32) -> vec3<f32> {
    let h = 0.42 * open * max(1.0 - q.x * q.x / 0.36, 0.0);
    let lid = smoothstep(0.02, -0.01, abs(q.y) - h);
    let rr = length(q);
    let iris = smoothstep(0.2, 0.17, rr) * lid;
    let pupil = smoothstep(0.035, 0.015, abs(q.x) * (1.0 + 0.0 * q.y)) * smoothstep(0.17, 0.12, abs(q.y)) * iris;
    let edge = smoothstep(0.03, 0.0, abs(abs(q.y) - h)) * step(abs(q.x), 0.6);
    return vec3<f32>(max(lid * 0.35, edge), iris, pupil);
}

fn go_mandala(sp: vec2<f32>) -> vec3<f32> {
    let trip = k_trip();
    let t = go_t();
    let c = go_proj(vec3<f32>(0.0, 0.15, -0.3));
    var q = (sp - c) / go_scale();
    // Domain warp, more of it with trip.
    q += trip * 0.12 * vec2<f32>(sin(q.y * 2.3 + t * 0.7), cos(q.x * 2.1 - t * 0.6));
    let r = max(length(q), 1e-3);
    var a = atan2(q.x, q.y);
    // Mirror-symmetric kaleidoscope: 12 wedges, each folded.
    let nseg = 12.0;
    let seg = GO_TAU / nseg;
    let aw = a + t * 0.04 * (0.3 + trip);
    let wi = floor(aw / seg + 0.5);
    let af = abs(aw - wi * seg);
    // Log-polar rings: each ring is the last one shrunk, so the pattern
    // recurses into the centre; trip makes it zoom forever.
    let ratio = 1.65;
    let lr = log(r) / log(ratio) - t * 0.18 * (0.15 + trip);
    let band = floor(lr);
    let v = fract(lr);
    // Local coords in the wedge, roughly square.
    let lx = af / seg * 2.0;                // 0 on the mirror axis, 1 at the edge
    let ly = v - 0.5;
    var col = vec3<f32>(0.0);
    // Eyes on the axis of every wedge.
    let blinkph = fract(t * 0.07 + hash11(wi * 1.7 + band * 3.1));
    let open = 1.0 - smoothstep(0.0, 0.03, blinkph) * smoothstep(0.08, 0.05, blinkph);
    let ge = go_glyph_eye(vec2<f32>(ly * 1.4, lx * 0.85), open);
    let ct = fract(band * 0.23 + wi * 0.05 + t * 0.03);
    col += go_neon(ct) * ge.x * 0.6;
    col += go_neon(ct + 0.33) * ge.y * (0.6 + 0.6 * k_ojos()) * (1.0 - ge.z);
    // Serpents coil along the wedge edges, braided across the mirror.
    let sw = sin(ly * GO_TAU * 1.0 + band * 1.3 + t * 1.2) * 0.22 + 0.72;
    let body = smoothstep(0.09, 0.03, abs(lx - sw));
    let stripes = 0.5 + 0.5 * sin((ly + lx * 0.3) * 40.0 - t * 3.0);
    col += go_neon(ct + 0.6) * body * (0.35 + 0.45 * stripes);
    // Sacred-geometry ring lines between bands.
    let ringl = smoothstep(0.04, 0.0, abs(v - 0.02)) + smoothstep(0.012, 0.0, abs(lx - 1.0) - 0.0);
    col += go_neon(ct + 0.15) * ringl * 0.35;
    // Near the head the mandala is quiet so the face reads; at trip 0 only
    // a ring of it shows, at trip 1 all of it, all the way in.
    let reach = mix(1.5, 0.0, trip * trip);
    let fade = smoothstep(reach, reach + 0.5, r) * smoothstep(3.6 + 4.0 * trip, 1.6, r);
    return col * fade * (0.08 + 0.92 * trip * trip);
}

fn go_background(sp: vec2<f32>, uv: vec2<f32>) -> vec3<f32> {
    let t = go_t();
    let trip = k_trip();
    let aura = k_aura();
    let c = go_proj(vec3<f32>(0.0, 0.1, 0.0));
    let q = (sp - c) / go_scale();
    let r = length(q * vec2<f32>(1.0, 0.85));
    // Base from fondo, darker toward the edges.
    var col = go_c(u.background, clamp(uv.y, 0.0, 1.0)) * (0.3 + 0.45 * smoothstep(3.5, 0.5, r));
    // Domain-warped energy around the head.
    let det = 2 + i32(3.0 * go_det());
    let wq = q * 1.3 + vec2<f32>(0.0, -t * 0.25);
    let w1 = fbm(wq + vec2<f32>(t * 0.1, 0.0), det);
    let w2 = fbm(wq * 1.7 + vec2<f32>(w1 * 2.0, t * 0.13), det);
    let field = smoothstep(0.35, 0.9, w2) * smoothstep(3.0, 0.9, r);
    col += go_neon(0.15 + 0.4 * w1) * field * aura * 0.45;
    // Halo hugging the silhouette.
    let halo = exp(-max(r - 0.95, 0.0) * 2.6) * smoothstep(0.75, 1.0, r);
    col += go_neon(0.55 + 0.1 * sin(atan2(q.x, q.y) * 3.0 + t)) * halo * aura * (0.35 + 0.65 * k_brillo());
    // The mandala.
    col += go_mandala(sp) * (0.35 + 0.65 * aura);
    // Sound rings out of the mouth, as wide as the mouth is open.
    let gap = mouth_gap();
    let mpos = go_proj(head_matrix() * mouth_centre());
    let md = length(sp - mpos) / go_scale();
    let ring = fract(md * 1.4 - k_time() * (0.8 + 1.5 * k_flujo()));
    let rings = smoothstep(0.1, 0.0, abs(ring - 0.5) - 0.02) * smoothstep(3.2, 0.4, md);
    col += go_neon(0.33 + 0.2 * ring) * rings * clamp(gap * 7.0, 0.0, 1.0) * (0.2 + 0.5 * aura);
    // Pulse: a stone-ripple shockwave rolling out from the head.
    let pul = k_pulso();
    let wave = (1.0 - pul) * 2.5 + 0.95;
    let shock = exp(-abs(r - wave) * 9.0) * pul;
    col += go_neon(0.9) * 1.6 * shock;
    col += go_c(u.background, 0.9) * pul * 0.15;
    return col;
}

// ---- eyes -----------------------------------------------------------------

// The iris colour and emission at head-local point ph on eyeball i.
fn go_iris(ph: vec3<f32>, i: i32) -> vec4<f32> {
    let iu = iris_uv(ph, i);
    let r = iu.x;
    let a = iu.y;
    let t = go_t();
    let ir = 0.62;
    // Vertical slit pupil, narrowing with glow; in the gaze frame.
    let nq = pupil_dir(ph, i);
    let slitw = mix(0.13, 0.05, k_ojos());
    let pupil = smoothstep(slitw, slitw - 0.03, abs(nq.x) * (1.0 + 2.5 * nq.y * nq.y)) * step(r, ir * 0.9);
    // Op-art: rings that run inward, spokes that turn, a moiré where they
    // cross.
    let rings = 0.5 + 0.5 * sin(r * 70.0 - t * 4.0);
    let spokes = 0.5 + 0.5 * sin(a * 18.0 + r * 12.0 + t * 1.5);
    let op = mix(rings, rings * spokes, 0.5 + 0.4 * k_trip());
    let irism = smoothstep(ir, ir - 0.04, r);
    let limbus = smoothstep(0.1, 0.0, abs(r - ir + 0.03)) * irism;
    let base = mix(go_neon(0.35 + 0.25 * r), go_neon(0.05 + r * 0.4), op);
    var col = base * (0.15 + 0.85 * op) * eye_iris_tint(r / ir);
    col = mix(col, vec3<f32>(0.0), limbus * 0.7);
    col = mix(col, eye_pupil_color(abs(nq.x) / slitw) * 0.4, pupil);
    let glow = (0.4 + 3.5 * k_ojos() * k_ojos()) * (0.6 + 0.5 * k_brillo()) + 3.0 * k_pulso();
    return vec4<f32>(col, irism) * vec4<f32>(1.0, 1.0, 1.0, 1.0) + vec4<f32>(0.0, 0.0, 0.0, 0.0) * glow;
}

fn go_eye_glow() -> f32 {
    return (0.15 + 1.5 * k_ojos() * k_ojos()) * (0.6 + 0.5 * k_brillo()) + 2.0 * k_pulso();
}

// Light beams and halos thrown by the eyes, in screen space.
fn go_beams(sp: vec2<f32>) -> vec3<f32> {
    var col = vec3<f32>(0.0);
    let o = k_ojos();
    let strength = o * o * (0.5 + k_brillo()) + 1.5 * k_pulso();
    if (strength < 0.01) { return col; }
    let t = go_t();
    for (var i = 0; i < 2; i++) {
        let lead = head_matrix() * (eye_centre(i) + vec3<f32>(0.0, 0.0, eye_radius(i) * 0.9));
        let e = go_proj(lead);
        let d = (sp - e) / go_scale();
        let open = clamp(eye_open(i), 0.0, 1.0);
        let r = length(d);
        let a = atan2(d.y, d.x);
        let star = pow(0.5 + 0.5 * cos(a * 8.0 + t * 0.6), 18.0) * exp(-r * 6.0);
        let streak = exp(-abs(d.y) * 70.0) * exp(-abs(d.x) * 3.5) * 0.6;
        let core = exp(-r * 22.0) * 2.0;
        let halo = smoothstep(0.03, 0.0, abs(r - 0.28 - 0.04 * sin(t * 2.0))) * 0.5;
        col += go_neon(0.33 + 0.1 * f32(i)) * (star * 1.2 + streak + core + halo) * strength * open;
    }
    return col;
}

// ---- marching -------------------------------------------------------------

// The rig's march, except that a ray which runs out of steps while grazing a
// surface (the mouth slab seen edge-on, a snake's flank) counts as a hit
// instead of punching a hole to the background.
fn go_march(ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    let b = dot(ro, rd);
    let c = dot(ro, ro) - 2.2 * 2.2;
    let h = b * b - c;
    if (h < 0.0) {
        return vec3<f32>(-1.0, 0.0, 0.0);
    }
    var t = max(-b - sqrt(h), 0.0);
    let tmax = -b + sqrt(h);
    let n = rig_steps();
    var last = vec2<f32>(1e3, 0.0);
    for (var i = 0; i < n; i++) {
        let r = rig_map(ro + rd * t);
        last = r;
        if (r.x < 0.0008 * t) {
            return vec3<f32>(t, r.y, f32(i) / f32(n));
        }
        t += r.x * 0.85;
        if (t > tmax) {
            return vec3<f32>(-1.0, 0.0, 1.0);
        }
    }
    if (last.x < 0.02) {
        return vec3<f32>(t, last.y, 1.0);
    }
    return vec3<f32>(-1.0, 0.0, 1.0);
}

// ---- shading --------------------------------------------------------------

struct GoLit {
    key: vec3<f32>,
    rim: vec3<f32>,
};

fn go_fresnel(n: vec3<f32>, v: vec3<f32>, p: f32) -> f32 {
    return pow(1.0 - max(dot(n, v), 0.0), p);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    surf_uv = in.uv;
    let cam = rig_camera(in.uv);
    let ro = cam[0];
    let rd = cam[1];
    let sp = go_screen(in.uv);
    let trip = k_trip();
    let shine = k_brillo();
    let pul = k_pulso();
    let t = go_t();
    let det = go_det();

    var col: vec3<f32>;
    // Chromatic fringe on the background with trip: three cheap 2D lookups.
    if (trip > 0.3) {
        let cc = go_proj(vec3<f32>(0.0, 0.1, 0.0));
        let off = (sp - cc) * 0.012 * (trip - 0.3);
        let r = go_background(sp + off, in.uv).r;
        let g = go_background(sp, in.uv).g;
        let b = go_background(sp - off, in.uv).b;
        col = vec3<f32>(r, g, b);
    } else {
        col = go_background(sp, in.uv);
    }

    let hit = go_march(ro, rd);
    if (hit.x > 0.0) {
        let p = ro + rd * hit.x;
        let n = rig_normal(p);
        let v = -rd;
        let hm = head_matrix();
        let ph = to_head(p);
        let nh = transpose(hm) * n;
        let mtl = hit.y;
        let ao = rig_ao(p, n);

        // Lights: a warm-white key up left, two coloured rims, a low fill.
        let kl = normalize(vec3<f32>(-0.45, 0.65, 0.75));
        let rl1 = normalize(vec3<f32>(0.9, 0.3, -0.5));
        let rl2 = normalize(vec3<f32>(-0.9, -0.1, -0.4));
        let fl = normalize(vec3<f32>(0.3, -0.7, 0.6));
        var shadow = 1.0;
        if (det > 0.45) {
            shadow = 0.25 + 0.75 * rig_shadow(p + n * 0.01, kl, 10.0);
        }
        let dif = max(dot(n, kl), 0.0) * shadow;
        let fil = max(dot(n, fl), 0.0) * 0.25;
        let fres = go_fresnel(n, v, 3.0);
        let rim1 = pow(max(dot(n, rl1), 0.0), 2.0) * (0.35 + fres);
        let rim2 = pow(max(dot(n, rl2), 0.0), 2.0) * (0.35 + fres);
        let rimc = (go_neon(0.0) * rim1 + go_neon(0.33) * rim2) * (0.35 + 1.2 * shine);
        let hv = normalize(kl + v);
        let amb = 0.07 + 0.05 * nh.y;

        var base = vec3<f32>(0.5);
        var emit = vec3<f32>(0.0);
        var spec_k = 0.3;
        var spec_p = 32.0;
        var rim_k = 1.0;

        // The ripple a hit sends over the stone, from the mouth outward.
        let mc = mouth_centre();
        let from_mouth = length(ph - mc);

        if (mtl == M_SKIN) {
            // Marble-to-jade: soft veining in the stone.
            let oct = 2 + i32(3.0 * det);
            let m = fbm(ph.xy * 2.2 + vec2<f32>(ph.z * 1.3, 0.0) + vec2<f32>(0.0, t * 0.02), oct);
            let vein = abs(sin(ph.x * 3.0 + ph.y * 5.0 + ph.z * 2.0 + m * 7.0));
            let marble = smoothstep(0.0, 0.25, vein);
            base = go_c(u.skin, 0.36 + 0.3 * m) * mix(1.15, 0.85, marble);
            base = mix(base, go_c(u.skin, 0.92), (1.0 - marble) * 0.28);
            spec_k = 0.55;
            spec_p = 60.0;

            // Serpent scales: temples, jaw, neck, the back of the skull.
            let ax = abs(ph.x);
            let temple = smoothstep(0.38, 0.58, ax) * smoothstep(-0.25, 0.05, ph.y) * smoothstep(0.75, 0.4, ph.y);
            let jawl = smoothstep(0.15, 0.4, ax) * smoothstep(-0.38, -0.62, ph.y);
            let neckm = smoothstep(-0.8, -1.0, ph.y);
            let backm = smoothstep(0.0, -0.45, ph.z) + smoothstep(0.5, 0.75, ph.y);
            let smask = clamp(max(max(temple, jawl), max(neckm, backm)), 0.0, 1.0) * clamp(s_scales() * 1.6, 0.0, 1.0);
            if (smask > 0.01) {
                // Project on the side for temples and on the front otherwise.
                let side = abs(nh.x);
                let suv = mix(vec2<f32>(ph.x * 1.2, ph.y), vec2<f32>(ph.z, ph.y), side) * 26.0;
                let sc = go_scales(suv);
                let irid = go_c(u.skin, fract(0.25 + 0.7 * fres + 0.15 * sc.y));
                let ir2 = go_neon(fract(0.5 * fres + sc.y * 0.2 + ph.y * 0.3));
                let scol = mix(irid, ir2, 0.25 + 0.3 * trip) * (0.35 + 0.75 * sc.x);
                base = mix(base, scol, smask);
                spec_k = mix(spec_k, 0.9, smask);
                // Scale edges flare on a hit.
                emit += go_neon(0.6 + sc.y * 0.2) * smask * smoothstep(0.15, 0.0, sc.x) * (0.1 * shine + 0.9 * pul);
            }

            // Brows: fine dark strokes along the rig's brows, glowing at the
            // edge.
            for (var s = 0; s < 2; s++) {
                let b0 = brow(s, 0);
                let b1 = brow(s, 1);
                let b2 = brow(s, 2);
                let qb = vec3<f32>(ph.x, ph.y, 0.0);
                let db = min(sd_round_cone(qb, vec3<f32>(b0.xy, 0.0), vec3<f32>(b1.xy, 0.0), 0.032, 0.026),
                             sd_round_cone(qb, vec3<f32>(b1.xy, 0.0), vec3<f32>(b2.xy, 0.0), 0.026, 0.008));
                let bm = smoothstep(0.01, -0.01, db) * step(0.5, ph.z);
                base = mix(base, go_c(u.skin, 0.05), bm * 0.9);
                emit += go_neon(0.0) * smoothstep(0.018, 0.0, abs(db)) * step(0.5, ph.z) * 0.35 * shine;
            }
            // Furrow creases between the brows: the raw control, not a
            // reading of the moved brow landmarks.
            let furrow = c_brow_furrow();
            let crease = smoothstep(0.012, 0.0, abs(abs(ph.x) - 0.045 + 0.4 * (ph.y - 0.3) * 0.0)) * smoothstep(0.18, 0.28, ph.y) * smoothstep(0.5, 0.38, ph.y);
            base *= 1.0 - 0.6 * crease * furrow;

            // Eye make-up: a dark wing past the outer corner, shadow on the
            // orbit.
            for (var i = 0; i < 2; i++) {
                let ec = eye_centre(i);
                let sx = sign(ec.x);
                let lq = vec2<f32>((ph.x - ec.x) * sx, ph.y - ec.y);
                let menace = c_squint();
                let shadowm = smoothstep(0.32, 0.14, length(lq * vec2<f32>(0.8, 1.2))) * step(0.0, lq.y + 0.04);
                base = mix(base, base * 0.35 + go_neon(0.0) * 0.12, shadowm * (0.6 + 0.35 * menace));
                // A tense crease under the eye when she narrows them.
                let und = smoothstep(0.012, 0.0, abs(length(lq * vec2<f32>(0.8, 1.6)) - 0.24)) * step(lq.y, -0.05) * step(0.6, ph.z);
                emit += go_neon(0.0) * und * menace * (0.3 + shine);
                let wing = sd_capsule(vec3<f32>(lq, 0.0), vec3<f32>(0.12, 0.02, 0.0), vec3<f32>(0.26, 0.09, 0.0), 0.004);
                let wingm = smoothstep(0.012, 0.0, wing) * step(0.6, ph.z);
                base = mix(base, vec3<f32>(0.0), wingm);
                emit += go_neon(0.33) * wingm * 0.4 * shine;
            }

            // Cracks that open into neon veins.
            let cv = go_cells(ph.xy * 3.2 + vec2<f32>(ph.z * 1.7, ph.z * 0.6));
            let calm = smoothstep(0.18, 0.42, from_mouth);
            let open = (0.04 + 0.96 * trip * trip) * (0.2 + 0.8 * shine) + pul;
            let width = 0.008 + 0.05 * open;
            let crack = smoothstep(width, 0.0, cv.x) * calm;
            let flowv = 0.5 + 0.5 * sin(from_mouth * 18.0 - k_time() * (1.0 + 4.0 * k_flujo()));
            emit += go_neon(0.66 + 0.3 * cv.y + 0.1 * flowv) * crack * open * (1.2 + 1.5 * flowv);
            base *= 1.0 - 0.7 * crack;

            // Stone ripple on a hit.
            let rip = sin(from_mouth * 30.0 - (1.0 - pul) * 25.0) * 0.5 + 0.5;
            emit += go_neon(0.9) * rip * pul * 0.3 * smoothstep(1.6, 0.2, from_mouth);
        } else if (mtl == M_LIP) {
            // Same marbled stone as the face, just darker toward the inner
            // fold of the lip — no separate pink liner, no berry flush.
            let lc = mouth_centre();
            let dy = max(abs(ph.y - lc.y) - 0.5 * mouth_gap(), 0.0);
            let m = fbm(ph.xy * 2.2 + vec2<f32>(ph.z * 1.3, 0.0) + vec2<f32>(0.0, t * 0.02), 2 + i32(3.0 * det));
            let inner = smoothstep(0.03, -0.02, dy - 0.02);
            base = go_c(u.skin, mix(0.36 + 0.3 * m, 0.08, inner));
            let lines = 0.9 + 0.1 * sin(ph.x * 260.0);
            base *= lines;
            spec_k = 0.4;
            spec_p = 60.0;
            rim_k = 0.4;
        } else if (mtl == M_TEETH) {
            base = go_c(u.mouth, 1.0) * 0.95;
            spec_k = 1.2;
            spec_p = 70.0;
        } else if (mtl == M_TONGUE) {
            // A subtle dark-skin tongue, not a warm red: the same stone
            // palette as the cavity, just a shade lighter so its own shape
            // still reads.
            let pap = noise2(ph.xz * 90.0);
            base = go_c(u.skin, 0.16) * (0.9 + 0.15 * pap);
            spec_k = 0.9;
            spec_p = 90.0;
        } else if (mtl == M_CAVITY) {
            // Dark interior in the face's own skin hue (hue shift included), no
            // mouth pink and no warm emissive accent.
            base = max(hue_rotate(mouth_cavity_base(), k_matiz()), vec3<f32>(0.0));
            spec_k = 0.15;
            rim_k = 0.0;
        } else if (mtl == M_EYE) {
            var i = 0;
            if (ph.x < 0.0) { i = 1; }
            let ir = go_iris(ph, i);
            // Sclera: pale marble, a little green.
            let sclera = go_c(u.skin, 0.75) * 0.45;
            base = mix(sclera, ir.rgb * 0.4, ir.a);
            let menace = c_squint();
            emit += ir.rgb * ir.a * go_eye_glow() * (1.0 + 1.5 * menace);
            emit += eye_catchlight(ph, i);
            spec_k = 2.0;
            spec_p = 200.0;
        } else if (mtl == M_LID) {
            let oct = 2;
            let m = fbm(ph.xy * 2.2, oct);
            base = go_c(u.skin, 0.48 + 0.3 * m) * 0.55 + go_neon(0.0) * 0.05;
            // Lash line: dark along the lid edge.
            var i = 0;
            if (ph.x < 0.0) { i = 1; }
            let q = ph - eye_centre(i);
            let phi = atan2(q.y, max(q.z, 1e-4));
            let lids = eye_lids(i);
            let edge = min(abs(phi - lids.x), abs(phi - lids.y));
            let lash = smoothstep(0.22, 0.04, edge);
            base = mix(base, vec3<f32>(0.01), lash);
            emit += go_neon(0.33) * lash * 0.15 * shine;
            spec_k = 0.5;
        } else if (mtl == M_SNAKE || mtl == M_SNAKE_MOUTH) {
            let info = go_snake_at(p_head_safe(ph));
            let id = info.x;
            let s = info.y;
            // Iridescent snake skin, banded, belly paler.
            let sc = go_scales(vec2<f32>(atan2(nh.y, nh.x) * 3.0, s * 40.0));
            let irid = go_c(u.skin, fract(0.2 + 0.8 * fres + 0.1 * sc.y + 0.13 * id));
            let ang2 = atan2(nh.y, nh.x);
            let band = smoothstep(0.55, 0.75, abs(sin(s * 14.0 - t * 1.5 + id) + 0.6 * sin(ang2 * 2.0)));
            let neon = go_neon(fract(id * 0.137 + s * 0.4 + t * 0.05));
            base = mix(irid, neon, 0.3 + 0.35 * trip) * (0.3 + 0.8 * sc.x);
            base = mix(base, base * 0.25, band * 0.6);
            emit += neon * band * (0.15 + 0.6 * shine) * (0.4 + sc.x);
            emit += neon * pul * 0.6;
            spec_k = 0.6;
            spec_p = 70.0;
            if (mtl == M_SNAKE_MOUTH) {
                base = go_c(u.mouth, 0.5);
                emit = go_neon(0.66) * (0.5 + 2.0 * pul);
            }
        } else if (mtl == M_SNAKE_EYE) {
            base = vec3<f32>(0.02);
            emit = go_neon(0.33 + 0.2 * sin(t)) * (0.6 + 2.5 * k_ojos() + 4.0 * pul);
            spec_k = 1.5;
            spec_p = 150.0;
        }

        if (mtl == M_SKIN || mtl == M_LID) { base = surf_skin_mix(base, ph, nh); }
        if (mtl == M_EYE) { base = surf_eyes_mix(base, ph, nh); }

        let spec = pow(max(dot(n, hv), 0.0), spec_p) * spec_k * shadow;
        var c = base * (amb + 0.95 * dif + fil) * ao;
        c += vec3<f32>(1.0, 0.97, 0.92) * spec * (0.4 + 0.6 * ao);
        // Iridescent sheen on the stone: a thin-film tint at grazing angles.
        let inside = select(1.0, 0.0, mtl == M_CAVITY || mtl == M_TONGUE || mtl == M_TEETH);
        c += go_neon(fract(0.2 + fres * 1.5 + trip * 0.2)) * fres * 0.15 * shine * ao * inside;
        if (mtl == M_CAVITY) { c *= 0.5 + 0.5 * clamp(1.0 - (mc.z - ph.z) * 3.0, 0.0, 1.0); }
        c += rimc * rim_k * ao * mix(0.25, 1.0, inside);
        c += emit;
        // Pulse flash over everything.
        c += go_neon(0.9) * pul * 0.08;
        col = c;
    }

    col += go_beams(sp);

    // Vignette and grain, then the film curve.
    let vig = 1.0 - 0.35 * pow(length(in.uv - vec2<f32>(0.5)) * 1.3, 2.5);
    col *= vig;
    col += (hash21(in.uv * 1000.0 + fract(k_time())) - 0.5) * 0.015;
    return vec4<f32>(tonemap(col), 1.0);
}

fn p_head_safe(p: vec3<f32>) -> vec3<f32> {
    return p;
}
