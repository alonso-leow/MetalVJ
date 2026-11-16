// simplexnoise: a palette stepped onto 3D simplex gradient noise. Behaviour
//; everything
// here is original.
//
// The noise is real simplex noise: the point is skewed onto a tetrahedral grid
// (F = 1/3), the four surrounding corners are found by ordering the offsets, and
// each contributes a radial kernel (0.6 - d^2)^4 times a hashed gradient dot
// product. The third axis is time, so the picture evolves instead of sliding.
// Octaves rotate the domain by a growing angle (no axis-aligned artefacts); a
// curl-like flow advects the domain along the perpendicular of a low-frequency
// noise gradient; the field is optionally ridged, biased, hardened, then cut
// into `pasos_color` x 4 bands mapped through the slot gradients.

fn sx_hash(i: vec3<i32>) -> u32 {
    var h = bitcast<u32>(i.x) * 0x9E3779B1u ^ bitcast<u32>(i.y) * 0x85EBCA77u ^ bitcast<u32>(i.z) * 0xC2B2AE3Du;
    h = (h ^ (h >> 15u)) * 0x2C1B3C6Du;
    h = (h ^ (h >> 12u)) * 0x297A2D39u;
    return h ^ (h >> 15u);
}

fn sx_grad(i: vec3<i32>) -> vec3<f32> {
    let h = sx_hash(i);
    // Direction from two hashed angles (uniform on the sphere).
    let z = f32(h & 0xFFFFu) / 32767.5 - 1.0;
    let a = f32((h >> 16u) & 0xFFFFu) / 65535.0 * TAU;
    let r = sqrt(max(1.0 - z * z, 0.0));
    return vec3<f32>(r * cos(a), r * sin(a), z);
}

fn sx_corner(i: vec3<i32>, d: vec3<f32>) -> f32 {
    let t = 0.6 - dot(d, d);
    if (t <= 0.0) { return 0.0; }
    let t2 = t * t;
    return t2 * t2 * dot(sx_grad(i), d);
}

// 3D simplex gradient noise, roughly -1..1.
fn sx_noise(v: vec3<f32>) -> f32 {
    let s = (v.x + v.y + v.z) * (1.0 / 3.0);
    let ci = floor(v + vec3<f32>(s));
    let g = (ci.x + ci.y + ci.z) * (1.0 / 6.0);
    let d0 = v - (ci - vec3<f32>(g));
    // Which simplex of the cube: order the offsets.
    let ge_xy = d0.x >= d0.y;
    let ge_yz = d0.y >= d0.z;
    let ge_xz = d0.x >= d0.z;
    var o1 = vec3<i32>(0);
    var o2 = vec3<i32>(0);
    if (ge_xy) {
        if (ge_yz) { o1 = vec3<i32>(1, 0, 0); o2 = vec3<i32>(1, 1, 0); }
        else if (ge_xz) { o1 = vec3<i32>(1, 0, 0); o2 = vec3<i32>(1, 0, 1); }
        else { o1 = vec3<i32>(0, 0, 1); o2 = vec3<i32>(1, 0, 1); }
    } else {
        if (!ge_yz) { o1 = vec3<i32>(0, 0, 1); o2 = vec3<i32>(0, 1, 1); }
        else if (!ge_xz) { o1 = vec3<i32>(0, 1, 0); o2 = vec3<i32>(0, 1, 1); }
        else { o1 = vec3<i32>(0, 1, 0); o2 = vec3<i32>(1, 1, 0); }
    }
    let i0 = vec3<i32>(ci);
    let G = 1.0 / 6.0;
    let d1 = d0 - vec3<f32>(o1) + vec3<f32>(G);
    let d2 = d0 - vec3<f32>(o2) + vec3<f32>(2.0 * G);
    let d3 = d0 - vec3<f32>(1.0) + vec3<f32>(3.0 * G);
    let n = sx_corner(i0, d0) + sx_corner(i0 + o1, d1) + sx_corner(i0 + o2, d2) + sx_corner(i0 + vec3<i32>(1), d3);
    return clamp(n * 32.0, -1.0, 1.0);
}

