// Model 26 — Moiré. Several rulings of lines laid over each other, slightly
// out of agreement.
//
// Nothing here draws the pattern. Each layer is a plain periodic ruling —
// straight lines or concentric rings, depending on `curva` — and the pattern is
// what appears *between* them, which is the whole point: at `capas` 1 the frame
// is a boring set of stripes, and the second one is where the model starts.
// A beat that nudges `giro` by a degree moves the interference across the whole
// frame, because that is what a beat frequency does.
//
// It is Geoterrain's counterpart. That model makes a figure out of maths and
// shows you the figure; this one makes two dull figures and shows you their
// difference.
//
// # `mezcla`, and why it is not a mode
//
// Two rulings can be combined by multiplying (a layer is a mask, so the frame
// is where *every* layer is bright — a fine dark weave with sparse bright
// nodes) or by adding (a layer is light, so the frame is where *any* layer is,
// and the interference reads as broad soft bands). They are the two classic
// moirés and they look nothing alike. `mezcla` walks between them because the
// interesting settings are in the middle, and a switch would have hidden them.

struct MoireUniform {
    lines: ColorSlot,
    background: ColorSlot,
    // aspect, time, capas, paso
    a: vec4<f32>,
    // giro, desfase, grosor, curva
    b: vec4<f32>,
    // zoom, mezcla, contraste, centro_sep
    c: vec4<f32>,
    // layout, _, _, _
    d: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: MoireUniform;

const MAX_CAPAS: i32 = 6;
const TAU: f32 = 6.28318530718;

// 2026-11-25: 16 ruling layouts (`layout`), the distance each layer rules.
fn mo_layout(q: vec2<f32>, dir: vec2<f32>, flat: f32, ring: f32, f: f32) -> f32 {
    let k = i32(clamp(round(u.d.x), 0.0, 15.0));
    let r = max(ring, 1e-4);
    let a = atan2(q.y, q.x);
    let ql = vec2<f32>(dot(q, dir), dot(q, vec2<f32>(-dir.y, dir.x)));
    switch (k) {
        case 1: { return ql.x * ql.y * 1.5; }                                   // hyperbolas
        case 2: { return (a / TAU) * 0.3 + log(r) * 0.12; }                     // log spiral
        case 3: { return a / TAU * 0.6 * (1.0 + f); }                           // spokes
        case 4: { return max(abs(ql.x), abs(ql.y)); }                           // square rings
        case 5: { return abs(ql.x) + abs(ql.y); }                               // diamonds
        case 6: { return flat + 0.05 * sin(ql.y * 9.0 + f); }                   // waves
        case 7: { return flat + 0.04 * abs(fract(ql.y * 4.0) * 2.0 - 1.0); }    // zigzag
        case 8: { return length(ql * vec2<f32>(1.0, 2.2)); }                    // ellipses
        case 9: { return log(r) * 0.15 + a / TAU * 0.05 * f; }                  // log-polar
        case 10: { let h = abs(ql); return max(h.x * 0.866 + h.y * 0.5, h.y); } // hexagons
        case 11: { return (ql.x + ql.y) * 0.7 + 0.02 * sin(ql.x * 20.0); }      // weave
        case 12: { return r * (1.0 + 0.2 * cos(a * 5.0)); }                     // stars
        case 13: { return r * r * cos(2.0 * a) + r * 0.2; }                     // lemniscates
        case 14: { return max(ql.y * 0.5 + abs(ql.x) * 0.866, -ql.y); }         // triangles
        case 15: { return r - 0.3 * cos(a); }                                   // cardioids
        default: { return flat; }
    }
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let layers = i32(clamp(u.a.z, 1.0, f32(MAX_CAPAS)));
    let step = max(u.a.w, 0.001);
    let turn = u.b.x;
    let shift = u.b.y;
    let thickness = clamp(u.b.z, 0.02, 0.98);
    let curve = clamp(u.b.w, 0.0, 1.0);
    let zoom = max(u.c.x, 0.05);
    let mezcla = clamp(u.c.y, 0.0, 1.0);
    let contrast = max(u.c.z, 0.0);
    let sep = u.c.w;

    let p0 = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0) / zoom;
    var col = backdrop_from_uv(u.background, in.uv, aspect);

    // The two combinations are accumulated side by side and blended at the
    // end. Doing it that way rather than branching costs one multiply-add per
    // layer and buys every setting in between the two.
    var prod = 1.0;
    var sum = 0.0;

    for (var i = 0; i < MAX_CAPAS; i = i + 1) {
        if (i >= layers) { break; }
        let f = f32(i);
        let share = f / max(f32(layers - 1), 1.0);

        // Each layer turns at its own angle. The `desfase` term is what puts
        // the layers out of agreement — at 0 they are the same ruling exactly
        // on top of itself and there is no pattern at all, which is a legal and
        // instructive thing to be able to dial to.
        let ang = t * turn + share * shift * TAU * 0.25;
        let dir = vec2<f32>(cos(ang), sin(ang));

        // Rulings do not all share a centre: pulled apart, the rings interfere
        // in the lens shape a moiré is famous for.
        let centre = dir * sep * share;
        let q = p0 - centre;

        // A straight ruling and a set of rings are the same function of a
        // different distance, so `curva` is a `mix` of the two arguments
        // rather than two code paths.
        let flat = dot(q, dir);
        let ring = length(q);
        let d = mix(mo_layout(q, dir, flat, ring, f), ring, curve);

        // One period of the ruling, antialiased against the screen derivative.
        // Without the `fwidth` term a fine ruling aliases into its own moiré —
        // a pattern the model did not put there and the operator cannot
        // control, on top of the one it did.
        let phase = fract(d / step);
        let px = max(fwidth(d / step), 1e-4);
        let edge = clamp(px, 0.001, 0.5);
        let line = smoothstep(0.5 - thickness * 0.5 - edge, 0.5 - thickness * 0.5 + edge, phase)
            * (1.0 - smoothstep(0.5 + thickness * 0.5 - edge, 0.5 + thickness * 0.5 + edge, phase));

        prod = prod * line;
        sum = sum + line / f32(layers);
    }

    var v = mix(prod, sum, mezcla);
    // Pushed around 0.5, so raising the contrast opens the bright nodes out of
    // the weave instead of just making the whole frame brighter.
    v = clamp((v - 0.5) * contrast + 0.5, 0.0, 1.0);

    col = mix(col, grad4(u.lines, v), v);
    return vec4<f32>(col, 1.0);
}
