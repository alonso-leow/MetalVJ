// Model 9 — Membrane. A living skin: a displaced, translucent surface (grid
// generated from the vertex index, no vertex buffer) with the old hair field
// rooted on it. Height = slow bulges pushed up from below + beat-driven
// ripples/impacts + pinch points + wrinkle detail, all analytic so the hairs
// can ride the very same surface.

struct SkinUniform {
    root_color: ColorSlot,
    tip_color: ColorSlot,
    floats: vec4<f32>,  // long, growth, density, wind_x
    floats2: vec4<f32>, // wind_y, wind_force, luminescence, time
    view_proj: mat4x4<f32>,
    f3: vec4<f32>,      // relief, bulge, wobble_speed, wrinkle
    f4: vec4<f32>,      // tension, pulse, impact, impact_age
    f5: vec4<f32>,      // impact_x, impact_y, pinch, translucency
    f6: vec4<f32>,      // rim, mode (0 surface, 1 hair), 0, 0
    cam: vec4<f32>,     // eye xyz
};

@group(0) @binding(0) var<uniform> u: SkinUniform;

const GRID_X: u32 = 200u;
const GRID_Y: u32 = 150u;
const HALF_X: f32 = 3.2;
const HALF_Y: f32 = 2.6;

fn ring(d: f32, r: f32, age: f32) -> f32 {
    let x = d - r;
    return sin(x * 11.0) * exp(-x * x * 5.0) * pow(max(1.0 - age, 0.0), 1.4);
}

fn height(p: vec2<f32>) -> f32 {
    let relief = u.f3.x;
    let ws = u.f3.z;
    let t = u.floats2.w * ws;
    var h = 0.0;

    // Pushes from below: broad blobs that swell and release.
    var bulge = 0.0;
    for (var k = 0; k < 4; k = k + 1) {
        let fk = f32(k);
        let c = vec2<f32>(sin(t * (0.23 + 0.07 * fk) + fk * 2.1) * 1.9,
                          cos(t * (0.19 + 0.05 * fk) + fk * 1.3) * 1.2);
        let d = length(p - c);
        let swell = 0.55 + 0.45 * sin(t * (0.8 + 0.2 * fk) + fk * 1.7);
        bulge = bulge + exp(-d * d * (1.1 + 0.3 * fk)) * swell;
    }
    h = h + u.f3.y * 0.9 * bulge;

    // Slow swell of the whole sheet.
    h = h + (quintic_noise(p * 0.8 + vec2<f32>(t * 0.13, -t * 0.09)) - 0.5) * 0.7 * relief;

    // Ambient impacts: three staggered rings, each born somewhere new.
    let pulse = u.f4.y;
    for (var k = 0; k < 3; k = k + 1) {
        let fk = f32(k);
        let ph = t * 0.42 + fk / 3.0;
        let cyc = floor(ph);
        let f = fract(ph);
        let o = vec2<f32>(hash11(cyc * 7.3 + fk * 3.1) - 0.5, hash11(cyc * 5.1 + fk * 9.7) - 0.5) * vec2<f32>(3.4, 2.4);
        h = h + pulse * 0.45 * ring(length(p - o), f * 3.6, f);
        // the dent at the point of impact
        h = h - pulse * 0.35 * exp(-dot(p - o, p - o) * 12.0) * pow(1.0 - f, 6.0);
    }

    // Driven impact (a beat ramps impact_age 0..1 from the loop).
    let age = clamp(u.f4.w, 0.0, 1.0);
    let io = vec2<f32>(u.f5.x, u.f5.y) * vec2<f32>(3.0, 2.2);
    let id = length(p - io);
    h = h + u.f4.z * 0.7 * ring(id, age * 4.0, age);
    h = h - u.f4.z * 0.6 * exp(-id * id * 9.0) * pow(1.0 - age, 5.0);

    // Pinch points: skin pulled into sharp peaks.
    var pinch = 0.0;
    for (var k = 0; k < 4; k = k + 1) {
        let fk = f32(k);
        let c = vec2<f32>(sin(t * 0.17 + fk * 1.9 + 0.5) * 2.1, cos(t * 0.14 + fk * 2.7) * 1.4);
        let d = length(p - c);
        let on = 0.5 + 0.5 * sin(t * 0.9 + fk * 2.3);
        pinch = pinch + pow(max(1.0 - d / 0.75, 0.0), 2.6) * (0.4 + 0.6 * on);
    }
    h = h + u.f5.z * 1.1 * pinch;

    // Wrinkles: ridged noise that smooths out where the skin is stretched.
    let q = p * 4.5 + vec2<f32>(t * 0.05, 0.0);
    let r1 = 1.0 - abs(quintic_noise(q) * 2.0 - 1.0);
    let r2 = 1.0 - abs(quintic_noise(q * 2.3 + 7.0) * 2.0 - 1.0);
    h = h + u.f3.w * (1.0 - 0.7 * u.f4.x) * 0.11 * (r1 * r1 + 0.5 * r2 * r2 - 0.8);

    return h * (0.18 + 0.55 * relief);
}

