// monstruo — the shared face rig. Concatenated ahead of every species file
// (`monstruo_<kind>.wgsl`); see `models/monstruo.rs`.
//
// Everything here is head-local, in head radii: +y up, +z toward the viewer,
// +x the creature's own left (the viewer's right). The packed landmarks come
// from `metal_face::Posed::pack` and are already posed by the rig floats; this
// file only turns them into distance fields. It never decides how a species
// looks — no colours, no light, no skull.
//
// THE CONTRACT WITH A SPECIES FILE
//
// A species file must define:
//     fn map_head(p: vec3<f32>) -> vec2<f32>   // (distance, material id)
//     @fragment fn fs_main(in: FsVOut) -> @location(0) vec4<f32>
// and may call anything below. `rig_march`, `rig_normal`, `rig_ao` and
// `rig_shadow` call `map_head` through `rig_map`, which applies the head
// rotation, so a species models its skull looking straight at the camera and
// the head still nods, turns and tilts.
//
// The usual composition, which `rig_face` does for you:
//     skull  → smax with −(cavity ∪ mouth hole)   (the mouth is open)
//            ∪ lips ∪ teeth ∪ tongue ∪ eyes ∪ lids
// Use the M_* material ids so species can share the mouth's shading.

struct Face {
    jaw: array<vec4<f32>, 2>,
    head: array<vec4<f32>, 3>,
    shape: array<vec4<f32>, 3>,
    lips: array<vec4<f32>, 12>,
    cavity: array<vec4<f32>, 2>,
    upper_teeth: array<vec4<f32>, 12>,
    lower_teeth: array<vec4<f32>, 12>,
    tongue: array<vec4<f32>, 8>,
    eyes: array<vec4<f32>, 4>,
    brows: array<vec4<f32>, 6>,
    nostrils: array<vec4<f32>, 2>,
    cheeks: array<vec4<f32>, 2>,
    // metal_face::pose::LAYOUT's last two sections, append-only:
    // the clamped FaceControl values in ALL order, zero-padded to
    // ceil(COUNT / 4) vec4s, then (rest mouth.xyz, jaw_pivot.z).
    controls: array<vec4<f32>, 12>,
    rest: array<vec4<f32>, 1>,
};

struct U {
    face: Face,
    // (aspect, time, species, trip)
    a: vec4<f32>,
    // (flow, glow, aura, pulse)
    b: vec4<f32>,
    // (distance, eyes, hue, detail)
    c: vec4<f32>,
    skin: ColorSlot,
    neon: ColorSlot,
    mouth: ColorSlot,
    background: ColorSlot,
    // oracle's eye, appended: (gaze_x, gaze_y, scan_rate, lid)
    d: vec4<f32>,
    // (pupil, iris, roll, eye_scan)
    e: vec4<f32>,
    iris: ColorSlot,
    ojo_brillo: ColorSlot,
    pupil_color: ColorSlot,
    // Materials (appended): the picture setting and the `mat_*` floats of each
    // part, one `MaterialU` each. See `material.wgsl`.
    mat_skin: MaterialU,
    mat_eyes: MaterialU,
};

@group(0) @binding(0) var<uniform> u: U;
// The pictures laid on `skin` and `eyes` (plan: surface sources). Bound by
// `Fullscreen::with_textures(.., 2)`: texture and sampler per part.
@group(0) @binding(1) var surf_skin_tex: texture_2d<f32>;
@group(0) @binding(2) var surf_skin_samp: sampler;
@group(0) @binding(3) var surf_eyes_tex: texture_2d<f32>;
@group(0) @binding(4) var surf_eyes_samp: sampler;
// The screen uv of the fragment being shaded, set first thing in `fs_main`
// so the projector mapping needs no argument threaded through the shaders.
var<private> surf_uv: vec2<f32>;

// The skin (and lids and jaw, which are skin), taken through the shared
// material: the picture and the `mat_*` kind, off at mix 0. `obj` is
// head-local, `n_obj` the head-local normal, `v` the direction to the eye from
// `cam_origin()` — the one view vector a face's own camera gives every point.
fn surf_skin(albedo: vec3<f32>, obj: vec3<f32>, n_obj: vec3<f32>) -> Shaded {
    let v = normalize(cam_origin() - obj);
    return material_apply(u.mat_skin, surf_skin_tex, surf_skin_samp, albedo, obj, n_obj, surf_uv, v, u.a.y);
}

// The albedo of [`surf_skin`], for the species files that only ever replaced
// their colour. The normal, roughness, metalness and alpha answer to a host
// that shades at the hit, not to a face's own fixed light.
fn surf_skin_mix(albedo: vec3<f32>, obj: vec3<f32>, n_obj: vec3<f32>) -> vec3<f32> {
    return surf_skin(albedo, obj, n_obj).albedo;
}

// The same for the eyes' own colour (the iris and sclera, not the lids).
fn surf_eyes(albedo: vec3<f32>, obj: vec3<f32>, n_obj: vec3<f32>) -> Shaded {
    let v = normalize(cam_origin() - obj);
    return material_apply(u.mat_eyes, surf_eyes_tex, surf_eyes_samp, albedo, obj, n_obj, surf_uv, v, u.a.y);
}

