// flutedglass: a picture seen through ribbed glass. Behaviour and
// parameter list written from the fluted glass" effect; everything
// here is original.
//
// The picture plane is rotated by `angulo` so the ribs run along its y axis.
// Across x it is cut into flutes of width `tam`; inside a flute the local
// coordinate s runs -1..1. A profile of s (prism: linear, lens: bulged,
// contour: cubic ridge, cascade: saw-tooth, flat: one offset per rib) times
// `distorsion` shifts where the picture is read, sideways. `desplaza` slides
// each rib along its length by a per-rib amount, `estira` stretches the picture
// along the ribs. Rib shape `forma` bends the rib lines (wave, zigzag), adds a
// second set of ribs across (grid) or curls them (petals). The read is a
// 5-tap blur across the rib, optionally with per-channel offsets (cromatico).
// Rib edges are shaded: `sombras` darkens one side with the `sombra` slot,
// `luces` brightens the other with `luz`; `bordes` is how much of the rib the
// shading covers. The margin leaves a border of plain background (`fondo`).

fn hsh(i: f32) -> f32 { return p_hash(vec2<f32>(i, 17.0 + f_seed() * 3.1)); }

fn tri(x: f32) -> f32 { return abs(fract(x) * 2.0 - 1.0) * 2.0 - 1.0; }

// Refraction profile of the local rib coordinate s in -1..1.
fn profile(s: f32, kind: i32, hc: f32) -> f32 {
    if (kind == 0) { return s; }
    if (kind == 1) { return s * (1.5 - 0.5 * s * s) * 0.8; }
    if (kind == 2) { let a = abs(s); return sign(s) * a * a * a * 1.2; }
    if (kind == 3) { return fract(s * 0.75 + 0.5 + hc) * 2.0 - 1.0; }
    return hc * 2.0 - 1.0;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let w = max(f_size(), 0.01);
    let ang = f_angle() * TAU;
    let q = p_rot(p, -ang);
    let shape = clamp(i32(floor(f_shape() + 0.5)), 0, 4);
    let dkind = clamp(i32(floor(f_distortion_shape() + 0.5)), 0, 4);

    // Rib coordinate x: bend the rib lines.
    var x = q.x + t * f_sweep() * w * 0.5;
    if (shape == 1) { x = x + sin(q.y * 5.0 / (w * 4.0 + 0.2)) * w * 0.6; }
    if (shape == 2) { x = x + tri(q.y / (w * 3.0)) * w * 0.5; }
    if (shape == 4) { x = x + sin(length(q) * 9.0 - t * 0.3) * w * 0.5 + length(q) * w * 0.8; }
    x = x + sin(q.y * f_wave_freq() * 8.0 + t * f_wave_speed() * 2.0) * w * f_wave_amp();
    x = x + (p_noise(vec2<f32>(q.x / w * 0.35, 3.0 + f_seed())) - 0.5) * w * f_irregular() * 3.0;

    let u = x / w;
    let cell = floor(u);
    var s = fract(u) * 2.0 - 1.0;
    let hc = hsh(cell);
    // Faceted glass: quantise the local coordinate into flat planes.
    let nf = 3.0;
    s = mix(s, clamp((floor((s * 0.5 + 0.5) * nf) + 0.5) / nf * 2.0 - 1.0, -1.0, 1.0), f_facets());

    // Refraction across the rib (and along it for the grid).
    let breath = 1.0 + f_breathe() * sin(t * f_breathe_speed() * 3.0);
    let flip = mix(1.0, select(-1.0, 1.0, hc > 0.5), f_alternate());
    let amp = f_distortion() * breath * w * 1.4 * flip;
    var d = vec2<f32>(profile(s, dkind, hc) * amp, 0.0);
    var s2 = 0.0;
    if (shape == 3) {
        let v = q.y / w;
        let c2 = floor(v);
        s2 = fract(v) * 2.0 - 1.0;
        d.y = profile(s2, dkind, hsh(c2 + 91.0)) * amp;
    }
    let along = f_displace() * (hc * 2.0 - 1.0) * 0.5;
    let stretch = 1.0 + f_stretch();

    // Film grain on the read position.
    let gp = floor(uv * 900.0) + vec2<f32>(floor(t * 24.0) * 13.0, 7.0);
    let gj = (vec2<f32>(p_hash(gp), p_hash(gp + vec2<f32>(5.0, 9.0))) - 0.5) * f_grain_mix() * w * 0.5;

    // Blur taps across the rib, each with optional per-channel offsets.
    var acc = vec3<f32>(0.0);
    var alpha = 0.0;
    var wsum = 0.0;
    let cr = f_chromatic() * w * 0.3 * (0.3 + abs(s));
    for (var k = -2; k <= 2; k = k + 1) {
        let wt = exp(-f32(k * k) * 0.5);
        let bo = f32(k) * f_blur() * w * 0.35;
        let base = vec2<f32>(q.x + d.x + bo, q.y / stretch + along * 2.0 * w + d.y);
        let r = p_src(p_rot(base + vec2<f32>(cr, 0.0) + gj, ang));
        let g = p_src(p_rot(base + gj, ang));
        let b = p_src(p_rot(base - vec2<f32>(cr, 0.0) + gj, ang));
        acc = acc + vec3<f32>(r.r, g.g, b.b) * wt;
        alpha = alpha + g.a * wt;
        wsum = wsum + wt;
    }
    var col = acc / wsum;
    alpha = alpha / wsum;

    // Rib shading.
    let bw = max(f_edges() * 2.0, 1e-3);
    let sh = f_shadows() * smoothstep(1.0 - bw, 1.0, s);
    let li = f_lights() * smoothstep(1.0 - bw, 1.0, -s);
    var glass = col;
    glass = glass * mix(vec3<f32>(1.0), p_pal(1u, hc), f_tint());
    glass = mix(glass, p_pal(0u, abs(s)), clamp(sh, 0.0, 1.0));
    glass = mix(glass, p_pal(1u, abs(s)), clamp(li, 0.0, 1.0));
    // Travelling specular streak over the ribs.
    let streak = exp(-pow((fract(u * 0.18 - t * f_reflection_speed() * 0.1) - 0.5) * 6.0, 2.0)) * (1.0 - abs(s) * 0.5);
    glass = glass + p_pal(1u, 0.2) * streak * f_reflection() * 0.6;
    glass = glass + vec3<f32>(p_grain(uv, t)) * f_grain_surface() * 0.5;

    // Margin: the pane is inset, the border shows the background.
    let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    let m = f_margin();
    let gmask = select(smoothstep(m - 0.001, m + 0.002 + f_margin_soft(), edge), 1.0, m < 1e-4);
    return vec4<f32>(max(glass, vec3<f32>(0.0)), alpha * gmask);
}
