// Model 8 — Chamber. A flat grid, instanced onto up to 6 interior cube
// faces. Height is a train of travelling Gaussian pulses in
// `(time - distance)` space, one front per cycle, never reset: a node's
// height rises as each front reaches it and falls once it passes — the "gravity" look falls out of the pulse shape itself, no
// separate fall stage to hand-author.

struct MeshUniform {
    color: ColorSlot,
    floats: vec4<f32>,  // origin_x, origin_y, altura, propagacion
    floats2: vec4<f32>, // delay, gravedad, time, caras
    view_proj: mat4x4<f32>,
    faces: array<mat4x4<f32>, 6>,
};

@group(0) @binding(0) var<uniform> u: MeshUniform;

fn height_at(x: f32, z: f32) -> f32 {
    let origin = vec2<f32>(u.floats.x * 2.0 - 1.0, u.floats.y * 2.0 - 1.0) * 0.9;
    let d = length(vec2<f32>(x, z) - origin);
    // A continuous train of fronts: one leaves the origin every cycle and the
    // ones already out keep travelling. (A single `fract` front flattened the
    // whole surface each time it wrapped — the box "reset" on every wave.)
    let p = fract(u.floats2.z * (0.15 + u.floats.w * 0.5) - d * (0.3 + u.floats2.x * 1.5));
    // A front is a ring, not a swell: the old single pulse left most of the
    // floor flat ahead of it, so the train keeps that look by falling ×2
    // faster than the old curve.
    let sharpness = (2.0 + u.floats2.y * 8.0) * 2.0;
    // Behind the front the pulse falls (`gravedad` sets how fast), reaching
    // exactly zero as the next front arrives; ahead of it a short, steep rise
    // brings the node up to meet it. The two meet at 1 on the front, so a
    // node's height is continuous in time — no jump, no flat frame.
    let tail = exp(-sharpness);
    let fall = (exp(-p * p * sharpness) - tail) / (1.0 - tail);
    let q = 1.0 - p;
    let rise = exp(-q * q * sharpness * 12.0);
    return u.floats.z * max(fall, rise);
}

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) t: f32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) t: f32,
};

@vertex
fn vs_main(in: VIn, @builtin(instance_index) face: u32) -> VOut {
    let e = 0.02;
    let h = height_at(in.pos.x, in.pos.z);
    let hx = height_at(in.pos.x + e, in.pos.z);
    let hz = height_at(in.pos.x, in.pos.z + e);
    let tangent_x = vec3<f32>(e, hx - h, 0.0);
    let tangent_z = vec3<f32>(0.0, hz - h, e);
    let n_local = normalize(cross(tangent_z, tangent_x));

    let local_pos = vec3<f32>(in.pos.x, h, in.pos.z);
    let face_xform = u.faces[face % 6u];
    let world_pos = (face_xform * vec4<f32>(local_pos, 1.0)).xyz;
    let world_normal = normalize((face_xform * vec4<f32>(n_local, 0.0)).xyz);

    var out: VOut;
    out.clip = u.view_proj * vec4<f32>(world_pos, 1.0);
    out.normal = world_normal;
    out.t = clamp(h / max(u.floats.z, 0.001), 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let base = grad4(u.color, in.t);
    return vec4<f32>(shade(in.normal, base), 1.0);
}