fn surf_eyes_mix(albedo: vec3<f32>, obj: vec3<f32>, n_obj: vec3<f32>) -> vec3<f32> {
    return surf_eyes(albedo, obj, n_obj).albedo;
}

const LIP_RING: i32 = 12;
const TEETH: i32 = 6;
const TONGUE_SEGMENTS: i32 = 4;
const RIG_PI: f32 = 3.14159265;

// Material ids. A species adds its own from 10 up.
const M_SKIN: f32 = 0.0;
const M_LIP: f32 = 1.0;
const M_TEETH: f32 = 2.0;
const M_TONGUE: f32 = 3.0;
const M_CAVITY: f32 = 4.0;
const M_EYE: f32 = 5.0;
const M_LID: f32 = 6.0;

// ---- the look floats, by name ---------------------------------------------

fn k_aspect() -> f32 { return u.a.x; }
fn k_time() -> f32 { return u.a.y; }
fn k_trip() -> f32 { return u.a.w; }
fn k_flujo() -> f32 { return u.b.x; }
fn k_brillo() -> f32 { return u.b.y; }
fn k_aura() -> f32 { return u.b.z; }
fn k_pulso() -> f32 { return u.b.w; }
fn k_distancia() -> f32 { return u.c.x; }
fn k_ojos() -> f32 { return u.c.y; }
fn k_matiz() -> f32 { return u.c.z; }
fn k_detalle() -> f32 { return u.c.w; }

// ---- species proportions (shape section) ----------------------------------

fn s_snout() -> f32 { return u.face.shape[0].x; }
fn s_jaw_width() -> f32 { return u.face.shape[0].y; }
fn s_brow_ridge() -> f32 { return u.face.shape[0].z; }
fn s_horns() -> f32 { return u.face.shape[0].w; }
fn s_scales() -> f32 { return u.face.shape[1].x; }
fn s_eye_spacing() -> f32 { return u.face.shape[1].y; }
fn s_fang_length() -> f32 { return u.face.shape[1].z; }
fn s_tongue_length() -> f32 { return u.face.shape[1].w; }
fn s_forked() -> bool { return u.face.shape[2].x > 0.5; }

// ---- small SDF kit --------------------------------------------------------

fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

fn smax(a: f32, b: f32, k: f32) -> f32 {
    return -smin(-a, -b, k);
}

fn sd_capsule(p: vec3<f32>, a: vec3<f32>, b: vec3<f32>, r: f32) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-8), 0.0, 1.0);
    return length(pa - ba * h) - r;
}

// A capsule whose radius goes from r1 at `a` to r2 at `b` (iq's round cone).
fn sd_round_cone(p: vec3<f32>, a: vec3<f32>, b: vec3<f32>, r1: f32, r2: f32) -> f32 {
    let ba = b - a;
    let l2 = max(dot(ba, ba), 1e-8);
    let rr = r1 - r2;
    let a2 = l2 - rr * rr;
    let il2 = 1.0 / l2;
    let pa = p - a;
    let y = dot(pa, ba);
    let z = y - l2;
    let xv = pa * l2 - ba * y;
    let x2 = dot(xv, xv);
    let y2 = y * y * l2;
    let z2 = z * z * l2;
    let k = sign(rr) * rr * rr * x2;
    if (sign(z) * a2 * z2 > k) {
        return sqrt(x2 + z2) * il2 - r2;
    }
    if (sign(y) * a2 * y2 < k) {
        return sqrt(x2 + y2) * il2 - r1;
    }
    return (sqrt(x2 * a2 * il2) + y * rr) * il2 - r1;
}

// Not exact, but a bound good enough to march (iq's ellipsoid).
fn sd_ellipsoid(p: vec3<f32>, r: vec3<f32>) -> f32 {
    let k0 = length(p / r);
    let k1 = length(p / (r * r));
    return k0 * (k0 - 1.0) / max(k1, 1e-6);
}

fn rot_x(p: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(p.x, c * p.y - s * p.z, s * p.y + c * p.z);
}

fn rot2(a: f32) -> mat2x2<f32> {
    let c = cos(a);
    let s = sin(a);
    return mat2x2<f32>(c, s, -s, c);
}

// ---- frames ---------------------------------------------------------------

fn head_matrix() -> mat3x3<f32> {
    return mat3x3<f32>(u.face.head[0].xyz, u.face.head[1].xyz, u.face.head[2].xyz);
}

// World → head-local. The head turns about the skull's centre (the origin).
fn to_head(p: vec3<f32>) -> vec3<f32> {
    return transpose(head_matrix()) * p;
}

fn jaw_pivot() -> vec3<f32> { return u.face.jaw[0].xyz; }
fn jaw_angle() -> f32 { return u.face.jaw[0].w; }
fn jaw_side() -> f32 { return u.face.jaw[1].x; }

// ---- the raw controls and the constant rest landmark ----------------------
//
// `controls` is the 23 clamped FaceControl values in FaceControl::ALL order
// (see metal_face::control), so a species can read the driver instead of guessing
// it backwards from landmarks the driver has already moved. Indices are the
// enum discriminants; the names are the contract. `rest` is the resting mouth
// landmark, and its z is the jaw hinge depth.

