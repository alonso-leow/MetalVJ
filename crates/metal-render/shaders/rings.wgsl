// Model 20 — Rings. One fullscreen fragment pass over a state the CPU already
// solved: where the ball is, and which rings are still there. Nothing here
// simulates anything — `rings.rs` does, because a bounce is a decision (does
// the ball go through the gap or off the wall) and a fragment shader has no
// place to keep the answer.
//
// Rings are bands in polar coordinates with a wedge cut out of them, which is
// the whole reason this is a field and not geometry: the gap is an angular
// interval, and an interval is one `smoothstep` here versus a re-tessellated
// arc per frame there.

struct RingsUniform {
    anillos: ColorSlot,
    bola: ColorSlot,
    background: ColorSlot,
    // aspect, grosor, brillo, how many rings are in the array
    misc: vec4<f32>,
    // x, y, radius, speed (0..1, for the ball's own glow)
    ball: vec4<f32>,
    // radius, gap centre angle, gap half-width in radians, 0..1 alive
    rings: array<vec4<f32>, 12>,
};

@group(0) @binding(0) var<uniform> u: RingsUniform;

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

const PI: f32 = 3.14159265;

// The shorter way round between two angles, in -pi..pi.
fn ang_delta(a: f32, b: f32) -> f32 {
    var d = a - b;
    d = d - 2.0 * PI * floor((d + PI) / (2.0 * PI));
    return d;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let aspect = max(u.misc.x, 0.01);
    let thickness = max(u.misc.y, 0.001);
    let shine = clamp(u.misc.z, 0.0, 1.0);
    let n = i32(clamp(u.misc.w, 0.0, 12.0));

    // The frame is a plane with y up and x widened by the aspect, so a ring is
    // a circle on a 16:9 wall and not an ellipse.
    let p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    var col = backdrop_from_uv(u.background, in.uv, aspect);

    let r = length(p);
    let a = atan2(p.y, p.x);
    // One pixel, in the units `r` is measured in. Everything is antialiased
    // against this so a ring is a hairline in a 160x90 rack tile too.
    let px = max(fwidth(r), 1e-4);

    for (var i = 0; i < 12; i = i + 1) {
        if (i >= n) { break; }
        let ring = u.rings[i];
        let alive = ring.w;
        if (alive <= 0.002) { continue; }

        // The band.
        let d = abs(r - ring.x) - thickness * 0.5;
        // The gap. `ring.z` is the half-width; a ring whose gap has grown to
        // pi is gone, which is not how they die here but is a legal state.
        let off = abs(ang_delta(a, ring.y));
        // The angular softness that matches one pixel depends on the radius:
        // at the centre a tenth of a radian is nothing, at the rim it is a gap.
        let soft = clamp(px / max(ring.x, 1e-3), 0.002, 0.4);
        let solid = smoothstep(ring.z - soft, ring.z + soft, off);
        if (solid <= 0.002) { continue; }

        let tint = grad4(u.anillos, f32(i) / max(f32(n - 1), 1.0));
        let body = (1.0 - smoothstep(-px, px, d)) * solid * alive;
        // A ring that is dying is also the one thing on screen worth looking
        // at, so it keeps its glow while its body fades.
        let glow = exp(-max(d, 0.0) * 26.0) * shine * 0.55 * solid * alive;
        col = col + tint * (body + glow) - col * body;
    }

    // The ball, over everything, with a radial four-stop gradient of its own
    // so it is a lit object rather than a dot.
    let bd = length(p - u.ball.xy) - u.ball.z;
    let bpx = max(fwidth(bd), 1e-4);
    let inside = 1.0 - smoothstep(-bpx, bpx, bd);
    if (inside > 0.001 || shine > 0.0) {
        let t = clamp(length(p - u.ball.xy) / max(u.ball.z, 1e-4), 0.0, 1.0);
        let ball_col = grad4(u.bola, t);
        let halo = exp(-max(bd, 0.0) * 18.0) * (0.25 + 0.75 * u.ball.w) * shine;
        col = col + ball_col * halo;
        col = mix(col, ball_col, inside);
    }

    return vec4<f32>(col, 1.0);
}
