// Warping, the step pass: one frame of a hydra-style chained patch, written
// from the documented behaviour of each transform. Fixed stage order:
//   geometry (scale, rotate, kaleid, pixelate, repeat, scroll)
//   -> warp (modulate: displace the coordinates by a driving field)
//   -> source (osc, noise, voronoi, shape, gradient, summed by their mixes)
//   -> colour (hue, colorama through the palette slot, saturate, contrast,
//              thresh, posterize, invert)
//   -> feedback (blend / layer / diff / mask against the previous frame).
// Every stage has an amount where the neutral value is an exact bypass.
// The result is written into the state texture; the display pass shows it.

struct WpU {
    pal: ColorSlot,
    // aspect, time, fresh (1 = ignore the previous frame), fb_modo
    a: vec4<f32>,
    // osc_mix, noise_mix, voronoi_mix, shape_mix
    b: vec4<f32>,
    // grad_mix, osc_freq, osc_sync, osc_off
    c: vec4<f32>,
    // noise_scale, noise_speed, voro_scale, voro_blend
    d: vec4<f32>,
    // forma_lados, forma_radio, forma_suave, rot
    e: vec4<f32>,
    // rot_vel, escala, kaleid, pixel
    f: vec4<f32>,
    // repite_x, repite_y, scroll_x, scroll_y
    g: vec4<f32>,
    // scroll_vel, mod_cant, mod_fuente, mod_escala
    h: vec4<f32>,
    // hue, colorama, satura, contrasta
    i: vec4<f32>,
    // umbral, posteriza, invierte, fb_cant
    j: vec4<f32>,
    // fb_zoom, fb_giro, 0, 0
    k: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: WpU;
@group(0) @binding(1) var prev: texture_2d<f32>;
@group(0) @binding(2) var prev_smp: sampler;

const WP_TAU: f32 = 6.28318530718;

fn wp_h2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453123);
}
fn wp_h2v(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(wp_h2(p), wp_h2(p + vec2<f32>(19.19, 7.77)));
}
fn wp_h3(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3<f32>(127.1, 311.7, 74.7))) * 43758.5453123);
}

// Smooth value noise over (x, y, z) with z the time axis.
fn wp_vnoise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let a = mix(wp_h3(i), wp_h3(i + vec3<f32>(1.0, 0.0, 0.0)), w.x);
    let b = mix(wp_h3(i + vec3<f32>(0.0, 1.0, 0.0)), wp_h3(i + vec3<f32>(1.0, 1.0, 0.0)), w.x);
    let c = mix(wp_h3(i + vec3<f32>(0.0, 0.0, 1.0)), wp_h3(i + vec3<f32>(1.0, 0.0, 1.0)), w.x);
    let d = mix(wp_h3(i + vec3<f32>(0.0, 1.0, 1.0)), wp_h3(i + vec3<f32>(1.0, 1.0, 1.0)), w.x);
    return mix(mix(a, b, w.y), mix(c, d, w.y), w.z);
}
fn wp_fbm(p: vec3<f32>) -> f32 {
    return wp_vnoise(p) * 0.6 + wp_vnoise(p * 2.07 + vec3<f32>(5.2, 1.3, 0.0)) * 0.4;
}

// Jittered-cell distance field. Returns (F1 distance, id hash in 0..1).
fn wp_voro(p: vec2<f32>, t: f32) -> vec2<f32> {
    let ip = floor(p);
    let fp = fract(p);
    var best = 8.0;
    var id = 0.0;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let c = vec2<f32>(f32(i), f32(j));
            let r = wp_h2v(ip + c);
            let pt = c + 0.5 + 0.5 * sin(t + WP_TAU * r);
            let d = length(pt - fp);
            if (d < best) {
                best = d;
                id = wp_h2(ip + c + vec2<f32>(3.1, 9.7));
            }
        }
    }
    return vec2<f32>(best, id);
}

fn wp_luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

// The summed source layers at a point of the (already transformed) plane.
fn wp_source(g: vec2<f32>, t: f32) -> vec3<f32> {
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    if (u.b.x > 0.0) {
        // osc: sine stripes along x; the offset spreads the three channels apart.
        let ph = g.x * u.c.y + t * u.c.z * 3.0;
        let v = 0.5 + 0.5 * sin(ph + vec3<f32>(0.0, 2.094, 4.189) * u.c.w);
        acc = acc + v * u.b.x;
        wsum = wsum + u.b.x;
    }
    if (u.b.y > 0.0) {
        let n = wp_fbm(vec3<f32>(g * u.d.x, t * u.d.y));
        acc = acc + vec3<f32>(clamp((n - 0.2) * 1.5, 0.0, 1.0)) * u.b.y;
        wsum = wsum + u.b.y;
    }
    if (u.b.z > 0.0) {
        let v = wp_voro(g * u.d.z, t * u.d.y);
        let edge = clamp(1.0 - v.x * 1.1, 0.0, 1.0);
        acc = acc + vec3<f32>(mix(v.y, edge, u.d.w)) * u.b.z;
        wsum = wsum + u.b.z;
    }
    if (u.b.w > 0.0) {
        // shape: regular polygon, distance by the apothem trick.
        let n = max(round(u.e.x), 3.0);
        let a = atan2(g.y, g.x) + 3.14159265;
        let s = WP_TAU / n;
        let r = cos(floor(0.5 + a / s) * s - a) * length(g);
        let m = 1.0 - smoothstep(u.e.y - u.e.z, u.e.y + u.e.z, r);
        acc = acc + vec3<f32>(m) * u.b.w;
        wsum = wsum + u.b.w;
    }
    if (u.c.x > 0.0) {
        // gradient: a slow angular sweep with a radial ramp.
        let v = 0.5 + 0.5 * sin(atan2(g.y, g.x) * 2.0 + length(g) * 5.0 - t * 0.6);
        acc = acc + vec3<f32>(v) * u.c.x;
        wsum = wsum + u.c.x;
    }
    return acc / max(wsum, 1.0);
}

