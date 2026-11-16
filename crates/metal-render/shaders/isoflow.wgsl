// isoflow: warped contour bands. Behaviour and parameter list follow the
// community "Fluid Contours" sketch (field warp, quantised bands, variable
// thickness, two palettes, layer shadows, paper border); everything here is
// original.
//
// The field is two warped sine/cosine sums; its value times `density` is cut
// into unit bands. Inside a band the fractional part below `bw` is ink, where
// `bw` comes from a hashed sine per band (irregularity) and `spacing` trims a
// paper margin from the band. `palette` 0 mixes ink toward accent inside broad
// island shapes; 1 cycles ink/accent/violet/blue/paper. The band lip gets a
// drop shadow and a lifted highlight from `shadow`. Bands whose index is a
// multiple of `text_every` are carved out and stamped with dotted ink strokes
// (lettering 1/2 differ in mark pitch) to stand in for the source's line text.
// `margin` masks a paper border outside the picture.

fn iso_field(p: vec2<f32>, t: f32) -> f32 {
    let s = f_seed() * 0.731;
    let q = p * (3.2 / max(f_size(), 0.05));
    let v = q + f_warp() * vec2<f32>(
        sin(q.y * 1.13 + s + t * 0.29)
            + 0.43 * cos(q.x * 0.77 - q.y * 0.57 + s * 1.9 - t * 0.21),
        cos(q.x * 1.09 + s * 0.73 - t * 0.24)
            + 0.42 * sin(q.y * 0.89 + q.x * 0.51 + s * 1.3 + t * 0.17),
    );
    return sin(v.x + s + t * 0.19 + 0.8 * sin(v.y * 0.81 - s * 0.31))
        + 0.65 * cos(v.y * 1.2 - v.x * 0.6 + s * 0.47 - t * 0.16)
        + 0.32 * sin(v.x * 1.7 + v.y * 1.3 - s * 0.63 + t * 0.13);
}

fn iso_width(k: f32) -> f32 {
    let r = 0.5 + 0.5 * sin(k * 2.399 + f_seed() * 1.31);
    let w = 0.5 * (1.0 - f_spacing()) * (1.0 + f_irregularity() * (r - 0.5) * 1.5);
    return clamp(w, 0.045, 0.87);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time() * f_speed();

    let paper = p_pal(0u, 0.0);
    let ink = p_pal(0u, 1.0);
    let accent = p_pal(1u, 0.0);

    let f = iso_field(p, t) * f_density();
    let k = floor(f);
    let v = fract(f);
    let bw = iso_width(k);

    // Screen-space derivatives, taken before any branching.
    let dg = vec2<f32>(dpdx(f), dpdy(f));
    let aa = max((abs(dg.x) + abs(dg.y)) * 0.65, 0.0001);

    var cover = 1.0 - smoothstep(bw - aa, bw + aa, v);
    cover = cover * smoothstep(0.0, aa, v);

    // Which band are we in, and is it an inscription band?
    let sel = step(0.5, f_lettering());
    let every = max(f_text_every(), 1.0);
    let pick = k - floor(k / every) * every;
    let picked = sel * step(pick, 0.5);
    cover = cover * (1.0 - picked);

    // Band colour.
    var layer = ink;
    if (f_palette() > 0.5) {
        let ci = k - 5.0 * floor(k / 5.0);
        if (ci < 0.5) {
            layer = ink;
        } else if (ci < 1.5) {
            layer = accent;
        } else if (ci < 2.5) {
            layer = p_pal(1u, 1.0);
        } else if (ci < 3.5) {
            layer = p_pal(2u, 1.0);
        } else {
            layer = paper;
        }
    } else {
        let s = f_seed() * 0.731;
        let island = sin(p.x * 4.1 / max(f_size(), 0.05) + s * 0.2)
            + cos(p.y * 4.8 / max(f_size(), 0.05) - s * 0.9 + t * 0.12);
        let red = smoothstep(1.62, 1.66, island);
        layer = mix(ink, accent, red);
    }

    var col = mix(paper, layer, cover);

    // Cast shadow under the band lip and a raised shine on it.
    let d = max(length(dg), 0.0001);
    if (picked < 0.5) {
        let lip = exp(-max(v - bw, 0.0) / max(d * 3.0, 0.012));
        let cast_shadow = (1.0 - cover) * lip * step(bw, v);
        let shine = exp(-max(v, 0.0) / max(d * 1.8, 0.008)) * cover;
        col = col * (1.0 - f_shadow() * 0.35 * cast_shadow);
        let lift = min(layer * 1.22 + vec3<f32>(0.03), vec3<f32>(1.0));
        col = mix(col, lift, f_shadow() * 0.28 * shine);
    }

    // Inscription bands: short dotted strokes along the band, in ink.
    if (picked > 0.5) {
        let glen = length(dg);
        if (glen > 0.000001) {
            let tangent = vec2<f32>(-dg.y, dg.x) / glen;
            let along = dot(p, tangent);
            let dense = select(1.0, 1.4, f_lettering() > 1.5);
            let freq = (10.0 + every * 1.4) * dense;
            let u = along * freq;
            let cell = floor(u);
            let frac = u - cell;
            let h = p_hash(vec2<f32>(cell, k * 3.0 + 1.0));
            let across = clamp(v / max(bw, 0.001), 0.0, 1.0);
            let band = smoothstep(0.08, 0.2, across) * (1.0 - smoothstep(0.8, 0.92, across));
            let stroke = step(0.18, h) * step(frac, 0.42 + 0.3 * h);
            col = mix(col, ink, band * stroke);
        }
    }

    // Paper border: outside the margin the sheet shows through.
    let a = p_aspect();
    let b = f_margin() * vec2<f32>(min(1.0, 1.0 / a), min(1.0, a));
    let inside = step(b.x, uv.x) * step(b.y, uv.y)
        * step(uv.x, 1.0 - b.x) * step(uv.y, 1.0 - b.y);
    col = mix(paper, col, inside);

    return vec4<f32>(col, inside);
}
