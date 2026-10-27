// fx.brush -- shared by the draw module (brush.wgsl) and the search module
// (brush_search.wgsl). Rust prepends this to both. See `shared/brush.rs` for
// the design.

struct U {
    a: vec4<f32>, // aspect, alfa, aleatoriedad, contorno
    b: vec4<f32>, // tam_ini, tam_fin, alargado, sigue
    c: vec4<f32>, // angulo (rad), color_jitter, saturacion, papel
    d: vec4<f32>, // papel_tono, grid columns, grid rows, cell size (height units)
    e: vec4<u32>, // forma, first stroke number, stroke count, grid cells
    f: vec4<f32>, // frame seed, fade rate, fade share (paper vs live frame), cant
    g: vec4<u32>, // search round, search stage, candidates, grid permutation step
    h: vec4<f32>, // 1 = the 64x64 proxy (no grain/outline), rounds in this frame, spare, spare
};
@group(0) @binding(0) var<uniform> u: U;

struct Stroke {
    c: vec2<f32>,   // centre, uv
    ang: f32,       // radians
    hl: f32,        // half length, height units
    hw: f32,        // half width
    bend: f32,      // bezier curvature
    shape: f32,     // 0 ellipse, 1 rect, 2 triangle, 3 bezier
    pad: f32,
    col: vec4<f32>, // rgb, alpha
};

const PI: f32 = 3.14159265;

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

// Uniform 0..1 from a stroke number and a channel.
fn rnd(k: u32, n: u32) -> f32 {
    return f32(pcg((k * 0x9E3779B1u) ^ pcg(n + 0x68E31DA4u))) / 4294967295.0;
}

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

fn wrap_ang(x: f32) -> f32 {
    return x - 2.0 * PI * round(x / (2.0 * PI));
}

// Signed distance (height units) from `p`, in the stroke's own frame (x along
// the stroke), to the shape.
fn stroke_d(shape: f32, p: vec2<f32>, hl: f32, hw: f32, bend: f32) -> f32 {
    let s = i32(round(shape));
    var d = 0.0;
    if (s == 0) {
        let r = vec2<f32>(hl, hw);
        let k0 = length(p / r);
        let k1 = length(p / (r * r));
        d = select(k0 * (k0 - 1.0) / max(k1, 1e-6), -min(hl, hw), k1 < 1e-6);
    } else if (s == 1) {
        let r = min(hl, hw) * 0.25;
        let q = abs(p) - vec2<f32>(hl, hw) + vec2<f32>(r);
        d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    } else if (s == 2) {
        // Isosceles triangle, apex at +x, base at -x.
        let qq = vec2<f32>(hw, 2.0 * hl);
        var t = vec2<f32>(abs(p.y), hl - p.x);
        let a = t - qq * clamp(dot(t, qq) / dot(qq, qq), 0.0, 1.0);
        let b = t - qq * vec2<f32>(clamp(t.x / qq.x, 0.0, 1.0), 1.0);
        let sg = -sign(qq.y);
        let m = min(vec2<f32>(dot(a, a), sg * (t.x * qq.y - t.y * qq.x)), vec2<f32>(dot(b, b), sg * (t.y - qq.y)));
        d = -sqrt(m.x) * sign(m.y);
    } else {
        let t = clamp(p.x / hl, -2.0, 2.0);
        let py = p.y - bend * hl * 0.5 * (1.0 - t * t);
        let w = hw * (1.0 - 0.6 * min(t * t, 1.0));
        d = max(abs(py) - w, abs(p.x) - hl);
    }
    return d;
}

// Which shape stroke number `k` uses under `forma` (4 = a mix of the four).
fn pick_shape(shape: u32, k: u32) -> f32 {
    if (shape >= 4u) {
        return floor(rnd(k, 8u) * 4.0);
    }
    return f32(shape);
}

fn paper_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let h = vec4<f32>(
        rnd(u32(i.x + 4096.0) + 8192u * u32(i.y + 4096.0), 101u),
        rnd(u32(i.x + 4097.0) + 8192u * u32(i.y + 4096.0), 101u),
        rnd(u32(i.x + 4096.0) + 8192u * u32(i.y + 4097.0), 101u),
        rnd(u32(i.x + 4097.0) + 8192u * u32(i.y + 4097.0), 101u),
    );
    return mix(mix(h.x, h.y, w.x), mix(h.z, h.w, w.x), w.y);
}

// Paper grain, 0..1, in height units so it keeps its scale at any size.
fn grain(uv: vec2<f32>) -> f32 {
    let p = vec2<f32>(uv.x * u.a.x, uv.y);
    return 0.6 * paper_noise(p * 320.0) + 0.4 * paper_noise(p * 90.0 + vec2<f32>(17.0, 3.0));
}

fn paper_color(uv: vec2<f32>) -> vec3<f32> {
    let tone = u.d.x;
    let base = vec3<f32>(0.93, 0.89, 0.80) * tone;
    return base * (1.0 - 0.10 * u.c.w * grain(uv));
}