const C_CHEEK_PUFF: i32 = 13;
const C_NOSTRIL_FLARE: i32 = 14;
const C_BROW_UP: i32 = 15;
const C_BROW_FURROW: i32 = 16;
const C_SQUINT: i32 = 19;
// The expression muscles appended on 2026-10-07 (FaceControl 23..).
const C_LIP_CORNER_UP: i32 = 8;
const C_LIP_CORNER_DOWN: i32 = 9;
const C_BROW_INNER_UP: i32 = 23;
const C_BROW_OUTER_UP_L: i32 = 24;
const C_BROW_DOWN_L: i32 = 26;
const C_CHEEK_RAISE_L: i32 = 32;
const C_NOSE_WRINKLE_L: i32 = 34;
const C_LIP_UPPER_UP_L: i32 = 36;
const C_SMILE_L: i32 = 38;
const C_FROWN_L: i32 = 40;
const C_LIP_STRETCH_L: i32 = 42;
const C_LIP_PRESS: i32 = 44;

// One clamped control by its FaceControl index: controls[i/4], component i%4.
fn ctl(i: i32) -> f32 {
    let v = u.face.controls[i / 4];
    let c = i % 4;
    if (c == 0) { return v.x; }
    if (c == 1) { return v.y; }
    if (c == 2) { return v.z; }
    return v.w;
}
fn c_brow_furrow() -> f32 { return ctl(C_BROW_FURROW); }
fn c_squint() -> f32 { return ctl(C_SQUINT); }
fn c_cheek_puff() -> f32 { return ctl(C_CHEEK_PUFF); }
fn c_nostril_flare() -> f32 { return ctl(C_NOSTRIL_FLARE); }
fn c_brow_up() -> f32 { return ctl(C_BROW_UP); }
fn c_brow_inner_up() -> f32 { return ctl(C_BROW_INNER_UP); }
fn c_lip_press() -> f32 { return ctl(C_LIP_PRESS); }
// A per-side muscle: side 0 is the creature's left (+x), 1 its right. The
// L and R controls are adjacent, L first.
fn c_side(first_l: i32, side: i32) -> f32 { return ctl(first_l + side); }
fn c_brow_outer_up(side: i32) -> f32 { return c_side(C_BROW_OUTER_UP_L, side); }
// AU4 on one side, with the symmetric furrow folded in.
fn c_brow_down(side: i32) -> f32 { return min(c_side(C_BROW_DOWN_L, side) + c_brow_furrow(), 1.0); }
fn c_cheek_raise(side: i32) -> f32 { return c_side(C_CHEEK_RAISE_L, side); }
fn c_nose_wrinkle(side: i32) -> f32 { return c_side(C_NOSE_WRINKLE_L, side); }
fn c_lip_upper_up(side: i32) -> f32 { return c_side(C_LIP_UPPER_UP_L, side); }
fn c_smile(side: i32) -> f32 { return min(c_side(C_SMILE_L, side) + ctl(C_LIP_CORNER_UP), 1.0); }
fn c_frown(side: i32) -> f32 { return min(c_side(C_FROWN_L, side) + ctl(C_LIP_CORNER_DOWN), 1.0); }
fn c_lip_stretch(side: i32) -> f32 { return c_side(C_LIP_STRETCH_L, side); }

// The side of the face a head-local point is on: 0 left (+x), 1 right.
fn side_of(p: vec3<f32>) -> i32 { return select(0, 1, p.x < 0.0); }

// The resting mouth landmark (mouth.xyz) from the rest topology: a constant,
// unlike the moving lip ring, so the jaw seam stays put as the jaw opens.
fn rest_mouth() -> vec3<f32> { return u.face.rest[0].xyz; }

// Head-local → the jaw's rest frame. Model the lower jaw (chin, lower snout,
// beard, mandible spikes) at rest and evaluate it at `jaw_rest(p)`: it then
// opens and shifts exactly as the rig moved the lower lip and lower teeth.
fn jaw_rest(p: vec3<f32>) -> vec3<f32> {
    let q = p - vec3<f32>(jaw_side(), 0.0, 0.0) - jaw_pivot();
    return jaw_pivot() + rot_x(q, -jaw_angle());
}

// 0 above the jaw line, 1 below it — the weight a skull uses to blend the
// upper head (evaluated at p) with the lower jaw (evaluated at jaw_rest(p)).
// The line runs from the pivot forward under the upper lip.
fn jaw_weight(p: vec3<f32>) -> f32 {
    // The seam is the REST landmark, not the moving lip ring, so it holds
    // still as the jaw opens instead of being dragged down with the lip.
    let m = rest_mouth();
    let q = p - jaw_pivot();
    // Rotate the line down by half the jaw angle so the seam sits in the gap.
    let line_y = mix(0.0, m.y - jaw_pivot().y, clamp(q.z / max(m.z - jaw_pivot().z, 1e-3), 0.0, 1.0));
    let r = rot_x(q, -0.5 * jaw_angle());
    return smoothstep(0.03, -0.03, r.y - line_y);
}

// ---- landmarks ------------------------------------------------------------

