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
    // the picture part's material (MaterialU in material.wgsl); mix 0 is off
    mat: MaterialU,
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
// 16 predefined composition layouts (2026-11-25): a domain transform a model
// applies before drawing, so one model reads as 16 different compositions.
// `id` is a held float rounded to 0..15. Output stays in about the input's range.
fn p_layout(p: vec2<f32>, id: f32) -> vec2<f32> {
    let k = i32(clamp(round(id), 0.0, 15.0));
    let r = length(p);
    let a = atan2(p.y, p.x);
    switch (k) {
        case 1: { return vec2<f32>(abs(p.x) - 0.45, p.y); }                 // mirrored pair
        case 2: { return vec2<f32>(p.x, abs(p.y) - 0.25); }                 // stacked pair
        case 3: { return abs(p) - vec2<f32>(0.45, 0.25); }                  // quad mirror
        case 4: { return (fract(p * 1.6 + 0.5) - 0.5) / 0.8; }              // 2x2-ish tiles
        case 5: { return (fract(p * 2.6 + 0.5) - 0.5) / 1.1; }              // 3x3 tiles
        case 6: { let w = 2.0944; let b = abs(a - w * floor(a / w) - w * 0.5); return vec2<f32>(cos(b), sin(b)) * r; }  // kaleido 3
        case 7: { let w = 1.2566; let b = abs(a - w * floor(a / w) - w * 0.5); return vec2<f32>(cos(b), sin(b)) * r; }  // kaleido 5
        case 8: { let w = 0.7854; let b = abs(a - w * floor(a / w) - w * 0.5); return vec2<f32>(cos(b), sin(b)) * r; }  // kaleido 8
        case 9: { var q = p * 2.0; q.x = q.x + 0.5 * floor(q.y + 0.5); return (fract(q + 0.5) - 0.5) * 0.9; }         // brick
        case 10: { return vec2<f32>(a / 3.1416 * 0.9, r * 1.6 - 0.6); }      // polar unwrap
        case 11: { return vec2<f32>(fract(r * 3.0) - 0.5, a / 3.1416 * 0.5); } // rings
        case 12: { let q = p_rot(p, 0.7854) * 1.8; return (fract(q + 0.5) - 0.5) * 0.9; }                          // diamonds
        case 13: { return vec2<f32>((fract(p.x * 2.5 + 0.5) - 0.5) * 0.8, p.y); }  // vertical strips
        case 14: { return vec2<f32>(p.x * 0.7, (fract(p.y * 2.0 + 0.5) - 0.5) * 0.9); } // horizontal bands
        case 15: { return p_rot(p, r * 4.0) * 1.1; }                         // spiral twist
        default: { return p; }
    }
}

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

    var m = scaffold_shade(p, in.uv);
    m = vec4<f32>(p_material(m.rgb, p, in.uv), m.a);
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
