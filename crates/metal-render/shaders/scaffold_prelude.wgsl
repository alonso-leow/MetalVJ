// Scaffold prelude (shared/scaffold.rs). Concatenated, in this order:
//   shared.wgsl (PRELUDE) + fullscreen VERTEX + kit::WGSL + this file
//   + generated `f_<name>()` accessors + the model's own body.
//
// The model body must define exactly one function:
//
//     fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32>
//
// returning (linear rgb, coverage 0..1). `p` is the picture coordinate AFTER
// CAM2D, TWIST (pulse scale, mirror fold, domain warp): centred, y down, one
// unit tall, so x spans +-aspect/2. `uv` is the untouched 0..1 screen
// coordinate (use it for things that must not move with the camera: grain,
// vignettes). Coverage is how much of the model's picture is present at the
// pixel; the rest is the `fondo` slot. Everything after that (background
// composite, LOOK, opacity, pulse brightness, alpha for layering) is done here
// so no model can forget it.
//
// Written from scratch for MetalVJ; no upstream shader code is used.

struct ScaffoldU {
    background: ColorSlot,
    s0: ColorSlot,
    s1: ColorSlot,
    s2: ColorSlot,
    // aspect, time, fondo_alpha, source picture aspect (0 = no source)
    a: vec4<f32>,
    // zoom, pan_x, pan_y, giro
    cam: vec4<f32>,
    // deriva, brillo, contraste, opacidad
    b: vec4<f32>,
    // fondo_nivel, tono, saturacion, detalle (see p_detail)
    c: vec4<f32>,
    // warp_cant, warp_freq, espejo, pulso
    tw: vec4<f32>,
    // the model's own floats, packed 4 to a vec4 in declaration order
    own: array<vec4<f32>, 12>,
};

@group(0) @binding(0) var<uniform> P: ScaffoldU;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;

fn p_time() -> f32 { return P.a.y; }
fn p_aspect() -> f32 { return max(P.a.x, 0.01); }
// How much of the authored counts this frame affords: 0.25..1.0, the tile's
// pixels over 1920x1080 (`Ctx::detail`). Scale particle/step counts by it.
fn p_detail() -> f32 { return P.c.w; }

fn p_rot(p: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a); let s = sin(a);
    return vec2<f32>(c * p.x - s * p.y, s * p.x + c * p.y);
}

// Integer-mixed hashes (no sin): safe at any coordinate, see shared.wgsl.
fn p_hash(p: vec2<f32>) -> f32 { return lattice(p); }
fn p_hash2(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(lattice(p), lattice(p + vec2<f32>(37.0, 91.0)));
}
fn p_noise(p: vec2<f32>) -> f32 { return quintic_noise(p); }
fn p_fbm(p: vec2<f32>, octaves: i32) -> f32 { return lattice_fbm(p, octaves); }

// Film-grain-like per-pixel noise, -0.5..0.5, changes every ~1/24 s.
fn p_grain(uv: vec2<f32>, t: f32) -> f32 {
    return p_hash(floor(uv * 1024.0) + vec2<f32>(floor(t * 24.0) * 17.0, 3.0)) - 0.5;
}

// Soft step centred on `edge` with half width `w` (w > 0): 0 below, 1 above.
fn p_edge(d: f32, edge: f32, w: f32) -> f32 {
    return smoothstep(edge - w, edge + w, d);
}

// Slot colour by index (0..2) at t in 0..1.
fn p_pal(k: u32, t: f32) -> vec3<f32> {
    if (k == 0u) { return grad4(P.s0, t); }
    if (k == 1u) { return grad4(P.s1, t); }
    return grad4(P.s2, t);
}
// Blend of two slot positions, convenient for "colour A to colour B".
fn p_pal2(k: u32, t: f32, k2: u32, t2: f32, m: f32) -> vec3<f32> {
    return mix(p_pal(k, t), p_pal(k2, t2), m);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = p_aspect();
    let t = p_time();
    var p = kit_cam2d(in.uv, aspect, P.cam.x, vec2<f32>(P.cam.y, P.cam.z), P.cam.w, P.b.x, t);
    p = p / (1.0 + 0.25 * P.tw.w);
    p = kit_mirror(p, P.tw.z);
    p = kit_warp(p, P.tw.x, P.tw.y, t);

    let m = scaffold_shade(p, in.uv);
    let cov = clamp(m.a, 0.0, 1.0);
    let lvl = P.c.x;
    let bg = backdrop_from_uv(P.background, in.uv, aspect) * lvl;
    var col = mix(bg, m.rgb, cov) * (1.0 + 0.5 * P.tw.w);
    col = kit_look(col, P.b.y, P.b.z, P.c.y, P.c.z);
    var alpha = P.b.w;
    if (P.a.z > 0.5) {
        alpha = alpha * max(cov, lvl);
    }
    return vec4<f32>(col, alpha);
}
