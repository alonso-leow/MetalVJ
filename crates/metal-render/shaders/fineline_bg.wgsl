// Fineline: the paper. fondo slot walked top to bottom, graded by the look
// kit. The strokes are drawn over it by fineline_line.wgsl.

struct U {
    background: ColorSlot,
    a: vec4<f32>, // aspect, fondo_alpha, fondo_nivel, opacidad
    b: vec4<f32>, // brillo, contraste, tono, saturacion
};

@group(0) @binding(0) var<uniform> u: U;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let bg = backdrop_from_uv(u.background, in.uv, u.a.x) * u.a.z;
    let rgb = kit_look(bg, u.b.x, u.b.y, u.b.z, u.b.w);
    var alpha = u.a.w;
    if (u.a.y > 0.5) {
        alpha = alpha * u.a.z;
    }
    return vec4<f32>(rgb, alpha);
}