fn lip(i: i32) -> vec3<f32> { return u.face.lips[i].xyz; }
fn lip_r(i: i32) -> f32 { return u.face.lips[i].w; }
fn cavity_centre() -> vec3<f32> { return u.face.cavity[0].xyz; }
fn cavity_radii() -> vec3<f32> { return u.face.cavity[1].xyz; }
fn eye_centre(i: i32) -> vec3<f32> { return u.face.eyes[2 * i].xyz; }
fn eye_radius(i: i32) -> f32 { return u.face.eyes[2 * i].w; }
fn eye_lids(i: i32) -> vec2<f32> { return u.face.eyes[2 * i + 1].xy; }
fn brow(side: i32, k: i32) -> vec3<f32> { return u.face.brows[3 * side + k].xyz; }
fn nostril(i: i32) -> vec4<f32> { return u.face.nostrils[i]; }
fn cheek(i: i32) -> vec4<f32> { return u.face.cheeks[i]; }

// A landmark of point p's own side, mirrored to +x — for species that model
// one half at |x| (the oni). Blended across a narrow band at the midline so
// an asymmetric face never steps there.
fn mirror_side(p: vec3<f32>, l: vec3<f32>, r: vec3<f32>) -> vec3<f32> {
    let rm = vec3<f32>(-r.x, r.y, r.z);
    return mix(rm, l, smoothstep(-0.04, 0.04, p.x));
}
fn brow_m(p: vec3<f32>, k: i32) -> vec3<f32> { return mirror_side(p, brow(0, k), brow(1, k)); }
fn cheek_m(p: vec3<f32>) -> vec4<f32> {
    let w = smoothstep(-0.04, 0.04, p.x);
    return vec4<f32>(mirror_side(p, cheek(0).xyz, cheek(1).xyz), mix(cheek(1).w, cheek(0).w, w));
}
fn nostril_m(p: vec3<f32>) -> vec4<f32> {
    let w = smoothstep(-0.04, 0.04, p.x);
    return vec4<f32>(mirror_side(p, nostril(0).xyz, nostril(1).xyz), mix(nostril(1).w, nostril(0).w, w));
}

// How far apart the lips are at the middle, ≥ 0. A shader-side reading of
// the mouth (for glow from inside, breath, sound rings) that agrees with the
// rig.
fn mouth_gap() -> f32 {
    let up = lip(LIP_RING / 4);
    let lo = lip(3 * LIP_RING / 4);
    return max(up.y - lo.y - lip_r(LIP_RING / 4) - lip_r(3 * LIP_RING / 4), 0.0);
}

fn mouth_centre() -> vec3<f32> {
    return 0.5 * (lip(LIP_RING / 4) + lip(3 * LIP_RING / 4));
}

// ---- the mouth ------------------------------------------------------------

// Lips: round cones between neighbouring ring balls, smooth-unioned.
fn sd_lips(p: vec3<f32>) -> f32 {
    var d = 1e9;
    for (var i = 0; i < LIP_RING; i++) {
        let j = (i + 1) % LIP_RING;
        d = smin(d, sd_round_cone(p, lip(i), lip(j), lip_r(i), lip_r(j)), 0.025);
    }
    return d;
}

// The y of the upper (or lower) lip's centreline at x, read off the ring.
fn lip_line(x: f32, upper: bool) -> f32 {
    var y = mouth_centre().y;
    for (var k = 0; k < LIP_RING / 2; k++) {
        var i = k;
        var j = k + 1;
        if (!upper) {
            i = LIP_RING / 2 + k;
            j = (i + 1) % LIP_RING;
        }
        let a = lip(i);
        let b = lip(j);
        let lo = min(a.x, b.x);
        let hi = max(a.x, b.x);
        if (x >= lo && x <= hi) {
            let t = (x - a.x) / select(b.x - a.x, 1e-5, abs(b.x - a.x) < 1e-5);
            y = mix(a.y, b.y, t);
        }
    }
    return y;
}

// The open slot between the lips, from the lip line back into the cavity.
// Negative inside. Carve `min(sd_cavity, sd_mouth_hole)` out of the skull.
fn sd_mouth_hole(p: vec3<f32>) -> f32 {
    let hw = 0.5 * abs(lip(0).x - lip(LIP_RING / 2).x);
    let xc = 0.5 * (lip(0).x + lip(LIP_RING / 2).x);
    let x = clamp(p.x, xc - hw, xc + hw);
    let yu = lip_line(x, true);
    let yl = lip_line(x, false);
    let dy = max(p.y - yu, yl - p.y);
    let dx = abs(p.x - xc) - hw;
    let dz = cavity_centre().z - p.z;
    return max(max(dy, dx), dz);
}

fn sd_cavity(p: vec3<f32>) -> f32 {
    return sd_ellipsoid(p - cavity_centre(), cavity_radii());
}

fn sd_teeth_row(p: vec3<f32>, upper: bool) -> f32 {
    var d = 1e9;
    for (var i = 0; i < TEETH; i++) {
        var root = u.face.upper_teeth[2 * i];
        var tip = u.face.upper_teeth[2 * i + 1];
        if (!upper) {
            root = u.face.lower_teeth[2 * i];
            tip = u.face.lower_teeth[2 * i + 1];
        }
        // Tapered: a fang ends in a point, a molar in a blunt edge.
        let len = length(tip.xyz - root.xyz);
        let sharp = clamp(len / 0.12 - 0.6, 0.0, 1.0);
        d = min(d, sd_round_cone(p, root.xyz, tip.xyz, root.w, root.w * mix(0.55, 0.12, sharp)));
    }
    return d;
}

fn sd_teeth(p: vec3<f32>) -> f32 {
    return min(sd_teeth_row(p, true), sd_teeth_row(p, false));
}

