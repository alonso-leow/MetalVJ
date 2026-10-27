// magnet-spikes: a ferrofluid sphere whose surface is pushed out by a
// time-varying 3D value-noise field. Behaviour and parameter list follow the
// built-in "Ferrofluid Spikes" tool; this implementation is original.
//
// The body is an analytic sphere SDF at the origin. Its radius is the blob
// size plus a small low-frequency wobble plus `spike_height` times a sharpened
// two-octave noise field sampled on the surface direction. A camera orbits the
// origin with yaw/pitch integration (`direction` picks the axis and sign,
// `rotation_speed` the rate) and rays are sphere-traced inside a bounding
// sphere. The hit point gets a cheap metal model: a fluid-colour body lit by
// one key light, a procedural sky/key/rim environment read through the
// reflection vector with Fresnel weight, a glossiness-driven specular lobe,
// a rim term and an SDF-based ambient-occlusion darkening. A miss stays
// transparent so the scaffold's `fondo` slot is the tool's background well.

fn ms_roty(v: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

fn ms_rotx(v: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(v.x, c * v.y - s * v.z, s * v.y + c * v.z);
}

// Smooth 3D value noise over the shared integer lattice hash.
fn ms_n3(q: vec3<f32>) -> f32 {
    let i = floor(q);
    let f = q - i;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);

    let n000 = lattice3(i);
    let n100 = lattice3(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = lattice3(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = lattice3(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = lattice3(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = lattice3(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = lattice3(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = lattice3(i + vec3<f32>(1.0, 1.0, 1.0));

    let nx00 = mix(n000, n100, u.x);
    let nx10 = mix(n010, n110, u.x);
    let nx01 = mix(n001, n101, u.x);
    let nx11 = mix(n011, n111, u.x);
    let nxy0 = mix(nx00, nx10, u.y);
    let nxy1 = mix(nx01, nx11, u.y);
    return mix(nxy0, nxy1, u.z);
}

// Sharpened spike height in 0..1. Two octaves at 2.03x, like the tool's
// density control, then `sharpness` biases the distribution towards tips.
fn ms_spikes(dir: vec3<f32>, tt: f32) -> f32 {
    let freq = max(f_spike_density(), 0.05);
    let seed = vec3<f32>(f_seed() * 3.7, f_seed() * 1.9, 0.0);
    let a = ms_n3(dir * freq + vec3<f32>(0.0, 0.0, tt * 0.55) + seed);
    let b = ms_n3(dir * (freq * 2.03) + vec3<f32>(tt * 0.37, 0.0, 0.0) + seed);
    var n = (a - 0.5) + 0.5 * (b - 0.5);
    n = n / 1.5;
    n = n * 0.5 + 0.5;
    return pow(clamp(n, 0.0, 1.0), max(f_sharpness(), 0.2));
}

fn ms_map(q: vec3<f32>, tt: f32) -> f32 {
    let r = max(length(q), 1e-4);
    let dir = q / r;
    var surface = max(f_blob_size(), 0.05);

    // Broad organic swell, separate from the sharp spikes.
    let wp = q * max(f_wobble_freq(), 0.1)
        + vec3<f32>(f_seed() * 3.1, -f_seed() * 2.3, tt * 0.3);
    surface = surface + (ms_n3(wp) - 0.5) * 2.0 * f_wobble();

    surface = surface + max(f_spike_height(), 0.0) * ms_spikes(dir, tt);
    return r - surface;
}

fn ms_normal(q: vec3<f32>, tt: f32) -> vec3<f32> {
    let e = 0.0018;
    let dx = ms_map(q + vec3<f32>(e, 0.0, 0.0), tt) - ms_map(q - vec3<f32>(e, 0.0, 0.0), tt);
    let dy = ms_map(q + vec3<f32>(0.0, e, 0.0), tt) - ms_map(q - vec3<f32>(0.0, e, 0.0), tt);
    let dz = ms_map(q + vec3<f32>(0.0, 0.0, e), tt) - ms_map(q - vec3<f32>(0.0, 0.0, e), tt);
    return normalize(vec3<f32>(dx, dy, dz));
}

// Procedural environment: a sky ramp, a hot key lobe and a cool back lobe.
fn ms_env(d: vec3<f32>, key: vec3<f32>) -> vec3<f32> {
    let up = clamp(d.y * 0.5 + 0.5, 0.0, 1.0);
    let sky = mix(vec3<f32>(0.008, 0.010, 0.016), p_pal(1u, 0.55) * 0.55, pow(up, 1.5));
    let hot = smoothstep(0.86, 1.0, dot(d, key));
    let back_dir = normalize(vec3<f32>(-key.x, key.y * 0.35, -key.z));
    let cold = smoothstep(0.72, 1.0, dot(d, back_dir));
    return sky
        + p_pal(1u, 0.96) * hot * 1.8
        + p_pal(1u, 0.38) * cold * 0.65;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let tt = p_time() * f_speed();

    // Direction enum: 0 left, 1 up, 2 down, 3 right. It picks the orbit axis
    // and sign; `rotation_speed` scales the accumulated angle.
    let di = i32(floor(f_direction() + 0.5));
    let rs = f_rotation_speed() * tt;
    var yaw = 0.0;
    var pitch = f_pitch();
    if (di == 0) {
        yaw = -rs;
    } else if (di == 1) {
        pitch = pitch - rs;
    } else if (di == 2) {
        pitch = pitch + rs;
    } else {
        yaw = rs;
    }

    // p is centred with a one-unit height; turn it into a y-up screen plane.
    let ndc = p * 2.0;
    let local_ro = vec3<f32>(0.0, 0.0, max(f_cam_dist(), 1.0));
    let local_rd = normalize(vec3<f32>(ndc.x, -ndc.y, -2.0));
    let ro = ms_roty(ms_rotx(local_ro, pitch), yaw);
    let rd = ms_roty(ms_rotx(local_rd, pitch), yaw);

    // Bounding sphere so background pixels cost one intersection test.
    let bound = max(f_blob_size(), 0.05)
        + max(f_spike_height(), 0.0)
        + f_wobble()
        + 0.08;
    let b = dot(ro, rd);
    let c = dot(ro, ro) - bound * bound;
    let disc = b * b - c;
    if (disc < 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let root = sqrt(disc);
    let t0 = -b - root;
    let t1 = -b + root;
    if (t1 < 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    var travel = max(t0, 0.0);
    let t_end = t1 + 0.06;
    var hit = false;
    var pnt = vec3<f32>(0.0);
    for (var i = 0; i < 96; i = i + 1) {
        let q = ro + rd * travel;
        let d = ms_map(q, tt);
        if (d < 0.0012) {
            hit = true;
            pnt = q;
            break;
        }
        travel = travel + max(d * 0.62, 0.0025);
        if (travel > t_end) {
            break;
        }
    }
    if (!hit) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    let n = ms_normal(pnt, tt);
    let view = -rd;
    let la = f_light_angle() * TAU;
    let light = normalize(vec3<f32>(cos(la) * 0.55, 0.78, sin(la) * 0.55));
    let half_v = normalize(light + view);

    let diff = max(dot(n, light), 0.0);
    let fres = pow(clamp(1.0 - max(dot(n, view), 0.0), 0.0, 1.0), 5.0) * f_fresnel();
    let refl = reflect(rd, n);
    let env_col = ms_env(refl, light);

    // Body colour walks the fluid slot with height and a slow surface mottle.
    let mottle = p_noise(pnt.xy * 2.3 + vec2<f32>(tt * 0.11, f_seed()));
    let body = p_pal(0u, clamp(0.30 + 0.48 * n.y + 0.18 * (mottle - 0.5), 0.0, 1.0));

    let amb = clamp(f_ambient(), 0.0, 1.0);
    var rgb = body * (amb + (1.0 - amb) * diff * 0.9);

    // Metal environment reflection weighted by Fresnel, plus the key specular.
    rgb = rgb + env_col * f_reflectivity() * mix(0.04, 1.0, clamp(fres, 0.0, 1.0));
    let spec = pow(max(dot(n, half_v), 0.0), max(f_glossiness(), 1.0));
    rgb = rgb + p_pal(1u, clamp(fres, 0.0, 1.0)) * spec * f_specular();

    // Rim light on the grazing silhouette.
    let rim = pow(clamp(1.0 - max(dot(n, view), 0.0), 0.0, 1.0), 3.0);
    rgb = rgb + p_pal(1u, 0.78) * rim * f_rim();

    // Cheap SDF ambient occlusion.
    let ao_d = 0.12;
    let ao = clamp(ms_map(pnt + n * ao_d, tt) / ao_d, 0.0, 1.0);
    rgb = rgb * mix(0.55, 1.0, ao);

    return vec4<f32>(max(rgb, vec3<f32>(0.0)), 1.0);
}
