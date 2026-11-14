// Solvent's accumulate and blur on a device with no compute shaders (WebGL2):
// the same `accumulate_value` / `blur_value` as the compute entry points, drawn
// into the field's ping-pong pair by the model's fullscreen triangle (`vs_main`).

@fragment
fn fs_accumulate(in: VOut) -> @location(0) vec4<f32> {
    return accumulate_value(vec2<i32>(in.pos.xy));
}

@fragment
fn fs_blur(in: VOut) -> @location(0) vec4<f32> {
    return blur_value(vec2<i32>(in.pos.xy));
}
