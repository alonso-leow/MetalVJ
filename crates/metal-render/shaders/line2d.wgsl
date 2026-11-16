// Shared thick 2D line pipeline: aspect-corrected NDC lines coloured by
// arc-length through grad4. Geoterrain (spiral math) and Escape (Verlet
// strings) both generate a vertex list on the CPU and draw it through this
// one pipeline instance.

struct LineUniform {
    color: ColorSlot,
    misc: vec4<f32>, // x: aspect (width/height), y: time
    mat_part: MaterialU, // the strokes' material; mat_mix 0 is off
};

@group(0) @binding(0) var<uniform> u: LineUniform;
@group(0) @binding(1) var t_part: texture_2d<f32>;
@group(0) @binding(2) var s_part: sampler;

struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) t: f32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) t: f32,
    @location(1) p: vec2<f32>,
};

@vertex
fn vs_main(in: VIn) -> VOut {
    var out: VOut;
    var p = in.pos;
    p.x = p.x / u.misc.x;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.t = in.t;
    out.p = in.pos;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let base = grad4(u.color, in.t);
    // A flat 2D contract for the material (plan M): the stroke is a surface
    // facing the camera, so the point is (uv, 0) and the normal is +z.
    if (u.mat_part.b.x <= 0.0 && u.mat_part.a.x <= 0.0) {
        return vec4<f32>(base, 1.0);
    }
    let uv = in.p / vec2<f32>(u.misc.x, 1.0) * 0.5 + vec2<f32>(0.5);
    let z = vec3<f32>(0.0, 0.0, 1.0);
    let sh = material_apply(u.mat_part, t_part, s_part, base, vec3<f32>(in.p, 0.0), z, uv, z, u.misc.y);
    // Roughness keeps the stroke's own colour; a smooth metal reads brighter.
    let sheen = (1.0 - sh.roughness) * sh.metalness * 0.25;
    return vec4<f32>(sh.albedo + vec3<f32>(sheen), sh.alpha);
}
