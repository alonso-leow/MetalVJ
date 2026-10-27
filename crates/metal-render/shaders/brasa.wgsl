// Brasa, the display: reads the heat grid and paints it through the fire
// palette. The camera, mirror fold and warp (shared kit) move the lookup.

struct BrasaU {
    fire: ColorSlot,
    background: ColorSlot,
    // aspect, time, res_x, res_y
    a: vec4<f32>,
    // pixel, paleta_pos, paleta_ancho, paleta_gamma
    b: vec4<f32>,
    // humo, distorsion_calor, brasas, pulse envelope
    c: vec4<f32>,
    // zoom, pan_x, pan_y, giro
    d: vec4<f32>,
    // deriva, brillo, contraste, opacidad
    e: vec4<f32>,
    // fondo_nivel, tono, saturacion, warp_cant
    f: vec4<f32>,
    // warp_freq, espejo, pulso, 0
    g: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: BrasaU;
@group(0) @binding(1) var state: texture_2d<f32>;
@group(0) @binding(2) var state_s: sampler;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    var p = kit_cam2d(in.uv, aspect, u.d.x, vec2<f32>(u.d.y, u.d.z), u.d.w, u.e.x, t);
    p = kit_mirror(p, u.g.y);
    p = kit_warp(p, u.f.w, u.g.x, t);
    var q = p / vec2<f32>(aspect, 1.0) + 0.5;
    let inside = q.x >= 0.0 && q.x <= 1.0 && q.y >= 0.0 && q.y <= 1.0;

    // Blockiness: snap the lookup to a coarser grid than the simulation.
    let res = vec2<f32>(u.a.z, u.a.w);
    let n = max(res * mix(1.0, 0.1, clamp(u.b.x, 0.0, 1.0)), vec2<f32>(4.0));
    if (u.b.x > 0.001) { q = (floor(q * n) + 0.5) / n; }

    // Heat haze: wobble the lookup in proportion to the heat under it.
    let h0 = textureSampleLevel(state, state_s, clamp(q, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).r;
    let wob = vec2<f32>(sin(q.y * 40.0 + t * 6.0), cos(q.x * 30.0 - t * 5.0)) * 0.012 * u.c.y * h0;
    let qs = clamp(q + wob, vec2<f32>(0.0), vec2<f32>(1.0));
    let st = textureSampleLevel(state, state_s, qs, 0.0);
    var heat = 0.0;
    var spark = 0.0;
    var smoke = 0.0;
    if (inside) { heat = st.r; spark = st.g; smoke = st.b; }

    var bg = backdrop_from_uv(u.background, in.uv, aspect) * u.f.x;
    bg = mix(bg, vec3<f32>(0.28, 0.27, 0.26), clamp(smoke, 0.0, 1.0) * 0.8);

    let v = pow(clamp(heat * u.b.z + u.b.y, 0.0, 1.0), max(u.b.w, 0.05));
    let cov = smoothstep(0.0, 0.06, v) * u.e.w;
    var col = mix(bg, grad4(u.fire, v), cov);
    col = col + vec3<f32>(1.0, 0.7, 0.3) * spark * spark * 1.5;
    col = col * (1.0 + u.g.z * u.c.w * 0.8);
    col = kit_look(col, u.e.y, u.e.z, u.f.y, u.f.z);
    return vec4<f32>(col, 1.0);
}
