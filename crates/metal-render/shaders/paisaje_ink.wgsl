// Paisaje: the ink. CPU-built triangles with per-vertex colour; ribbons carry a
// `side` (-1..1 across the stroke) and an `along` (distance along it) so the
// edge can be feathered and the brush can run dry (`trazo_ruido`). Fills carry
// along = -1 and are solid. A second entry point lays the paper grain over the
// finished picture.

struct InkU {
    look: vec4<f32>,  // brillo, contraste, tono, saturacion
    misc: vec4<f32>,  // aspect, opacidad, trazo_ruido, time
    grain: vec4<f32>, // papel_grano, alpha gain (fondo), pixel scale, 0
    gdark: vec4<f32>,
    glight: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: InkU;

struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) side: f32,
    @location(2) along: f32,
    @location(3) color: vec4<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) side: f32,
    @location(1) along: f32,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_ink(in: VIn) -> VOut {
    var out: VOut;
    out.clip = vec4<f32>(in.pos.x / max(u.misc.x, 0.01), in.pos.y, 0.0, 1.0);
    out.side = in.side;
    out.along = in.along;
    out.color = in.color;
    return out;
}

@fragment
fn fs_ink(in: VOut) -> @location(0) vec4<f32> {
    var a = in.color.a;
    if (in.along >= 0.0) {
        let s = abs(in.side);
        let edge = 1.0 - smoothstep(0.5, 1.0, s);
        // Dry brush: streaks running along the stroke that eat into the edges.
        let n = quintic_noise(vec2<f32>(in.along * 0.55, in.side * 2.6 + 11.0));
        let dry = smoothstep(0.12, 0.62, n + 0.5 * (1.0 - s));
        a = a * edge * mix(1.0, dry, clamp(u.misc.z, 0.0, 1.0));
    }
    let rgb = kit_look(in.color.rgb, u.look.x, u.look.y, u.look.z, u.look.w);
    return vec4<f32>(rgb, a * u.misc.y);
}

struct GOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_grain(@builtin(vertex_index) vi: u32) -> GOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: GOut;
    out.clip = vec4<f32>(p[vi], 0.0, 1.0);
    return out;
}

// Paper: speckle, long horizontal fibres, and a slow mottling.
@fragment
fn fs_grain(in: GOut) -> @location(0) vec4<f32> {
    let px = in.clip.xy * u.grain.z;
    let speck = lattice(floor(px * 0.8));
    let fibre = quintic_noise(vec2<f32>(px.x * 0.035, px.y * 0.8));
    let mott = quintic_noise(px * 0.06);
    let n = 0.4 * speck + 0.35 * fibre + 0.25 * mott - 0.5;
    var col = u.gdark.rgb;
    if (n < 0.0) {
        col = u.glight.rgb;
    }
    let rgb = kit_look(col, u.look.x, u.look.y, u.look.z, u.look.w);
    let a = clamp(abs(n) * 2.0 * u.grain.x * 0.45, 0.0, 0.6) * u.grain.y * u.misc.y;
    return vec4<f32>(rgb, a);
}
