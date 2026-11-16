// The shared background: one fullscreen triangle filling the frame with a
// colour slot walked along a direction. Every model that draws *objects* —
// lines, hairs, figures, panels — runs this first and then loads over it,
// instead of clearing to the black literal each of them used to carry.

struct BackdropUniform {
    color: ColorSlot,
    // x, y: the direction the slot is walked along, in NDC.
    // z: aspect (width/height), so a diagonal is a diagonal on a wide frame.
    misc: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: BackdropUniform;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

// The three-vertex fullscreen triangle: bigger than the frame, so there is no
// diagonal seam down the middle where two triangles of a quad would meet.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VOut {
    var out: VOut;
    let p = vec2<f32>(
        f32(i32(index) % 2) * 4.0 - 1.0,
        f32(i32(index) / 2) * 4.0 - 1.0,
    );
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let dir = u.misc.xy;
    let aspect = max(u.misc.z, 0.01);

    let p = vec2<f32>(in.ndc.x * aspect, in.ndc.y);
    let a = select(1.0, 0.0, u.misc.w > 0.5);
    return vec4<f32>(grad4(u.color, backdrop_t(p, dir, aspect)), a);
}
