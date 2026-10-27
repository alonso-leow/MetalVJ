// swirl: a vortex of colour bands. Written from first principles.
//
// Around a centre (optionally stretched into an ellipse) the picture is a
// function of angle and radius. The radius twists the angle (torsion, terraced
// by facetas, pulsing by torsion_pulso), the result is scaled to an integer
// number of bands (bandas) and perturbed by a radial ripple (ondulacion) and by
// noise (ruido, ruido_frec, turbulencia). Inside each band a split
// (proporcion, suavidad) chooses between slot 0 and slot 1 tones, degradado
// turns the split into a smooth ramp, borde draws slot 2 outlines between
// bands. A second counter-rotating vortex (gemelo) can take over part of the
// frame. Coverage fades towards the edge (desvanece) and opens a hole at the
// core (radio_nucleo), letting the fondo show through.

fn sw_ellipse(q: vec2<f32>) -> vec2<f32> {
    let e = f_eccentricity();
    let r = p_rot(q, -f_eccentricity_angle() * PI);
    return p_rot(vec2<f32>(r.x / (1.0 + e), r.y * (1.0 + e * 0.5)), f_eccentricity_angle() * PI);
}

// Band coordinate (angle in band units) and the radius used, for one vortex.
fn sw_u(q0: vec2<f32>, t: f32, dir: f32) -> vec3<f32> {
    let q = sw_ellipse(q0);
    var r = length(q);
    let fac = f_facets();
    r = mix(r, floor(r * 7.0 + 0.5) / 7.0, fac);
    let ang = atan2(q.y, q.x);
    let tor = f_torsion() * (1.0 + f_torsion_pulse() * sin(t * 1.3));
    var a = ang * dir + tor * 7.0 * log(1.0 + 4.0 * r) * dir + t * f_speed() * 0.6;
    let n = max(floor(f_bands() + 0.5), 1.0);
    var u = a / TAU * n;
    u = u + f_ripple() * 1.5 * sin(r * f_ripple_freq() * 18.0 - t * f_speed() * 2.0);
    // Noise sampled in the plane (no angle seam), plus a curl-ish second layer.
    let fq = q * (1.0 + f_noise_freq() * 9.0);
    let nz = p_fbm(fq + vec2<f32>(t * 0.15 * f_speed(), 3.0), 3) - 0.5;
    let tz = p_noise(fq * 0.7 + vec2<f32>(nz * 4.0, t * 0.2)) - 0.5;
    u = u + (nz * f_noise() * 2.0 + tz * f_turbulence() * 3.0) * max(n * 0.5, 1.0);
    return vec3<f32>(u, r, ang);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let c1 = vec2<f32>(f_center_x(), f_center_y()) * vec2<f32>(p_aspect() * 0.5, 0.5);
    let sep = f_twin_distance() * p_aspect();
    let c2 = c1 + vec2<f32>(sep, 0.15 * sep);
    let v1 = sw_u(p - c1, t, 1.0);
    var v = v1;
    if (f_twin() > 0.001) {
        let v2 = sw_u(p - c2, t, -1.0);
        let d1 = length(p - c1);
        let d2 = length(p - c2);
        let w = smoothstep(-0.12, 0.12, (d1 - d2) / max(f_twin(), 0.05) * 0.5 + (f_twin() - 0.5) * 0.2);
        v = mix(v1, v2, w * min(f_twin() * 2.0, 1.0));
    }
    let u = v.x;
    let r = v.y;
    let n = max(floor(f_bands() + 0.5), 1.0);
    let idx = floor(u);
    let f = u - idx;

    // Split inside the band: proporcion is where the second tone starts.
    let w = 0.002 + f_softness() * 0.5;
    let split = p_edge(f, clamp(f_proportion(), 0.02, 0.98), w);
    let ramp = mix(split, f, f_gradient());
    let pos = fract(idx / max(n, 1.0) * 0.999 + f_color_turn() * t * 0.1 + p_hash(vec2<f32>(idx, 7.0)) * 0.0);
    // Band tone alternates along the palette; slot 0 is the first tone, slot 1 the second.
    let ca = p_pal(0u, pos);
    let cb = p_pal(1u, fract(pos + 0.5 * f_proportion()));
    var col = mix(ca, cb, ramp);

    // Shading across each band: darker towards the band edge.
    let edge_d = min(f, 1.0 - f);
    col = col * (1.0 - f_shadow() * (1.0 - smoothstep(0.0, 0.5, edge_d)) * 0.8);

    // Outline between bands in slot 2.
    let bw = 0.01 + 0.08 * f_edge();
    let line = (1.0 - smoothstep(0.0, bw, edge_d)) * clamp(f_edge() * 4.0, 0.0, 1.0);
    col = mix(col, p_pal(2u, fract(pos * 2.0)), line);

    // Coverage: fade outwards, hole at the core.
    var cov = 1.0 - f_fade() * smoothstep(0.25, 1.1, length(p - c1));
    cov = cov * smoothstep(f_radius_core() * 0.5, f_radius_core() + 0.001, r);
    return vec4<f32>(col, cov);
}
