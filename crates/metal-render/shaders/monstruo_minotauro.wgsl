// monstruo — MINOTAURO. The bull-man in the middle of the labyrinth.
//
// A broad, flat bovine muzzle with flaring nostrils that snort steam, a
// swinging nose ring, a massive brow over deep glowing eyes, wide horns that
// curve forward with op-art banding, heavy jowls carried on the jaw, a thick
// neck. Velvet fur in the shading (never in the SDF), neon labyrinth
// markings that crawl with `flujo` and melt with `trip`. Behind him the
// Cretan maze: concentric walls in log-polar perspective, an infinite zoom
// that turns hypnotic as `trip` rises; `pulso` stomps a shockwave through it.
//
// Helpers are `mi_`; materials from 10 up. The mouth, eyes and lids are the
// rig's (`rig_face`); the jaw is modelled at rest and evaluated at
// `jaw_rest(p)`.

const MI_HORN: f32 = 10.0;
const MI_RING: f32 = 11.0;
const MI_NOSE: f32 = 12.0;
const MI_TAU: f32 = 6.2831853;

// ---- small helpers --------------------------------------------------------

fn mi_round_box(p: vec3<f32>, b: vec3<f32>, r: f32) -> f32 {
    let q = abs(p) - b;
    return length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0) - r;
}

fn mi_torus(p: vec3<f32>, R: f32, r: f32) -> f32 {
    // Ring in the xy plane (axis z).
    let q = vec2<f32>(length(p.xy) - R, p.z);
    return length(q) - r;
}

fn mi_noise(p: vec2<f32>) -> f32 {
    return quintic_noise(p);
}

// Rest-frame mouth line: from the jaw pivot forward to the lip corners' rest
// height. y of the seam at depth z.
fn mi_seam_y(z: f32) -> f32 {
    let s = s_snout();
    let mouth_y = -0.45 - 0.05 * s;
    let mouth_z = 0.88 + 0.45 * s;
    let pv = jaw_pivot();
    let k = clamp((z - pv.z) / max(mouth_z - pv.z, 1e-3), 0.0, 1.0);
    return mix(pv.y, mouth_y + 0.005, k);
}

// The nostril, as the skull sees it: the rig's ball spread out across the
// broad bovine nose pad. xyz centre, w radius (grows with nostril_flare).
// How far the nose pad rides up and forward off the mouth: the jaw's
// opening and the upper lip's lift push the muzzle with them, so an open
// mouth never carves the pad (or hangs the ring) over the mouth hole.
fn mi_lift() -> vec3<f32> {
    let open = clamp(jaw_angle() / 0.38, 0.0, 1.0);
    let lip_up = max(lip(LIP_RING / 4).y - (rest_mouth().y + 0.05), 0.0);
    let up = 0.13 * open + 1.1 * lip_up;
    return vec3<f32>(0.0, up, 0.35 * up);
}

fn mi_nostril(i: i32) -> vec4<f32> {
    let n = nostril(i) + vec4<f32>(mi_lift(), 0.0);
    let flare = c_nostril_flare();
    let sx = select(-1.0, 1.0, n.x > 0.0);
    let x = n.x * 2.1 + sx * 0.02 * flare;
    return vec4<f32>(x, n.y + 0.01, n.z - 0.045, n.w);
}

fn mi_flare() -> f32 {
    return c_nostril_flare();
}

// ---- horns ----------------------------------------------------------------

