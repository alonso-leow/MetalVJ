// Fluido: every simulation and post pass in one shader, picked by `p0.x`.
// Navier-Stokes on a grid, the standard stable-fluids pipeline: splat, curl,
// vorticity confinement, divergence, pressure (Jacobi), gradient subtract,
// semi-Lagrangian advection of velocity and dye. Then the bloom and sunray
// helpers. Written from the algorithm; two textures are bound (a, b) and a
// pass reads the ones its mode names.
//
// Velocity is in grid cells per second, pressure/divergence/curl in the
// reference's units, dye in linear colour. Everything written is clamped so a
// float extreme cannot leave a non-finite or out-of-range texel behind.

struct FluidU {
    ink: ColorSlot,
    // mode, texel.x, texel.y, dt
    p0: vec4<f32>,
    // per-mode parameters
    p1: vec4<f32>,
    // velocity texel.x, velocity texel.y, aspect, resolution scale
    p2: vec4<f32>,
    // splat: x, y, force.x, force.y
    sp0: array<vec4<f32>, 12>,
    // splat: palette position, dye amount, radius (uv^2), 0
    sp1: array<vec4<f32>, 12>,
};

@group(0) @binding(0) var<uniform> u: FluidU;
@group(0) @binding(1) var ta: texture_2d<f32>;
@group(0) @binding(2) var sa_smp: sampler;
@group(0) @binding(3) var tb: texture_2d<f32>;
@group(0) @binding(4) var sb_smp: sampler;

fn fa(uv: vec2<f32>) -> vec4<f32> { return textureSampleLevel(ta, sa_smp, uv, 0.0); }
fn fb(uv: vec2<f32>) -> vec4<f32> { return textureSampleLevel(tb, sb_smp, uv, 0.0); }