fn surf_normal(p: vec2<f32>) -> vec3<f32> {
    let e = 0.018;
    let dx = height(p + vec2<f32>(e, 0.0)) - height(p - vec2<f32>(e, 0.0));
    let dy = height(p + vec2<f32>(0.0, e)) - height(p - vec2<f32>(0.0, e));
    return normalize(vec3<f32>(-dx, -dy, 2.0 * e));
}

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) t: f32,
    @location(1) world: vec3<f32>,
    @location(2) nrm: vec3<f32>,
    @location(3) hair: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    var out: VOut;
    if (u.f6.y < 0.5) {
        // Surface grid: 6 vertices per cell.
        let cell = vi / 6u;
        let corner = vi % 6u;
        var offs = array<vec2<u32>, 6>(
            vec2<u32>(0u, 0u), vec2<u32>(1u, 0u), vec2<u32>(0u, 1u),
            vec2<u32>(1u, 0u), vec2<u32>(1u, 1u), vec2<u32>(0u, 1u),
        );
        let gx = cell % GRID_X + offs[corner].x;
        let gy = cell / GRID_X + offs[corner].y;
        let p = vec2<f32>(
            (f32(gx) / f32(GRID_X) * 2.0 - 1.0) * HALF_X,
            (f32(gy) / f32(GRID_Y) * 2.0 - 1.0) * HALF_Y,
        );
        let h = height(p);
        let n = surf_normal(p);
        out.world = vec3<f32>(p, h);
        out.nrm = n;
        out.t = h;
        out.hair = 0.0;
        out.clip = u.view_proj * vec4<f32>(out.world, 1.0);
        return out;
    }

    let id = f32(ii);
    let rp = vec2<f32>(
        (hash11(id * 12.9) * 2.0 - 1.0) * HALF_X,
        (hash11(id * 78.2 + 3.0) * 2.0 - 1.0) * HALF_Y,
    );
    let root = vec3<f32>(rp, height(rp));
    let n = surf_normal(rp);

    let long = u.floats.x;
    let growth = clamp(u.floats.y, 0.0, 1.0);
    let wind_dir = normalize(vec2<f32>(u.floats.w, u.floats2.x) + vec2<f32>(1e-4, 0.0));
    let wind_force = u.floats2.y;
    let time = u.floats2.w;

    let per_hair_len = long * (0.55 + 0.45 * hash11(id * 3.31 + 7.0)) * growth;
    let sway = sin(time * 1.6 * u.f3.z + id * 0.7) * wind_force * 0.35;
    let bend = wind_dir * (wind_force * 0.5 + sway);
    let dir = normalize(n * 1.5 + vec3<f32>(bend, 0.0));
    let tip = root + dir * per_hair_len;

    let width = 0.006 * (0.5 + 0.5 * hash11(id * 5.5));
    let side = normalize(cross(dir, vec3<f32>(0.0, 1.0, 0.0)) + vec3<f32>(1e-4, 0.0, 0.0));
    var corners = array<vec3<f32>, 3>(root + side * width, root - side * width, tip);
    var ts = array<f32, 3>(0.0, 0.0, 1.0);
    out.world = corners[vi];
    out.t = ts[vi];
    out.nrm = n;
    out.hair = 1.0;
    out.clip = u.view_proj * vec4<f32>(corners[vi], 1.0);
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let glow_amt = u.floats2.z;
    let translucency = u.f5.w;
    let rim_amt = u.f6.x;
    let eye = u.cam.xyz;
    let V = normalize(eye - in.world);

    if (in.hair > 0.5) {
        let base = mix(grad4(u.root_color, 0.6) * 0.8, grad4(u.tip_color, 1.0), in.t);
        let fres = pow(1.0 - clamp(abs(dot(normalize(in.nrm), V)), 0.0, 1.0), 2.0);
        let glow = glow_amt * smoothstep(0.5, 1.0, in.t);
        let col = base * (0.55 + glow * 2.2) + grad4(u.tip_color, 1.0) * rim_amt * fres * in.t * 0.8;
        return vec4<f32>(col, 1.0);
    }

    // Bump the interpolated normal with fine wrinkle detail.
    var N = normalize(in.nrm);
    let wr = u.f3.w;
    let bp = in.world.xy * 17.0 + vec2<f32>(u.floats2.w * 0.2 * u.f3.z, 0.0);
    let b0 = quintic_noise(bp);
    let bx = quintic_noise(bp + vec2<f32>(0.6, 0.0)) - b0;
    let by = quintic_noise(bp + vec2<f32>(0.0, 0.6)) - b0;
    N = normalize(N + vec3<f32>(-bx, -by, 0.0) * wr * 0.9);

    let h = in.world.z;
    let ht = clamp(0.5 + h * 0.9, 0.0, 1.0);
    let stretch = clamp((1.0 - in.nrm.z) * 3.0 * (0.4 + u.f4.x), 0.0, 1.0);
    let skin = mix(grad4(u.root_color, ht), grad4(u.tip_color, ht), ht * ht * 0.3);
    let flesh = grad4(u.tip_color, 0.85);

    let L = normalize(vec3<f32>(-0.45, 0.55, 0.75));
    let wrap = clamp((dot(N, L) + 0.45) / 1.45, 0.0, 1.0);
    // Light that passes through: strongest where the skin is raised or stretched thin.
    let thin = clamp(0.25 + ht * 0.9 + stretch * 1.2, 0.0, 1.6);
    let back = pow(clamp(dot(V, -normalize(L + N * 0.6)), 0.0, 1.0), 2.0);
    let sss = translucency * thin * (0.12 + 0.55 * back);

    // Capillaries: thin lines, brighter in the thin parts.
    let vn = abs(quintic_noise(in.world.xy * 3.2 + 3.0) * 2.0 - 1.0);
    let veins = (1.0 - smoothstep(0.0, 0.09, vn)) * (0.3 + 0.7 * translucency);

    let fres = pow(1.0 - clamp(dot(N, V), 0.0, 1.0), 3.0);
    let H = normalize(L + V);
    let spec = pow(clamp(dot(N, H), 0.0, 1.0), 70.0) * (0.5 + 0.8 * (1.0 - wr * 0.3));

    var col = skin * (0.08 + 0.6 * wrap * wrap);
    col = col + flesh * sss * (0.7 + glow_amt * 1.0);
    col = col * (1.0 - veins * 0.55) + grad4(u.root_color, 0.8) * veins * thin * 0.5;
    col = col + mix(skin, grad4(u.tip_color, 1.0), 0.35) * fres * rim_amt * 1.3;
    col = col + vec3<f32>(spec) * 0.3;
    // Ridges catch the glow, valleys fall into shadow.
    col = col * (0.75 + 0.5 * clamp(0.5 + h * 1.2, 0.0, 1.0));
    col = vec3<f32>(1.0) - exp(-col * 1.5);
    return vec4<f32>(col, 1.0);
}
