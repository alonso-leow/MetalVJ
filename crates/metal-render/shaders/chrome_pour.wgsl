// chrome-pour: a pool of liquid chrome with an animated 3D-lit surface.
// Behaviour follows the "liquid metal" tool: a beveled liquid body lit by a
// procedural environment with key, rim and fill lights, a Fresnel metal
// reflection, and a rotating surface that sweeps the highlights. Everything
// here is original.
//
// The pool is a polar blob (`pour`, `lobes`, `lobe_depth`) whose boundary is
// warped by noise (`warp`) and whose interior rises into a dome (`bulge`) with
// travelling ripples and flow noise. The height field is differenced for a
// normal, rotated in 3D by `speed`/`spin_dir`, then reflected into an
// environment built from the three slots. Coverage falls to zero at the pool
// rim and at the frame border, so the backdrop stays visible there.

fn cp_phase(a: f32, t: f32) -> f32 {
    let l = max(floor(f_lobes() + 0.5), 0.0);
    let s1 = sin(a * l + t * 0.35 + f_seed() * 1.7);
    let s2 = sin(a * (l + 2.0) - t * 0.27 + f_seed() * 0.9);
    return 1.0 + f_lobe_depth() * (0.42 * s1 + 0.20 * s2);
}

fn cp_q(p: vec2<f32>, t: f32) -> vec2<f32> {
    let s = max(f_pour_zoom(), 0.05);
    var q = p / s;
    q = p_rot(q, f_rotate() * TAU);
    let ph = t * f_flow_speed();
    let n = vec2<f32>(
        p_noise(q * 1.4 + vec2<f32>(ph, f_seed())),
        p_noise(q * 1.4 + vec2<f32>(3.7, -ph) - f_seed())
    ) - vec2<f32>(0.5);
    return q + n * f_warp() * 0.45;
}

fn cp_radius(q: vec2<f32>, t: f32) -> f32 {
    let a = atan2(q.y, q.x);
    let base = 0.20 + clamp(f_pour(), 0.0, 1.0) * 0.95;
    return max(base * cp_phase(a, t), 0.05);
}

fn cp_height(p: vec2<f32>, t: f32) -> f32 {
    let q = cp_q(p, t);
    let r = length(q);
    let rad = cp_radius(q, t);
    let d = rad - r;
    let bw = max(f_bevel() * clamp(f_stroke_weight(), 0.05, 2.0), 0.004);
    let wall = smoothstep(0.0, bw, d);
    let inv = clamp(r / rad, 0.0, 1.0);
    let dome = sqrt(max(1.0 - inv * inv, 0.0));
    var h = wall * (mix(0.55, 1.2, clamp(f_bulge(), 0.0, 1.0)) * dome + 0.16);
    let a = atan2(q.y, q.x);
    let ph = t * (0.8 + f_flow_speed());
    h = h + f_ripple() * 0.13 * sin(r * f_ripple_freq() - ph * 2.0 + a * 3.0 + f_seed());
    h = h + f_flow() * 0.24 * (p_fbm(q * 2.3 + vec2<f32>(ph * 0.4, f_seed()), 3) - 0.5);
    return h;
}

fn cp_spin(n: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    let k = i32(floor(clamp(f_spin_dir(), 0.0, 3.0) + 0.5));
    if (k == 0) {
        return vec3<f32>(c * n.x + s * n.z, n.y, -s * n.x + c * n.z);
    }
    if (k == 1) {
        return vec3<f32>(c * n.x - s * n.z, n.y, s * n.x + c * n.z);
    }
    if (k == 2) {
        return vec3<f32>(n.x, c * n.y - s * n.z, s * n.y + c * n.z);
    }
    return vec3<f32>(n.x, c * n.y + s * n.z, -s * n.y + c * n.z);
}

