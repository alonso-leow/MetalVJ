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
    // A lit page: a soft vignette and a faint paper tooth, so the plot sits on
    // a surface instead of a flat fill.
    let c = (in.uv - vec2<f32>(0.5)) * vec2<f32>(u.a.x, 1.0);
    let vig = 1.0 - 0.45 * smoothstep(0.15, 0.85, dot(c, c) * 1.6);
    let tc = floor(in.uv * vec2<f32>(u.a.x, 1.0) * 540.0);
    let tooth = 0.97 + 0.06 * fract(sin(dot(tc, vec2<f32>(127.1, 311.7))) * 43758.547);
    let rgb = kit_look(bg * vig * tooth, u.b.x, u.b.y, u.b.z, u.b.w);
    var alpha = u.a.w;
    if (u.a.y > 0.5) {
        alpha = alpha * u.a.z;
    }
    return vec4<f32>(rgb, alpha);
}
