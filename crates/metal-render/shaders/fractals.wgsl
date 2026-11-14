// Model 12 — Fractals. A Julia orbit, one fullscreen fragment pass, no
// geometry at all: the same shape Domain has, which is why this was the
// cheapest of the five renderers asked for on 2026-08-17.
//
// Colour is escape time through `grad4`, and that is the whole reason a
// 4-colour slot exists: four equal stops give hard bands, four near tones give
// a smooth ramp, four unrelated ones give the psychedelic version — without
// this shader knowing which of the three it was handed.

struct FracUniform {
    color: ColorSlot,
    floats: vec4<f32>,  // zoom, c_re, c_im, iterations
    floats2: vec4<f32>, // drift, fold, time, aspect (w/h)
};

@group(0) @binding(0) var<uniform> u: FracUniform;

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

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let zoom = max(u.floats.x, 0.05);
    let iterations = i32(clamp(u.floats.w, 4.0, 400.0));
    let drift = u.floats2.x;
    let fold = u.floats2.y;
    let time = u.floats2.z;
    let aspect = max(u.floats2.w, 0.01);
    let escape = 4.0;

    // The plane. Widened on x by the frame's aspect so the set is round on a
    // wide output rather than an ellipse — there is no vertex stage here to
    // carry the correction, so it rides in the uniform.
    var z = (in.uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(2.0 * aspect, 2.0) / zoom;

    // `fold` bends the plane before the iteration — the difference between a
    // Julia set and a Julia set that breathes. At 0 this is exactly the plain
    // orbit, which is what the default is.
    if (fold > 0.0) {
        let r = length(z);
        z = z + fold * 0.35 * vec2<f32>(sin(z.y * 3.0 + time * 0.5), cos(z.x * 3.0 - time * 0.4)) * r;
    }

    // The constant moves on a slow circle, so the set is never the same set
    // twice without ever leaving the neighbourhood the operator picked.
    let wander = drift * 0.12;
    let c = vec2<f32>(
        u.floats.y + wander * cos(time * 0.21),
        u.floats.z + wander * sin(time * 0.17),
    );

    var i = 0;
    var escaped = 0.0;
    var r2 = dot(z, z);
    loop {
        if (i >= iterations || r2 > escape * escape) { break; }
        z = vec2<f32>(z.x * z.x - z.y * z.y, 2.0 * z.x * z.y) + c;
        r2 = dot(z, z);
        i = i + 1;
    }
    escaped = f32(i);

    // Smooth (continuous) escape time: the integer count bands the image into
    // rings a colour ramp cannot hide, and the fractional correction is two
    // logs. Points that never escaped stay at the far end of the ramp rather
    // than being punched out black, which is what keeps the interior of the
    // set part of the picture instead of a hole in it.
    var t = 1.0;
    if (i < iterations) {
        let nu = log2(max(log2(max(r2, 1.0001)) * 0.5, 1e-6));
        t = clamp((escaped + 1.0 - nu) / f32(iterations), 0.0, 1.0);
        // A gamma on the ramp: escape counts crowd near zero, and linear in
        // count is almost all one colour.
        t = pow(t, 0.42);
    }

    let col = grad4(u.color, t);
    return vec4<f32>(col, 1.0);
}
