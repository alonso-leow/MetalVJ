// Parallax worlds (`parallax-cueva`, `parallax-cielo`, `parallax-campos`,
// `parallax-ciudad`) — one shader, four worlds, chosen by `u.f0.y`.
//
// The picture is five flat layers of procedural 2D "stickers" standing in a
// real 3D scene: layer i is the plane z = -depth_i. Each pixel casts a ray from
// the camera, hits every plane, and reads the layer there in *layer units*
// `s = (p.xy + scroll) / depth` — so a camera that moves (the camera block, the
// `vaiven` bob, head tracking's cam_x/y/z) slides near layers more than far
// ones, which is the parallax, without anything faking it. The scroll is a
// world-space offset per layer computed on the CPU (time * speed * direction),
// so a layer's apparent speed is its own speed over its depth.
//
// Every sticker is drawn in cells along x with a lattice hash per cell, so the
// strip never repeats and never ends: there is no texture to tile and no seam.
//
// Colour: `fondo` is the sky/backdrop (vertical), `capas` the silhouettes (far
// -> near), `objetos` the secondary parts (moss, caps, birds, flowers, fence,
// rubble), `luces` every light source (crystals, sun, stars, windows, neon),
// `fog_c` the atmosphere layers fade into with distance.

struct ParallaxUniform {
    background: ColorSlot,
    layers: ColorSlot,
    objects: ColorSlot,
    lights: ColorSlot,
    fog_c: ColorSlot,
    // eye.xyz, time
    eye: vec4<f32>,
    // forward.xyz, tan(fov/2)
    fwd: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    // aspect, world (0 cueva, 1 cielo, 2 campos, 3 ciudad), fondo_alpha, seed
    f0: vec4<f32>,
    // scroll offsets (world units) of layers 0..3, and their depths
    off0: vec4<f32>,
    dep0: vec4<f32>,
    // layer 4 offset, layer 4 depth, pixel size in layer units, frame height px
    f1: vec4<f32>,
    // densidad, tamano, luz, halo
    look: vec4<f32>,
    // parpadeo, niebla, pixel, horizonte
    look2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: ParallaxUniform;

const NL: i32 = 5;

struct Smp {
    // premultiplied colour and coverage of this layer
    col: vec3<f32>,
    a: f32,
    // additive light (glow from light sources), never fogged away entirely
    glow: vec3<f32>,
};

// ---------------------------------------------------------------- helpers --

fn seed() -> f32 { return floor(u.f0.w); }
fn tim() -> f32 { return u.eye.w; }
fn dens() -> f32 { return u.look.x; }
fn size() -> f32 { return max(u.look.y, 0.05); }
fn luz() -> f32 { return u.look.z; }
fn halo() -> f32 { return u.look.w; }

// A hash per integer cell and channel, stable across the whole strip.
fn hh(id: f32, ch: f32) -> f32 {
    return lattice3(vec3<f32>(id, ch, seed()));
}

fn cover(d: f32) -> f32 {
    return clamp(0.5 - d / max(u.f1.z * 1.3, 1e-5), 0.0, 1.0);
}

fn put(s: ptr<function, Smp>, c: vec3<f32>, a: f32) {
    (*s).col = (*s).col * (1.0 - a) + c * a;
    (*s).a = (*s).a * (1.0 - a) + a;
}

// A light: solid core where d < 0, a soft halo outside. `h` picks the
// colour along `luces` and the flicker phase.
fn light(s: ptr<function, Smp>, d: f32, h: f32, r: f32, gain: f32) {
    let fl = 1.0 - u.look2.x * 0.6 * (0.5 + 0.5 * sin(tim() * (2.0 + 9.0 * h) + h * 61.0));
    let c = grad4(u.lights, h) * luz() * fl * gain;
    let core = cover(d);
    put(s, min(c, vec3<f32>(1.0)), core);
    let hl = halo() * exp(-max(d, 0.0) / max(r, 1e-4)) * (1.0 - core);
    (*s).glow = (*s).glow + c * hl * 0.55;
}

fn sd_box(p: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = abs(p) - b;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
}

fn sd_seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// Isosceles triangle, apex at the origin, base at y = q.y (iq).
fn sd_tri(p0: vec2<f32>, q: vec2<f32>) -> f32 {
    let p = vec2<f32>(abs(p0.x), p0.y);
    let a = p - q * clamp(dot(p, q) / dot(q, q), 0.0, 1.0);
    let b = p - q * vec2<f32>(clamp(p.x / q.x, 0.0, 1.0), 1.0);
    let sg = -sign(q.y);
    let d = min(vec2<f32>(dot(a, a), sg * (p.x * q.y - p.y * q.x)),
                vec2<f32>(dot(b, b), sg * (p.y - q.y)));
    return -sqrt(d.x) * sign(d.y);
}

fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

fn nz(x: f32, row: f32) -> f32 {
    return lattice_fbm(vec2<f32>(x, row + seed() * 3.7), 4);
}

// Where the ground of a layer sits: near layers lower, far ones near the
// horizon, all shifted by `horizonte`.
fn ground(li: i32) -> f32 {
    let lt = f32(li) / 4.0;
    return -0.04 - 0.36 * lt * lt - 0.06 * lt + u.look2.w * 0.25;
}

fn layer_col(lt: f32) -> vec3<f32> {
    return grad4(u.layers, lt);
}

// ------------------------------------------------------------------ cueva --

// The cave's ceiling and floor per layer: the far wall is a narrow slot of
// light, the near layers frame the picture with heavy dark masses.
fn cave_ceil(x: f32, li: i32) -> f32 {
    let lt = f32(li) / 4.0;
    let k = size();
    let row = f32(li) * 17.0;
    let amp = mix(0.10, 0.34, lt) + 0.12 * step(3.5, f32(li));
    return mix(0.30, 0.52, lt) - amp * nz(x * mix(1.4, 2.2, lt) / k, row) + u.look2.w * 0.1;
}

fn cave_floor(x: f32, li: i32) -> f32 {
    let lt = f32(li) / 4.0;
    let k = size();
    let row = f32(li) * 17.0;
    let amp = mix(0.10, 0.30, lt);
    return mix(-0.22, -0.46, lt) + 0.06 * step(3.5, f32(li)) + amp * nz(x * mix(1.3, 2.0, lt) / k, row + 5.0) + u.look2.w * 0.2;
}

// A hanging spike from `top` down `len`: tapered (not straight-sided),
// slightly bent, with knuckles along it. Distance is approximate.
fn stalactite(s: vec2<f32>, top: vec2<f32>, len: f32, w: f32, id: f32) -> f32 {
    let t = clamp((top.y - s.y) / len, 0.0, 1.0);
    let bend = (hh(id, 91.0) - 0.5) * w * 0.6 * t * t;
    let knuckle = 1.0 + 0.12 * sin(t * (9.0 + 6.0 * hh(id, 92.0)) + id);
    let half_w = w * pow(1.0 - t, 1.35) * knuckle;
    let dx = abs(s.x - top.x - bend) - half_w;
    let dy = max(s.y - top.y, (top.y - len) - s.y);
    return max(dx * 0.9, dy);
}

fn cueva(s: vec2<f32>, li: i32) -> Smp {
    var o: Smp;
    o.col = vec3<f32>(0.0); o.a = 0.0; o.glow = vec3<f32>(0.0);
    let lt = f32(li) / 4.0;
    let row = f32(li) * 17.0;
    let x = s.x;
    let base = layer_col(lt);
    let moss = grad4(u.objects, fract(lt * 0.7 + 0.1));
    let k = size();

    // ceiling and floor bands with lumpy edges
    let ce = cave_ceil(x, li) - 0.012 * sin(x * 23.0 + row);
    let fl = cave_floor(x, li) + 0.01 * sin(x * 19.0 + row);

    // stalactites / stalagmites
    let cw = 0.10 * k * (0.8 + 0.25 * lt);
    let cid = floor(x / cw);
    var spikes = 1e3;
    for (var j = -1; j <= 1; j = j + 1) {
        let id = cid + f32(j);
        let h = hh(id, row + 1.0);
        if (h < (0.35 + 0.6 * dens()) * (0.35 + 0.65 * lt)) {
            let cx = (id + 0.2 + 0.6 * hh(id, row + 2.0)) * cw;
            let ceil_here = cave_ceil(cx, li);
            let len = (0.05 + 0.22 * hh(id, row + 3.0)) * k;
            let w = cw * (0.22 + 0.25 * hh(id, row + 4.0));
            spikes = min(spikes, stalactite(s, vec2<f32>(cx, ceil_here + 0.03), len + 0.03, w, id + row));
        }
        let h2 = hh(id, row + 9.0);
        if (h2 < 0.25 * dens() + 0.1) {
            let cx = (id + 0.3 + 0.4 * hh(id, row + 10.0)) * cw;
            let fl_here = cave_floor(cx, li);
            let len = (0.04 + 0.12 * hh(id, row + 11.0)) * k;
            let w = cw * 0.3;
            let sm = stalactite(vec2<f32>(s.x, 2.0 * fl_here - s.y), vec2<f32>(cx, fl_here - 0.03), len + 0.03, w, id + row + 5.0);
            spikes = min(spikes, sm);
        }
    }
    let d_ceil = s.y - ce;          // >0 inside the ceiling
    let d_floor = fl - s.y;         // >0 inside the floor
    let d_rock = min(-d_ceil, -d_floor); // sdf-ish, <0 inside either band
    let dall = smin(d_rock, spikes, 0.012);

    // rock: darker deep inside, a soft moss rim only where light reaches —
    // patches along the edges, and the spike tips lit from the crystals below
    let inner = smoothstep(0.0, -0.08 * k, dall);
    var rock_col = base * mix(1.0, 0.55, inner);
    let lit_patch = smoothstep(0.45, 0.68, nz(x * 5.0 / k, row + 90.0));
    let rim = smoothstep(-0.02 * k, 0.0, dall) * lit_patch;
    rock_col = mix(rock_col, moss * (0.45 + 0.3 * (1.0 - lt)), rim * 0.55);
    let below_ceiling = d_ceil < 0.0 && spikes < 0.0;
    if (below_ceiling) {
        let tip = clamp((ce - s.y) / (0.25 * k), 0.0, 1.0);
        rock_col = mix(rock_col, grad4(u.lights, 0.3) * 0.35 * min(luz(), 1.5), smoothstep(0.35, 1.0, tip) * 0.45);
    }
    put(&o, rock_col, cover(dall));
    o.glow = o.glow + moss * rim * 0.05 * luz() * smoothstep(0.015 * k, 0.0, abs(dall));

    // hanging glow vines from the ceiling (middle layers)
    if (li >= 1 && li <= 3) {
        let vw = 0.23 * k;
        let vid = floor(x / vw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = vid + f32(j);
            if (hh(id, row + 40.0) < 0.5 * dens()) {
                let cx = (id + 0.5) * vw + (hh(id, row + 41.0) - 0.5) * vw * 0.6;
                let c0 = cave_ceil(cx, li);
                let len = (0.12 + 0.3 * hh(id, row + 42.0)) * k;
                let sway = 0.01 * sin(tim() * 1.3 + id) * (c0 - s.y) / max(len, 0.01);
                let dv = sd_seg(s, vec2<f32>(cx, c0), vec2<f32>(cx + sway * 3.0, c0 - len)) - 0.0035 * k;
                put(&o, moss * 0.7, cover(dv));
                // beads of light along the vine
                let by = (c0 - s.y) / max(len, 1e-3);
                if (by > 0.0 && by < 1.0) {
                    let bi = floor(by * 6.0);
                    let bc = vec2<f32>(cx + sway * 3.0 * (bi + 0.5) / 6.0, c0 - (bi + 0.5) / 6.0 * len);
                    light(&o, length(s - bc) - 0.006 * k, hh(id + bi, row + 43.0), 0.02 * k, 0.9);
                }
            }
        }
    }

    // crystals on the floor: clusters of shards, the main light sources
    let kw = 0.42 * k;
    let kid = floor(x / kw);
    for (var j = -1; j <= 1; j = j + 1) {
        let id = kid + f32(j);
        if (hh(id, row + 50.0) < 0.25 + 0.5 * dens()) {
            let cx = (id + 0.25 + 0.5 * hh(id, row + 51.0)) * kw;
            let fy = cave_floor(cx, li) - 0.01;
            let hc = hh(id, row + 52.0);
            var dc = 1e3;
            for (var n = 0; n < 3; n = n + 1) {
                let fn_ = f32(n);
                let ang = (fn_ - 1.0) * 0.45 + (hh(id, row + 53.0 + fn_) - 0.5) * 0.3;
                let ln = (0.05 + 0.09 * hh(id, row + 56.0 + fn_)) * k * (1.0 - abs(fn_ - 1.0) * 0.35);
                let w = 0.012 * k;
                var q = s - vec2<f32>(cx + (fn_ - 1.0) * w * 1.4, fy);
                let ca = cos(ang); let sa = sin(ang);
                q = vec2<f32>(ca * q.x - sa * q.y, sa * q.x + ca * q.y);
                // a shard: a box with a pointed top
                let body = sd_box(q - vec2<f32>(0.0, ln * 0.4), vec2<f32>(w, ln * 0.4));
                let tip = sd_tri(vec2<f32>(q.x, -(q.y - ln * 1.15)), vec2<f32>(w, ln * 0.35));
                dc = min(dc, min(body, tip));
            }
            light(&o, dc, hc, 0.07 * k, 1.0);
        }
    }

    // glowing mushrooms on the floor of the nearer layers
    if (li >= 2) {
        let mw = 0.27 * k;
        let mid = floor(x / mw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = mid + f32(j);
            if (hh(id, row + 60.0) < 0.2 + 0.6 * dens()) {
                let cx = (id + 0.2 + 0.6 * hh(id, row + 61.0)) * mw;
                let fy = cave_floor(cx, li) - 0.01;
                let hgt = (0.04 + 0.07 * hh(id, row + 62.0)) * k;
                let cr = (0.025 + 0.03 * hh(id, row + 63.0)) * k;
                let bob = 0.003 * sin(tim() * 1.7 + id);
                let stem = sd_box(s - vec2<f32>(cx, fy + hgt * 0.5), vec2<f32>(0.006 * k, hgt * 0.5));
                put(&o, mix(base, moss, 0.35) * 1.2, cover(stem));
                let cp = s - vec2<f32>(cx, fy + hgt + bob);
                let cap = max(length(cp * vec2<f32>(1.0, 1.7)) - cr, -cp.y);
                let capc = grad4(u.objects, hh(id, row + 64.0));
                put(&o, capc, cover(cap));
                // glow under the cap and spots on it
                light(&o, max(length(cp * vec2<f32>(1.0, 3.0)) - cr * 0.9, cp.y), hh(id, row + 65.0), 0.06 * k, 0.8);
                let sp = length(cp - vec2<f32>(cr * 0.35, cr * 0.25)) - cr * 0.12;
                light(&o, max(sp, cap), hh(id, row + 66.0), 0.01 * k, 1.1);
            }
        }
    }

    // spores / fireflies drifting in the open air
    if (li >= 1 && li <= 3) {
        let g = 0.12 * k;
        let cell = floor(s / g);
        let hc = hh(cell.x * 31.0 + cell.y, row + 70.0);
        if (hc < 0.10 * dens() + 0.03 && d_rock > 0.0) {
            let jit = vec2<f32>(hh(cell.x, cell.y + row + 71.0), hh(cell.y, cell.x + row + 72.0));
            let drift = vec2<f32>(sin(tim() * 0.7 + hc * 40.0), cos(tim() * 0.9 + hc * 30.0)) * 0.25;
            let c = (cell + 0.5 + (jit - 0.5) * 0.4 + drift * 0.3) * g;
            light(&o, length(s - c) - 0.0035 * k, hc * 5.0, 0.012 * k, 1.0);
        }
    }
    return o;
}

// ------------------------------------------------------------------ cielo --

// A cumulus: a wide soft base and a few billows on top, its edge broken by
// noise so no two read as the same cut-out. Returns the distance; `w` is the
// half-width.
fn cloud(q: vec2<f32>, w: f32, id: f32, row: f32) -> f32 {
    // the base: a flattened ellipse, so the underside is one soft line
    var d = length(q * vec2<f32>(1.7, 3.6) - vec2<f32>(0.0, w * 0.3)) - w;
    let n = 4 + i32(hh(id, row + 3.0) * 3.0);
    for (var k = 0; k < n; k = k + 1) {
        let fk = f32(k);
        let px = (fk / f32(max(n - 1, 1)) - 0.5) * w * 1.1 + (hh(id + fk, row + 1.0) - 0.5) * w * 0.3;
        let r = w * (0.24 + 0.34 * hh(id + fk, row + 2.0)) * (1.0 - abs(px) / (w * 1.3));
        let py = w * 0.05 + r * (0.3 + 0.5 * hh(id + fk, row + 4.0));
        d = smin(d, length(q - vec2<f32>(px, py)) - r, w * 0.18);
    }
    // a ragged edge, never a compass circle
    d = d + (lattice_fbm(q / w * 3.0 + vec2<f32>(id, row), 3) - 0.5) * w * 0.12;
    // the underside fades out instead of ending on a ruled line
    return max(d, -(q.y + w * 0.08));
}

fn sky(s: vec2<f32>, li: i32) -> Smp {
    var o: Smp;
    o.col = vec3<f32>(0.0); o.a = 0.0; o.glow = vec3<f32>(0.0);
    let lt = f32(li) / 4.0;
    let row = f32(li) * 17.0 + 200.0;
    let k = size();
    let base = layer_col(lt);
    let sun_c = grad4(u.lights, 0.1);

    if (li == 0) {
        // stars in the upper sky
        let g = 0.035;
        let cell = floor(s / g);
        let hs = hh(cell.x * 57.0 + cell.y, 300.0);
        if (hs < 0.25 * dens() + 0.05 && s.y > -0.05) {
            let c = (cell + 0.25 + 0.5 * vec2<f32>(hh(cell.x, cell.y + 301.0), hh(cell.y, cell.x + 302.0))) * g;
            let tw = 0.5 + 0.5 * sin(tim() * (1.0 + 3.0 * hs) + hs * 90.0);
            let a = smoothstep(-0.05, 0.25, s.y);
            light(&o, length(s - c) - 0.0016 * (0.5 + tw), fract(hs * 7.0), 0.004, a * (0.4 + 0.6 * tw));
        }
        // the sun / moon, one every few frame-widths
        let sw = 3.2;
        let sid = floor(s.x / sw + 0.5);
        let sc = vec2<f32>(sid * sw + (hh(sid, 310.0) - 0.5) * 0.6, 0.18 + 0.15 * hh(sid, 311.0) + u.look2.w * 0.2);
        let r = 0.085 * k;
        let ds = length(s - sc) - r;
        light(&o, ds, 0.05 + 0.15 * hh(sid, 312.0), 0.12 * k, 1.0);
        o.glow = o.glow + sun_c * luz() * halo() * 0.18 * exp(-max(ds, 0.0) / (0.45 * k));
        // god rays fanning down from it
        let ang = atan2(s.y - sc.y, s.x - sc.x);
        let ray = pow(max(sin(ang * 9.0 + tim() * 0.05), 0.0), 6.0) * smoothstep(0.0, -0.4, s.y - sc.y);
        o.glow = o.glow + sun_c * luz() * halo() * 0.05 * ray * exp(-length(s - sc) / 0.8);
        // a far haze band of thin streaks
        let wy = 0.02 + 0.04 * sin(s.x * 3.0);
        let wisp = abs(s.y - wy - 0.05 * nz(s.x * 2.0, row)) - 0.006 * (0.5 + nz(s.x * 5.0, row + 3.0));
        put(&o, mix(base, sun_c, 0.3), cover(wisp) * 0.45);
        return o;
    }

    // floating islands, far (layers 1, 2): a rock wedge, grass on top, a tree
    if (li <= 2) {
        let iw = 1.3 * k;
        let iid = floor(s.x / iw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = iid + f32(j);
            if (hh(id, row + 60.0) < 0.35 + 0.3 * dens()) {
                let c = vec2<f32>((id + 0.5) * iw + (hh(id, row + 61.0) - 0.5) * iw * 0.4,
                                  0.02 + 0.22 * hh(id, row + 62.0) + 0.012 * sin(tim() * 0.4 + id));
                let w = (0.07 + 0.06 * hh(id, row + 63.0)) * k;
                let q = s - c;
                var di = sd_tri(vec2<f32>(q.x, q.y + w * 1.1), vec2<f32>(w, w * 1.1));
                di = di + (lattice_fbm(q / w * 4.0 + id, 2) - 0.5) * w * 0.15;
                let top = sd_box(q - vec2<f32>(0.0, -w * 0.02), vec2<f32>(w * 0.98, w * 0.05)) - w * 0.04;
                let rock = mix(base * 0.55, base * 0.85, smoothstep(-w, 0.0, q.y));
                put(&o, rock, cover(di));
                put(&o, mix(grad4(u.objects, 0.6), base, 0.3) * 0.9, cover(top));
                let tr = length(q - vec2<f32>(w * 0.3, w * 0.28)) - w * 0.22;
                put(&o, mix(grad4(u.objects, 0.6), base, 0.45) * 0.75, cover(tr));
                // a lit window on some: a light source
                if (hh(id, row + 64.0) < 0.5) {
                    light(&o, sd_box(q - vec2<f32>(-w * 0.35, w * 0.12), vec2<f32>(w * 0.06, w * 0.08)), 0.6, 0.02 * k, 1.0);
                    put(&o, base * 0.6, cover(sd_box(q - vec2<f32>(-w * 0.35, w * 0.12), vec2<f32>(w * 0.14, w * 0.14))) * (1.0 - cover(sd_box(q - vec2<f32>(-w * 0.35, w * 0.12), vec2<f32>(w * 0.06, w * 0.08)))));
                }
            }
        }
    }

    // clouds: far ones small and hazy, near ones big, dark-bellied silhouettes
    let cw = (0.45 + 0.45 * lt) * k;
    let cid = floor(s.x / cw);
    for (var j = -2; j <= 2; j = j + 1) {
        let id = cid + f32(j);
        if (hh(id, row + 5.0) < 0.3 + 0.55 * dens()) {
            let w = cw * (0.45 + 0.35 * hh(id, row + 6.0));
            let cy = 0.30 - lt * 0.42 + (hh(id, row + 7.0) - 0.5) * 0.3 + u.look2.w * 0.2;
            let c = vec2<f32>((id + 0.5) * cw + (hh(id, row + 8.0) - 0.5) * cw * 0.4, cy);
            let q = s - c;
            let dc = cloud(q, w, id * 13.0, row);
            if (dc < 0.05) {
                // height inside the cloud: belly 0 -> crown 1
                let hgt = clamp((q.y + w * 0.1) / (w * 0.9), 0.0, 1.0);
                let depth_in = clamp(-dc / (w * 0.35), 0.0, 1.0);
                let belly = mix(base, grad4(u.background, 0.35), 0.25) * (0.85 - 0.25 * lt);
                let crown = mix(base * 1.25, sun_c, 0.5 - 0.2 * lt);
                var cc = mix(belly, crown, smoothstep(0.1, 0.9, hgt));
                // soft inner gradient: the core is denser than the edge
                cc = mix(cc * 1.1, cc * 0.85, depth_in * (1.0 - hgt));
                // a lit rim along the top only
                let rim = smoothstep(-0.02 * k, 0.0, dc) * smoothstep(0.35, 0.9, hgt);
                cc = mix(cc, sun_c * min(luz(), 1.4), rim * 0.55);
                put(&o, cc, cover(dc) * mix(0.75, 1.0, lt));
            }
        }
    }

    // a cloud sea along the bottom of the nearest layer
    if (li == 4) {
        let by = ground(li) + 0.06 + 0.05 * nz(s.x * 2.0, row + 20.0);
        let bumps = 0.05 * k * pow(abs(sin(s.x * 7.0 / k + 2.0 * nz(s.x * 3.0, row + 21.0))), 0.6);
        let db = s.y - (by + bumps);
        let hgt = clamp((s.y - by + 0.1) / 0.16, 0.0, 1.0);
        put(&o, mix(base * 0.45, mix(base, sun_c, 0.35), hgt), cover(db));
    }

    // balloons drifting (layers 2, 3)
    if (li == 2 || li == 3) {
        let bw = 0.9 * k;
        let bid = floor(s.x / bw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = bid + f32(j);
            if (hh(id, row + 70.0) < 0.3 * dens() + 0.08) {
                let c = vec2<f32>((id + 0.5) * bw, -0.05 + 0.3 * hh(id, row + 71.0) + 0.015 * sin(tim() * 0.5 + id));
                let r = (0.03 + 0.015 * hh(id, row + 72.0)) * k * (0.6 + lt);
                let q = s - c;
                let env = length(q * vec2<f32>(1.0, 0.85)) - r;
                let stripes = step(0.5, fract(atan2(q.y, q.x * 2.0) * 1.0 + 0.25));
                let bc = mix(grad4(u.objects, 0.3), grad4(u.objects, 0.7), stripes) * mix(0.6, 1.0, smoothstep(-r, r, q.y + q.x * 0.5));
                put(&o, bc, cover(env));
                let basket = sd_box(q - vec2<f32>(0.0, -r * 1.55), vec2<f32>(r * 0.22, r * 0.16));
                let ropes = min(sd_seg(q, vec2<f32>(-r * 0.6, -r * 0.7), vec2<f32>(-r * 0.2, -r * 1.4)), sd_seg(q, vec2<f32>(r * 0.6, -r * 0.7), vec2<f32>(r * 0.2, -r * 1.4))) - 0.0012 * k;
                put(&o, base * 0.4, cover(min(basket, ropes)));
                light(&o, length(q - vec2<f32>(0.0, -r * 1.25)) - r * 0.08, 0.2, r * 0.6, 1.0);
            }
        }
    }

    // flocks of birds in a V (layers 2, 3)
    if (li == 2 || li == 3) {
        let fw = 0.8 * k;
        let fid = floor(s.x / fw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = fid + f32(j);
            if (hh(id, row + 30.0) < 0.4 * dens() + 0.15) {
                let lead = vec2<f32>((id + 0.5) * fw, 0.1 + 0.25 * hh(id, row + 31.0) + 0.02 * sin(tim() * 0.6 + id));
                let sz = (0.018 + 0.01 * hh(id, row + 32.0)) * k * (0.7 + lt);
                let nb = 3 + i32(hh(id, row + 35.0) * 6.0);
                for (var b = 0; b < nb; b = b + 1) {
                    let fb = f32(b);
                    let side = select(-1.0, 1.0, b % 2 == 0);
                    let rank = floor((fb + 1.0) / 2.0);
                    let c = lead + vec2<f32>(rank * sz * 2.2, side * rank * sz * 1.2 * 0.6 - rank * sz * 0.4);
                    let flap = sin(tim() * (6.0 + 2.0 * hh(id + fb, row + 33.0)) + fb * 1.7);
                    var q = (s - c) / sz;
                    q.x = abs(q.x);
                    let wing = sd_seg(q, vec2<f32>(0.0, 0.0), vec2<f32>(0.9, 0.3 + 0.6 * flap));
                    let tip = sd_seg(q, vec2<f32>(0.9, 0.3 + 0.6 * flap), vec2<f32>(1.7, 0.05 + 0.8 * flap));
                    let db = (min(wing, tip) - 0.16) * sz;
                    put(&o, grad4(u.objects, 0.05) * (1.0 - 0.3 * lt), cover(db));
                }
            }
        }
    }

    // sky lanterns rising slowly (light sources)
    if (li == 1 || li == 3) {
        let lw = 0.4 * k;
        let lid = floor(s.x / lw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = lid + f32(j);
            if (hh(id, row + 40.0) < 0.3 * dens()) {
                let rise = fract(tim() * 0.02 * (0.5 + hh(id, row + 41.0)) + hh(id, row + 42.0));
                let c = vec2<f32>((id + 0.5) * lw + 0.02 * sin(tim() + id), -0.5 + rise * 1.2);
                let sz = 0.012 * k * (0.6 + lt);
                let dl = sd_box(s - c, vec2<f32>(sz * 0.7, sz)) - sz * 0.3;
                light(&o, dl, 0.5 + 0.5 * hh(id, row + 43.0), sz * 4.0, 1.0);
            }
        }
    }
    return o;
}

// ----------------------------------------------------------------- campos --

fn campos(s: vec2<f32>, li: i32) -> Smp {
    var o: Smp;
    o.col = vec3<f32>(0.0); o.a = 0.0; o.glow = vec3<f32>(0.0);
    let lt = f32(li) / 4.0;
    let row = f32(li) * 17.0 + 400.0;
    let k = size();
    let base = layer_col(lt);
    let x = s.x;

    if (li == 0) {
        // sun, then mountains in front of it
        let sw = 3.6;
        let sid = floor(x / sw + 0.5);
        let sc = vec2<f32>(sid * sw + (hh(sid, 410.0) - 0.5), 0.30 + 0.08 * hh(sid, 411.0) + u.look2.w * 0.25);
        let ds = length(s - sc) - 0.075 * k;
        light(&o, ds, 0.05, 0.08 * k, 1.0);
        // jagged ridges: ridged noise, a sunlit face and a shadowed one
        let rn = 1.0 - abs(nz(x * 1.4 / k, row) * 2.0 - 1.0);
        let ridge = ground(li) + 0.04 + 0.16 * rn * rn + 0.025 * nz(x * 7.0, row + 2.0);
        let dm = s.y - ridge;
        let slope = ridge - (ground(li) + 0.04 + 0.16 * pow(1.0 - abs(nz((x + 0.01) * 1.4 / k, row) * 2.0 - 1.0), 2.0));
        var mc = mix(base * 0.8, base * 1.15, step(0.0, slope));
        let snow = smoothstep(ridge - 0.03, ridge - 0.004, s.y) * smoothstep(ground(li) + 0.12, ground(li) + 0.19, ridge);
        mc = mix(mc, mix(base, grad4(u.lights, 0.0), 0.5) * 1.2, snow * 0.5);
        put(&o, mc, cover(dm));
        return o;
    }

    // rolling hills
    let amp = 0.05 + 0.03 * lt;
    let hy = ground(li) + 0.04 + amp * (sin(x * (2.5 - lt) / k + row) * 0.5 + nz(x * 1.8 / k, row) * 1.4);
    let dh = s.y - hy;
    // a lighter crest catches the sun
    let crest = smoothstep(-0.03, 0.0, dh);
    let tex = 0.9 + 0.2 * lattice_fbm(s * vec2<f32>(40.0, 60.0) / k, 2);
    let hill = mix(base * 0.8, base * 1.12 + grad4(u.lights, 0.1) * 0.06, crest) * tex;
    put(&o, hill, cover(dh));

    // trees on the hills (layers 1..3)
    if (li >= 1 && li <= 3) {
        let tw = (0.16 + 0.04 * lt) * k;
        let tid = floor(x / tw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = tid + f32(j);
            if (hh(id, row + 5.0) < 0.2 + 0.55 * dens()) {
                let cx = (id + 0.25 + 0.5 * hh(id, row + 6.0)) * tw;
                let gy = ground(li) + 0.04 + amp * (sin(cx * (2.5 - lt) / k + row) * 0.5 + nz(cx * 1.8 / k, row) * 1.4) - 0.005;
                let th = (0.05 + 0.06 * hh(id, row + 7.0)) * k * (0.6 + lt);
                let trunk = sd_box(s - vec2<f32>(cx, gy + th * 0.35), vec2<f32>(0.004 * k * (0.6 + lt), th * 0.35));
                put(&o, grad4(u.objects, 1.0) * 0.8, cover(trunk));
                let sway = 0.004 * sin(tim() * 1.1 + id) * k;
                var dt: f32;
                if (hh(id, row + 8.0) < 0.5) {
                    // round tree: three blobs
                    let c = vec2<f32>(cx + sway, gy + th);
                    let r = th * 0.45;
                    dt = smin(length(s - c) - r, length(s - c - vec2<f32>(r * 0.7, -r * 0.35)) - r * 0.7, r * 0.3);
                    dt = smin(dt, length(s - c - vec2<f32>(-r * 0.7, -r * 0.3)) - r * 0.65, r * 0.3);
                } else {
                    // pine: two stacked triangles
                    // apex up: flip y so the base sits below the tip
                    let p1 = vec2<f32>(s.x - cx - sway, gy + th * 1.9 - s.y);
                    let p2 = vec2<f32>(s.x - cx - sway * 0.6, gy + th * 1.4 - s.y);
                    dt = min(sd_tri(p1, vec2<f32>(th * 0.32, th * 0.85)), sd_tri(p2, vec2<f32>(th * 0.45, th * 1.0)));
                }
                let leaf = mix(base * 1.1, grad4(u.layers, min(lt + 0.25, 1.0)), 0.5);
                let lit = smoothstep(gy + th * 0.6, gy + th * 1.4, s.y);
                put(&o, mix(leaf * 0.85, leaf * 1.25, lit), cover(dt));
            }
        }
    }

    // fence with lanterns (layer 3)
    if (li == 3) {
        let pw = 0.12 * k;
        let pid = floor(x / pw);
        let fy = ground(li) + 0.03;
        for (var j = -1; j <= 1; j = j + 1) {
            let id = pid + f32(j);
            let cx = (id + 0.5) * pw;
            let post = sd_box(s - vec2<f32>(cx, fy + 0.035 * k), vec2<f32>(0.004 * k, 0.035 * k)) - 0.001;
            put(&o, grad4(u.objects, 0.95), cover(post));
            if (hh(id, row + 20.0) < 0.25 * dens()) {
                let lc = vec2<f32>(cx, fy + 0.08 * k);
                light(&o, length(s - lc) - 0.007 * k, 0.4 + 0.3 * hh(id, row + 21.0), 0.05 * k, 1.0);
            }
        }
        let rails = min(abs(s.y - fy - 0.05 * k), abs(s.y - fy - 0.025 * k)) - 0.0025 * k;
        put(&o, grad4(u.objects, 0.95) * 0.9, cover(rails));
    }

    // flowers in the grass (layers 3, 4)
    if (li >= 3) {
        let fw = (0.05 + 0.04 * (lt - 0.75) * 4.0) * k;
        let fid = floor(x / fw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = fid + f32(j);
            if (hh(id, row + 30.0) < 0.3 + 0.6 * dens()) {
                let cx = (id + 0.2 + 0.6 * hh(id, row + 31.0)) * fw;
                let gy = ground(li) + 0.04 + amp * (sin(cx * (2.5 - lt) / k + row) * 0.5 + nz(cx * 1.8 / k, row) * 1.4);
                let sh = (0.03 + 0.05 * hh(id, row + 32.0)) * k * (0.5 + lt);
                let sway = 0.006 * sin(tim() * 1.6 + id * 1.3) * k;
                let top = vec2<f32>(cx + sway, gy + sh);
                let stem = sd_seg(s, vec2<f32>(cx, gy - 0.02), top) - 0.0018 * k;
                put(&o, base * 1.3, cover(stem));
                let p = s - top;
                let r = (0.008 + 0.006 * hh(id, row + 33.0)) * k * (0.6 + lt);
                let ang = atan2(p.y, p.x);
                let petals = length(p) - r * (0.75 + 0.35 * cos(ang * 5.0 + id));
                put(&o, grad4(u.objects, hh(id, row + 34.0) * 0.75), cover(petals));
                light(&o, length(p) - r * 0.3, 0.3 + 0.5 * hh(id, row + 35.0), r * 0.8, 0.45);
            }
        }
    }

    // rocks and spotted mushrooms in the grass (layers 3, 4)
    if (li >= 3) {
        let rw = 0.31 * k;
        let rid = floor(x / rw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = rid + f32(j);
            let cx = (id + 0.2 + 0.6 * hh(id, row + 80.0)) * rw;
            let gy = ground(li) + 0.04 + amp * (sin(cx * (2.5 - lt) / k + row) * 0.5 + nz(cx * 1.8 / k, row) * 1.4) - 0.01;
            if (hh(id, row + 81.0) < 0.5 * dens() + 0.1) {
                let r = (0.02 + 0.025 * hh(id, row + 82.0)) * k * (0.5 + lt);
                let q = s - vec2<f32>(cx, gy);
                let dr = max(length(q * vec2<f32>(0.8, 1.3)) - r, -q.y - r * 0.1) + (lattice_fbm(q / r * 2.0 + id, 2) - 0.5) * r * 0.25;
                let rc = mix(vec3<f32>(dot(base, vec3<f32>(0.33))) * 0.9 + base * 0.2, base * 1.3, smoothstep(0.0, r, q.y - q.x * 0.5));
                put(&o, rc, cover(dr));
            } else if (hh(id, row + 83.0) < 0.6 * dens()) {
                let h = (0.025 + 0.02 * hh(id, row + 84.0)) * k * (0.5 + lt);
                let q = s - vec2<f32>(cx, gy);
                let stem = sd_box(q - vec2<f32>(0.0, h * 0.5), vec2<f32>(h * 0.13, h * 0.5));
                put(&o, vec3<f32>(0.92, 0.88, 0.8) * mix(0.6, 1.0, lt), cover(stem));
                let cp = q - vec2<f32>(0.0, h);
                let cap = max(length(cp * vec2<f32>(1.0, 1.6)) - h * 0.7, -cp.y);
                put(&o, grad4(u.objects, 0.02), cover(cap));
                let spot = length(fract(cp / (h * 0.32)) - 0.5) - 0.18;
                put(&o, vec3<f32>(0.95), cover(max(spot * h * 0.32, cap)) * 0.9);
            }
        }
    }

    // tall grass blades in the foreground
    if (li == 4) {
        let gw = 0.012 * k;
        let gid = floor(x / gw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = gid + f32(j);
            let cx = (id + hh(id, row + 40.0)) * gw;
            let gy = ground(li) + 0.03;
            let gh = (0.03 + 0.08 * hh(id, row + 41.0) * dens()) * k;
            let bend = (0.01 + 0.01 * sin(tim() * 1.4 + cx * 3.0)) * k;
            let bl = sd_seg(s, vec2<f32>(cx, gy - 0.05), vec2<f32>(cx + bend, gy + gh)) - 0.0022 * k * clamp((gy + gh - s.y) / gh, 0.2, 1.0);
            put(&o, base * (0.8 + 0.5 * hh(id, row + 42.0)), cover(bl));
        }
        // fireflies above the grass
        let g = 0.1 * k;
        let cell = floor(s / g);
        let hc = hh(cell.x * 31.0 + cell.y, row + 50.0);
        if (hc < 0.15 * dens() + 0.03 && s.y > ground(li) && s.y < ground(li) + 0.25) {
            let drift = vec2<f32>(sin(tim() * 0.6 + hc * 40.0), cos(tim() * 0.8 + hc * 30.0)) * 0.3;
            let c = (cell + 0.5 + drift * 0.3) * g;
            light(&o, length(s - c) - 0.003 * k, 0.6 + 0.4 * hc, 0.015 * k, 1.0);
        }
    }
    return o;
}

// ----------------------------------------------------------------- ciudad --

fn ciudad(s: vec2<f32>, li: i32) -> Smp {
    var o: Smp;
    o.col = vec3<f32>(0.0); o.a = 0.0; o.glow = vec3<f32>(0.0);
    let lt = f32(li) / 4.0;
    let row = f32(li) * 17.0 + 600.0;
    let k = size();
    let base = layer_col(lt);
    let x = s.x;
    let gy = ground(li) + 0.02;

    if (li == 0) {
        let sw = 3.4;
        let sid = floor(x / sw + 0.5);
        let sc = vec2<f32>(sid * sw + (hh(sid, 610.0) - 0.5) * 0.8, 0.30 + 0.1 * hh(sid, 611.0) + u.look2.w * 0.2);
        let dm = length(s - sc) - 0.04 * k;
        light(&o, dm, 0.66, 0.15 * k, 1.0);
    }

    // buildings
    if (li <= 3) {
        let bw = (0.09 + 0.05 * lt) * k;
        let bid = floor(x / bw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = bid + f32(j);
            let h1 = hh(id, row + 1.0);
            if (h1 < 0.55 + 0.45 * dens()) {
                let w = bw * (0.35 + 0.25 * hh(id, row + 2.0));
                let cx = (id + 0.5) * bw + (hh(id, row + 3.0) - 0.5) * bw * 0.25;
                let tall = hh(id, row + 4.0);
                let ht = (0.12 + 0.45 * tall * tall + 0.08 * (1.0 - lt)) * k;
                let p = s - vec2<f32>(cx, gy + ht * 0.5);
                var db = sd_box(p, vec2<f32>(w, ht * 0.5 + 0.05));
                // ruined top: a jagged bite out of the roof on some
                let ruin = hh(id, row + 5.0);
                if (ruin < 0.45) {
                    let jag = 0.03 * k * (0.5 + 0.5 * sin((s.x - cx) * 140.0 / k + id)) + 0.06 * k * ruin * (s.x - cx + w) / (2.0 * w);
                    db = max(db, s.y - (gy + ht - jag));
                }
                // a spire on a few towers
                if (tall > 0.8) {
                    let sp = sd_tri(vec2<f32>(s.x - cx, -(s.y - (gy + ht + 0.08 * k))), vec2<f32>(w * 0.35, 0.09 * k));
                    db = min(db, sp);
                }
                let shade_x = 0.85 + 0.25 * step(0.0, p.x);
                let wall = mix(base, grad4(u.objects, 0.2) * 0.6 + base * 0.6, 0.2);
                put(&o, wall * shade_x, cover(db));
                if (db < 0.0) {
                    // windows
                    let wg = vec2<f32>(0.018, 0.026) * k * (0.6 + 0.4 * lt);
                    let wc = floor((s - vec2<f32>(cx - w, gy)) / wg);
                    let wp = fract((s - vec2<f32>(cx - w, gy)) / wg) - 0.5;
                    let lit = hh(wc.x + id * 37.0, wc.y + row + 6.0);
                    let horiz = hh(id, row + 7.0) < 0.35; // some towers have strip windows
                    var dwin: f32;
                    if (horiz) {
                        dwin = (abs(wp.y) - 0.18) * wg.y;
                    } else {
                        dwin = sd_box(wp * wg, vec2<f32>(0.28, 0.26) * wg);
                    }
                    let edge = sd_box(p, vec2<f32>(w - 0.008 * k, ht * 0.5 - 0.01 * k));
                    dwin = max(dwin, edge);
                    if (lit < (0.05 + 0.2 * dens()) * (0.3 + 0.7 * lt)) {
                        let on = step(0.15, fract(lit * 13.0 + floor(tim() * 0.2 + lit * 7.0) * 0.37));
                        light(&o, dwin, fract(lit * 5.0 + id * 0.13), 0.004 * k, (0.7 * on + 0.15) * (0.35 + 0.65 * lt));
                    } else {
                        put(&o, base * 0.75, cover(dwin) * 0.5 * lt);
                    }
                    // neon sign on the facade
                    if (hh(id, row + 8.0) < 0.3 && li >= 1) {
                        let sy = gy + ht * (0.35 + 0.4 * hh(id, row + 9.0));
                        let sz = vec2<f32>(w * 0.6, 0.02 * k);
                        let q = s - vec2<f32>(cx, sy);
                        let frame = abs(sd_box(q, sz) - 0.004 * k) - 0.0025 * k;
                        put(&o, base * 0.4, cover(sd_box(q, sz)));
                        let flick = step(0.08, fract(tim() * 0.5 + hh(id, row + 10.0)));
                        light(&o, frame, hh(id, row + 11.0), 0.025 * k, 1.2 * flick + 0.1);
                        // glyph strokes inside the sign
                        let gl = abs(fract(q.x / (0.012 * k)) - 0.5) * 0.012 * k - 0.0015 * k;
                        light(&o, max(gl, abs(q.y) - sz.y * 0.5), hh(id, row + 11.0), 0.01 * k, 0.9 * flick);
                    }
                }
                // antenna with a blinking light
                if (tall > 0.6 && ruin > 0.45) {
                    let top = vec2<f32>(cx + w * 0.4, gy + ht);
                    let ant = sd_seg(s, top, top + vec2<f32>(0.0, 0.05 * k)) - 0.0012 * k;
                    put(&o, base, cover(ant));
                    let blink = step(0.5, fract(tim() * 0.8 + id * 0.31));
                    light(&o, length(s - top - vec2<f32>(0.0, 0.05 * k)) - 0.004 * k, 0.33, 0.02 * k, blink);
                }
            }
        }
    }

    // ground strip
    let dg = s.y - gy;
    put(&o, base * 0.9, cover(dg));

    // foreground rubble, rebar and street lamps
    if (li == 4) {
        let rw = 0.5 * k;
        let rid = floor(x / rw);
        for (var j = -1; j <= 1; j = j + 1) {
            let id = rid + f32(j);
            let cx = (id + 0.5) * rw + (hh(id, row + 20.0) - 0.5) * rw * 0.4;
            if (hh(id, row + 21.0) < 0.4 + 0.5 * dens()) {
                var dr = 1e3;
                for (var n = 0; n < 6; n = n + 1) {
                    let fn_ = f32(n);
                    let ox = (hh(id + fn_, row + 22.0) - 0.5) * 0.2 * k;
                    let r = (0.03 + 0.05 * hh(id + fn_, row + 23.0)) * k * (1.0 - abs(ox) * 3.0 / k);
                    let rp = s - vec2<f32>(cx + ox, gy);
                    dr = smin(dr, sd_box(rp, vec2<f32>(r, r * 0.9)) - r * 0.2, 0.008 * k);
                }
                let tint = grad4(u.objects, hh(id, row + 24.0) * 0.4);
                put(&o, mix(base, tint, 0.3) * mix(0.6, 1.1, smoothstep(gy, gy + 0.08 * k, s.y)), cover(dr));
                // rebar sticking out
                let ra = vec2<f32>(cx + 0.02 * k, gy + 0.04 * k);
                let rb = ra + vec2<f32>(0.05, 0.08) * k * (0.5 + hh(id, row + 25.0));
                put(&o, grad4(u.objects, 0.4) * 0.8, cover(sd_seg(s, ra, rb) - 0.0015 * k));
                // a lost screen glowing in the pile
                if (hh(id, row + 26.0) < 0.5) {
                    let sc = vec2<f32>(cx - 0.05 * k, gy + 0.02 * k);
                    light(&o, length(s - sc) - 0.008 * k, hh(id, row + 27.0), 0.03 * k, 0.9);
                }
            }
            // a scrap robot slumped on some piles, one glowing eye
            if (hh(id, row + 28.0) < 0.45) {
                let rc = vec2<f32>(cx + 0.09 * k, gy + 0.035 * k);
                let q = s - rc;
                let body = length(q * vec2<f32>(1.0, 1.15)) - 0.03 * k;
                let leg = min(sd_seg(q, vec2<f32>(-0.02, -0.02) * k, vec2<f32>(-0.05, -0.04) * k), sd_seg(q, vec2<f32>(0.02, -0.02) * k, vec2<f32>(0.06, -0.035) * k)) - 0.004 * k;
                put(&o, mix(base, grad4(u.objects, 0.1), 0.6) * 0.8, cover(min(body, leg)));
                let eye = length(q - vec2<f32>(0.006, 0.004) * k);
                put(&o, base * 0.3, cover(eye - 0.014 * k));
                light(&o, abs(eye - 0.009 * k) - 0.002 * k, 0.0, 0.03 * k, 1.1);
                light(&o, eye - 0.003 * k, 0.0, 0.01 * k, 1.0);
            }
            // sagging wires from this lamp to the next
            let wx0 = (id + 0.05) * rw;
            let wt = clamp((s.x - wx0) / rw, 0.0, 1.0);
            let sag = gy + 0.21 * k - 0.06 * k * 4.0 * wt * (1.0 - wt);
            let wire = abs(s.y - sag) - 0.0012 * k;
            if (s.x > wx0 && s.x < wx0 + rw) { put(&o, base * 0.35, cover(wire)); }
            // street lamp between piles
            let lx = (id + 0.05) * rw;
            let pole = sd_box(s - vec2<f32>(lx, gy + 0.11 * k), vec2<f32>(0.0025 * k, 0.11 * k));
            let arm = sd_seg(s, vec2<f32>(lx, gy + 0.22 * k), vec2<f32>(lx + 0.03 * k, gy + 0.23 * k)) - 0.002 * k;
            put(&o, base * 0.8, cover(min(pole, arm)));
            let lamp = sd_box(s - vec2<f32>(lx + 0.032 * k, gy + 0.225 * k), vec2<f32>(0.008 * k, 0.003 * k));
            light(&o, lamp, 0.55, 0.12 * k, 1.0);
        }
    }
    return o;
}

// ------------------------------------------------------------------ frame --

fn sample_layer(world: i32, s: vec2<f32>, li: i32) -> Smp {
    if (world == 0) { return cueva(s, li); }
    if (world == 1) { return sky(s, li); }
    if (world == 2) { return campos(s, li); }
    return ciudad(s, li);
}

fn layer_off(i: i32) -> f32 {
    if (i < 4) { return u.off0[i]; }
    return u.f1.x;
}

fn layer_dep(i: i32) -> f32 {
    if (i < 4) { return u.dep0[i]; }
    return u.f1.y;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = u.f0.x;
    let world = i32(u.f0.y + 0.5);
    let fondo_alpha = u.f0.z > 0.5;
    let H = max(u.f1.w, 1.0);

    // pixel-art blocks: 1 px at pixel = 0, up to 8 px at pixel = 1
    let bs = 1.0 + floor(u.look2.z * 7.0 + 0.5);
    let res = vec2<f32>(H * aspect, H);
    let uv = (floor(in.uv * res / bs) + 0.5) * bs / res;

    let th = u.fwd.w;
    let ndc = vec2<f32>((uv.x * 2.0 - 1.0) * aspect * th, (1.0 - uv.y * 2.0) * th);
    let rd = normalize(u.fwd.xyz + u.right.xyz * ndc.x + u.up.xyz * ndc.y);
    let eye = u.eye.xyz;

    // backdrop
    var acc = grad4(u.background, uv.y);
    var cov = 0.0;
    var glow = vec3<f32>(0.0);

    // draw order: far to near by depth, whatever the depth floats say
    var order = array<i32, 5>(0, 1, 2, 3, 4);
    for (var a = 0; a < NL; a = a + 1) {
        for (var b = 0; b < NL - 1 - a; b = b + 1) {
            if (layer_dep(order[b]) < layer_dep(order[b + 1])) {
                let t = order[b];
                order[b] = order[b + 1];
                order[b + 1] = t;
            }
        }
    }

    for (var n = 0; n < NL; n = n + 1) {
        let li = order[n];
        let d = max(layer_dep(li), 0.05);
        let pz = -d;
        if (rd.z > -1e-4) { continue; }
        let t = (pz - eye.z) / rd.z;
        if (t <= 0.0) { continue; }
        let p = eye + rd * t;
        let s = vec2<f32>(p.x + layer_off(li), p.y) / d;
        let smp = sample_layer(world, s, li);
        // atmosphere: far layers sink into the fog colour
        let lt = f32(li) / 4.0;
        // by real depth, so a layer moved close loses its fog
        let fogw = clamp(u.look2.y * 0.8 * log(d / 1.4) / log(14.0), 0.0, 1.0);
        let fogc = grad4(u.fog_c, uv.y);
        let col = select(vec3<f32>(0.0), smp.col / max(smp.a, 1e-4), smp.a > 1e-4);
        // atmospheric perspective: near layers darker and more contrasted
        let near = 1.0 - clamp(log(d / 1.4) / log(14.0), 0.0, 1.0);
        let lum = dot(col, vec3<f32>(0.3, 0.5, 0.2));
        let contrasted = mix(vec3<f32>(lum), col, 1.0 + 0.25 * near) * mix(1.0, 0.72, near);
        let fc = mix(max(contrasted, vec3<f32>(0.0)), fogc, fogw) * smp.a;
        acc = acc * (1.0 - smp.a) + fc;
        glow = glow * (1.0 - smp.a * 0.85) + smp.glow * (1.0 - fogw * 0.6);
        cov = cov + (1.0 - cov) * smp.a;
    }
    // a little fog haze over the bottom of the frame
    let haze = u.look2.y * 0.12 * smoothstep(0.6, 1.0, uv.y);
    acc = mix(acc, grad4(u.fog_c, uv.y), haze);
    // glow rolls off instead of summing to white when many lights stack
    acc = acc + glow / (1.0 + 0.6 * max(max(glow.r, glow.g), glow.b));
    // soft clip so stacked glows do not burn to flat white
    acc = acc / (1.0 + max(acc - vec3<f32>(0.8), vec3<f32>(0.0)));

    let alpha = select(1.0, clamp(cov + length(glow), 0.0, 1.0), fondo_alpha);
    return vec4<f32>(acc, alpha);
}
