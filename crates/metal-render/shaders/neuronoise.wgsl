// neuronoise: a glowing web of neural filaments. Behaviour and parameter
// list written from the neuro noise" effect; everything here is
// original.
//
// The picture is built by folding the plane several times (iteraciones). Each
// pass rotates the coordinate (giro_capas), bends it with travelling sine
// swirls (cant_warp, turbulencia) and reads a family of thin lines from a
// sine of a noise-bent axis (densidad, anisotropia, grosor, nitidez). Where two
// line families cross, nodes light up (nodos, pulso_nodos). Layers add with a
// falloff (caida). Colour walks front -> mid with the pass index and the line
// intensity (mezcla_medio, fase_color); chispas adds travelling bright beads.

fn nn_layer(q0: vec2<f32>, i: f32, t: f32, speed: f32) -> vec3<f32> {
    // returns (line, node, bead)
    var q = q0;
    let sk = f_symmetry();
    q = vec2<f32>(mix(q.x, abs(q.x), sk), q.y);
    let w = t * speed;
    let sw = f_amount_warp() * 0.7;
    q = q + sw * vec2<f32>(sin(q.y * 1.7 + w + i * 1.3), cos(q.x * 1.4 - w * 0.8 + i * 2.1));
    let turb = p_noise(q * 0.9 + vec2<f32>(i * 5.0, w * 0.35));
    q = q + f_turbulence() * (turb - 0.5) * 2.2;

    let dens = 2.0 + f_density() * 9.0;
    let ax = vec2<f32>(1.0, mix(1.0, 0.15, f_anisotropy()));
    let bend = (p_noise(q * 0.8 * ax + vec2<f32>(0.0, i * 3.7)) - 0.5) * 6.0;
    let s1 = sin(q.x * dens * ax.x + bend + f_drift_fiber() * w);
    let s2 = sin(q.y * dens * ax.y * 0.8 - bend * 0.7 - f_drift_fiber() * w * 0.6);

    // line = 1 near zero crossings, width from grosor, edge sharpness nitidez.
    let wd = 0.02 + f_thickness() * 0.5;
    let sharp = 1.0 + f_sharpness() * 5.0;
    let l1 = pow(clamp(1.0 - abs(s1) / (wd + 0.02), 0.0, 1.0), sharp);
    let l2 = pow(clamp(1.0 - abs(s2) / (wd + 0.02), 0.0, 1.0), sharp);
    let line = max(l1, l2 * 0.8);

    let pulse = 0.6 + 0.4 * sin(t * 2.4 * f_pulse_nodes() * 2.0 + i * 1.9 + bend);
    let pn = mix(1.0, pulse, clamp(f_pulse_nodes(), 0.0, 1.0));
    let node = l1 * l2 * pn;

    let bead_p = p_noise(vec2<f32>(q.y * 3.0 + i * 7.0, q.x * 0.5 - w * 2.2));
    let bead = line * smoothstep(0.72, 0.9, bead_p);
    return vec3<f32>(line, node, bead);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let speed = f_speed();
    var q = p * (0.6 + f_scale() * 2.4);
    let n = clamp(floor(f_iterations() + 0.5), 1.0, 8.0);

    var acc = 0.0;
    var nodes = 0.0;
    var beads = 0.0;
    var cmix = 0.0;
    var wgt = 1.0;
    var wsum = 0.0;
    for (var k = 0; k < 8; k = k + 1) {
        if (f32(k) >= n) { break; }
        let i = f32(k);
        q = p_rot(q, f_turn_layers() * 0.9 + 0.35 + 0.1 * i);
        q = q * (1.0 + 0.12 * i);
        let r = nn_layer(q, i, t, speed);
        acc = acc + r.x * wgt;
        nodes = nodes + r.y * wgt;
        beads = beads + r.z * wgt;
        cmix = cmix + i * r.x * wgt;
        wsum = wsum + wgt;
        wgt = wgt * mix(0.45, 0.95, f_fall());
    }
    let inv = 1.0 / max(wsum, 1e-3);
    let line = acc * inv * (1.0 + 0.6 * f32(n) * 0.25);
    let layer_pos = cmix / max(acc, 1e-3) / max(n, 1.0);

    // Contrast around mid grey of the line field, then brightness.
    let c = 0.5 + f_contrast_neural() * 2.5;
    var v = clamp((line - 0.5) * c + 0.5, 0.0, 1.5);
    v = max(v + f_shine_neural() * 0.5, 0.0);

    let tc = clamp(v * f_mix_medium() + layer_pos * (1.0 - f_mix_medium()) + f_phase_color() * 0.5 * sin(t * 0.5), 0.0, 1.0);
    var col = mix(p_pal(1u, tc), p_pal(0u, clamp(v, 0.0, 1.0)), clamp(v, 0.0, 1.0));
    col = col * v;
    col = col + p_pal(0u, 0.8) * nodes * inv * f_nodes() * 3.0;
    col = col + p_pal(0u, 1.0) * beads * inv * f_sparks() * 4.0;
    col = vec3<f32>(1.0) - exp(-col * 1.5);
    let cov = clamp(max(col.r, max(col.g, col.b)), 0.0, 1.0);
    return vec4<f32>(col, cov);
}
