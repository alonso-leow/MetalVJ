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

// Walls meet flat: the pulse dies out over the last strip of every face, so
// six displaced faces still close into a room instead of showing black cracks
// down each seam.
fn seam(x: f32, z: f32) -> f32 {
    let m = max(abs(x), abs(z));
    return 1.0 - smoothstep(0.78, 0.98, m);
}

const WAVE_GAIN: f32 = 0.4;

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
    // 2026-11-25 review: waves were too strong, intensity cut 60%.
    return u.floats.z * WAVE_GAIN * max(fall, rise) * seam(x, z);
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
    @location(2) local: vec2<f32>,
    @location(3) world: vec3<f32>,
    @location(4) dist: f32,
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
    out.t = clamp(h / max(u.floats.z * WAVE_GAIN, 0.001), 0.0, 1.0);
    out.local = in.pos.xz;
    out.world = world_pos;
    out.dist = out.clip.w;
    return out;
}

// A lit grid room for a stage: the surface keeps the slot gradient by height,
// the crests glow, a fine grid of light is etched into every wall and burns
// brighter where a front is passing, and the far walls fall into the dark.
@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let t = in.t;
    let base = grad4(u.color, t);
    let n = normalize(in.normal);
    let v = normalize(-in.world);
    let l = normalize(vec3<f32>(0.35, 0.8, 0.45));
    let diff = max(dot(n, l), 0.0);
    let h = normalize(l + v);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * (0.25 + 0.75 * t);
    let rim = pow(1.0 - clamp(abs(dot(n, v)), 0.0, 1.0), 3.0);

    // grid lines, anti-aliased in screen space
    let g = in.local * 8.0;
    let w = fwidth(g) * 1.2;
    let gl = abs(fract(g - 0.5) - 0.5) / max(w, vec2<f32>(1e-4));
    let line = 1.0 - min(min(gl.x, gl.y), 1.0);
    let major_g = in.local * 2.0;
    let mw = fwidth(major_g) * 1.6;
    let ml = abs(fract(major_g - 0.5) - 0.5) / max(mw, vec2<f32>(1e-4));
    let major = 1.0 - min(min(ml.x, ml.y), 1.0);

    let hot = grad4(u.color, 1.0);
    var col = base * (0.22 + 0.7 * diff);
    col = col + base * t * t * 0.9;                                  // crests glow
    col = col + hot * (line * (0.18 + 1.4 * t) + major * (0.35 + 1.2 * t));
    col = col + vec3<f32>(spec) * 0.55 + hot * rim * (0.25 + 0.6 * t);
    let fog = exp(-max(in.dist - 0.6, 0.0) * 0.55);
    col = col * (0.18 + 0.82 * fog);
    col = vec3<f32>(1.0) - exp(-col * 1.25);
    return vec4<f32>(col, 1.0);
}
