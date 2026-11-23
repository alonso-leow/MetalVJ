// Objeto: a Gielis superformula object, ray-marched.
//
// The surface is the spherical product of two superformula curves,
//   P(th, ph) = ( r1(th) cos th * r2(ph) cos ph,
//                 r1(th) cos th * r2(ph) sin ph,
//                 r1(th) sin th ),   th in [-pi/2, pi/2], ph in [-pi, pi].
// In the half-plane at azimuth ph the surface is the curve r1 with its
// horizontal axis scaled by r2(ph). So membership is exact and cheap: for a
// point q, take rho = |q.xy|, ph = atan2(q.y, q.x), map (rho, z) back to
// (rho / r2(ph), z) and compare its length with r1 of its polar angle. That is
// the implicit function below; its value times min(r2, 1) is a (conservative)
// distance estimate, which sphere tracing walks with a safety factor `lip`.
// No mesh, no vertex stage: one fullscreen pass.
//
// The formula in `ob_sf` and the clamps in it are mirrored by `gielis()` in
// models/objeto.rs, which the tests check. Every pow is exp(n * log(x)) on a
// floored x, and the result is clamped, so no parameter can make it NaN/Inf.

struct ObjU {
    sup: ColorSlot,
    background: ColorSlot,
    // aspect, time, fondo_alpha, bound (object units per world unit)
    a: vec4<f32>,
    // m1, n1_1, n2_1, n3_1
    g1: vec4<f32>,
    // a1, b1, zmax (base half-height), lip
    g1b: vec4<f32>,
    // m2, n1_2, n2_2, n3_2
    g2: vec4<f32>,
    // a2, b2, torsion, taper
    g2b: vec4<f32>,
    // stretch xyz, pixel angle
    d: vec4<f32>,
    // light direction xyz, luz_dura
    light: vec4<f32>,
    // ambiente, especular, brillo_borde, niebla
    mat: vec4<f32>,
    // tex_escala, tex_tipo, tex_contraste, tex_vel
    tex: vec4<f32>,
    // brillo, contraste, opacidad, fondo_nivel
    l: vec4<f32>,
    // tono, saturacion, tan(fov/2), cam distance
    l2: vec4<f32>,
    eye: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    fwd: vec4<f32>,
    // The `body` part's picture and `mat_*`, one `MaterialU` — see
    // material.wgsl, prepended ahead of this file.
    mat_body: MaterialU,
};

@group(0) @binding(0) var<uniform> u: ObjU;
// The picture laid on `body` (materials M5/M7), bound by
// `shared::surface::layout_entries`.
@group(0) @binding(1) var surf_body_tex: texture_2d<f32>;
@group(0) @binding(2) var surf_body_samp: sampler;

const OB_PI: f32 = 3.14159265;

fn ob_sf(phi: f32, g: vec4<f32>, ab: vec2<f32>) -> f32 {
    let arg = g.x * phi * 0.25;
    let a = max(ab.x, 0.05);
    let b = max(ab.y, 0.05);
    let t1 = max(abs(cos(arg)) / a, 1e-6);
    let t2 = max(abs(sin(arg)) / b, 1e-6);
    let s = exp(max(g.z, 0.05) * log(t1)) + exp(max(g.w, 0.05) * log(t2));
    let e = clamp(-log(max(s, 1e-9)) / max(g.y, 0.05), -6.0, 2.0);
    return exp(e);
}

fn ob_atan(y: f32, x: f32) -> f32 {
    if (abs(x) + abs(y) < 1e-7) { return 0.0; }
    return atan2(y, x);
}

fn ob_rot(v: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a); let s = sin(a);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

// World point (unit-radius object, y up) to the undeformed object space
// (z is the pole axis): undo the normalisation, stretch, torsion and taper.
fn ob_inv(p: vec3<f32>) -> vec3<f32> {
    var q = p * u.a.w;
    q = vec3<f32>(q.x, -q.z, q.y);
    q = q / max(u.d.xyz, vec3<f32>(0.05));
    let zn = clamp(q.z / max(u.g1b.z, 0.01), -1.5, 1.5);
    let xy = ob_rot(q.xy, -u.g2b.z * zn * OB_PI) / max(1.0 - u.g2b.w * zn, 0.1);
    return vec3<f32>(xy, q.z);
}

// Implicit value in object space: <0 inside. Returns (value, r2, theta).
fn ob_field(q: vec3<f32>) -> vec3<f32> {
    let rho = length(q.xy);
    let ph = ob_atan(q.y, q.x);
    let r2 = max(ob_sf(ph, u.g2, u.g2b.xy), 0.02);
    let uu = vec2<f32>(rho / r2, q.z);
    let th = ob_atan(uu.y, uu.x);
    let r1 = ob_sf(th, u.g1, u.g1b.xy);
    return vec3<f32>((length(uu) - r1) * min(r2, 1.0), r2, th);
}

fn ob_map(p: vec3<f32>) -> f32 {
    return ob_field(ob_inv(p)).x * u.g1b.w / u.a.w;
}

fn ob_normal(p: vec3<f32>) -> vec3<f32> {
    let h = 0.002;
    let k = vec2<f32>(1.0, -1.0);
    let n = k.xyy * ob_map(p + k.xyy * h) + k.yyx * ob_map(p + k.yyx * h)
          + k.yxy * ob_map(p + k.yxy * h) + k.xxx * ob_map(p + k.xxx * h);
    let l = length(n);
    if (l < 1e-9) { return vec3<f32>(0.0, 1.0, 0.0); }
    return n / l;
}

