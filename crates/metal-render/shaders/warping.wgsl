// Warping, the display pass: shows the state texture through a 2D camera
// (zoom, pan, roll, drift), then LOOK (brightness, contrast, hue, saturation),
// with the fondo slot filling the dark parts. The camera sits after the
// feedback loop, so it moves the view without compounding into the frames.

struct WpDU {
    background: ColorSlot,
    // aspect, time, fondo_alpha, 0
    a: vec4<f32>,
    // zoom, pan_x, pan_y, giro
    b: vec4<f32>,
    // deriva, brillo, contraste, opacidad
    c: vec4<f32>,
    // fondo_nivel, tono, saturacion, 0
    d: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: WpDU;
@group(0) @binding(1) var st: texture_2d<f32>;
@group(0) @binding(2) var st_smp: sampler;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let p = kit_cam2d(in.uv, aspect, u.b.x, vec2<f32>(u.b.y, u.b.z), u.b.w, u.c.x, u.a.y);
    let suv = p / vec2<f32>(aspect, 1.0) + vec2<f32>(0.5);
    var col = textureSampleLevel(st, st_smp, suv, 0.0).rgb;
    let l = clamp(dot(col, vec3<f32>(0.299, 0.587, 0.114)), 0.0, 1.0);
    let bg = backdrop_from_uv(u.background, in.uv, aspect) * u.d.x;
    col = col + bg * (1.0 - l);
    col = kit_look(col, u.c.y, u.c.z, u.d.y, u.d.z);
    var alpha = u.c.w;
    if (u.a.z > 0.5) {
        alpha = alpha * max(l, u.d.x);
    }
    return vec4<f32>(col, alpha);
}
