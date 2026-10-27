// Catenoide: a ray-cast tunnel whose wall profile is a catenoid,
//   r(z) = radio * cosh(z / c),  z wrapped to [-P/2, P/2],
// so the neck (radius `radio`) repeats every P units and flares out between.
// The cross-section is a radial function rho(angle) (circle / square / hex /
// star, morphed continuously by `seccion`), twisted along z. The wall is the
// level set  d(p) = radio*cosh(..)*rho(angle) - |p.xy| = 0; the camera lives
// inside (d > 0) and sphere-traces it. Mirrored on the CPU by
// `catenoid_radius` / `section` in models/catenoide.rs.

struct CatU {
    wall: ColorSlot,
    neon: ColorSlot,
    background: ColorSlot,
    // aspect, time, fondo_alpha, camera z
    a: vec4<f32>,
    // cuello, radio, seccion, torsion
    s: vec4<f32>,
    // espiral, rodillo, tan(fov/2), 0
    m: vec4<f32>,
    // textura_tipo, escala, vel, anillos_dens
    tx: vec4<f32>,
    // anillos_ancho, panel_dens, luces_dens, luces_pulso
    t2: vec4<f32>,
    // niebla, niebla_color_mix, bordes_neon, 0
    f: vec4<f32>,
    // brillo, contraste, opacidad, fondo_nivel
    l: vec4<f32>,
    // tono, saturacion, 0, 0
    l2: vec4<f32>,
    eye: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    fwd: vec4<f32>,
    // The `body` part's picture and `mat_*`, one `MaterialU` — see
    // material.wgsl, prepended ahead of this file.
    mat_body: MaterialU,
};

@group(0) @binding(0) var<uniform> u: CatU;
@group(0) @binding(1) var surf_body_tex: texture_2d<f32>;
@group(0) @binding(2) var surf_body_samp: sampler;

const CA_TAU: f32 = 6.2831853;
const CA_P: f32 = 8.0;
const CA_TMAX: f32 = 44.0;

fn ca_atan(y: f32, x: f32) -> f32 {
    if (abs(x) + abs(y) < 1e-7) { return 0.0; }
    return atan2(y, x);
}

fn ca_poly(a: f32, n: f32, ins: f32) -> f32 {
    let seg = CA_TAU / n;
    let ph = (fract(a / seg) - 0.5) * seg;
    return ins / max(cos(ph), 0.2);
}

fn ca_star(a: f32) -> f32 {
    let s = abs(fract(a * 5.0 / CA_TAU) * 2.0 - 1.0);
    return mix(0.55, 1.05, s);
}

fn ca_rho_kind(a: f32, k: i32) -> f32 {
    if (k <= 0) { return 1.0; }
    if (k == 1) { return ca_poly(a, 4.0, 0.85); }
    if (k == 2) { return ca_poly(a, 6.0, 0.92); }
    return ca_star(a);
}

fn ca_rho(a: f32) -> f32 {
    let x = clamp(u.s.z, 0.0, 3.0);
    let i = min(floor(x), 2.0);
    let f = smoothstep(0.0, 1.0, x - i);
    return mix(ca_rho_kind(a, i32(i)), ca_rho_kind(a, i32(i) + 1), f);
}

fn ca_prof(z: f32) -> f32 {
    let zw = z - CA_P * floor(z / CA_P + 0.5);
    return u.s.y * cosh(min(abs(zw) / max(u.s.x, 0.05), 4.0));
}

fn ca_ang(p: vec3<f32>) -> f32 {
    return ca_atan(p.y, p.x) + u.s.w * 0.3 * p.z + u.m.y * u.a.y;
}

fn ca_d(p: vec3<f32>) -> f32 {
    return ca_prof(p.z) * ca_rho(ca_ang(p)) - length(p.xy);
}

fn ca_normal(p: vec3<f32>) -> vec3<f32> {
    let h = 0.004;
    let k = vec2<f32>(1.0, -1.0);
    let n = k.xyy * ca_d(p + k.xyy * h) + k.yyx * ca_d(p + k.yyx * h)
          + k.yxy * ca_d(p + k.yxy * h) + k.xxx * ca_d(p + k.xxx * h);
    let l = length(n);
    if (l < 1e-9) { return vec3<f32>(0.0, 0.0, -1.0); }
    return n / l;
}