fn ob_h3(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.3183099 + vec3<f32>(0.1, 0.2, 0.3));
    q = q * 17.0;
    return fract(q.x * q.y * q.z * (q.x + q.y + q.z));
}
fn ob_noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    var v = 0.0;
    for (var k = 0; k < 8; k = k + 1) {
        let o = vec3<f32>(f32(k & 1), f32((k >> 1) & 1), f32((k >> 2) & 1));
        let c = mix(1.0 - w, w, o);
        v = v + ob_h3(i + o) * c.x * c.y * c.z;
    }
    return v;
}

// Surface pattern 0..1 from the object-space point.
fn ob_tex(q: vec3<f32>, th: f32, t: f32) -> f32 {
    let esc = max(u.tex.x, 0.1);
    let kind = i32(floor(clamp(u.tex.y, 0.0, 4.0) + 0.5));
    let vv = th / OB_PI + 0.5;
    let uu = ob_atan(q.y, q.x) / (2.0 * OB_PI) + 0.5;
    let sc = t * u.tex.w * 0.2;
    if (kind == 0) { return 0.5 + 0.5 * sin(2.0 * OB_PI * (vv * esc + sc)); }
    if (kind == 1) { return 0.5 + 0.5 * sin(2.0 * OB_PI * (uu * floor(esc + 0.5) + sc)); }
    if (kind == 2) {
        let c = floor(vv * esc + sc) + floor(uu * floor(esc + 0.5));
        return abs(c - 2.0 * floor(c * 0.5));
    }
    if (kind == 3) {
        let zm = max(u.g1b.z, 0.01);
        return ob_noise(q / zm * esc * 0.5 + vec3<f32>(0.0, 0.0, sc * 4.0));
    }
    return 0.5 + 0.5 * sin(2.0 * OB_PI * (uu * floor(esc + 0.5) + vv * esc + sc));
}

fn ob_shade(p: vec3<f32>, rd: vec3<f32>, t: f32, uv: vec2<f32>) -> vec3<f32> {
    var n = ob_normal(p);
    let q = ob_inv(p);
    let fld = ob_field(q);
    let hn = clamp(0.5 + 0.5 * q.z / max(u.g1b.z, 0.01), 0.0, 1.0);
    var tt = 0.55 * (0.5 + 0.5 * ob_normal(p).y) + 0.45 * hn;
    tt = clamp(tt + (ob_tex(q, fld.z, u.a.y) - 0.5) * u.tex.z, 0.0, 1.0);
    let V = -rd;

    // The material is the surface: a `mat.*` kind seasons or replaces the
    // albedo and hands back a perturbed normal, so the same grid that is line
    // geometry in `models::matrix` lands on any 3D model here. `q` is object
    // space, which is where the grid's line pattern reads.
    let shaded = material_apply(
        u.mat_body, surf_body_tex, surf_body_samp, grad4(u.sup, tt), q, n, uv, normalize(V), u.a.y,
    );
    var base = shaded.albedo;
    n = shaded.normal;

    let L = normalize(u.light.xyz);
    let dif = dot(n, L);
    let dura = clamp(u.light.w, 0.0, 1.0);
    let soft = clamp(dif * 0.5 + 0.5, 0.0, 1.0);
    let lam = mix(soft * soft, smoothstep(0.0, 0.08, dif), dura);
    let H = normalize(L + V);
    var spec_amt = u.mat.y;
    var spec_exp = mix(8.0, 160.0, dura);
    if (u.mat_body.b.x > 0.0) {
        spec_amt = mix(u.mat.y, 1.0, clamp(shaded.metalness, 0.0, 1.0));
        spec_exp = mix(4.0, 160.0, 1.0 - clamp(shaded.roughness, 0.0, 1.0));
    }
    let spec = pow(max(dot(n, H), 0.0), spec_exp) * spec_amt * step(0.0, dif);
    let rim = pow(1.0 - clamp(dot(n, V), 0.0, 1.0), 3.0) * u.mat.z;
    var col = base * (u.mat.x + (1.0 - u.mat.x) * lam);
    col = col + vec3<f32>(spec) + grad4(u.sup, min(tt + 0.35, 1.0)) * rim;
    let fog = clamp(u.mat.w * (t - (u.l2.w - 1.0)) * 0.45, 0.0, 1.0);
    return mix(col, backdrop_from_uv(u.background, vec2<f32>(0.5), u.a.x) * u.l.w, fog);
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

    // Bounding sphere (object units are normalised to radius 1).
    let R = 1.1;
    let b = dot(ro, rd);
    let c = dot(ro, ro) - R * R;
    let h2 = b * b - c;
    if (h2 > 0.0) {
        let sq = sqrt(h2);
        let tend = -b + sq;
        var t = max(-b - sq, 0.0);
        var dmin = 1e9;
        var tm = t;
        var hit = false;
        let px = max(u.d.w, 1e-5);
        if (tend > 0.0) {
            for (var i = 0; i < 112; i = i + 1) {
                let d = ob_map(ro + rd * t);
                let rr = d / (max(t, 0.1) * px * u.g1b.w);
                if (rr < dmin) { dmin = rr; tm = t; }
                if (d < 0.0006 + t * px * 0.1 * u.g1b.w) { hit = true; break; }
                t = t + max(d, 0.0004);
                if (t > tend) { break; }
            }
            if (hit || dmin < 1.0) {
                let ts = select(tm, t, hit);
                let sc = ob_shade(ro + rd * ts, rd, ts, in.uv);
                cover = select(0.5 * (1.0 - smoothstep(0.0, 1.0, dmin)), 1.0, hit);
                col = mix(bgc, sc, cover);
            }
        }
    }
    col = kit_look(col, u.l.x, u.l.y, u.l2.x, u.l2.y);
    var alpha = u.l.z;
    if (u.a.z > 0.5) { alpha = alpha * max(cover, u.l.w); }
    return vec4<f32>(col, alpha);
}
