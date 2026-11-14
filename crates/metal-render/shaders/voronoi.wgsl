// voronoi: cellular mosaic, written from first principles.
//
//
// The plane is cut into jittered grid cells, each with one wandering site. A
// 3x3 neighbourhood gives the nearest (F1) and second nearest (F2) distance and
// the id of the nearest site. The distance is a blend of euclid / manhattan /
// chebyshev (metrica, cheb) after an optional stretch (estiramiento,
// angulo_celda) and radial size waves (ondas). F2-F1 is the distance to the
// cell border: border width (gap, ancho_borde) selects the gap colour (slot 1),
// glow and the site dot use slot 2, cells use slot 0 quantised to pasos_color
// levels. Dead cells (huecos) let the backdrop show through.

struct Vor {
    f1: f32,
    f2: f32,
    id: vec2<f32>,
    site: vec2<f32>,
}

fn vor_dist(d: vec2<f32>) -> f32 {
    let a = abs(d);
    let e = length(d);
    let m = (a.x + a.y) * 0.72;
    let c = max(a.x, a.y);
    return mix(mix(e, m, clamp(f_metric(), 0.0, 1.0)), c, clamp(f_cheb(), 0.0, 1.0));
}

fn vor_site(cell: vec2<f32>, t: f32) -> vec2<f32> {
    let s = cell + vec2<f32>(f_seed() * 13.7, f_seed() * 5.3);
    let h = p_hash2(s);
    let h2 = p_hash2(s + vec2<f32>(71.0, 19.0));
    let base = vec2<f32>(0.5) + (h - vec2<f32>(0.5)) * 0.6 * clamp(f_jitter(), 0.0, 1.0);
    let ph = t * f_speed() * (0.4 + h2.x) + h2.y * TAU;
    let amp = 0.3 * f_motion();
    return base + amp * vec2<f32>(sin(ph), cos(ph * 0.83 + h.x * TAU));
}

fn vor_eval(q: vec2<f32>, t: f32) -> Vor {
    let ip = floor(q);
    let fp = q - ip;
    var r: Vor;
    r.f1 = 8.0;
    r.f2 = 8.0;
    r.id = ip;
    r.site = vec2<f32>(0.0);
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let g = vec2<f32>(f32(i), f32(j));
            let sp = vor_site(ip + g, t);
            let d = vor_dist(g + sp - fp);
            if (d < r.f1) {
                r.f2 = r.f1;
                r.f1 = d;
                r.id = ip + g;
                r.site = g + sp - fp;
            } else if (d < r.f2) {
                r.f2 = d;
            }
        }
    }
    return r;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    var q = p * f_scale();
    // Radial size waves.
    q = q * (1.0 + f_waves() * 0.35 * sin(length(p) * 3.0 - t * 0.8));
    // Anisotropy.
    let ang = f_angle_cell() * PI;
    q = p_rot(q, ang);
    q.y = q.y * (1.0 + f_stretch());
    q = p_rot(q, -ang);
    // Domain distortion.
    let dn = vec2<f32>(
        p_fbm(q * 0.6 + vec2<f32>(t * 0.1, 3.0), 2),
        p_fbm(q * 0.6 + vec2<f32>(7.0, -t * 0.1), 2)
    );
    q = q + (dn - vec2<f32>(0.5)) * f_distortion() * 2.4;

    let v = vor_eval(q, t);
    let edge = max(v.f2 - v.f1, 0.0);
    let hid = p_hash(v.id + vec2<f32>(f_seed() * 13.7, f_seed() * 5.3));
    let hid2 = p_hash(v.id + vec2<f32>(101.0, 57.0));

    // Cell colour: stepped random level, blended with a distance gradient.
    let steps = max(floor(f_steps_color() + 0.5), 1.0);
    let lvl = floor(hid * steps) / max(steps - 1.0, 1.0);
    let grad = clamp(v.f1 * 0.9, 0.0, 1.0);
    var tc = mix(grad, min(lvl, 1.0), clamp(f_color_cell(), 0.0, 1.0));
    // Facets: quantise the shading inside each cell.
    let fq = max(steps, 2.0);
    tc = mix(tc, floor(tc * fq) / fq, f_facets());
    var col = p_pal(0u, clamp(tc, 0.0, 1.0));

    // Bevel and per-cell flicker.
    col = col * (1.0 + f_relief() * (smoothstep(0.0, 0.6, edge) - 0.5) * 1.2);
    let fl = 0.5 + 0.5 * sin(t * (2.0 + 4.0 * hid2) + hid * TAU);
    col = col * (1.0 - f_blink() * 0.6 * fl);

    // Glow along the borders and a dot at each site, slot 2.
    let gcol = p_pal(2u, clamp(0.3 + 0.7 * hid2, 0.0, 1.0));
    col = col + gcol * f_glow() * exp(-edge * 5.0) * 1.2;
    col = col + gcol * f_core() * exp(-v.f1 * v.f1 * 60.0) * 1.5;

    // Gap line, slot 1, between cells.
    let w = f_gap() * 0.8 + f_width_edge() * 0.35;
    let soft = 0.01 + f_edge_soft() * 0.25;
    let line = 1.0 - smoothstep(w, w + soft, edge);
    col = mix(col, p_pal(1u, clamp(edge * 2.0, 0.0, 1.0)), line * step(0.0005, w));

    // Dead cells show the backdrop; the gap lines stay opaque.
    let dead = step(1.0 - clamp(f_holes(), 0.0, 1.0), hid2) * step(0.001, f_holes());
    let cov = clamp(1.0 - dead * (1.0 - line), 0.0, 1.0);
    return vec4<f32>(col * cov, cov);
}
