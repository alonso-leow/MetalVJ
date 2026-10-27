// Model 21 — Scratch-off. One fullscreen fragment pass, and no history buffer:
// how much of the frame has come off is recomputed from the elements' paths
// every frame rather than accumulated into a texture.
//
// That is the whole design. A scratch card is a *swept region* — the set of
// points an element has already gone over — and a swept region has a closed
// form: within half a width of the element's line, and behind where its head
// has got to. So coverage is a `max` over the elements, evaluated per pixel,
// and the model stays a pure function of `(floats, time)` like the rest of the
// suite instead of growing the ping-pong pair and the seek problem Turing has.

struct ScratchUniform {
    scratchable: ColorSlot,
    develop: ColorSlot,
    element: ColorSlot,
    // aspect, how many elements, 0..1 through the cycle, grain scale
    misc: vec4<f32>,
    // remnant toughness, edge raggedness, head glow, unused
    misc2: vec4<f32>,
    // origin xy, direction xy — the line the element travels along
    paths: array<vec4<f32>, 24>,
    // how far the head has got, half width, hashed seed, 0 before it launches
    heads: array<vec4<f32>, 24>,
};

@group(0) @binding(0) var<uniform> u: ScratchUniform;

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
    let aspect = max(u.misc.x, 0.01);
    let n = i32(clamp(u.misc.y, 0.0, 24.0));
    let grain = max(u.misc.w, 0.5);
    let tough_max = clamp(u.misc2.x, 0.0, 1.0);
    let ragged = clamp(u.misc2.y, 0.0, 1.0);
    let head_glow = clamp(u.misc2.z, 0.0, 1.0);

    let p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);

    // How hard this particular speck is to get off. This is what leaves
    // *remnants*: an element passing over a tough pixel does not take it,
    // and the pixel sits there being the last thing on the screen.
    let tough = tough_max * fbm(p * grain, 4);

    var scratched = 0.0;
    var heat = 0.0;
    for (var i = 0; i < 24; i = i + 1) {
        if (i >= n) { break; }
        let head = u.heads[i];
        if (head.w <= 0.0) { continue; }

        let path = u.paths[i];
        let o = path.xy;
        let d = path.zw;
        let nrm = vec2<f32>(-d.y, d.x);
        let rel = p - o;
        let along = dot(rel, d);
        let across = dot(rel, nrm);

        // A ragged edge, so a pass reads as something dragged over the surface
        // rather than as a rectangle.
        let wobble = 1.0 + ragged * 0.55 * (noise2(vec2<f32>(along * 5.0, head.z * 37.0)) - 0.5) * 2.0;
        let half = max(head.y * wobble, 1e-4);

        // Behind the head, and inside the width.
        let behind = clamp((head.x - along) / max(half * 0.6, 1e-4), 0.0, 1.0);
        let inside = 1.0 - smoothstep(half * 0.55, half, abs(across));
        scratched = max(scratched, behind * inside);

        // The element itself, at the head of its own trail.
        if (head_glow > 0.0) {
            let at = o + d * head.x;
            heat = max(heat, exp(-length(p - at) * 9.0 / max(half * 3.0, 0.25)));
        }
    }

    // `tough` is the threshold and `scratched` the effort; the band between
    // them is the crumbling edge.
    let off = smoothstep(tough, tough + 0.16, scratched);

    let top = grad4(u.scratchable, backdrop_t(p, vec2<f32>(0.7, -0.7), aspect));
    let under = grad4(u.develop, backdrop_t(p, vec2<f32>(-0.4, -0.9), aspect));
    var col = mix(top, under, off);

    // Dust along the crumbling edge: the one cue that says the surface is
    // coming off rather than fading out.
    let crumb = (1.0 - abs(off * 2.0 - 1.0)) * 0.8;
    col = mix(col, grad4(u.element, 0.35), crumb * 0.35);

    if (head_glow > 0.0 && heat > 0.001) {
        col = col + grad4(u.element, 0.8) * heat * head_glow;
    }

    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