// The tongue: a tapered chain, flattened top-to-bottom, forked if the
// species says so.
fn sd_tongue(p: vec3<f32>) -> f32 {
    var d = 1e9;
    for (var i = 0; i < TONGUE_SEGMENTS; i++) {
        let a = u.face.tongue[2 * i];
        let b = u.face.tongue[2 * i + 1];
        var r2 = a.w * 0.6;
        if (i + 1 < TONGUE_SEGMENTS) {
            r2 = u.face.tongue[2 * i + 2].w;
        }
        // Flatten in y around the segment.
        let m = 0.5 * (a.xyz + b.xyz);
        let q = vec3<f32>(p.x, m.y + (p.y - m.y) * 1.6, p.z);
        var seg = sd_round_cone(q, a.xyz, b.xyz, a.w, r2) / 1.6;
        if (i == TONGUE_SEGMENTS - 1 && s_forked()) {
            let dir = normalize(b.xyz - a.xyz);
            let side = normalize(cross(dir, vec3<f32>(0.0, 1.0, 0.0)) + vec3<f32>(1e-4, 0.0, 0.0));
            let mid = mix(a.xyz, b.xyz, 0.35);
            let tip_l = b.xyz + side * 0.05 + dir * 0.06 * s_tongue_length();
            let tip_r = b.xyz - side * 0.05 + dir * 0.06 * s_tongue_length();
            let base = sd_round_cone(q, a.xyz, mid, a.w, r2 * 1.2) / 1.6;
            let fl = sd_round_cone(q, mid, tip_l, r2 * 0.9, 0.006) / 1.6;
            let fr = sd_round_cone(q, mid, tip_r, r2 * 0.9, 0.006) / 1.6;
            seg = min(base, min(fl, fr));
        }
        d = smin(d, seg, 0.02);
    }
    return d;
}

// ---- eyes -----------------------------------------------------------------
//
// oracle's eye parameters, on the face (appended 2026-10-07). Same meaning as
// in `oracle.wgsl`: `mirada` biases where the eyes look, `vel_escaneo` is the
// saccade rate (snap to a hashed target every tick, then hold), `pupila` and
// `iris` size the pupil and the iris, `giro` rolls the eye. `ojo_escaneo`
// (face-only) is how far the saccades jump; 0 holds the gaze on `mirada`.

fn k_mirada() -> vec2<f32> { return u.d.xy; }
fn k_vel_escaneo() -> f32 { return u.d.z; }
fn k_parpado() -> f32 { return u.d.w; }
fn k_pupila() -> f32 { return u.e.x; }
fn k_iris() -> f32 { return u.e.y; }
fn k_giro() -> f32 { return u.e.z; }
fn k_ojo_escaneo() -> f32 { return u.e.w; }

fn eye_hash2(id: f32) -> vec2<f32> {
    return vec2<f32>(hash11(id * 17.3), hash11(id * 91.7 + 4.0));
}

// The gaze as (x, y) over z = 1 in head space, both eyes together (a
// creature's eyes converge on one thing). oracle's saccade, verbatim but for
// the amplitude knob.
fn eye_gaze() -> vec2<f32> {
    let speed = max(k_vel_escaneo(), 0.05);
    let time = k_time();
    let tick = floor(time * speed);
    let frac = fract(time * speed);
    let bias = (k_mirada() * 2.0 - vec2<f32>(1.0)) * vec2<f32>(0.6, 0.45);
    let amp = 0.6 * clamp(k_ojo_escaneo(), 0.0, 1.0);
    let prev = bias + (eye_hash2(tick - 1.0) * 2.0 - 1.0) * amp * vec2<f32>(1.0, 0.6);
    let next = bias + (eye_hash2(tick) * 2.0 - 1.0) * amp * vec2<f32>(1.0, 0.6);
    let ease = 1.0 - exp(-frac * 16.0);
    return mix(prev, next, ease);
}

fn eye_gaze_dir() -> vec3<f32> {
    let g = eye_gaze();
    return normalize(vec3<f32>(g.x, g.y, 1.0));
}

// Eye i's roll in radians: `giro` mirrored left/right so + lifts both outer
// corners (an upward slant) and − droops them.
fn eye_roll(i: i32) -> f32 {
    let s = select(1.0, -1.0, i == 1);
    return k_giro() * 0.6 * s;
}

// Iris and pupil radius multipliers. 1.0 at oracle's defaults (iris 0.55,
// pupil 0.4), so the species' own eyes are unchanged until someone moves them.
fn iris_scale() -> f32 { return 0.45 + clamp(k_iris(), 0.0, 1.0); }
fn pupil_scale() -> f32 { return 0.35 + 1.625 * clamp(k_pupila(), 0.0, 1.0); }

// The direction from eyeball i's centre to p, in the eye's own gaze frame:
// +z is where the eye looks, xy is rolled by `giro` and then divided by the
// iris scale — so every species that reads `xy` against fixed radii gets a
// bigger iris from `iris` without knowing it.
fn eye_dir(p: vec3<f32>, i: i32) -> vec3<f32> {
    let n = normalize(p - eye_centre(i));
    let g = eye_gaze_dir();
    let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), g));
    let up = cross(g, right);
    let q = vec3<f32>(dot(n, right), dot(n, up), dot(n, g));
    let r = rot2(eye_roll(i)) * q.xy;
    return vec3<f32>(r / iris_scale(), q.z);
}

