// A model's picture laid on another model's 3D part (plan: surface sources).
// Concatenated ahead of the species' own WGSL; the texture and sampler are
// bound by the model, so this file only holds the sampling rule.
//
// `object` (mapping 0): triplanar in object space, so the picture sticks to
// the mesh and follows it when it turns. The three planes are blended by the
// object-space normal. `projector` (mapping 1): the picture is projected from
// the screen, by `screen_uv`, like a projector on a venue surface.
//
// `textureSampleLevel` at level 0, not `textureSample`: the callers sample
// inside the march's branches, where derivatives are not uniform.
fn surface_sample(
    tex: texture_2d<f32>,
    samp: sampler,
    obj_pos: vec3<f32>,
    normal: vec3<f32>,
    screen_uv: vec2<f32>,
    mapping: u32,
    scale: f32,
) -> vec4<f32> {
    let s = max(scale, 1e-3);
    if (mapping == 1u) {
        let suv = (screen_uv - vec2<f32>(0.5)) / s + vec2<f32>(0.5);
        return textureSampleLevel(tex, samp, suv, 0.0);
    }
    let q = obj_pos * (0.5 / s) + vec3<f32>(0.5);
    var w = pow(abs(normal), vec3<f32>(4.0));
    w = w / max(w.x + w.y + w.z, 1e-5);
    let ax = textureSampleLevel(tex, samp, q.zy, 0.0);
    let ay = textureSampleLevel(tex, samp, q.xz, 0.0);
    let az = textureSampleLevel(tex, samp, q.xy, 0.0);
    return ax * w.x + ay * w.y + az * w.z;
}