// Octave sum: domain rotated per octave, optional ridging. Returns 0..1.
fn sx_field(q: vec2<f32>, z: f32, oct: i32, lac: f32, pers: f32, rot: f32, ridge: f32) -> f32 {
    var p = q;
    var amp = 1.0;
    var sum = 0.0;
    var norm = 0.0;
    for (var i = 0; i < 5; i = i + 1) {
        if (i >= oct) { break; }
        let n = sx_noise(vec3<f32>(p, z + f32(i) * 3.7));
        let r = 1.0 - abs(n) * 2.0;           // ridged: crest where n crosses 0
        sum = sum + amp * mix(n, r, ridge);
        norm = norm + amp;
        p = p_rot(p, 0.4 + rot * 2.2 + f32(i) * rot * 0.7) * lac + vec2<f32>(5.2, 1.3);
        amp = amp * pers;
    }
    return 0.5 + 0.5 * sum / max(norm, 1e-4);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let T = p_time();
    let t = T * f_speed();
    var q = p_rot(p, f_rotation() * TAU) * 2.2 * f_scale() + vec2<f32>(f_off_x(), f_off_y()) * 2.0;
    q = vec2<f32>(q.x / f_stretch(), q.y * f_stretch());
    let z = t * 0.35 + f_phase_z() * 8.0;

    // Curl-like flow: perpendicular of the gradient of a slow low-frequency field.
    let fl = f_flow();
    if (fl > 0.001) {
        let fq = q * 0.45 * f_flow_scale();
        let fz = z * 0.5 + T * f_flow_speed() * 0.3 + 11.0;
        let e = 0.08;
        let nx = sx_noise(vec3<f32>(fq + vec2<f32>(e, 0.0), fz)) - sx_noise(vec3<f32>(fq - vec2<f32>(e, 0.0), fz));
        let ny = sx_noise(vec3<f32>(fq + vec2<f32>(0.0, e), fz)) - sx_noise(vec3<f32>(fq - vec2<f32>(0.0, e), fz));
        q = q + vec2<f32>(ny, -nx) * (fl * 3.2 / (2.0 * e)) * 0.35;
    }

    let oct = clamp(i32(floor(f_octaves() + 0.5)), 1, 5);
    var v = sx_field(q, z, oct, f_lacunarity(), f_persistence(), f_rot_octave(), f_crests());

    // Bias and hardness (histogram stretch around the middle).
    v = v + f_bias();
    v = 0.5 + (v - 0.5) * (0.7 + f_hardness() * 2.6);
    v = clamp(v, 0.0, 0.9999);

    // Steps: `pasos_color` bands per palette colour (4), hard unless softened.
    let nb = max(floor(f_steps_color() + 0.5), 1.0) * 4.0;
    let x = v * nb;
    let id = floor(x);
    let fr = x - id;
    let w = clamp(fwidth(x) + f_softness(), 0.001, 1.0);
    let edge = smoothstep(1.0 - w, 1.0, fr);
    let banded = (id + edge) / nb;
    let s2 = f_softness() * f_softness();
    var u = mix(banded, v, s2);
    // Re-expand so the first and last band reach the end colours.
    u = clamp(u * nb / max(nb - 1.0, 1.0), 0.0, 1.0);

    // Outlines on the band boundaries.
    let line = (1.0 - smoothstep(0.0, f_edges_width(), min(fr, 1.0 - fr))) * f_edges();

    // Colour: two palettes, blended globally and by a slow spatial field.
    let ph = u + f_drift_color() * T * 0.1;
    let uc = fract(ph + 4.0) ;
    let uu = select(clamp(u, 0.0, 1.0), uc, abs(f_drift_color()) > 0.001);
    let sv = sx_noise(vec3<f32>(q * 0.25, z * 0.3 + 40.0)) * 0.5 + 0.5;
    let mz = clamp(f_palette_mix() + (sv - 0.5) * f_palette_variation() * 1.6, 0.0, 1.0);
    var col = p_pal2(0u, uu, 1u, uu, mz);
    col = mix(col, p_pal(2u, uu), clamp(line, 0.0, 1.0));

    // Grain.
    let gs = max(f_grain_size(), 0.1);
    let n = p_hash(floor(uv * vec2<f32>(P.a.x, 1.0) * 700.0 / gs) + vec2<f32>(floor(T * 12.0) * 13.0, 5.0));
    col = col * (1.0 + (n - 0.5) * f_grain() * 0.8);

    // Coverage: the lowest steps let the backdrop through.
    let a = 1.0 - f_empty() * (1.0 - smoothstep(0.0, 0.5, u));
    return vec4<f32>(col, clamp(a, 0.0, 1.0));
}
