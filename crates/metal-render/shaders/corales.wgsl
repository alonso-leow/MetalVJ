// Corales: background + the optional fill of the closed curve. The CPU
// rasterises the curve's even-odd interior into a small coverage mask (the
// camera already applied); this pass composes fondo, the fill (solid from the
// relleno slot, or a source picture laid by screen position) and the look
// kit. The strokes are drawn over it by corales_line.wgsl.

struct U {
    fill: ColorSlot,
    background: ColorSlot,
    a: vec4<f32>, // aspect, time, fondo_alpha, fondo_nivel
    b: vec4<f32>, // modo (0 none, 1 solid, 2 source), relleno_alfa, escala, giro
    c: vec4<f32>, // brillo, contraste, opacidad, tono
    d: vec4<f32>, // saturacion, has_picture, picture mix, mask texel x
    e: vec4<f32>, // mask texel y, 0, 0, 0
    mat: MaterialU, // the fill part's material; mat_mix 0 is off
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var mask_tex: texture_2d<f32>;
@group(0) @binding(2) var mask_samp: sampler;
@group(0) @binding(3) var pic_tex: texture_2d<f32>;
@group(0) @binding(4) var pic_samp: sampler;

fn cov(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(mask_tex, mask_samp, uv, 0.0).r;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = u.a.x;
    let bg = backdrop_from_uv(u.background, in.uv, aspect) * u.a.w;
    let mode = u.b.x;
    var m = 0.0;
    var col = vec3<f32>(0.0);
    if (mode > 0.5) {
        m = cov(in.uv);
        // Interior depth: a wide blur of the mask, so the fill is lit from the
        // inside and falls off towards the stroke.
        let tx = vec2<f32>(u.d.w, u.e.x) * 5.0;
        var acc = m;
        acc += cov(in.uv + vec2<f32>(tx.x, 0.0));
        acc += cov(in.uv - vec2<f32>(tx.x, 0.0));
        acc += cov(in.uv + vec2<f32>(0.0, tx.y));
        acc += cov(in.uv - vec2<f32>(0.0, tx.y));
        acc += cov(in.uv + tx);
        acc += cov(in.uv - tx);
        acc += cov(in.uv + vec2<f32>(tx.x, -tx.y));
        acc += cov(in.uv + vec2<f32>(-tx.x, tx.y));
        let depth = acc / 9.0;
        let base = grad4(u.fill, 0.2 + 0.8 * depth);
        if (mode > 1.5) {
            var p = (in.uv - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0);
            let a = u.b.w * 3.14159;
            let cs = cos(a);
            let sn = sin(a);
            p = vec2<f32>(cs * p.x - sn * p.y, sn * p.x + cs * p.y) / max(u.b.z, 0.01);
            if (u.d.y > 0.5) {
                let pic = textureSampleLevel(pic_tex, pic_samp, p + vec2<f32>(0.5), 0.0).rgb;
                col = mix(base, pic, clamp(u.d.z, 0.0, 1.0));
            } else {
                // No source connected: a procedural stand-in, stripes and a
                // slow checker graded through the relleno slot.
                let s = 0.5 + 0.5 * sin(p.x * 22.0 + 3.0 * sin(p.y * 7.0));
                let k = 0.5 + 0.5 * sin(p.y * 17.0);
                col = grad4(u.fill, clamp(0.15 + 0.6 * s + 0.25 * k * depth, 0.0, 1.0));
            }
        } else {
            col = base;
        }
    }
    // The fill part's material, on the flat 2D contract: the point is (uv, 0)
    // and the normal faces the camera, so it seasons the fill uniformly.
    if (m > 0.0 && u.mat.b.x > 0.0) {
        let z = vec3<f32>(0.0, 0.0, 1.0);
        let sh = material_apply(u.mat, pic_tex, pic_samp, col, vec3<f32>(in.uv, 0.0), z, in.uv, z, u.a.y);
        col = max(sh.albedo + vec3<f32>((1.0 - sh.roughness) * sh.metalness * 0.25), vec3<f32>(0.0));
    }
    let fa = m * u.b.y;
    var out = mix(bg, col, fa);
    out = kit_look(out, u.c.x, u.c.y, u.c.w, u.d.x);
    var alpha = u.c.z;
    if (u.a.z > 0.5) {
        alpha = alpha * max(fa, u.a.w);
    }
    return vec4<f32>(out, alpha);
}
