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

// 2026-11-25: 16 panel forms. (x across, y along) both in [-1, 1]; returns
// a metric that is 1 on the panel's outline.
fn cp_form(x: f32, y: f32) -> f32 {
    let k = i32(clamp(round(f_panel_form()), 0.0, 15.0));
    let ax = abs(x); let ay = abs(y);
    switch (k) {
        case 1: { return 0.6 + length(max(vec2<f32>(ax, ay) - vec2<f32>(0.6), vec2<f32>(0.0))); }
        case 2: { return length(vec2<f32>(x, y)); }
        case 3: { return ax + ay; }
        case 4: { return ax / max((1.0 - y) * 0.5, 0.02); }
        case 5: { return ax / max((1.0 + y) * 0.5, 0.02); }
        case 6: { return select(ax, length(vec2<f32>(x, (y + 0.4) / 0.6)), y < -0.4); }
        case 7: { return max(ax, (ax * 0.5 + ay * 0.866) * 1.05); }
        case 8: { return ax / (0.6 + 0.4 * (y * 0.5 + 0.5)); }
        case 9: { return abs(x + y * 0.5) / 0.75; }
        case 10: { return length(vec2<f32>(x, y)) * 1.3; }
        case 11: { return min(max(ax, ay * 3.0), max(ax * 3.0, ay)); }
        case 12: { let a = atan2(y, x); return length(vec2<f32>(x, y)) / (0.7 + 0.3 * cos(a * 5.0)); }
        case 13: { return ax + 0.15 * sin(y * 9.0); }
        case 14: { return max(ax, 1.0 - min(abs(fract(y * 2.0) - 0.5) * 4.0, 1.0) * 0.4 + ax * 0.6); }
        case 15: { return ax / (0.35 + 0.65 * ay); }
        default: { return ax; }
    }
}

fn panel(p: vec2<f32>, i: i32, count: f32, back: bool, acc: ptr<function, vec4<f32>>) {
    let fi = f32(i);
    let weight = clamp(count - fi, 0.0, 1.0);
    if (weight <= 0.0) { return; }

    let a = f_phase() * TAU + p_time() * f_speed() + fi * TAU / max(count, 1.0);
    let z = cos(a);
    if ((z < 0.0) != back) { return; }

    let persp = 1.0 / max(1.0 - f_perspective() * z, 0.2);
    let long = f_length() * persp;

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
    let side = 1.0 - smoothstep(1.0 - soft - w, 1.0 + w, cp_form(dx, tn * 2.0 - 1.0));

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

// 2026-11-25 review: zooming far out left the panels floating in padding.
// The camera may zoom out only to ZOOM_MIN; below that the frame holds.
const ZOOM_MIN: f32 = 0.9;

fn scaffold_shade(p_in: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let zf = max(P.cam.x, 0.01);
    let p = p_layout(select(p_in, p_in * zf / ZOOM_MIN, zf < ZOOM_MIN), f_layout());
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