fn cp_env(d: vec3<f32>) -> vec3<f32> {
    let h = clamp(d.y * 0.5 + 0.5, 0.0, 1.0);
    let w = 0.02 + clamp(f_roughness(), 0.0, 1.0) * 0.36;
    let ground = p_pal(0u, 0.02) * 0.6;
    let horizon = p_pal(0u, 0.70);
    let zenith = p_pal(1u, 0.12) * 0.30 + p_pal(0u, 0.95) * 0.35;
    var sky = mix(ground, horizon, smoothstep(0.5 - w, 0.5 + w, h));
    sky = mix(sky, zenith, smoothstep(0.55 - w, 0.95 + w, h));

    let keyAlign = dot(d, normalize(vec3<f32>(0.5, 0.78, 0.42)));
    let rimAlign = dot(d, normalize(vec3<f32>(-0.65, 0.15, -0.5)));
    let fillAlign = dot(d, normalize(vec3<f32>(-0.2, -0.55, 0.8)));
    let key = smoothstep(0.55 - w, 1.0, keyAlign);
    let shoulder = smoothstep(0.25 - w, 0.55 + w, keyAlign);
    let rim = smoothstep(0.72 - w, 1.0, rimAlign);
    let fill = smoothstep(0.78 - w, 1.0, fillAlign);

    sky = sky + key * vec3<f32>(1.25, 1.2, 1.1) * f_key_light() * 0.9;
    sky = sky + shoulder * vec3<f32>(0.3, 0.32, 0.42) * f_key_light() * 0.5;
    sky = sky + rim * p_pal(1u, 0.35) * f_rim_light();
    sky = sky + fill * p_pal(2u, 0.65) * f_fill_light();
    return sky;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let q = cp_q(p, t);
    let sd = length(q) - cp_radius(q, t);
    let soft = 0.004 + clamp(f_edge_soft(), 0.0, 1.0) * 0.22;
    var cov = smoothstep(soft, -soft, sd);
    // Guaranteed transparent frame border, independent of the pool size.
    let ax = abs(p.x) / max(p_aspect() * 0.5, 0.01);
    let ay = abs(p.y) / 0.5;
    cov = cov * (1.0 - smoothstep(0.86, 0.995, max(ax, ay)));

    // Surface normal from the height field.
    let e = 0.006;
    let hx = cp_height(p + vec2<f32>(e, 0.0), t) - cp_height(p - vec2<f32>(e, 0.0), t);
    let hy = cp_height(p + vec2<f32>(0.0, e), t) - cp_height(p - vec2<f32>(0.0, e), t);
    var n = normalize(vec3<f32>(-hx * 0.30 / (2.0 * e), -hy * 0.30 / (2.0 * e), 1.0));
    n = normalize(cp_spin(n, t * f_speed() * 1.4 + f_seed() * 0.6));

    let h = cp_height(p, t);
    let metal = p_pal(0u, clamp(0.28 + 0.58 * n.y + 0.14 * h, 0.0, 1.0));
    let L = normalize(vec3<f32>(0.5, 0.75, 0.65));
    let V = vec3<f32>(0.0, 0.0, 1.0);

    let diff = max(dot(n, L), 0.0);
    var col = metal * (clamp(f_ambient(), 0.0, 1.0) + clamp(f_diffuse(), 0.0, 1.0) * diff);

    let refl = reflect(-V, n);
    let env = cp_env(refl);
    let fres = pow(clamp(1.0 - n.z, 0.0, 1.0), 5.0) * clamp(f_fresnel(), 0.0, 2.0);
    let floor_ref = 0.02 + clamp(f_reflection(), 0.0, 1.0) * 0.34;
    let reflectance = mix(floor_ref, 1.0, clamp(fres, 0.0, 1.0));
    let envmix = mix(metal * 0.8, env, clamp(f_background_reflection(), 0.0, 1.0));
    col = col + envmix * reflectance;

    let H = normalize(L + V);
    let spec = pow(max(dot(n, H), 0.0), 48.0);
    col = col + p_pal(1u, 0.9) * spec * clamp(f_specular(), 0.0, 2.0);

    let tint = p_pal(2u, clamp(0.5 + 0.5 * n.y, 0.0, 1.0));
    col = mix(col, col * tint * 1.6 + tint * 0.03, clamp(f_tint(), 0.0, 1.0));

    col = max(col, vec3<f32>(0.0));
    return vec4<f32>(col * cov * (1.0 + 0.06 * clamp(f_specular() / 2.0, 0.0, 1.0)), cov);
}
