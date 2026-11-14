// Paisaje: the paper, the sky wash and the sun disc. Everything else (clouds,
// mountains, mist, water, trees) is CPU ink drawn over this by paisaje_ink.wgsl.
// The sky is the `cielo` slot fading into the `papel` slot towards the horizon;
// the sun is a feathered acento disc with a dry-brush rim and a wide halo. The
// camera (zoom / pan / roll) is the same one the CPU strokes use, inverted here
// so the sun and sky stay glued to the picture.

struct U {
    sky: ColorSlot,
    paper: ColorSlot,
    accent: ColorSlot,
    a: vec4<f32>, // aspect, time, fondo_alpha, fondo_nivel
    b: vec4<f32>, // sol_x, sol_y, sol_tam, horizon world y
    c: vec4<f32>, // brillo, contraste, opacidad, tono
    d: vec4<f32>, // saturacion, cos(giro), sin(giro), zoom
    e: vec4<f32>, // pan_x, pan_y, centre world y, half frame height (world)
};

@group(0) @binding(0) var<uniform> u: U;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = u.a.x;
    let p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    // Inverse of the CPU camera: view -> world (frame height 2 units).
    let w = u.e.xy + vec2<f32>(u.d.y * p.x - u.d.z * p.y, u.d.z * p.x + u.d.y * p.y) / max(u.d.w, 0.01);
    let wy = u.e.z - w.y * u.e.w;

    let paper = grad4(u.paper, in.uv.y);
    let hz = max(u.b.w, 1.0);
    let sky_t = clamp((wy + 40.0) / (hz + 40.0), 0.0, 1.0);
    let sky_amt = 1.0 - smoothstep(0.0, 1.0, sky_t);
    var col = mix(paper, grad4(u.sky, sky_t), clamp(sky_amt * 0.95, 0.0, 1.0));

    // Sun: an ink-wash disc, a faint wet halo, a slightly irregular rim.
    let c = vec2<f32>(u.b.x * aspect, (0.5 - u.b.y) * 2.0);
    let r = max(u.b.z, 0.005);
    let rim = (quintic_noise(w * 14.0 + vec2<f32>(3.0, 9.0)) - 0.5) * 0.06 * r;
    let dd = length(w - c) + rim;
    let disc = 1.0 - smoothstep(r * 0.94, r, dd);
    let halo = exp(-pow(dd / (r * 2.4), 2.0)) * 0.5;
    col = mix(col, grad4(u.accent, 0.85), halo * (0.35 + 0.65 * sky_amt));
    let wash = 0.72 + 0.28 * lattice_fbm(w * 5.0 + vec2<f32>(1.0, 4.0), 3);
    col = mix(col, mix(grad4(u.accent, 0.45), grad4(u.accent, 0.2), 1.0 - wash), disc);

    col = col * u.a.w;
    col = kit_look(col, u.c.x, u.c.y, u.c.w, u.d.x);
    var alpha = u.c.z;
    if (u.a.z > 0.5) {
        alpha = alpha * u.a.w;
    }
    return vec4<f32>(col, alpha);
}