// The same, for the pupil: further divided by the pupil scale. A species
// shapes its pupil (slit, bar, round) in these coordinates.
fn pupil_dir(p: vec3<f32>, i: i32) -> vec3<f32> {
    let q = eye_dir(p, i);
    return vec3<f32>(q.xy / pupil_scale(), q.z);
}

// The iris tint (the `iris` slot, white = the species' own colour) at radial
// position r (0 centre … 1 limbus).
fn eye_iris_tint(r: f32) -> vec3<f32> { return grad4(u.iris, clamp(r, 0.0, 1.0)); }

// The pupil's colour (`pupila_color`), walked from the centre out.
fn eye_pupil_color(r: f32) -> vec3<f32> { return grad4(u.pupil_color, clamp(r, 0.0, 1.0)); }

// oracle's catchlight: a small highlight up and to the side of the gaze,
// coloured by `ojo_brillo`. Returns the colour to add.
fn eye_catchlight(p: vec3<f32>, i: i32) -> vec3<f32> {
    let n = normalize(p - eye_centre(i));
    let hd = normalize(eye_gaze_dir() + vec3<f32>(-0.22, 0.30, 0.05));
    let h = smoothstep(0.975, 0.995, dot(n, hd));
    return grad4(u.ojo_brillo, 0.5) * h * 1.2;
}

fn sd_eyeball(p: vec3<f32>, i: i32) -> f32 {
    return length(p - eye_centre(i)) - eye_radius(i);
}

// The lids: a shell just outside the eyeball, covering every direction whose
// elevation angle (atan2(y, z) about the eye's centre) is above the upper
// lid's angle or below the lower lid's. The rig moves only those two angles.
fn sd_lids(p: vec3<f32>, i: i32) -> f32 {
    let q0 = p - eye_centre(i);
    let r = eye_radius(i);
    let shell = abs(length(q0) - r * 1.06) - 0.022;
    // `giro` tilts the lid line with the eye (mirrored, see eye_roll).
    let rq = rot2(-eye_roll(i)) * q0.xy;
    let q = vec3<f32>(rq, q0.z);
    let phi = atan2(q.y, max(q.z, 1e-4));
    let lids = eye_lids(i);
    let open = min(lids.x - phi, phi - lids.y) * r;
    let back = -q.z - r * 0.2;
    return max(max(shell, open), back);
}

// How open eye i is, 0 shut … ~1 wide, for a species that wants its eyes to
// glow less when they narrow.
fn eye_open(i: i32) -> f32 {
    let l = eye_lids(i);
    return clamp((l.x - l.y) / 1.0, 0.0, 1.5);
}

// Iris coordinates on eyeball i: (radial distance from the gaze axis in
// 0..1 of the eye radius, angle). The gaze is +z.
fn iris_uv(p: vec3<f32>, i: i32) -> vec2<f32> {
    let n = eye_dir(p, i);
    return vec2<f32>(length(n.xy), atan2(n.y, n.x));
}

// ---- expression creases ---------------------------------------------------
//
// The folds a gesture throws into the skin, as a displacement a species adds
// to its upper-head skin distance (+ carves). Built from the posed landmarks
// and the raw controls, so it is zero on a neutral face and follows every
// species' proportions: the nasolabial fold (smile, sneer, cheek raise), the
// nose-bridge wrinkles (AU9), the glabella's vertical furrows (AU4) and the
// forehead's horizontal lines (AU1/AU2/brow_up). Only the front of the face.

fn crease_seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn crease_groove(d: f32, w: f32) -> f32 {
    return exp(-(d * d) / (w * w));
}

fn rig_creases(p: vec3<f32>) -> f32 {
    if (p.z < 0.3 || p.y > 1.0 || p.y < -0.9) {
        return 0.0;
    }
    let side = side_of(p);
    let sx = select(1.0, -1.0, side == 1);
    var w = 0.0;
    // Nasolabial fold: from above the nostril wing, out and down past the
    // lip corner. Deepens with a smile, a sneer and a raised cheek.
    let fold = 0.6 * c_smile(side) + 0.8 * c_nose_wrinkle(side) + 0.5 * c_cheek_raise(side)
        + 0.4 * c_lip_upper_up(side) + 0.3 * c_lip_stretch(side);
    if (fold > 0.001) {
        let n = nostril(side).xyz;
        let corner = lip(select(0, LIP_RING / 2, side == 1));
        let a = vec2<f32>(n.x + sx * 0.07, n.y + 0.05);
        let b = vec2<f32>(corner.x + sx * 0.09, corner.y - 0.08);
        let d = crease_seg(p.xy, a, b);
        w += 0.03 * min(fold, 1.2) * crease_groove(d, 0.03);
        // The cheek bulges above the fold.
        w -= 0.018 * min(fold, 1.2) * crease_groove(crease_seg(p.xy, a + vec2<f32>(sx * 0.07, 0.03), b + vec2<f32>(sx * 0.06, 0.05)), 0.06);
    }
    // Nose-bridge wrinkles: short horizontal creases above the nostrils.
    let nw = c_nose_wrinkle(side);
    if (nw > 0.001) {
        let n = nostril(side).xyz;
        for (var k = 0; k < 3; k++) {
            let y = n.y + 0.09 + 0.05 * f32(k);
            let d = abs(p.y - y);
            let span = smoothstep(0.2, 0.04, abs(p.x));
            w += 0.014 * nw * crease_groove(d, 0.014) * span;
        }
    }
    // Glabella: two vertical furrows each side of the midline under AU4.
    let bd = c_brow_down(side);
    if (bd > 0.001) {
        let b0 = brow(side, 0);
        for (var k = 0; k < 2; k++) {
            let x = sx * (0.03 + 0.045 * f32(k));
            let d = crease_seg(p.xy, vec2<f32>(x, b0.y - 0.06), vec2<f32>(x + sx * 0.02, b0.y + 0.16));
            w += (0.02 - 0.006 * f32(k)) * bd * crease_groove(d, 0.018);
        }
    }
    // Forehead lines, curved over the brows, under the raisers.
    let fh = max(c_brow_inner_up() * smoothstep(0.35, 0.05, abs(p.x)), c_brow_up()) + c_brow_outer_up(side) * smoothstep(0.05, 0.35, abs(p.x));
    if (fh > 0.001) {
        let b1 = brow(side, 1);
        for (var k = 0; k < 3; k++) {
            let y = b1.y + 0.13 + 0.07 * f32(k) + 0.18 * p.x * p.x;
            w += 0.012 * min(fh, 1.0) * crease_groove(p.y - y, 0.016) * smoothstep(0.55, 0.25, abs(p.x));
        }
    }
    return w;
}

