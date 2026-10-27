// Optional source picture for image-input scaffold shaders (shared/scaffold.rs,
// "Image-input shaders"). Included only when the ScaffoldDef declares a surface.
// Binding 1/2 = the part's texture/sampler; P.a.w = picture aspect, 0 = none.
// Written from scratch for MetalVJ.

@group(0) @binding(1) var p_src_tex: texture_2d<f32>;
@group(0) @binding(2) var p_src_smp: sampler;

fn p_has_src() -> bool { return P.a.w > 0.0; }

// Picture coordinate p (centred, y down, one unit tall) -> 0..1 picture uv,
// "cover" fitted so the picture fills the frame without stretching.
fn p_src_uv(p: vec2<f32>) -> vec2<f32> {
    let fa = p_aspect();
    var u = vec2<f32>(p.x / fa + 0.5, p.y + 0.5);
    let ia = select(fa, P.a.w, P.a.w > 0.0);
    if (fa > ia) { u.y = (u.y - 0.5) * (ia / fa) + 0.5; }
    else { u.x = (u.x - 0.5) * (fa / ia) + 0.5; }
    return u;
}

// Procedural stand-in: hue gradient, diagonal stripes, a disc and a ring.
fn p_test_image(u: vec2<f32>) -> vec4<f32> {
    let hue = u.x * 0.85 + u.y * 0.35;
    var c = 0.5 + 0.5 * cos(TAU * (hue + vec3<f32>(0.0, 0.33, 0.67)));
    let st = step(0.5, fract(u.x * 9.0 + u.y * 2.0));
    c = mix(c, c * 0.2, st * 0.75);
    let d = length(u - vec2<f32>(0.5, 0.5));
    c = mix(c, vec3<f32>(1.0, 0.95, 0.8), 1.0 - smoothstep(0.17, 0.19, d));
    c = mix(c, vec3<f32>(0.05, 0.05, 0.1), (1.0 - smoothstep(0.012, 0.02, abs(d - 0.3))) * 0.9);
    return vec4<f32>(c, 1.0);
}

fn p_mirror01(u: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(1.0) - abs(fract(u * 0.5) * 2.0 - vec2<f32>(1.0));
}

// Linear rgba of the picture (or the test image) at a 0..1 picture uv.
fn p_src_at_uv(uv: vec2<f32>) -> vec4<f32> {
    let m = p_mirror01(uv);
    if (p_has_src()) { return textureSampleLevel(p_src_tex, p_src_smp, m, 0.0); }
    return p_test_image(m);
}

fn p_src(p: vec2<f32>) -> vec4<f32> { return p_src_at_uv(p_src_uv(p)); }
