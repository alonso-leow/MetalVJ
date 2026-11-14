// Turing's reaction on a device with no compute shaders (WebGL2): the same
// `seed_value` / `step_value` as the compute entry points, drawn into the
// other half of the field's ping-pong pair by a fullscreen triangle.

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_seed(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return seed_value(vec2<i32>(pos.xy));
}

@fragment
fn fs_step(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return step_value(vec2<i32>(pos.xy));
}
