// Nubes, the display pass: the Euler state drawn as clouds. The state is
// tiled (the domain is periodic), pushed through the 2D camera, a domain warp
// and a mirror fold, then read as density, pressure, vorticity or a schlieren
// image of the density gradient and walked through the `nube` ramp. Density
// gets a cloud look: a bump term from the density gradient and a short march
// toward the light that darkens what sits behind thicker cloud.

struct NubesU {
    cloud: ColorSlot,
    background: ColorSlot,
    // aspect, time, grid w, grid h
    a: vec4<f32>,
    // modo, contraste_nube, schlieren, sombra
    b: vec4<f32>,
    // fondo_alpha, luz_x, luz_y, gamma
    c: vec4<f32>,
    // rho_lo, rho_hi, ambient pressure, Eref
    d: vec4<f32>,
    // zoom, pan_x, pan_y, giro
    e: vec4<f32>,
    // deriva, brillo, contraste, opacidad
    f: vec4<f32>,
    // fondo_nivel, tono, saturacion, vorticity scale
    g: vec4<f32>,
    // warp_cant, warp_freq, espejo, pulso
    h: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: NubesU;
@group(0) @binding(1) var st: texture_2d<f32>;
@group(0) @binding(2) var st_smp: sampler;

fn fetch(g: vec2<f32>) -> vec4<f32> {
    let s = textureSampleLevel(st, st_smp, g / vec2<f32>(u.a.z, u.a.w), 0.0);
    return clamp(s, vec4<f32>(-1.0e3), vec4<f32>(1.0e3));
}

// Normalised density in [0, 1] (a little outside while it overshoots).
fn dens(g: vec2<f32>) -> f32 {
    let span = max(u.d.y - u.d.x, 0.05);
    return clamp((fetch(g).x - u.d.x) / span, -0.3, 1.3);
}

fn velocity(g: vec2<f32>) -> vec2<f32> {
    let s = fetch(g);
    return s.yz / max(s.x, 0.02);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let size = vec2<f32>(u.a.z, u.a.w);
    let pulse = clamp(u.h.w, 0.0, 1.0);
    let uv = (in.uv - 0.5) / (1.0 + pulse * 0.12) + 0.5;

    var p = kit_cam2d(uv, aspect, u.e.x, vec2<f32>(u.e.y, u.e.z), u.e.w, u.f.x, t);
    p = kit_warp(p, u.h.x, u.h.y, t);
    p = kit_mirror(p, u.h.z);
    // The grid spans the frame height; x is tiled by the repeat sampler.
    let g = p * size.y + size * 0.5;

    let mode = i32(floor(u.b.x + 0.5));
    let contrast = u.b.y;
    let light = normalize(vec2<f32>(u.c.y, -u.c.z) + vec2<f32>(1.0e-4, 0.0));
    let shadow = u.b.w;

    let gxm = dens(g - vec2<f32>(1.0, 0.0));
    let gxp = dens(g + vec2<f32>(1.0, 0.0));
    let gym = dens(g - vec2<f32>(0.0, 1.0));
    let gyp = dens(g + vec2<f32>(0.0, 1.0));
    let grad = 0.5 * vec2<f32>(gxp - gxm, gyp - gym);
    let gmag = length(grad);

    var tone = 0.5;
    var cover = 1.0;
    var shade = 1.0;
    let rho_n = dens(g);

    if (mode == 0) {
        tone = clamp(0.5 + (rho_n - 0.5) * contrast, 0.0, 1.0);
        cover = smoothstep(0.12, 0.7, tone);
        // Bump: the side facing the light is brighter.
        let bump = clamp(-dot(grad, light) * 6.0, -1.0, 1.0);
        // March toward the light: denser cloud between here and the light casts a
        // shadow, so a flat body stays bright and the lee of a billow goes dark.
        var occl = 0.0;
        for (var k = 1; k <= 6; k = k + 1) {
            let q = clamp(0.5 + (dens(g + light * (2.5 * f32(k))) - 0.5) * contrast, 0.0, 1.0);
            occl = occl + max(q - tone, 0.0);
        }
        let trans = exp(-shadow * 1.6 * occl);
        shade = mix(1.0, trans, shadow) * (1.0 + 0.6 * shadow * bump);
    } else if (mode == 1) {
        let s = fetch(g);
        let rho = max(s.x, 0.02);
        let v = s.yz / rho;
        let pr = (s.w + u.d.w - 0.5 * rho * dot(v, v)) * (u.c.w - 1.0);
        tone = 0.5 + 0.5 * tanh((pr - u.d.z) / u.d.z * 3.0 * contrast);
    } else if (mode == 2) {
        let vm = velocity(g - vec2<f32>(1.0, 0.0));
        let vp = velocity(g + vec2<f32>(1.0, 0.0));
        let vd = velocity(g - vec2<f32>(0.0, 1.0));
        let vu = velocity(g + vec2<f32>(0.0, 1.0));
        let w = 0.5 * (vp.y - vm.y) - 0.5 * (vu.x - vd.x);
        tone = 0.5 + 0.5 * tanh(w / max(u.g.w, 1.0e-3) * contrast * 2.0);
    }

    // Schlieren: the density gradient as an edge image (the whole picture in
    // mode 3, an overlay in the others).
    let sk = u.b.z + select(0.0, 1.0, mode == 3);
    let edge = 1.0 - exp(-gmag * 7.0 * sk);
    var col = grad4(u.cloud, tone) * shade;
    if (mode == 3) {
        tone = clamp(edge * contrast * 0.7, 0.0, 1.0);
        col = grad4(u.cloud, tone);
        cover = clamp(edge * contrast, 0.0, 1.0);
    } else if (u.b.z > 0.0) {
        col = mix(col, grad4(u.cloud, 1.0), clamp(edge, 0.0, 1.0) * 0.8);
        cover = max(cover, edge * 0.8);
    }

    let bg = backdrop_from_uv(u.background, in.uv, aspect) * u.g.x;
    var out = mix(bg, col, cover);
    out = out * (1.0 + pulse * 0.5);
    out = kit_look(out, u.f.y, u.f.z, u.g.y, u.g.z);
    var alpha = u.f.w;
    if (u.c.x > 0.5) {
        alpha = alpha * max(cover, u.g.x);
    }
    return vec4<f32>(out, alpha);
}
