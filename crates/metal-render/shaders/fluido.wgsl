// Fluido, the display pass: the dye field through the 2D camera, a warp and a
// mirror fold, lit as a thin liquid film (bump shading and a specular glint
// from the dye's own gradient), with iridescence, bloom and sunrays added,
// over the `fondo` backdrop.

struct FluidoU {
    background: ColorSlot,
    // aspect, time, dye texel.x, dye texel.y
    a: vec4<f32>,
    // sombreado, iridiscencia, bloom, sunrays
    b: vec4<f32>,
    // fondo_alpha, 0, 0, 0
    c: vec4<f32>,
    // zoom, pan_x, pan_y, giro
    e: vec4<f32>,
    // deriva, brillo, contraste, opacidad
    f: vec4<f32>,
    // fondo_nivel, tono, saturacion, 0
    g: vec4<f32>,
    // warp_cant, warp_freq, espejo, pulso
    h: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: FluidoU;
@group(0) @binding(1) var dye: texture_2d<f32>;
@group(0) @binding(2) var dye_smp: sampler;
@group(0) @binding(3) var bloom: texture_2d<f32>;
@group(0) @binding(4) var bloom_smp: sampler;
@group(0) @binding(5) var rays: texture_2d<f32>;
@group(0) @binding(6) var rays_smp: sampler;

fn lum_of(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.299, 0.587, 0.114)); }
fn dye_at(uv: vec2<f32>) -> vec3<f32> { return textureSampleLevel(dye, dye_smp, uv, 0.0).rgb; }

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let pulse = clamp(u.h.w, 0.0, 1.0);
    let uv0 = (in.uv - 0.5) / (1.0 + pulse * 0.08) + 0.5;

    var p = kit_cam2d(uv0, aspect, u.e.x, vec2<f32>(u.e.y, u.e.z), u.e.w, u.f.x, t);
    p = kit_warp(p, u.h.x, u.h.y, t);
    p = kit_mirror(p, u.h.z);
    let uv = vec2<f32>(p.x / aspect + 0.5, p.y + 0.5);
    let inside = select(0.0, 1.0, uv.x >= 0.0 && uv.y >= 0.0 && uv.x <= 1.0 && uv.y <= 1.0);

    let d = dye_at(uv);
    let lum = lum_of(d);
    var col = d;

    // Thin-film shading: the dye density is a height field.
    let e = u.a.zw * 1.5;
    let gx = lum_of(dye_at(uv + vec2<f32>(e.x, 0.0))) - lum_of(dye_at(uv - vec2<f32>(e.x, 0.0)));
    let gy = lum_of(dye_at(uv + vec2<f32>(0.0, e.y))) - lum_of(dye_at(uv - vec2<f32>(0.0, e.y)));
    let glen = length(vec2<f32>(gx, gy));
    let n = normalize(vec3<f32>(-gx * 6.0, -gy * 6.0, 1.0));
    let l = normalize(vec3<f32>(-0.45, -0.55, 0.7));
    let diff = clamp(dot(n, l), 0.0, 1.0);
    let sh = u.b.x;
    col = col * mix(1.0, 0.45 + 0.75 * diff, sh);
    let hv = normalize(l + vec3<f32>(0.0, 0.0, 1.0));
    let spec = pow(max(dot(n, hv), 0.0), 60.0);
    col = col + vec3<f32>(spec) * sh * 0.9 * smoothstep(0.02, 0.4, lum);

    // Iridescence: a cosine palette indexed by thickness and slope.
    let ir = 0.5 + 0.5 * cos(6.28318 * (vec3<f32>(0.0, 0.33, 0.67) + lum * 1.2 + glen * 14.0));
    col = mix(col, col * ir * 1.9 + ir * lum * 0.25, u.b.y * clamp(lum * 3.0, 0.0, 1.0));

    col = col + textureSampleLevel(bloom, bloom_smp, uv, 0.0).rgb * u.b.z;
    col = col + textureSampleLevel(rays, rays_smp, uv, 0.0).rgb * u.b.w;
    col = (vec3<f32>(1.0) - exp(-col * 1.3)) * inside;

    let bg = backdrop_from_uv(u.background, in.uv, aspect) * u.g.x;
    var out_col = bg + col * (1.0 - 0.5 * lum_of(bg));
    out_col = out_col * (1.0 + pulse * 0.5);
    out_col = kit_look(out_col, u.f.y, u.f.z, u.g.y, u.g.z);
    var alpha = u.f.w;
    if (u.c.x > 0.5) {
        alpha = alpha * max(clamp(lum_of(col) * 4.0, 0.0, 1.0), u.g.x);
    }
    return vec4<f32>(out_col, alpha);
}
