// Particulas' draw on a device with no storage buffers (WebGL2): the bodies are
// read from the state textures (shaders/particulas_step_pp.wgsl) in the vertex
// stage, body i at texel (i % 256, i / 256).

@group(0) @binding(1) var tex_pos: texture_2d<f32>;
@group(0) @binding(2) var tex_vel: texture_2d<f32>;

fn body_pos(i: u32) -> vec4<f32> {
    return textureLoad(tex_pos, vec2<i32>(i32(i % 256u), i32(i / 256u)), 0);
}
fn body_vel(i: u32) -> vec4<f32> {
    return textureLoad(tex_vel, vec2<i32>(i32(i % 256u), i32(i / 256u)), 0);
}