fn tidy(v: vec4<f32>, m: f32) -> vec4<f32> {
    let z = select(v, vec4<f32>(0.0), v != v);
    return clamp(z, vec4<f32>(-m), vec4<f32>(m));
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let mode = i32(floor(u.p0.x + 0.5));
    let t = u.p0.yz;
    let dt = u.p0.w;
    let uv = in.uv;
    let dx = vec2<f32>(t.x, 0.0);
    let dy = vec2<f32>(0.0, t.y);

    if (mode == 0) {
        // curl of velocity
        let l = fa(uv - dx).y;
        let r = fa(uv + dx).y;
        let tp = fa(uv + dy).x;
        let b = fa(uv - dy).x;
        return tidy(vec4<f32>(0.5 * (r - l - tp + b), 0.0, 0.0, 1.0), 20000.0);
    }
    if (mode == 1) {
        // vorticity confinement: a = velocity, b = curl
        let l = fb(uv - dx).x;
        let r = fb(uv + dx).x;
        let tp = fb(uv + dy).x;
        let b = fb(uv - dy).x;
        let c = fb(uv).x;
        var f = 0.5 * vec2<f32>(abs(tp) - abs(b), abs(r) - abs(l));
        f = f / (length(f) + 0.0001);
        f = f * u.p1.x * c;
        f.y = -f.y;
        var v = fa(uv);
        v = vec4<f32>(v.xy + f * dt, 0.0, 1.0);
        return tidy(v, u.p1.y);
    }
    if (mode == 2) {
        // divergence, with reflecting walls
        let c = fa(uv).xy;
        var l = fa(uv - dx).x;
        var r = fa(uv + dx).x;
        var tp = fa(uv + dy).y;
        var b = fa(uv - dy).y;
        if (uv.x - t.x < 0.0) { l = -c.x; }
        if (uv.x + t.x > 1.0) { r = -c.x; }
        if (uv.y + t.y > 1.0) { tp = -c.y; }
        if (uv.y - t.y < 0.0) { b = -c.y; }
        return tidy(vec4<f32>(0.5 * (r - l + tp - b), 0.0, 0.0, 1.0), 20000.0);
    }
    if (mode == 3) {
        // pressure carried to the next frame, scaled
        return tidy(fa(uv) * u.p1.x, 20000.0);
    }
    if (mode == 4) {
        // one Jacobi iteration: a = pressure, b = divergence
        let l = fa(uv - dx).x;
        let r = fa(uv + dx).x;
        let tp = fa(uv + dy).x;
        let b = fa(uv - dy).x;
        let d = fb(uv).x;
        return tidy(vec4<f32>((l + r + b + tp - d) * 0.25, 0.0, 0.0, 1.0), 20000.0);
    }
    if (mode == 5) {
        // subtract the pressure gradient: a = pressure, b = velocity
        let l = fa(uv - dx).x;
        let r = fa(uv + dx).x;
        let tp = fa(uv + dy).x;
        let b = fa(uv - dy).x;
        var v = fb(uv).xy;
        v = v - vec2<f32>(r - l, tp - b);
        return tidy(vec4<f32>(v, 0.0, 1.0), u.p1.y);
    }
    if (mode == 6) {
        // advect: a = the field, b = velocity
        let speed = fb(uv).xy;
        let coord = uv - dt * speed * u.p2.xy;
        var r = fa(coord);
        r = r / (1.0 + u.p1.x * dt);
        return tidy(r, u.p1.y);
    }
    if (mode == 7) {
        // velocity splats and buoyancy: a = velocity, b = dye
        var v = fa(uv);
        let lum = dot(fb(uv).rgb, vec3<f32>(0.333));
        v = vec4<f32>(v.xy + u.p1.xy * (0.25 + clamp(lum, 0.0, 4.0)), 0.0, 1.0);
        let n = i32(u.p1.z);
        for (var i = 0; i < 12; i = i + 1) {
            if (i >= n) { break; }
            var d = uv - u.sp0[i].xy;
            d.x = d.x * u.p2.z;
            let g = exp(-dot(d, d) / max(u.sp1[i].z, 1e-5));
            v = vec4<f32>(v.xy + u.sp0[i].zw * g, 0.0, 1.0);
        }
        return tidy(v, u.p1.w);
    }
    if (mode == 8) {
        // dye splats: a = dye
        var c = fa(uv);
        let n = i32(u.p1.z);
        for (var i = 0; i < 12; i = i + 1) {
            if (i >= n) { break; }
            var d = uv - u.sp0[i].xy;
            d.x = d.x * u.p2.z;
            let g = exp(-dot(d, d) / max(u.sp1[i].z, 1e-5));
            c = vec4<f32>(c.rgb + grad4(u.ink, u.sp1[i].x) * u.sp1[i].y * g, 1.0);
        }
        return clamp(tidy(c, u.p1.y), vec4<f32>(0.0), vec4<f32>(u.p1.y));
    }
    if (mode == 9) {
        // bloom bright-pass with a soft knee
        let c = fa(uv).rgb;
        let br = max(c.r, max(c.g, c.b));
        let knee = 0.2;
        let th = u.p1.x;
        let soft = clamp(br - th + knee, 0.0, 2.0 * knee);
        let w = max(soft * soft / (4.0 * knee), br - th) / max(br, 0.0001);
        return vec4<f32>(c * max(w, 0.0), 1.0);
    }
    if (mode == 10) {
        // separable 9-tap blur: p1.xy is the step in uv
        let d = u.p1.xy;
        var s = fa(uv) * 0.2270;
        s = s + (fa(uv + d) + fa(uv - d)) * 0.1945;
        s = s + (fa(uv + d * 2.0) + fa(uv - d * 2.0)) * 0.1216;
        s = s + (fa(uv + d * 3.0) + fa(uv - d * 3.0)) * 0.0541;
        s = s + (fa(uv + d * 4.0) + fa(uv - d * 4.0)) * 0.0162;
        return vec4<f32>(s.rgb, 1.0);
    }
    if (mode == 11) {
        // sunray mask: only the bright part of the dye casts rays
        let c = fa(uv).rgb;
        let br = max(c.r, max(c.g, c.b));
        return vec4<f32>(c * smoothstep(0.12, 0.8, br), 1.0);
    }
    if (mode == 12) {
        // march toward the light centre, accumulating with decay
        let centre = u.p1.xy;
        let dec = u.p1.z;
        var acc = vec3<f32>(0.0);
        var w = 1.0;
        for (var i = 0; i < 24; i = i + 1) {
            let k = f32(i) / 24.0;
            acc = acc + fa(mix(uv, centre, k * 0.9)).rgb * w;
            w = w * dec;
        }
        return vec4<f32>(min(acc / 8.0, vec3<f32>(8.0)), 1.0);
    }
    // clear
    return vec4<f32>(0.0);
}
