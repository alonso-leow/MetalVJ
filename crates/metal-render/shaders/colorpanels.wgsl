// colorpanels: a carousel of translucent glass panels standing in a ring
// around a vertical axis, turning slowly, seen from the front. Back panels are
// laid down first, front panels over them, so the colours stack like gels.
//
// Each panel i sits at azimuth a_i = fase*TAU + t*velocidad + i*TAU/count.
// A flat panel tangent to the ring has projected half-width ancho*|cos a|, a
// centre at radio*sin a, and a depth z = cos a that scales it by a
// perspective factor. Tilt (angulo_1 at the top edge, angulo_2 at the bottom)
// shears it sideways along its length. Along the length t in 0..1 the alpha
// ramps in over `fade_in` and out over `fade_out`; `bordes` lights the rim,
// `blur` softens the sides, `gradiente` lets the colour travel along the panel.
//
// Written from the visible behaviour only; nothing here is upstream code.

const MAXP: i32 = 24;

fn panel(p: vec2<f32>, i: i32, count: f32, back: bool, acc: ptr<function, vec4<f32>>) {
    let fi = f32(i);
    let weight = clamp(count - fi, 0.0, 1.0);
    if (weight <= 0.0) { return; }

    let a = f_phase() * TAU + p_time() * f_speed() + fi * TAU / max(count, 1.0);
    let z = cos(a);
    if ((z < 0.0) != back) { return; }

    let persp = 1.0 / max(1.0 - f_perspective() * z, 0.2);
    let long = f_long() * persp;

    // Vertical position along the panel, 0 at the top end, 1 at the bottom.
    let t = p.y / long + 0.5;
    if (t < -0.02 || t > 1.02) { return; }
    let tn = clamp(t, 0.0, 1.0);

    // Optional sway so a long panel is not a ruler.
    let sway = f_ripple() * 0.12 * sin(tn * 5.0 + fi * 1.7 + p_time() * 0.9);
    let tilt = mix(f_angle_1(), f_angle_2(), tn) * (tn - 0.5) * long;
    let xc = f_radius() * sin(a) * persp + tilt + sway;
    let hw = max(f_width() * abs(z) * persp, 0.004);

    let dx = (p.x - xc) / hw;
    let w = max(fwidth(dx), 1e-4);
    let soft = 0.02 + f_blur() * 0.9;
    let side = 1.0 - smoothstep(1.0 - soft - w, 1.0 + w, abs(dx));

    // Alpha along the length.
    let fin = select(smoothstep(0.0, f_fade_in(), tn), 1.0, f_fade_in() < 0.001);
    let fout = select(1.0 - smoothstep(1.0 - f_fade_out(), 1.0, tn), 1.0, f_fade_out() < 0.001);
    let ends = smoothstep(-0.02, 0.0, t) * (1.0 - smoothstep(1.0, 1.02, t));
    var alpha = side * fin * fout * ends * f_crystal() * weight;
    if (alpha <= 0.0005) { return; }

    // Colour: one stop per panel (golden-ratio hop keeps neighbours apart),
    // dragged along the length and across the width by `gradiente`.
    let base = fract(fi * 0.618034 + 0.07);
    let ct = clamp(base + f_gradient() * ((tn - 0.5) * 0.7 + dx * 0.12), 0.0, 1.0);
    var col = p_pal(0u, ct);

    // Lit rim: brighter toward the side edges, strongest on the front panels.
    let rim = smoothstep(0.72, 1.0, abs(dx)) * side;
    col = col + f_edges() * rim * (0.35 + 0.65 * col) * (0.6 + 0.4 * z);
    alpha = clamp(alpha + f_edges() * rim * 0.35 * weight * fin * fout, 0.0, 1.0);

    // Back panels read darker, as seen through the glass in front.
    col = col * mix(0.6, 1.0, z * 0.5 + 0.5);

    // Premultiplied "over".
    let prev = *acc;
    *acc = vec4<f32>(prev.rgb * (1.0 - alpha) + col * alpha, prev.a * (1.0 - alpha) + alpha);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let count = max(f_density() * 2.5, 1.0);
    var acc = vec4<f32>(0.0);
    for (var pass_i = 0; pass_i < 2; pass_i = pass_i + 1) {
        for (var i = 0; i < MAXP; i = i + 1) {
            panel(p, i, count, pass_i == 0, &acc);
        }
    }
    let cov = clamp(acc.a, 0.0, 1.0);
    return vec4<f32>(acc.rgb / max(cov, 1e-4), cov);
}
