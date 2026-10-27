// Particulas, the draw pass: the background field, then one instanced sprite
// per body (six vertices, additive). Positions and velocities are read from
// the simulation's storage buffers in the vertex stage. Prepended with the
// prelude, the fullscreen vertex half and the kit WGSL.
//
// A sprite is a capsule: a round head of radius `rad` and a tail along the
// body's projected velocity (`estela`, `estela_largo`, or always for the
// streak texture). Everything is a function of the state and the clock, so a
// frame that advanced nothing is the same frame.

struct RU {
    vp: mat4x4<f32>,
    particle: ColorSlot,
    background: ColorSlot,
    // aspect, time, fondo_alpha, height in pixels
    a: vec4<f32>,
    // tam, tam_var, tam_audio, brillo_var
    b: vec4<f32>,
    // estela, estela_largo, textura, velocidad_color
    c: vec4<f32>,
    // destello, vref, 1 / radius scale, world scale
    d: vec4<f32>,
    // brillo, contraste, opacidad, fondo_nivel
    e: vec4<f32>,
    // tono, saturacion, base intensity, view distance
    f: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: RU;
@group(0) @binding(1) var<storage, read> pos: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> speed: array<vec4<f32>>;

@fragment
fn fs_bg(in: FsVOut) -> @location(0) vec4<f32> {
    let bg = backdrop_from_uv(u.background, in.uv, u.a.x) * u.e.w;
    let col = kit_look(bg, 1.0, 1.0, u.f.x, u.f.y);
    var alpha = u.e.z;
    if (u.a.z > 0.5) {
        alpha = alpha * u.e.w;
    }
    return vec4<f32>(col, alpha);
}

fn p_pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

fn p_unit(i: u32, k: u32) -> f32 {
    return f32(p_pcg(p_pcg(i * 0x9E3779B1u + 0x1234567u) ^ (k * 0x85EBCA6Bu + 0x27D4EB2Fu)) >> 8u) / 16777216.0;
}

struct POut {
    @builtin(position) pos: vec4<f32>,
    @location(0) q: vec2<f32>,
    @location(1) col: vec3<f32>,
    // tail length in radii, brightness, estela weight, kind
    @location(2) w: vec4<f32>,
};

@vertex
fn vs_p(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> POut {
    var corner = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let cn = corner[vi];
    var o: POut;
    o.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    o.q = vec2<f32>(0.0);
    o.col = vec3<f32>(0.0);
    o.w = vec4<f32>(0.0);

    let aspect = u.a.x;
    let kind = floor(u.c.z + 0.5);
    let p = pos[ii].xyz * u.d.w;
    let v = speed[ii].xyz;
    let clip = u.vp * vec4<f32>(p, 1.0);
    if (clip.w < 0.05) {
        return o;
    }
    let ndc = clip.xy / clip.w;

    let h1 = p_unit(ii, 1u);
    let h2 = p_unit(ii, 2u);
    let h3 = p_unit(ii, 3u);
    let speed = length(v);
    let sn = clamp(speed / max(u.d.y * 1.6, 1.0e-4), 0.0, 1.0);
    let rn = clamp(length(pos[ii].xy) * u.d.z, 0.0, 1.0);

    // Size: world radius projected, floored at a pixel so a far body still
    // reads, and dimmed by what the floor added.
    var scale = u.b.x * (1.0 + u.b.y * (h1 * 2.0 - 1.0)) * (1.0 + 2.0 * u.b.z * sn);
    if (kind > 2.5) {
        scale = scale * 2.4;
    }
    let persp = u.f.w / clip.w;
    let px = 2.0 / max(u.a.w, 1.0);
    let rad_true = 0.0032 * scale * persp;
    let rad = max(rad_true, 0.9 * px);
    var bright = u.f.z * mix(1.0, h2 * 2.0, u.b.w) * min(1.0, rad_true / rad);
    if (kind > 2.5) {
        bright = bright * 0.45;
    }
    // Twinkle: a per-body phase on a per-body rate.
    let tw = 0.5 + 0.5 * sin(u.a.y * (2.0 + 7.0 * h3) + h2 * 6.2831853);
    bright = bright * mix(1.0, tw * tw * 2.2, u.d.x);

    // Velocity on the screen, in aspect space (x scaled by aspect).
    let c2 = u.vp * vec4<f32>(p + v * u.d.w * 0.06, 1.0);
    var sv = (c2.xy / max(c2.w, 0.05) - ndc) * vec2<f32>(aspect, 1.0);
    let sl = length(sv);
    var axis = vec2<f32>(1.0, 0.0);
    if (sl > 1.0e-6) {
        axis = -sv / sl;
    }
    var est = u.c.x;
    var tail = sl * u.c.y * 2.0;
    if (kind > 1.5 && kind < 2.5) {
        tail = sl * (u.c.y * 2.0 + 2.0);
        est = max(est, 0.8);
    }
    tail = min(tail, 0.5);
    tail = select(0.0, tail, tail > rad);

    let along = select(-rad, tail, cn.x > 0.0);
    let off = axis * along + vec2<f32>(-axis.y, axis.x) * (cn.y * rad);
    o.pos = vec4<f32>(ndc + vec2<f32>(off.x / aspect, off.y), 0.0, 1.0);
    o.q = vec2<f32>(along / rad, cn.y);

    // Colour by radius and speed, then the look kit.
    let t = mix(rn, sn, u.c.w);
    var col = grad4(u.particle, t);
    col = kit_look(col, u.e.x, 1.0, u.f.x, u.f.y);
    col = pow(max(col, vec3<f32>(0.0)), vec3<f32>(u.e.y));
    o.col = col;
    o.w = vec4<f32>(tail / rad, bright, est, kind);
    return o;
}

fn profile(kind: f32, d: f32) -> f32 {
    if (kind < 0.5 || (kind > 1.5 && kind < 2.5)) {
        return exp(-d * d * 5.0) * smoothstep(1.0, 0.8, d);
    } else if (kind < 1.5) {
        let b = (d - 0.7) / 0.17;
        return exp(-b * b) * smoothstep(1.0, 0.9, d);
    }
    let g = 1.0 + d * d * 9.0;
    return smoothstep(1.0, 0.75, d) / (g * sqrt(g));
}

@fragment
fn fs_p(in: POut) -> @location(0) vec4<f32> {
    let L = in.w.x;
    let dx = clamp(in.q.x, 0.0, max(L, 0.0));
    let d = length(vec2<f32>(in.q.x - dx, in.q.y));
    if (d >= 1.0) {
        discard;
    }
    var cov = profile(in.w.w, d);
    if (L > 0.0) {
        let tt = clamp(in.q.x / L, 0.0, 1.0);
        let fade = in.w.z * (1.0 - tt) * (1.0 - tt);
        cov = cov * mix(1.0, fade, smoothstep(0.0, 0.6, in.q.x));
    }
    cov = cov * in.w.y;
    return vec4<f32>(in.col * cov, min(cov, 1.0) * u.e.z);
}
