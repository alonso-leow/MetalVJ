// fx.warp — a displacement field written as a 17x17 mesh of control points,
// Catmull-Rom interpolated per pixel. The notation and the CPU twin of this
// sum are `metal_schema::warp`; keep the two in step.

struct Warp {
    // x mosh, y has_prev (0/1), z N (control points per side), w unused
    p: vec4<f32>,
    // N*N displacements, two per vec4 (xy = even index, zw = odd)
    mesh: array<vec4<f32>, 145>,
};

@group(0) @binding(0) var<uniform> u: Warp;
@group(0) @binding(1) var cur: texture_2d<f32>;
@group(0) @binding(2) var prev: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let xy = p[vi];
    var out: VOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}

fn point(i: i32, j: i32, n: i32) -> vec2<f32> {
    let k = clamp(j, 0, n - 1) * n + clamp(i, 0, n - 1);
    let v = u.mesh[k / 2];
    if (k % 2 == 0) {
        return v.xy;
    }
    return v.zw;
}

fn cr(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return 0.5 * vec4<f32>(-t3 + 2.0 * t2 - t, 3.0 * t3 - 5.0 * t2 + 2.0, -3.0 * t3 + 4.0 * t2 + t, t3 - t2);
}

fn field(uv: vec2<f32>) -> vec2<f32> {
    let n = i32(u.p.z);
    let g = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * f32(n - 1);
    let base = min(vec2<i32>(floor(g)), vec2<i32>(n - 2));
    let t = g - vec2<f32>(base);
    let wx = cr(t.x);
    let wy = cr(t.y);
    var d = vec2<f32>(0.0);
    for (var b = 0; b < 4; b++) {
        for (var a = 0; a < 4; a++) {
            d += point(base.x + a - 1, base.y + b - 1, n) * wx[a] * wy[b];
        }
    }
    return d;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let at = in.uv + field(in.uv);
    let a = textureSampleLevel(cur, samp, at, 0.0);
    if (u.p.y < 0.5 || u.p.x <= 0.0) {
        return a;
    }
    let b = textureSampleLevel(prev, samp, at, 0.0);
    return mix(a, b, u.p.x);
}