// The field that drives the warp, -1..1 per axis.
fn wp_field(g: vec2<f32>, kind: i32, t: f32) -> vec2<f32> {
    let s = u.h.w;
    if (kind == 0) {
        let q = g * s;
        return vec2<f32>(
            wp_fbm(vec3<f32>(q, t * u.d.y)),
            wp_fbm(vec3<f32>(q + vec2<f32>(17.3, 5.1), t * u.d.y))
        ) * 2.0 - 1.0;
    }
    if (kind == 1) {
        return vec2<f32>(sin(g.y * s * 3.0 + t), cos(g.x * s * 3.0 - t * 0.8));
    }
    if (kind == 2) {
        let ip = floor(g * s);
        return wp_h2v(ip) * 2.0 - 1.0;
    }
    // The picture drives itself: its brightness picks the direction.
    let l = wp_luma(wp_source(g, t));
    return vec2<f32>(cos(l * WP_TAU), sin(l * WP_TAU));
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    var g = (in.uv - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0);

    // --- geometry ---
    g = g / max(u.f.y, 0.01);
    let ang = (u.e.w + u.f.x * t * 0.25) * WP_TAU;
    if (ang != 0.0) {
        let c = cos(ang);
        let s = sin(ang);
        g = vec2<f32>(c * g.x - s * g.y, s * g.x + c * g.y);
    }
    g = kit_mirror(g, u.f.z);
    if (u.f.w > 0.001) {
        let n = mix(256.0, 6.0, u.f.w);
        g = (floor(g * n) + 0.5) / n;
    }
    if (u.g.x > 1.001) {
        g.x = (fract(g.x * u.g.x / aspect + 0.5) - 0.5) * aspect;
    }
    if (u.g.y > 1.001) {
        g.y = fract(g.y * u.g.y + 0.5) - 0.5;
    }
    var sdir = vec2<f32>(u.g.z, u.g.w);
    if (length(sdir) > 0.01) {
        sdir = normalize(sdir);
    } else {
        sdir = vec2<f32>(1.0, 0.0);
    }
    g = g + vec2<f32>(u.g.z, u.g.w) + sdir * (u.h.x * t * 0.3);

    // --- warp ---
    if (u.h.y > 0.0) {
        let kind = i32(clamp(round(u.h.z), 0.0, 3.0));
        g = g + wp_field(g, kind, t) * u.h.y * 0.35;
    }

    // --- source ---
    var col = wp_source(g, t);

    // --- colour ---
    let lum0 = clamp(wp_luma(col), 0.0, 1.0);
    col = mix(col, grad4(u.pal, lum0), u.i.y);
    col = kit_hue(col, u.i.x);
    let l1 = wp_luma(col);
    col = mix(vec3<f32>(l1), col, u.i.z);
    col = (col - 0.5) * u.i.w + 0.5;
    col = max(col, vec3<f32>(0.0));
    if (u.j.x > 0.001) {
        col = col * smoothstep(u.j.x - 0.04, u.j.x + 0.04, wp_luma(col));
    }
    if (u.j.y > 0.001) {
        let levels = max(floor(mix(32.0, 2.0, u.j.y)), 2.0);
        col = floor(col * levels + 0.5) / levels;
    }
    col = mix(col, vec3<f32>(1.0) - clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), u.j.z);

    // --- feedback ---
    let k = u.j.w;
    if (k > 0.0 && u.a.z < 0.5) {
        let ca = u.k.y * 0.2;
        let cc = cos(ca);
        let ss = sin(ca);
        var q = (in.uv - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0) / max(1.0 + u.k.x, 0.05);
        q = vec2<f32>(cc * q.x - ss * q.y, ss * q.x + cc * q.y);
        let puv = q / vec2<f32>(aspect, 1.0) + vec2<f32>(0.5);
        let pv = textureSampleLevel(prev, prev_smp, puv, 0.0).rgb;
        let mode = i32(clamp(round(u.a.w), 0.0, 3.0));
        if (mode == 0) {
            col = mix(col, pv, k * 0.96);
        } else if (mode == 1) {
            col = mix(col, pv, k * 0.96 * smoothstep(0.04, 0.5, wp_luma(pv)));
        } else if (mode == 2) {
            col = mix(col, abs(col - pv), k);
        } else {
            let m = 0.15 + 0.85 * clamp(wp_luma(pv) * 2.0, 0.0, 1.0);
            col = mix(col, col * m, k);
        }
    }
    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(8.0)), 1.0);
}
