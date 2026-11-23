// Ondas, the display pass: the Ez/H field of the FDTD grid drawn through the
// 2D camera, a domain warp and a mirror fold. The signed field walks the
// `campo` palette; the dielectric and the slit wall are tinted by `material`;
// iso-lines and the flux monitor use `marca`.

struct OndasU {
    field: ColorSlot,
    material: ColorSlot,
    mark: ColorSlot,
    background: ColorSlot,
    // aspect, time, grid w, grid h
    a: vec4<f32>,
    // campo_modo, ganancia, isolineas, flujo_marcas
    b: vec4<f32>,
    // fondo_alpha, 0, 0, 0
    c: vec4<f32>,
    m0: vec4<f32>,
    m1: vec4<f32>,
    // zoom, pan_x, pan_y, giro
    e: vec4<f32>,
    // deriva, brillo, contraste, opacidad
    f: vec4<f32>,
    // fondo_nivel, tono, saturacion, 0
    g: vec4<f32>,
    // warp_cant, warp_freq, espejo, pulso
    h: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: OndasU;
@group(0) @binding(1) var st: texture_2d<f32>;
@group(0) @binding(2) var st_smp: sampler;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let size = vec2<f32>(u.a.z, u.a.w);
    let pulse = clamp(u.h.w, 0.0, 1.0);
    var uv = (in.uv - 0.5) / (1.0 + pulse * 0.12) + 0.5;

    var p = kit_cam2d(uv, aspect, u.e.x, vec2<f32>(u.e.y, u.e.z), u.e.w, u.f.x, t);
    p = kit_warp(p, u.h.x, u.h.y, t);
    p = kit_mirror(p, u.h.z);
    let g = p * size.y + size * 0.5;
    let inside = select(0.0, 1.0, g.x >= 0.0 && g.y >= 0.0 && g.x < size.x && g.y < size.y);
    let s = textureSampleLevel(st, st_smp, g / size, 0.0);
    let aa = max(max(fwidth(g.x), fwidth(g.y)), 0.001);

    let mode = i32(floor(u.b.x + 0.5));
    let gain = u.b.y;
    let sd = on_sd(g, u.m0, u.m1, size.y);
    let eps = on_eps(sd, u.m0);
    let cover_m = 1.0 - smoothstep(-aa, aa, sd);

    // The field value that picks the colour, and how much of it shows.
    var tcol = 0.5;
    var weight = 0.0;
    var fcol = vec3<f32>(0.0);
    if (mode == 3) {
        let sv = vec2<f32>(-s.x * s.z, s.x * s.y);
        let mag = length(sv);
        weight = tanh(gain * mag * 1.2);
        tcol = 0.5 + 0.5 * sv.x / max(mag, 1e-6);
        fcol = grad4(u.field, mix(0.5, 1.0, tcol)) * weight;
        tcol = 0.5 + 0.5 * weight;
    } else {
        var v = s.x;
        if (mode == 1) { v = abs(s.x); }
        if (mode == 2) { v = 0.5 * (eps * s.x * s.x + s.y * s.y + s.z * s.z) * 4.0; }
        weight = tanh(gain * v);
        tcol = 0.5 + 0.5 * weight;
        if (mode == 0) { weight = abs(weight); }
        fcol = grad4(u.field, tcol);
    }
    // Where there is no field the palette's middle shows; keep it in the
    // background's place by fading in with the field strength.
    var col = backdrop_from_uv(u.background, in.uv, aspect) * u.g.x;
    let bg = col;

    // Material: a faint fill for the dielectric, the wall solid.
    let tint = grad4(u.material, clamp((eps - 1.0) / 11.0, 0.0, 1.0));
    let walled = on_is_wall(sd, u.m0);
    if (walled) {
        col = mix(col, grad4(u.material, 1.0), cover_m * inside);
    } else {
        col = col + tint * 0.22 * cover_m * select(0.0, 1.0, eps > 1.0001) * inside;
    }

    var field = fcol;
    if (walled) { field = field * (1.0 - cover_m); }
    col = col + field * inside;

    // Iso-lines of the displayed value.
    if (u.b.z > 0.0) {
        let x = tcol * 10.0;
        let d = abs(fract(x + 0.5) - 0.5) / max(fwidth(x), 1e-4);
        let line = (1.0 - smoothstep(0.4, 1.4, d)) * u.b.z * clamp(weight * 4.0, 0.0, 1.0);
        col = mix(col, grad4(u.mark, 0.85), line * inside * 0.8);
    }

    // The flux monitor: a vertical line near the right edge, and beside it the
    // time-averaged intensity along that line as a bar profile.
    if (u.b.w > 0.0) {
        let xm = 0.88 * size.x;
        let ia = textureSampleLevel(st, st_smp, vec2<f32>(xm / size.x, g.y / size.y), 0.0).w;
        let len = tanh(gain * ia * 3.0) * 0.18 * size.x;
        let dl = abs(g.x - xm);
        let line = 1.0 - smoothstep(0.4 * aa, 1.4 * aa + 0.4, dl);
        let bar = select(0.0, 1.0, g.x <= xm && g.x > xm - len) * 0.55;
        let m = clamp(max(line, bar), 0.0, 1.0) * u.b.w * inside;
        col = mix(col, grad4(u.mark, mix(0.35, 1.0, line)), m);
    }

    col = col * (1.0 + pulse * 0.5);
    col = kit_look(col, u.f.y, u.f.z, u.g.y, u.g.z);
    var alpha = u.f.w;
    if (u.c.x > 0.5) {
        let cover = max(inside * clamp(weight + cover_m * 0.5, 0.0, 1.0), 0.0);
        alpha = alpha * max(cover, u.g.x);
    }
    return vec4<f32>(col, alpha);
}
