// gemsmoke: a glassy gem with smoke flowing inside and around it.
//;
// everything here is original.
//
// A signed distance `d` of the chosen shape (diamond, circle, hexagon, square,
// four-point star; rotated, stretched, rounded, breathing) splits the picture.
// Smoke is a domain-warped fbm that rises (`llama`), swirls (`remolino`) and
// drifts (`humo_vel`). Inside the gem it is sampled through faceted, lens-like
// refraction (`facetas`, `faceta_fuerza`, `refraccion`, bent by
// `distorsion_in`), coloured from slot 0 and tinted toward slot 1 by `cuerpo`,
// with an inner glow (`brillo_in`), a rim (`borde`, `borde_brillo`) and facet
// sparkles (`destello`). Outside, a halo of smoke (bent by `distorsion_out`,
// reach `halo_radio`, strength `brillo_out`) fades with distance; coverage is
// the union of the gem and the halo. Slot 2 colours the glows and sparkles.

fn gem_sd(q: vec2<f32>, r: f32, kind: i32) -> f32 {
    if (kind == 0) { return (abs(q.x) + abs(q.y)) * 0.70710678 - r * 0.70710678 * 1.2; }
    if (kind == 1) { return length(q) - r; }
    if (kind == 2) {
        let k = vec3<f32>(-0.8660254, 0.5, 0.57735027);
        var a = abs(q);
        a = a - 2.0 * min(dot(k.xy, a), 0.0) * k.xy;
        a = a - vec2<f32>(clamp(a.x, -k.z * r, k.z * r), r);
        return length(a) * sign(a.y);
    }
    if (kind == 3) { return max(abs(q.x), abs(q.y)) - r * 0.8; }
    // Four-point star: pinched diamond.
    let a = abs(q);
    return pow(pow(a.x, 0.5) + pow(a.y, 0.5), 2.0) - r * 0.55;
}

fn smoke_at(x: vec2<f32>, distance: f32) -> f32 {
    let t = p_time();
    var s = x * f_smoke_scale() * 3.0;
    let sw = f_swirl() * (1.6 - min(length(x) * 2.0, 1.6));
    s = p_rot(s, sw);
    s = s + vec2<f32>(t * f_smoke_speed() * 0.6, -t * f_flame());
    let w = vec2<f32>(p_fbm(s + vec2<f32>(0.0, t * 0.1), 3), p_fbm(s + vec2<f32>(5.2, 1.3 - t * 0.1), 3));
    let n = p_fbm(s + (w - 0.5) * 2.5 * distance, 4);
    return clamp((n - 0.5) * f_contrast_smoke() + 0.5, 0.0, 1.0);
}

fn tint(ink: f32) -> vec3<f32> {
    let cyc = ink + p_time() * f_smoke_color_speed() * 0.1;
    let ab = f_aberration() * 0.12;
    let c = fract(cyc);
    let tri = 1.0 - abs(c * 2.0 - 1.0);
    return vec3<f32>(p_pal(0u, clamp(tri + ab, 0.0, 1.0)).r, p_pal(0u, tri).g, p_pal(0u, clamp(tri - ab, 0.0, 1.0)).b);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let breath = 1.0 + f_beat() * 0.18 * sin(t * 3.0);
    let r = f_size() * 0.45 * breath;
    var q = p_rot(p, f_angle() * TAU);
    q.x = q.x / f_stretch();
    let kind = clamp(i32(floor(f_shape() + 0.5)), 0, 4);
    let e = f_corners() * 0.5;
    let d = (gem_sd(q, r * (1.0 - e), kind) - e * r) ;
    let aa = max(fwidth(d), 1e-4);
    let inside = 1.0 - smoothstep(-aa, aa, d);
    let nd = clamp(-d / r, 0.0, 1.0);
    let glowc = p_pal(2u, 0.5);

    var rgb = vec3<f32>(0.0);
    var cov = 0.0;

    // Halo of smoke around the gem.
    let dd = max(d, 0.0);
    let reach = 0.02 + f_halo_radius() * 0.5;
    let fall = exp(-dd / reach);
    let ink_o = smoke_at(p + normalize(q + vec2<f32>(1e-4, 0.0)) * dd * 0.5, f_distortion_out());
    let ho = f_shine_out() * fall * (0.35 + 0.9 * ink_o);
    let halo_a = clamp(ho, 0.0, 1.0);
    let halo_c = mix(tint(ink_o), glowc, clamp(fall * f_shine_out() * 0.35, 0.0, 1.0));
    rgb = halo_c;
    cov = halo_a * (1.0 - inside);

    if (inside > 0.0) {
        // Faceted refraction.
        let n = max(floor(f_facets() + 0.5), 3.0);
        let ang = atan2(q.y, q.x) / TAU * n;
        let fi = floor(ang);
        let fh = p_hash2(vec2<f32>(fi, 7.0));
        let fl = fract(ang);
        var qi = q * (1.0 - f_refraction() * (1.0 - nd) * 0.9);
        qi = qi + (fh - 0.5) * f_facet_force() * r * 0.9;
        let ink_i = smoke_at(qi / max(r, 1e-3) * 0.35, f_distortion_in());
        var col = tint(ink_i);
        col = mix(col, p_pal(1u, ink_i), f_body());
        // Facet shading and seams.
        col = col * (1.0 - f_facet_force() * 0.35 * (fh.x - 0.5));
        col = col * (1.0 - 0.5 * f_facet_force() * (1.0 - smoothstep(0.0, 0.06, min(fl, 1.0 - fl))));
        // Inner glow toward the edge.
        let ig = pow(1.0 - nd, 2.5) * f_shine_in();
        col = col + glowc * ig * (0.4 + ink_i);
        col = col * (0.6 + 0.4 * f_shine_in());
        // Rim.
        let rim = 1.0 - smoothstep(0.0, max(f_edge() * r + aa, 1e-4), -d);
        col = col + glowc * rim * f_edge_shine();
        // Sparkle.
        let sp = step(0.85, fract(fh.y + t * 0.35)) * smoothstep(0.0, 0.3, nd) * (1.0 - nd);
        col = col + vec3<f32>(1.0) * sp * f_flash() * 1.5;
        rgb = mix(rgb, col, inside);
        cov = max(cov, inside);
    }
    return vec4<f32>(rgb, clamp(cov, 0.0, 1.0));
}