// ---- composition ----------------------------------------------------------

// The skull with the mouth carved and every face part added. `skull` is the
// species' own skin distance at p (upper head and lower jaw already blended).
// Returns (distance, material).
fn rig_face(p: vec3<f32>, skull: f32) -> vec2<f32> {
    return rig_face_keep(p, skull, 1e9);
}

// rig_face, with a region the mouth's carve may not enter (`keep` < 0
// inside it): a species whose nose sits just above the mouth keeps it solid
// as the cavity grows with the jaw.
fn rig_face_keep(p: vec3<f32>, skull: f32, keep: f32) -> vec2<f32> {
    // smin, not min: a hard min leaves the box-cornered mouth_hole wall
    // visible right where it meets the round cavity, reading as a dark
    // rectangle inside the mouth. Blending the two rounds that seam away.
    let hole = max(smin(sd_cavity(p), sd_mouth_hole(p), 0.05), -keep);
    var d = smax(skull, -hole, 0.03);
    var res = vec2<f32>(d, M_SKIN);
    // The inside wall of the mouth: where the carve, not the skin, is the
    // surface.
    if (-hole > skull - 0.008 && d < 0.03) {
        res.y = M_CAVITY;
    }
    // The lips sit fuller than the carve so their own (skin-toned) surface
    // buries the seam: a wider fillet here, not just a wider hole blend,
    // keeps the join from reading as a hole cut into the head.
    let dl = sd_lips(p);
    if (dl < res.x + 0.035) {
        res = vec2<f32>(smin(res.x, dl, 0.035), select(res.y, M_LIP, dl < res.x));
    }
    let dt = sd_teeth(p);
    if (dt < res.x) { res = vec2<f32>(dt, M_TEETH); }
    let dg = sd_tongue(p);
    if (dg < res.x) { res = vec2<f32>(dg, M_TONGUE); }
    for (var i = 0; i < 2; i++) {
        let de = sd_eyeball(p, i);
        if (de < res.x) { res = vec2<f32>(de, M_EYE); }
        let dd = sd_lids(p, i);
        if (dd < res.x + 0.01) {
            res = vec2<f32>(smin(res.x, dd, 0.01), select(res.y, M_LID, dd < res.x));
        }
    }
    return res;
}

// ---- marching -------------------------------------------------------------

fn rig_map(p_world: vec3<f32>) -> vec2<f32> {
    return map_head(to_head(p_world));
}

// ONE camera for all five species. It sits on +z looking straight at the
// head's centre, pulled back far enough that the dragon's frills, the
// minotaur's horns, the gorgona's serpent crown, the oni's swept horns and the
// quetzalcóatl's long muzzle and plume crest all stay in frame, with no hard
// clip against the ±1 / ±aspect frame. `distancia` still scales the distance.
//
//     position (0, -0.12, 4.6), target (0, -0.12, 0), focal 1.6 / 0.62
//
// Returns (origin, direction) for the pixel at uv (0..1, y down).
const CAM_RO: vec3<f32> = vec3<f32>(0.0, -0.12, 4.6);
const CAM_FOCAL: f32 = 1.6 / 0.62;

fn cam_origin() -> vec3<f32> {
    return vec3<f32>(CAM_RO.x, CAM_RO.y, CAM_RO.z * k_distancia());
}

fn cam_focal() -> f32 { return CAM_FOCAL; }

fn rig_camera(uv: vec2<f32>) -> array<vec3<f32>, 2> {
    let p = (uv - vec2<f32>(0.5)) * vec2<f32>(2.0 * k_aspect(), -2.0);
    let rd = normalize(vec3<f32>(p * 0.62, -1.6));
    return array<vec3<f32>, 2>(cam_origin(), rd);
}