fn ca_h2(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q = q + dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

// Texture sample: (value 0..1, edge 0..1, emission 0..1).
fn ca_tex(turns: f32, zt: f32) -> vec3<f32> {
    let sc = max(u.tx.y, 0.05);
    let kind = clamp(u.tx.x, 0.0, 3.0);
    var val = 0.0; var edge = 0.0; var emit = 0.0;

    // Rings
    let w0 = max(0.0, 1.0 - abs(kind));
    if (w0 > 0.0) {
        let f = fract(zt * u.tx.w * sc);
        let wd = clamp(u.t2.x, 0.05, 0.9);
        let r = 1.0 - smoothstep(wd - 0.03, wd, f);
        let e = exp(-min(f, abs(f - wd)) * 30.0);
        val = val + w0 * r; edge = edge + w0 * e;
    }
    // Panels
    let w1 = max(0.0, 1.0 - abs(kind - 1.0));
    if (w1 > 0.0) {
        let nu = max(floor(u.t2.y + 0.5), 2.0);
        let cu = turns * nu;
        let cv = zt * nu * sc / max(CA_TAU * u.s.y, 0.5);
        let id = vec2<f32>(floor(cu) - nu * floor(floor(cu) / nu), floor(cv));
        let fu = fract(cu); let fv = fract(cv);
        let seam = min(min(fu, 1.0 - fu), min(fv, 1.0 - fv));
        let h = ca_h2(id);
        val = val + w1 * (0.3 + 0.7 * h) * smoothstep(0.0, 0.08, seam);
        edge = edge + w1 * (1.0 - smoothstep(0.0, 0.07, seam));
    }
    // Lights
    let w2 = max(0.0, 1.0 - abs(kind - 2.0));
    if (w2 > 0.0) {
        let cu = turns * 8.0;
        let cv = zt * u.t2.z * sc;
        let id = vec2<f32>(floor(cu) - 8.0 * floor(floor(cu) / 8.0), floor(cv));
        let d = vec2<f32>(fract(cu) - 0.5, (fract(cv) - 0.5) * 0.7);
        let h = ca_h2(id + vec2<f32>(7.0, 3.0));
        let pulse = max(0.0, 1.0 + u.t2.w * sin(u.a.y * 5.0 + h * CA_TAU) * 1.0 - u.t2.w * 0.5);
        let g = exp(-dot(d, d) * 40.0) * select(0.0, 1.0, h > 0.3) * pulse;
        val = val + w2 * 0.25;
        emit = emit + w2 * g;
    }
    // Stripes
    let w3 = max(0.0, 1.0 - abs(kind - 3.0));
    if (w3 > 0.0) {
        let ns = max(floor(6.0 * sc + 0.5), 2.0);
        let f = fract(turns * ns);
        let s = abs(f - 0.5) * 2.0;
        val = val + w3 * smoothstep(0.35, 0.65, s);
        edge = edge + w3 * exp(-abs(s - 0.5) * 20.0);
    }
    return vec3<f32>(val, edge, emit);
}

fn ca_shade(p: vec3<f32>, rd: vec3<f32>, t: f32, fogc: vec3<f32>, suv: vec2<f32>) -> vec4<f32> {
    let n0 = ca_normal(p);
    let ang = ca_ang(p);
    let turns = ang / CA_TAU + u.m.x * p.z;
    let zt = p.z + u.tx.z * u.a.y;
    let tx = ca_tex(turns, zt);
    let depth = 1.0 - exp(-t * 0.1);
    let tt = clamp(0.02 + 0.4 * tx.x + 0.3 * depth, 0.0, 1.0);
    let base0 = grad4(u.wall, tt);

    // The shared material, off at mix 0 (a migrated row): the wall's slot
    // colour is what a kind or a `tex.body` picture replaces, and its
    // bump/glitch perturb the normal the lamp term below reads.
    let V = -rd;
    let shaded = material_apply(
        u.mat_body, surf_body_tex, surf_body_samp, base0, p, n0, suv, V, u.a.y,
    );
    let n = shaded.normal;
    let base = shaded.albedo;

    let lamp = clamp(dot(n, -rd), 0.0, 1.0);
    let light = (0.25 + 0.9 * lamp) / (1.0 + 0.04 * t * t * 0.25);
    // Crease of the cross-section: where rho(angle) kinks.
    let e = 0.04;
    let cr = clamp(abs(ca_rho(ang + e) + ca_rho(ang - e) - 2.0 * ca_rho(ang)) / e * 0.35, 0.0, 1.0);
    let ncol = grad4(u.neon, 0.5 + 0.5 * sin(p.z * 0.25 + u.a.y * 0.3));
    var col = base * light;
    col = col + ncol * (tx.y * 0.8 + cr) * u.f.z + ncol * tx.z * 2.0;
    let fog = 1.0 - exp(-u.f.x * t * 0.12);
    return vec4<f32>(mix(col, fogc, fog), fog);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let ndc = vec2<f32>((in.uv.x * 2.0 - 1.0) * aspect, 1.0 - in.uv.y * 2.0);
    let rd = normalize(u.fwd.xyz + u.right.xyz * ndc.x * u.m.z + u.up.xyz * ndc.y * u.m.z);
    let ro = u.eye.xyz;
    let bgc = backdrop_from_uv(u.background, in.uv, aspect) * u.l.w;
    let far = grad4(u.wall, 0.85) * 0.5;
    let fogc = mix(bgc, far, clamp(u.f.y, 0.0, 1.0));

    var t = 0.0;
    var hit = false;
    for (var i = 0; i < 110; i = i + 1) {
        let d = ca_d(ro + rd * t);
        if (d < 0.0015 * (1.0 + t)) { hit = true; break; }
        t = t + max(d * 0.62, 0.004);
        if (t > CA_TMAX) { break; }
    }
    var col = fogc;
    var cover = 0.0;
    if (hit) {
        let sh = ca_shade(ro + rd * t, rd, t, fogc, in.uv);
        col = sh.rgb;
        cover = 1.0 - sh.a;
    }
    col = kit_look(col, u.l.x, u.l.y, u.l2.x, u.l2.y);
    var alpha = u.l.z;
    if (u.a.z > 0.5) { alpha = alpha * max(cover, u.l.w); }
    return vec4<f32>(col, alpha);
}
