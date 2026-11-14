// Model 22 — Lazer Bounce. One instanced capsule, drawn once per segment of
// every trail on screen and once per border-hit flash. The paths were
// traced on the CPU (`lazer.rs`) because a reflection is a decision about what
// was hit first and a fragment shader has nowhere to put the answer.
//
// A segment is drawn as a quad expanded by the beam's half width and capped at
// both ends, with the exact distance to the segment recomputed per fragment —
// so a beam is round-ended and antialiased at any thickness, and two segments
// meeting at a bounce join without a notch.

struct LazerUniform {
    // aspect, brillo (halo strength), glow (halo reach), unused
    misc: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: LazerUniform;

struct VIn {
    // ax, ay, bx, by — in the aspect-corrected space (x in ±aspect, y in ±1)
    @location(0) seg: vec4<f32>,
    // half width, alpha at a, alpha at b, unused
    @location(1) shape: vec4<f32>,
    @location(2) ca: vec4<f32>,
    @location(3) cb: vec4<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec2<f32>,
    @location(1) @interpolate(flat) a: vec2<f32>,
    @location(2) @interpolate(flat) b: vec2<f32>,
    @location(3) @interpolate(flat) shape: vec4<f32>,
    @location(4) @interpolate(flat) ca: vec4<f32>,
    @location(5) @interpolate(flat) cb: vec4<f32>,
};

@vertex
fn vs_main(in: VIn, @builtin(vertex_index) vi: u32) -> VOut {
    let a = in.seg.xy;
    let b = in.seg.zw;
    let w = max(in.shape.x, 1e-4);

    let span = b - a;
    let len = max(length(span), 1e-5);
    let d = span / len;
    let nrm = vec2<f32>(-d.y, d.x);

    // Strip of four: (a,-) (a,+) (b,-) (b,+), each end pushed out by one width
    // so a round cap has quad to live in.
    let cap = 1.0;
    let along = select(0.0, 1.0, (vi & 2u) == 2u);
    let side = select(-1.0, 1.0, (vi & 1u) == 1u);
    let world = mix(a, b, along) + d * (along * 2.0 - 1.0) * w * cap + nrm * side * w;

    var out: VOut;
    out.clip = vec4<f32>(world.x / max(u.misc.x, 0.01), world.y, 0.0, 1.0);
    out.world = world;
    out.a = a;
    out.b = b;
    out.shape = in.shape;
    out.ca = in.ca;
    out.cb = in.cb;
    return out;
}

// Distance from `p` to the segment `a`–`b`, and how far along it the closest
// point is. The second output is what carries the trail's fade and its
// gradient, so both come out of one projection rather than two.
fn seg_project(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let ab = b - a;
    let denom = max(dot(ab, ab), 1e-9);
    let t = clamp(dot(p - a, ab) / denom, 0.0, 1.0);
    return vec2<f32>(length(p - (a + ab * t)), t);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let w = max(in.shape.x, 1e-4);
    let hit = seg_project(in.world, in.a, in.b);
    let distance = hit.x;
    let t = hit.y;

    let aa = max(fwidth(distance), 1e-5);
    let core = 1.0 - smoothstep(w - aa, w + aa, distance);
    // A halo several widths wide (`glow` sets the reach): a beam this thin
    // with a halo two widths wide reads as a scratch on the lens rather than
    // as light.
    let reach = mix(2.0, 10.0, clamp(u.misc.z, 0.0, 1.0));
    let glow = exp(-max(distance - w, 0.0) / max(w * reach, 1e-4))
             * clamp(u.misc.y, 0.0, 1.0);

    let alpha = mix(in.shape.y, in.shape.z, t) * clamp(core + glow * 0.75, 0.0, 1.0);
    if (alpha <= 0.002) { discard; }

    let col = mix(in.ca.rgb, in.cb.rgb, t);
    // Additive blending upstream, so `alpha` is how much of this colour lands.
    return vec4<f32>(col, alpha);
}