// Head-local → screen (x in ±aspect, y in ±1, y up) through that camera.
fn cam_project(p_local: vec3<f32>) -> vec2<f32> {
    let d = head_matrix() * p_local - cam_origin();
    return d.xy / max(-d.z, 1e-3) * CAM_FOCAL;
}

// March budget from `detalle`: 64 … 160 steps.
fn rig_steps() -> i32 {
    return 64 + i32(96.0 * clamp(k_detalle(), 0.0, 1.0));
}

// (t, material, steps used / budget). t < 0 is a miss.
fn rig_march(ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    var t = 0.0;
    let n = rig_steps();
    // Skip empty space up to the bounding sphere (radius 2.2 covers horns
    // and snouts).
    let b = dot(ro, rd);
    let c = dot(ro, ro) - 2.2 * 2.2;
    let h = b * b - c;
    if (h < 0.0) {
        return vec3<f32>(-1.0, 0.0, 0.0);
    }
    t = max(-b - sqrt(h), 0.0);
    let tmax = -b + sqrt(h);
    for (var i = 0; i < n; i++) {
        let r = rig_map(ro + rd * t);
        if (r.x < 0.0008 * t) {
            return vec3<f32>(t, r.y, f32(i) / f32(n));
        }
        t += r.x * 0.85;
        if (t > tmax) {
            break;
        }
    }
    return vec3<f32>(-1.0, 0.0, 1.0);
}

fn rig_normal(p: vec3<f32>) -> vec3<f32> {
    let e = vec2<f32>(1.0, -1.0) * 0.0012;
    return normalize(
        e.xyy * rig_map(p + e.xyy).x + e.yyx * rig_map(p + e.yyx).x +
        e.yxy * rig_map(p + e.yxy).x + e.xxx * rig_map(p + e.xxx).x
    );
}

fn rig_ao(p: vec3<f32>, n: vec3<f32>) -> f32 {
    var occ = 0.0;
    var w = 1.0;
    let taps = 3 + i32(3.0 * k_detalle());
    for (var i = 1; i <= taps; i++) {
        let h = 0.02 + 0.06 * f32(i);
        occ += (h - rig_map(p + n * h).x) * w;
        w *= 0.7;
    }
    return clamp(1.0 - 2.2 * occ, 0.0, 1.0);
}

fn rig_shadow(ro: vec3<f32>, rd: vec3<f32>, k: f32) -> f32 {
    var res = 1.0;
    var t = 0.02;
    for (var i = 0; i < 32; i++) {
        let h = rig_map(ro + rd * t).x;
        res = min(res, k * h / t);
        t += clamp(h, 0.02, 0.2);
        if (res < 0.001 || t > 3.0) {
            break;
        }
    }
    return clamp(res, 0.0, 1.0);
}

// ---- colour ---------------------------------------------------------------

// Rotate a colour's hue by `a` turns (YIQ rotation; keeps luminance).
fn hue_rotate(c: vec3<f32>, a: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735);
    let ang = a * 2.0 * RIG_PI;
    let cs = cos(ang);
    return c * cs + cross(k, c) * sin(ang) + k * dot(k, c) * (1.0 - cs);
}

// iq's cosine palette, for psychedelic ramps a species does not want to
// take from a slot.
fn cos_palette(t: f32, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    return a + b * cos(2.0 * RIG_PI * (c * t + d));
}

// ---- the mouth interior, one palette walk for all five --------------------
//
// The cavity and gums are shaded skin, not a separate pink: both are built
// from `piel` (the species' own head palette), darkened into shadow for the
// cavity and lifted a touch for the gums, so the inside of the mouth reads
// as the same creature as the head. `boca` still gives the tongue its clear,
// saturated step above that shaded skin, so the tongue always reads against
// the cavity and gums — a species may add its own fire, wetness or hue drift
// on top of any of the three.

fn mouth_cavity_base() -> vec3<f32> {
    // Deep in shadow: the darkest, coolest stop of the head's own skin.
    return max(grad4(u.skin, 0.04), vec3<f32>(0.0)) * 0.28;
}

fn mouth_gum_base() -> vec3<f32> {
    // Flesh just inside the lips: a mid skin tone, slightly warmed toward
    // `boca` rather than a flat pink.
    let skin = max(grad4(u.skin, 0.22), vec3<f32>(0.0));
    let warm = max(grad4(u.mouth, 0.3), vec3<f32>(0.0));
    return mix(skin, warm, 0.2) * 0.55;
}

fn mouth_tongue_base() -> vec3<f32> {
    // Drop the blue: the tongue becomes a saturated red the cavity can never
    // be mistaken for.
    let c = max(grad4(u.mouth, 0.72), vec3<f32>(0.0));
    return vec3<f32>(c.r * 1.05, c.g * 0.65, c.b * 0.4);
}

// The lip surface's own colour: mostly the head's skin (darker, in shadow of
// the fold), with a small lift from `boca` so the lip line still reads as a
// lip and not bare skin. `w` lets a species weight that lift (0 = pure skin).
fn mouth_lip_base(w: f32) -> vec3<f32> {
    let skin = max(grad4(u.skin, 0.06), vec3<f32>(0.0)) * 0.65;
    let lift = max(grad4(u.mouth, 0.3), vec3<f32>(0.0)) * 0.6;
    return mix(skin, lift, clamp(w, 0.0, 1.0));
}

// ACES-ish filmic curve: big neon values roll off instead of clipping flat.
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}