// One horn (the creature's left, +x), as a chain of round cones. Returns
// (distance, t along the horn 0 root … 1 tip).
fn mi_horn(p: vec3<f32>) -> vec2<f32> {
    let h = s_horns();
    var pts = array<vec4<f32>, 6>(
        vec4<f32>(0.45, 0.50, -0.02, 0.17),
        vec4<f32>(0.92, 0.58, 0.02, 0.14),
        vec4<f32>(1.38, 0.60, 0.18, 0.11),
        vec4<f32>(1.70, 0.68, 0.45, 0.08),
        vec4<f32>(1.80, 0.86, 0.72, 0.05),
        vec4<f32>(1.68, 1.02, 0.88, 0.014),
    );
    let root = pts[0].xyz;
    var best = vec2<f32>(1e9, 0.0);
    for (var i = 0; i < 5; i++) {
        let a = mix(root, pts[i].xyz, h);
        let b = mix(root, pts[i + 1].xyz, h);
        let ra = pts[i].w;
        let rb = pts[i + 1].w;
        let ba = b - a;
        let tt = clamp(dot(p - a, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
        let d = sd_round_cone(p, a, b, ra, rb);
        let gt = (f32(i) + tt) / 5.0;
        if (d < best.x) {
            best = vec2<f32>(d, gt);
        }
    }
    // Annular growth ridges, strongest at the root, fading toward the tip.
    let ridge = sin(best.y * 40.0) * 0.004 * smoothstep(0.35, 0.0, best.y);
    best.x = best.x - ridge;
    return best;
}

// Both horns via the mirror, behind a cheap bound.
fn mi_horns(p: vec3<f32>) -> vec2<f32> {
    let q = vec3<f32>(abs(p.x), p.y, p.z);
    let bound = sd_capsule(q, vec3<f32>(0.5, 0.55, 0.0), vec3<f32>(1.7, 0.8, 0.6), 0.5);
    if (bound > 0.15) {
        return vec2<f32>(bound, 0.0);
    }
    return mi_horn(q);
}

// ---- the nose ring --------------------------------------------------------

// A heavy ring through the septum. It swings forward on a pendulum: a slow
// idle sway, kicked out by the mouth opening and by the hit.
fn mi_ring_angle() -> f32 {
    let t = k_time();
    let idle = 0.22 * sin(t * 2.3) + 0.08 * sin(t * 5.1 + 1.0);
    // Open, the ring swings up onto the pad instead of hanging in the mouth.
    let open = clamp(jaw_angle() / 0.38, 0.0, 1.0);
    return idle + 1.6 * mouth_gap() + 1.3 * open + 0.5 * k_pulso();
}

// Roughly at the nose pad's own front surface (z ≈ 1.06), not out at the
// nostril rig points (z ≈ 1.2, a good way past it) — close enough to the
// skin that `mi_septum` below can bridge over its top arc.
fn mi_ring_pivot() -> vec3<f32> {
    return vec3<f32>(0.0, -0.35, 1.17) + mi_lift();
}

fn mi_ring_local(p: vec3<f32>) -> vec3<f32> {
    let piv = mi_ring_pivot();
    let ang = mi_ring_angle();
    let sway = 0.12 * sin(k_time() * 1.7);
    var q = rot_x(p - piv, ang);
    // A little side-to-side twist.
    let r = rot2(sway) * q.xz;
    q = vec3<f32>(r.x, q.y, r.y);
    return q;
}

fn mi_ring(p: vec3<f32>) -> f32 {
    return mi_torus(mi_ring_local(p), 0.085, 0.017);
}

// A tab of skin bridging over the ring's upper arc, so the ring reads as
// pierced through the septum (top hidden, bottom hanging free) instead of
// as one whole loop floating in front of the face. Deliberately its own
// small shape rather than relying on the pad's general volume lining up
// with the ring everywhere (it doesn't: shapes this different in scale
// either swallow the whole ring or miss it, as the pad did).
fn mi_septum(p: vec3<f32>) -> f32 {
    let piv = mi_ring_pivot();
    let q = p - piv - vec3<f32>(0.0, 0.08, 0.0);
    return sd_ellipsoid(q, vec3<f32>(0.12, 0.07, 0.04));
}

// ---- the skull ------------------------------------------------------------

// Upper head: cranium, forehead slab, nasal bridge, the broad muzzle with its
// nostrils, brow ridge, cheeks, ears, the poll, the neck. Evaluated at p.
fn mi_upper(p: vec3<f32>) -> f32 {
    let w = s_jaw_width();
    let ridge = s_brow_ridge();
    // Cranium and the long flat face.
    var d = sd_ellipsoid(p - vec3<f32>(0.0, 0.22, -0.08), vec3<f32>(0.80, 0.74, 0.78));
    d = smin(d, sd_ellipsoid(p - vec3<f32>(0.0, 0.08, 0.40), vec3<f32>(0.64, 0.56, 0.50)), 0.12);
    // Nasal bridge, flat-topped, running down to the muzzle.
    d = smin(d, sd_ellipsoid(p - vec3<f32>(0.0, -0.05, 0.74), vec3<f32>(0.30 * w, 0.42, 0.34)), 0.12);
    // The muzzle: a broad flat nose pad.
    let mz = p - vec3<f32>(0.0, -0.27, 0.93) - mi_lift();
    let muzzle = mi_round_box(mz, vec3<f32>(0.20 * w, 0.075, 0.13), 0.115);
    d = smin(d, muzzle, 0.10);
    // Nostrils: an alar rim that bulges with the flare, then the hole.
    for (var i = 0; i < 2; i++) {
        let n = mi_nostril(i);
        let sx = select(-1.0, 1.0, n.x > 0.0);
        var q = p - n.xyz;
        // Comma-shaped: tilted, the outer end higher.
        let r2 = rot2(-0.45 * sx) * q.xy;
        q = vec3<f32>(r2.x, r2.y, q.z);
        let rim = sd_ellipsoid(q, vec3<f32>(n.w * 2.3, n.w * 1.6, n.w * 1.7));
        d = smin(d, rim, 0.035);
        let hole = sd_ellipsoid(q - vec3<f32>(0.0, 0.0, 0.01), vec3<f32>(n.w * 1.5, n.w * 0.85, n.w * 2.6));
        d = smax(d, -hole, 0.018);
    }
    // Cheeks, from the rig (cheek_puff grows them).
    for (var i = 0; i < 2; i++) {
        let c = cheek(i);
        d = smin(d, length(p - c.xyz - vec3<f32>(-sign(c.x) * 0.08, 0.04, -0.1)) - c.w * 0.75, 0.22);
    }
    // The brow: a massive ridge along the rig's brow points.
    for (var s = 0; s < 2; s++) {
        let b0 = brow(s, 0);
        let b1 = brow(s, 1);
        let b2 = brow(s, 2);
        let rb = 0.085 + 0.06 * ridge;
        var db = sd_round_cone(p, b0 - vec3<f32>(0.0, 0.0, 0.05), b1 - vec3<f32>(0.0, 0.0, 0.06), rb, rb * 0.95);
        db = smin(db, sd_round_cone(p, b1 - vec3<f32>(0.0, 0.0, 0.06), b2 - vec3<f32>(0.0, 0.03, 0.09), rb * 0.95, rb * 0.7), 0.05);
        d = smin(d, db, 0.10);
    }
    // Furrow wrinkles between the brows when they pull together: the raw
    // control, not a reading of the moved brow landmarks.
    let furrow = c_brow_furrow();
    let bx = abs(p.x);
    if (bx < 0.2 && p.y > 0.15 && p.y < 0.7 && p.z > 0.6) {
        let crease = sin(p.x * 90.0) * smoothstep(0.2, 0.0, bx) * smoothstep(0.7, 0.4, p.y) * smoothstep(0.15, 0.35, p.y);
        d += 0.006 * furrow * crease;
    }
    // Eye sockets, carved under the brow so the eyes sit deep.
    for (var i = 0; i < 2; i++) {
        let e = eye_centre(i);
        let r = eye_radius(i);
        let sock = sd_ellipsoid(p - e - vec3<f32>(0.0, 0.0, 0.03), vec3<f32>(r * 1.2, r * 1.05, r * 0.95));
        d = smax(d, -sock, 0.05);
    }
    // The poll between the horns, and the horn bosses.
    let qx = vec3<f32>(abs(p.x), p.y, p.z);
    d = smin(d, sd_ellipsoid(p - vec3<f32>(0.0, 0.70, 0.02), vec3<f32>(0.50, 0.22, 0.34)), 0.12);
    d = smin(d, length(qx - vec3<f32>(0.50, 0.55, -0.02)) - 0.20, 0.10);
    // Ears, out sideways under the horns, drooping.
    var eq = qx - vec3<f32>(0.86, 0.26, -0.12);
    let er = rot2(0.35) * eq.xy;
    eq = vec3<f32>(er.x, er.y, eq.z);
    let ear = sd_ellipsoid(eq, vec3<f32>(0.26, 0.075, 0.13));
    d = smin(d, ear, 0.06);
    // The neck: a bull's, wider than the head.
    let neck = sd_round_cone(p, vec3<f32>(0.0, -0.40, -0.45), vec3<f32>(0.0, -1.35, -0.75), 0.80, 1.15);
    d = smin(d, neck, 0.25);
    // Everything above the mouth line only: the jaw brings the rest.
    // The seam rises with the muzzle at the front, so the face under the
    // lifted pad goes with it instead of staying behind as a second nose.
    let cut = mi_seam_y(p.z) + mi_lift().y * smoothstep(0.55, 0.85, p.z) - p.y;
    d = smax(d, cut, 0.03);
    // The neck comes back below the seam at the back.
    d = smin(d, smax(neck, -(p.z - 0.1), 0.1), 0.1);
    return d + rig_creases(p);
}

// Lower jaw at rest (q = jaw_rest(p)): chin, lower muzzle, mandible, jowls.
fn mi_lower(q: vec3<f32>) -> f32 {
    let w = s_jaw_width();
    let s = s_snout();
    let mz = 0.88 + 0.45 * s;
    var d = sd_ellipsoid(q - vec3<f32>(0.0, -0.60, mz - 0.33), vec3<f32>(0.33 * w, 0.15, 0.37));
    let qx = vec3<f32>(abs(q.x), q.y, q.z);
    // Mandible branches back to the hinge.
    d = smin(d, sd_round_cone(qx, vec3<f32>(0.55, -0.42, -0.02), vec3<f32>(0.26 * w, -0.60, mz - 0.40), 0.20, 0.13), 0.12);
    // Jowls and dewlap under the jaw.
    d = smin(d, sd_ellipsoid(q - vec3<f32>(0.0, -0.80, 0.28), vec3<f32>(0.52, 0.24, 0.45)), 0.14);
    // Only below the mouth line.
    let cut = q.y - mi_seam_y(q.z);
    d = smax(d, cut, 0.03);
    return d;
}

fn mi_skull(p: vec3<f32>) -> f32 {
    let up = mi_upper(p);
    let lo = mi_lower(jaw_rest(p));
    // Skin webs the jaw at the sides, beyond the mouth corners, so an open
    // mouth reads as a cheek stretching rather than a pac-man wedge.
    let hw = 0.26 * s_jaw_width();
    let web = mix(0.03, 0.16, smoothstep(hw * 0.95, hw * 1.5, abs(p.x)));
    return smin(up, lo, web);
}

// The nose pad and its nostrils on their own: the region the mouth's carve
// must leave alone (see map_head). Clipped above the upper lip so the hole
// still opens under it.
fn mi_pad(p: vec3<f32>) -> f32 {
    let w = s_jaw_width();
    let mz = p - vec3<f32>(0.0, -0.27, 0.93) - mi_lift();
    var d = mi_round_box(mz, vec3<f32>(0.20 * w, 0.075, 0.13), 0.115);
    for (var i = 0; i < 2; i++) {
        let n = mi_nostril(i);
        let sx = select(-1.0, 1.0, n.x > 0.0);
        var q = p - n.xyz;
        let r2 = rot2(-0.45 * sx) * q.xy;
        q = vec3<f32>(r2.x, r2.y, q.z);
        d = smin(d, sd_ellipsoid(q, vec3<f32>(n.w * 2.3, n.w * 1.6, n.w * 1.7)), 0.035);
        let hole = sd_ellipsoid(q - vec3<f32>(0.0, 0.0, 0.01), vec3<f32>(n.w * 1.5, n.w * 0.85, n.w * 2.6));
        d = smax(d, -hole, 0.018);
    }
    return smax(d, lip(LIP_RING / 4).y + lip_r(LIP_RING / 4) + 0.01 - p.y, 0.03);
}

fn map_head(p: vec3<f32>) -> vec2<f32> {
    // One surface: the mouth's carve is forbidden inside the (moved) pad, so
    // an open jaw can never eat the nose from below.
    var res = rig_face_keep(p, mi_skull(p), mi_pad(p));
    let hn = mi_horns(p);
    if (hn.x < res.x) {
        res = vec2<f32>(smin(res.x, hn.x, 0.04), MI_HORN);
    }
    let sep = mi_septum(p);
    if (sep < res.x) {
        res = vec2<f32>(sep, M_SKIN);
    }
    let rg = mi_ring(p);
    if (rg < res.x) {
        res = vec2<f32>(rg, MI_RING);
    }
    return res;
}

// ---- marching with an aura -----------------------------------------------

// Like rig_march, but also returns how close the ray came to the head
// (for the aura around the silhouette). (t, material, closest).
fn mi_march(ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    let b = dot(ro, rd);
    let c = dot(ro, ro) - 2.2 * 2.2;
    let h = b * b - c;
    if (h < 0.0) {
        return vec3<f32>(-1.0, 0.0, 9.0);
    }
    var t = max(-b - sqrt(h), 0.0);
    let tmax = -b + sqrt(h);
    let n = rig_steps();
    var closest = 9.0;
    for (var i = 0; i < n; i++) {
        let r = rig_map(ro + rd * t);
        closest = min(closest, r.x);
        if (r.x < 0.0008 * t) {
            return vec3<f32>(t, r.y, 0.0);
        }
        t += r.x * 0.85;
        if (t > tmax) {
            break;
        }
    }
    return vec3<f32>(-1.0, 0.0, closest);
}

// ---- screen helpers -------------------------------------------------------

// Head-local point → screen p (x ±aspect, y ±1 up), the same space as the
// pixel's `pp` in fs_main. Through the one shared camera.
fn mi_project(p_local: vec3<f32>) -> vec2<f32> {
    return cam_project(p_local);
}

// ---- the labyrinth --------------------------------------------------------

fn mi_ring_spin(k: f32) -> f32 {
    let t = k_time();
    let dir = select(-1.0, 1.0, fract(k * 0.5) < 0.25);
    return dir * t * (0.01 + 0.05 * k_flujo()) * (0.5 + hash11(k * 1.37 + 3.0));
}

// Distance (in log-polar u units) to the maze walls of ring k at angle a
// (turns), given the radial position fr inside the ring.
fn mi_wall(k: f32, a: f32, fr: f32, cells: f32) -> f32 {
    let ang = (a + mi_ring_spin(k)) * cells;
    let cell = floor(ang);
    let fa = ang - cell;
    let ce = vec2<f32>(k + 300.0, (cell % cells + cells) % cells);
    var d = 9.0;
    // The ring's inner wall, with gaps (the doors).
    if (lattice(ce) > 0.28) {
        d = min(d, fr);
    }
    // Radial walls between neighbouring cells.
    let span = MI_TAU / cells * 2.2;
    if (lattice(ce + vec2<f32>(57.0, 11.0)) > 0.5) {
        d = min(d, fa * span);
    }
    let ce2 = vec2<f32>(k + 300.0, ((cell + 1.0) % cells + cells) % cells);
    if (lattice(ce2 + vec2<f32>(57.0, 11.0)) > 0.5) {
        d = min(d, (1.0 - fa) * span);
    }
    return d;
}

fn mi_maze(pp: vec2<f32>, pix: f32) -> vec3<f32> {
    let t = k_time();
    let trip = k_trip();
    let fl = k_flujo();
    let pul = k_pulso();
    let c = vec2<f32>(0.0, 0.22);
    var q = pp - c;
    // Stomp: a shockwave ring travelling out as the hit decays.
    let r0 = length(q);
    let shock_r = 0.95 + 1.9 * (1.0 - pul);
    let sx0 = (r0 - shock_r) * 5.0;
    let shock = pul * exp(-sx0 * sx0);
    q *= 1.0 - 0.07 * shock;
    let r = max(length(q), 1e-3);
    var a = atan2(q.y, q.x) / MI_TAU;
    let lr = log(r);
    // Melt and spiral as trip rises.
    a += trip * (0.12 * lr + 0.03 * sin(lr * 4.0 - t * 0.6 * (0.2 + fl)));
    let zoom = t * (0.03 + 0.22 * fl) * (0.35 + 1.8 * trip);
    let uu = lr * 2.2 - zoom;
    let k = floor(uu);
    let fr = uu - k;
    let cells = 9.0 + floor(4.0 * trip);
    // The outer wall of ring k is ring k+1's inner wall, 1 - fr away.
    let d_out = mi_wall(k + 1.0, a, 1.0 - fr, cells);
    let dd = min(mi_wall(k, a, fr, cells), d_out);
    let aa = 2.2 * pix / r;
    let wall_w = 0.06;
    let core = smoothstep(wall_w + aa, wall_w - aa, dd);
    let halo = exp(-dd * 7.0);
    // Depth: the centre of the tunnel is far away.
    let near = smoothstep(-3.2, 0.6, lr);
    let hue = 0.72 * fract(k * 0.137 + t * 0.03 * fl + trip * (a * 2.0 + 0.2 * lr));
    let wallc = grad4(u.neon, hue);
    // Op-art floor: fine concentric rings beating against radial rays.
    let rings = 0.5 + 0.5 * sin(uu * MI_TAU * 6.0);
    let rays = 0.5 + 0.5 * sin(a * MI_TAU * cells * 4.0 + lr * trip * 6.0);
    let opart = mix(rings, rings * rays, 0.5 + 0.5 * trip);
    let floor_c = grad4(u.background, clamp(0.25 + 0.5 * near, 0.0, 1.0));
    var col = floor_c * 0.35 * (0.45 + 0.55 * opart * (0.3 + 0.7 * trip));
    let em = (0.15 + 0.6 * k_aura()) * near;
    col += wallc * (core * 1.3 + halo * 0.25) * em;
    // A dark well behind the head so the face stays the subject.
    col *= mix(0.35, 1.0, smoothstep(0.55, 1.5, length((pp - vec2<f32>(0.0, -0.05)) * vec2<f32>(0.85, 1.0))));
    // The vanishing point: a far portal glow.
    col += grad4(u.neon, fract(0.6 + t * 0.02)) * exp(-r * 3.0) * (0.3 + 0.7 * k_aura());
    // The shockwave itself.
    col += grad4(u.neon, 0.85) * shock * 1.6;
    // Keep the edges darker so the head stays the subject.
    col *= 0.55 + 0.45 * smoothstep(3.2, 0.6, r0);
    return col;
}

// ---- 2D overlays: steam, breath, dust ----------------------------------

fn mi_plume(pp: vec2<f32>, origin: vec2<f32>, dir: vec2<f32>, strength: f32, seed: f32) -> f32 {
    let v = pp - origin;
    let along = dot(v, dir);
    let across = dot(v, vec2<f32>(-dir.y, dir.x));
    if (along < -0.02 || along > 0.8) {
        return 0.0;
    }
    let t = k_time();
    let speed = 0.6 + 1.4 * strength;
    let width = 0.025 + along * 0.45;
    let body = smoothstep(width, 0.0, abs(across)) * smoothstep(-0.02, 0.04, along) * smoothstep(0.8, 0.15, along);
    let n = mi_noise(vec2<f32>(along * 9.0 - t * speed * 3.0, across * 9.0 + seed));
    let n2 = mi_noise(vec2<f32>(along * 19.0 - t * speed * 5.0 + 7.0, across * 17.0 - seed));
    return body * smoothstep(0.2, 0.75, n * 0.7 + n2 * 0.45) * strength;
}

// ---- shading --------------------------------------------------------------

// The labyrinth markings as a field over the skin: 1 on a neon line.
fn mi_maze_lines(uv: vec2<f32>) -> f32 {
    let cell = floor(uv);
    let f = uv - cell;
    var d = 9.0;
    // Binary-tree maze: each cell walls its east or north edge.
    if (lattice(cell) < 0.5) { d = min(d, 1.0 - f.x); } else { d = min(d, 1.0 - f.y); }
    if (lattice(cell - vec2<f32>(1.0, 0.0)) < 0.5) { d = min(d, f.x); }
    if (lattice(cell - vec2<f32>(0.0, 1.0)) >= 0.5) { d = min(d, f.y); }
    return smoothstep(0.11, 0.05, d);
}

// Where the tribal markings live on the face (head-local p): a blaze down the
// forehead, arcs over the brows, bands down the cheeks; trip spreads them.
fn mi_marking_mask(p: vec3<f32>) -> f32 {
    let trip = k_trip();
    let ax = abs(p.x);
    let blaze = smoothstep(0.22, 0.12, ax - 0.06 * (p.y - 0.2)) * smoothstep(-0.15, 0.05, p.y) * smoothstep(0.85, 0.6, p.y);
    let arc = smoothstep(0.06, 0.0, abs(length(vec2<f32>(ax - 0.36, p.y - 0.12)) - 0.32)) * step(0.25, p.y);
    let cheekband = smoothstep(0.05, 0.0, abs(ax - 0.5 - 0.25 * (p.y + 0.1))) * smoothstep(-0.55, -0.3, p.y) * smoothstep(0.15, -0.05, p.y);
    let spread = smoothstep(0.55, 0.2, mi_noise(p.xy * 3.0 + p.z * 1.7 + vec2<f32>(k_time() * 0.05)) - 0.35 * trip);
    return clamp(max(max(blaze, arc), cheekband) + trip * trip * spread, 0.0, 1.0);
}

fn mi_env(dir: vec3<f32>) -> vec3<f32> {
    let up = dir.y * 0.5 + 0.5;
    var c = grad4(u.background, up) * 0.6;
    c += grad4(u.neon, fract(atan2(dir.x, dir.z) / MI_TAU + 0.5)) * pow(1.0 - abs(dir.y), 6.0) * 1.2;
    c += vec3<f32>(1.0, 0.95, 0.85) * pow(max(dot(dir, normalize(vec3<f32>(-0.5, 0.7, 0.6))), 0.0), 24.0) * 3.0;
    return c;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    surf_uv = in.uv;
    let aspect = k_aspect();
    let pp = (in.uv - vec2<f32>(0.5)) * vec2<f32>(2.0 * aspect, -2.0);
    let pix = max(length(fwidth(pp)), 1e-4);
    let t = k_time();
    let trip = k_trip();
    let fl = k_flujo();
    let shine = k_brillo();
    let pul = k_pulso();
    let det = clamp(k_detalle(), 0.0, 1.0);
    let gap = mouth_gap();

    let cam = rig_camera(in.uv);
    let ro = cam[0];
    let rd = cam[1];

    var col = mi_maze(pp, pix);
    let hit = mi_march(ro, rd);

    // Aura: a velvet glow hugging the silhouette.
    if (hit.x < 0.0) {
        let glow = exp(-hit.z * 9.0) * (0.25 + 0.9 * k_aura());
        let ahue = fract(0.15 + 0.3 * pp.y + t * 0.04 * fl + trip * 0.5 * sin(atan2(pp.y, pp.x) * 3.0 + t));
        col += grad4(u.neon, ahue) * glow * (0.6 + 0.6 * shine + 1.5 * pul);
    } else {
        let pw = ro + rd * hit.x;
        let n = rig_normal(pw);
        let v = -rd;
        let mat = hit.y;
        let p = to_head(pw);
        let nl = to_head(n);

        let key_d = normalize(vec3<f32>(-0.55, 0.65, 0.55));
        let key_c = vec3<f32>(1.0, 0.9, 0.8) * 1.6;
        let rim1 = normalize(vec3<f32>(0.8, 0.3, -0.6));
        let rim2 = normalize(vec3<f32>(-0.8, 0.2, -0.6));
        let rim1_c = grad4(u.neon, 0.0) * (0.8 + 1.4 * shine);
        let rim2_c = grad4(u.neon, 0.34) * (0.8 + 1.4 * shine);
        let fill_c = grad4(u.background, 0.6) * 0.18 + grad4(u.neon, 0.6) * 0.1 * k_aura();

        var nn = n;
        var rim_k = 1.0;
        var albedo = grad4(u.skin, 0.3);
        var spec_k = 0.0;
        var spec_pow = 16.0;
        var sheen = 0.0;
        var emit = vec3<f32>(0.0);
        var wet = 0.0;

        let ndv0 = max(dot(n, v), 0.0);
        let ao = rig_ao(pw, n);

        // Light spilling out from inside the mouth.
        let inner = cavity_centre() + vec3<f32>(0.0, 0.02, 0.15);
        let to_in = inner - p;
        let in_att = (0.25 + 2.2 * gap) / (1.0 + dot(to_in, to_in) * 28.0);
        // A dim bleed from inside the mouth onto the lips/teeth/tongue/
        // cavity — the skin's own dark tone, not a separate warm orange.
        let warm = grad4(u.skin, 0.18) * 0.7;

        if (mat == M_SKIN || mat == M_LID) {
            // Velvet fur: anisotropic streaks flowing down the face, in the
            // shading only.
            let octs = 1 + i32(2.0 * det);
            let fdir = vec2<f32>(p.x * 70.0, (p.y + 0.35 * p.z) * 9.0);
            var fur = 0.0;
            var amp = 0.5;
            var fq = 1.0;
            for (var o = 0; o < 3; o++) {
                if (o < octs) {
                    fur += amp * mi_noise(fdir * fq + vec2<f32>(p.z * 30.0 * fq, 0.0));
                }
                amp *= 0.5;
                fq *= 2.03;
            }
            let tangent = normalize(head_matrix() * vec3<f32>(0.0, -1.0, 0.25));
            nn = normalize(n + tangent * (fur - 0.4) * 0.35 + cross(n, tangent) * (fur - 0.4) * 0.15);
            // Darker in the creases, a little paler on the forehead curls.
            let tone = 0.03 + 0.16 * fur + 0.12 * smoothstep(0.2, 0.7, p.y) * mi_noise(p.xy * 14.0);
            albedo = grad4(u.skin, tone);
            sheen = 1.0;
            // Wet nose pad: dark and glossy around the nostrils.
            // Follows the pad wherever the jaw has pushed it.
            let lf = mi_lift();
            let pp = p - lf;
            let pad = smoothstep(0.1, 0.0, mi_round_box(pp - vec3<f32>(0.0, -0.27, 0.93), vec3<f32>(0.20 * s_jaw_width(), 0.075, 0.13), 0.115) + 0.0) * smoothstep(-0.48, -0.40, pp.y) * smoothstep(0.95, 1.05, pp.z);
            albedo = mix(albedo, grad4(u.skin, 0.03) * 0.7, pad);
            wet = pad;
            sheen = sheen * (1.0 - pad);
            // Labyrinth markings: crawl with flujo, melt with trip.
            let tri = abs(nl);
            var muv = mix(p.xy, vec2<f32>(p.z * sign(p.x), p.y), smoothstep(0.4, 0.8, tri.x)) * (12.0 - 3.0 * trip);
            muv.y += t * fl * 0.7;
            muv += trip * vec2<f32>(mi_noise(muv * 0.35 + vec2<f32>(t * 0.2 * fl, 0.0)), mi_noise(muv * 0.35 + vec2<f32>(5.0, t * 0.17 * fl))) * 2.2;
            let ml = mi_maze_lines(muv);
            let mask = mi_marking_mask(p);
            let mh = 0.7 * fract(0.1 * floor(muv.x * 0.25) + t * 0.03 * fl + trip * (0.6 * p.y + 0.4 * ndv0));
            let mark = ml * mask * (1.0 - pad);
            emit += grad4(u.neon, mh) * mark * (0.4 + 2.2 * shine) * (1.0 + 3.0 * pul);
            albedo = mix(albedo, grad4(u.neon, mh) * 0.15, mark * 0.6);
            // The neck sinks into the dark.
            albedo *= smoothstep(-1.45, -0.7, p.y);
            emit *= smoothstep(-1.3, -0.75, p.y);
            if (mat == M_LID) {
                albedo *= 0.85;
            }
            spec_k = 0.06;
            spec_pow = 10.0;
        } else if (mat == MI_HORN) {
            let q = vec3<f32>(abs(p.x), p.y, p.z);
            let hr = mi_horn(q);
            let ht = hr.y;
            albedo = mix(grad4(u.skin, 0.08), grad4(u.mouth, 0.95) * 0.75, smoothstep(0.1, 0.95, ht));
            // Op-art banding: two near-equal frequencies beat into moiré, the
            // phase walks with the view so it shimmers as the head turns.
            let shimmer = dot(nn, v) * 7.0 + t * (0.4 + 2.5 * fl);
            let b1 = sin(ht * 34.0 - shimmer);
            let b2 = sin(ht * 37.0 + shimmer * 0.6 + atan2(q.z - 0.2, q.y - 0.8));
            let bands = smoothstep(0.1, 0.6, b1 * b2) ;
            let fine = smoothstep(0.85, 0.98, sin(ht * 90.0 - shimmer * 2.0)) * 0.5;
            let bh = fract(ht * 1.5 + t * 0.05 * fl + trip * dot(nn, v));
            let bcol = grad4(u.neon, bh);
            albedo = mix(albedo, bcol * 0.5, bands * 0.6);
            emit += bcol * (bands * 0.8 + fine * 0.3) * (0.25 + 1.6 * shine) * smoothstep(0.05, 0.35, ht) * (1.0 + 5.0 * pul);
            spec_k = 0.5;
            spec_pow = 40.0;
        } else if (mat == MI_RING) {
            let refl = reflect(rd, n);
            let gold = mix(vec3<f32>(1.0, 0.78, 0.36), grad4(u.neon, 0.9), 0.25);
            let fres = 0.6 + 0.4 * pow(1.0 - ndv0, 3.0);
            emit += mi_env(refl) * gold * fres * 1.3;
            albedo = vec3<f32>(0.02);
            spec_k = 2.0;
            spec_pow = 90.0;
        } else if (mat == M_LIP) {
            // Same velvet fur tone as the face, just darker toward the
            // inner edge of the lip — no separate glossy material.
            let fur = mi_noise(vec2<f32>(p.x * 40.0, p.y * 140.0));
            let inner = smoothstep(0.03, -0.02, abs(p.y - mouth_centre().y) - 0.5 * gap - 0.02);
            albedo = grad4(u.skin, mix(0.1 + 0.16 * fur, 0.03, inner));
            sheen = 0.3;
        } else if (mat == M_TEETH) {
            // Off-white incisors below; a dark-skin dental pad above (cattle
            // have no upper incisors) — distinguished by which side of the
            // bite they're on, not by a separate white-everywhere material.
            if (p.y > mouth_centre().y - 0.02) {
                albedo = grad4(u.skin, 0.07);
                spec_k = 0.15;
                spec_pow = 20.0;
            } else {
                albedo = grad4(u.mouth, 1.0) * 0.9;
                spec_k = 0.5;
                spec_pow = 40.0;
            }
        } else if (mat == M_TONGUE) {
            // A muted dark rose-brown, from the skin itself — flat, no
            // bump noise, no warm orange glow. Less wet than before: the
            // sharp specular ring a tapered cone catches at grazing
            // angles was reading as stripes/banding.
            albedo = mix(grad4(u.skin, 0.05), vec3<f32>(0.3, 0.15, 0.14), 0.6);
            wet = 0.15;
        } else if (mat == M_CAVITY) {
            // Dark skin hue, falling to near black with depth — no boca
            // warmth, like the dragon.
            let depth = clamp((cavity_centre().z - p.z) * 2.2, 0.0, 1.0);
            albedo = mouth_cavity_base() * mix(1.0, 0.25, depth);
            wet = 0.15;
        } else if (mat == M_EYE) {
            // Glowing eye with a horizontal bovine pupil.
            var side = 0;
            if (p.x < 0.0) { side = 1; }
            // In the gaze frame (gaze, scan, turn, iris/pupil size).
            let en = eye_dir(p, side);
            let pn = pupil_dir(p, side);
            let rr = length(en.xy);
            let pupil = length(vec2<f32>(pn.x * 1.0, pn.y * 2.8));
            let open = clamp(eye_open(side), 0.0, 1.2);
            let ihue = fract(0.12 + 0.6 * rr + t * 0.05 * fl + trip * 0.3 * sin(atan2(en.y, en.x) * 5.0 + t));
            let iris = grad4(u.neon, ihue) * eye_iris_tint(rr / 0.75);
            let streaks = 0.6 + 0.4 * sin(atan2(en.y, en.x) * 24.0 + rr * 10.0);
            let glow = k_ojos() * (0.3 + 3.5 * smoothstep(0.75, 0.3, rr) * streaks);
            // A pale, wet sclera round the glowing iris so the gaze, the lids
            // and the pupil read from across the room.
            let sclera = mix(grad4(u.skin, 0.95), vec3<f32>(0.85, 0.82, 0.75), 0.6) * 0.75;
            let irisw = smoothstep(0.62, 0.55, rr);
            albedo = mix(sclera, vec3<f32>(0.02), irisw);
            albedo = mix(eye_pupil_color(pupil / 0.26), albedo, smoothstep(0.22, 0.3, pupil));
            emit += iris * glow * (0.4 + 0.6 * open) * smoothstep(0.22, 0.3, pupil) * irisw;
            emit += eye_catchlight(p, side);
            emit += vec3<f32>(1.0, 0.9, 0.8) * smoothstep(0.12, 0.05, pupil) * 0.0;
            spec_k = 1.2;
            spec_pow = 120.0;
            rim_k = 0.15;
        }

        if (mat == M_SKIN || mat == M_LID) { albedo = surf_skin_mix(albedo, p, nl); }
        if (mat == M_EYE) { albedo = surf_eyes_mix(albedo, p, nl); }

        // Lighting.
        let ndl = max(dot(nn, key_d), 0.0);
        var shadow = 1.0;
        if (det > 0.45 && ndl > 0.0) {
            shadow = rig_shadow(pw + n * 0.01, key_d, 10.0);
        }
        let ndv = max(dot(nn, v), 0.0);
        let hvec = normalize(key_d + v);
        var lit = albedo * key_c * ndl * shadow;
        // Wrap fill from below (the glowing maze floor).
        lit += albedo * fill_c * (0.5 + 0.5 * dot(nn, vec3<f32>(0.0, -1.0, 0.3)));
        // Ambient.
        lit += albedo * grad4(u.background, 0.6) * 0.12 * ao;
        // Rims from behind, both neon.
        let rimf = pow(1.0 - ndv, 3.0);
        lit += rim1_c * rimf * max(dot(nn, rim1) + 0.4, 0.0) * 0.7 * rim_k;
        lit += rim2_c * rimf * max(dot(nn, rim2) + 0.4, 0.0) * 0.7 * rim_k;
        // Velvet: fuzzy grazing sheen.
        let sheen_c = mix(grad4(u.skin, 0.85), grad4(u.neon, fract(0.2 + trip * (1.0 - ndv))), 0.35 + 0.4 * trip);
        lit += sheen * sheen_c * pow(1.0 - ndv, 1.6) * 0.45 * (0.4 + ndl) * ao;
        // Specular (teeth, horns, ring, eyes) and wet gloss (tongue, nose).
        let spec = pow(max(dot(nn, hvec), 0.0), spec_pow) * shadow;
        lit += key_c * spec * spec_k;
        if (wet > 0.0) {
            let ws = pow(max(dot(n, hvec), 0.0), 70.0) * shadow;
            lit += key_c * ws * wet * 1.2 + mi_env(reflect(rd, n)) * wet * 0.15 * pow(1.0 - ndv0, 2.0);
        }
        // The warm light from inside the mouth on lips, teeth, tongue, cavity.
        let in_l = max(dot(nn, normalize(to_in)), 0.0) * 0.6 + 0.4;
        lit += albedo * warm * in_att * in_l * 0.5;
        lit *= mix(1.0, ao, 0.85);
        col = lit + emit;
        // Pulso: a white-hot flash over everything for the hit.
        col += grad4(u.neon, 0.95) * pul * 0.25 * pow(1.0 - ndv, 1.5);
    }

    // ---- overlays in front of everything ----
    // Steam from the nostrils: strength from the flare plus a slow breath.
    let flare = mi_flare();
    let breath = 0.04 + 0.1 * smoothstep(0.6, 1.0, sin(t * 1.3));
    let strength = clamp(breath + 0.9 * flare + 0.4 * pul, 0.0, 1.2);
    var steam = 0.0;
    var steam_c = vec3<f32>(0.0);
    for (var i = 0; i < 2; i++) {
        let nlc = mi_nostril(i);
        let sx = select(-1.0, 1.0, nlc.x > 0.0);
        let o = mi_project(nlc.xyz + vec3<f32>(0.0, -0.02, 0.06));
        let o2 = mi_project(nlc.xyz + vec3<f32>(sx * 0.3, -0.25, 0.3));
        let dir = normalize(o2 - o + vec2<f32>(0.0, -1e-4));
        let s = mi_plume(pp, o, dir, strength, f32(i) * 13.0);
        steam += s;
    }
    steam = clamp(steam, 0.0, 1.0);
    let steam_col = mix(vec3<f32>(0.85, 0.85, 0.9), grad4(u.neon, fract(0.5 + t * 0.03)), 0.35 + 0.4 * trip);
    // Thinner over an open mouth, so the snort never smears the nose and the
    // mouth into one haze.
    let open_m = smoothstep(0.01, 0.12, gap);
    col = mix(col, steam_col * (0.6 + 0.6 * shine), steam * 0.7 * (1.0 - 0.75 * open_m));

    // Breath and sound rings out of an open mouth.
    let mc = mi_project(mouth_centre() + vec3<f32>(0.0, 0.0, 0.05));
    let mw = length(mi_project(lip(0)) - mi_project(lip(LIP_RING / 2))) * 0.5;
    let dm = length((pp - mc) * vec2<f32>(1.0, 1.4));
    let sing = smoothstep(0.01, 0.12, gap);
    if (sing > 0.0) {
        var rings = 0.0;
        for (var k = 0; k < 3; k++) {
            let ph = fract(t * (0.5 + 0.8 * fl) + f32(k) / 3.0);
            let rr = mw * (1.15 + ph * 2.2);
            let rx = (dm - rr) / (0.006 + 0.02 * ph);
            rings += exp(-rx * rx) * (1.0 - ph);
        }
        let mist = mi_noise((pp - mc) * 7.0 + vec2<f32>(0.0, -t * 1.5)) * exp(-max(dm - mw, 0.0) * 4.0) * smoothstep(mw * 0.9, mw * 1.4, dm);
        // Only below the lip line: never painted across the nose pad.
        let below = smoothstep(mc.y + 0.4 * mw, mc.y - 0.1 * mw, pp.y);
        col += grad4(u.mouth, 0.55) * (rings * 0.7 + mist * 0.25) * sing * (0.4 + shine) * below;
    }

    // Eye bloom.
    for (var i = 0; i < 2; i++) {
        let ec = mi_project(eye_centre(i) + vec3<f32>(0.0, 0.0, eye_radius(i)));
        let de = length(pp - ec);
        col += grad4(u.neon, 0.15) * exp(-de * 18.0) * k_ojos() * clamp(eye_open(i), 0.0, 1.0) * 0.35;
    }

    // Dust burst from the stomp.
    if (pul > 0.01) {
        let dc = vec2<f32>(0.0, -1.1);
        let dv = pp - dc;
        let dr = length(dv);
        let front = 0.2 + 1.6 * (1.0 - pul);
        let cellp = floor(pp * 70.0);
        let sp = step(0.93, lattice(cellp + vec2<f32>(floor(t * 20.0), 0.0)));
        let band = smoothstep(front - 0.5, front, dr) * smoothstep(front + 0.05, front - 0.05, dr);
        col += grad4(u.skin, 0.95) * sp * band * pul * 1.5;
        col += grad4(u.skin, 0.7) * band * pul * 0.12 * mi_noise(pp * 6.0 + vec2<f32>(t, 0.0));
    }

    // Hue: the whole palette turns with matiz, and drifts a little with trip.
    col = hue_rotate(col, k_matiz() + trip * 0.08 * sin(t * 0.13));
    col = tonemap(col);
    return vec4<f32>(col, 1.0);
}
