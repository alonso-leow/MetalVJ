// shatter-mask: a fractured quadrilateral grid over a coloured field.
// The plane is cut by weighted, irregular column and row cuts; each cell is
// shrunk toward its centre by sqrt(fragment_fill), and coverage chooses which
// cells survive. The field slot paints behind the surviving shards, the seam
// slot traces their cracks and rims. Drift, wobble and speed make the whole
// fracture breathe over time. Original WGSL for the procedural scaffold.

const SH_EPS: f32 = 0.0001;

fn sh_scalar_hash(x: f32) -> f32 {
    return hash11(x * 12.9898 + 78.233);
}

// One cut's weight. Small weights cluster cuts, the asymmetry exponent makes
// the spacing uneven, and the bias pulls the cuts toward one end.
fn sh_cut_weight(i: i32, count: i32, s: f32, asym: f32, bias: f32) -> f32 {
    let fi = f32(i);
    let h1 = max(0.0001, sh_scalar_hash(s + fi * 17.13));
    let h2 = max(0.0001, sh_scalar_hash(s * 1.71 + fi * 31.77 + 9.13));
    let mixed = pow(h1, 0.55) * 0.72 + pow(h2, 2.4) * 0.28;
    let exponent = 1.0 + asym * 5.5;
    var w = pow(max(0.02, mixed), exponent);
    if (bias != 0.0) {
        let t = select(0.5, fi / max(f32(count - 1), 1.0), count > 1);
        let directional = 0.38 + 1.62 * select(1.0 - t, t, bias > 0.0);
        w = w * pow(directional, abs(bias) * asym * 1.4);
    }
    return max(0.003, w);
}

// Returns (column index, cut before, cut after) for an x in 0..1.
fn sh_x_cell(x: f32, cols: i32, s: f32, asym: f32, bias: f32) -> vec3<f32> {
    var total = 0.0;
    for (var i: i32 = 0; i < cols; i = i + 1) {
        total = total + sh_cut_weight(i, cols, s, asym, bias);
    }
    let inv = 1.0 / max(total, SH_EPS);
    var acc = 0.0;
    var prev = 0.0;
    for (var i: i32 = 0; i < cols; i = i + 1) {
        let c = acc + sh_cut_weight(i, cols, s, asym, bias) * inv;
        if (x < c) {
            return vec3<f32>(f32(i), prev, c);
        }
        acc = c;
        prev = c;
    }
    return vec3<f32>(f32(max(cols - 1, 0)), prev, 1.0);
}

// Returns (row index, top cut, bottom cut) at column-local x `u`. The left and
// right column boundaries each carry their own row cuts; their straight edges
// are interpolated by u, and blended with the shared backbone by col_free.
fn sh_y_cell(
    y: f32,
    u: f32,
    col: i32,
    rows: i32,
    s: f32,
    asym: f32,
    col_bias: f32,
    col_free: f32,
    back_bias: f32,
) -> vec3<f32> {
    let sL = s + 1000.0 + f32(col) * 79.31;
    let hL = sh_scalar_hash(s + f32(col) * 43.19);
    let varL = asym * (0.45 + 0.55 * hL);
    let sR = s + 1000.0 + f32(col + 1) * 79.31;
    let hR = sh_scalar_hash(s + f32(col + 1) * 43.19);
    let varR = asym * (0.45 + 0.55 * hR);
    let sB = s + 5000.5;

    var totalL = 0.0;
    var totalR = 0.0;
    var totalB = 0.0;
    for (var i: i32 = 0; i < rows; i = i + 1) {
        totalL = totalL + sh_cut_weight(i, rows, sL, varL, col_bias);
        totalR = totalR + sh_cut_weight(i, rows, sR, varR, col_bias);
        totalB = totalB + sh_cut_weight(i, rows, sB, asym, back_bias);
    }
    let invL = 1.0 / max(totalL, SH_EPS);
    let invR = 1.0 / max(totalR, SH_EPS);
    let invB = 1.0 / max(totalB, SH_EPS);

    var accL = 0.0;
    var accR = 0.0;
    var accB = 0.0;
    var prev = 0.0;
    let uu = clamp(u, 0.0, 1.0);
    for (var i: i32 = 0; i < rows; i = i + 1) {
        accL = accL + sh_cut_weight(i, rows, sL, varL, col_bias) * invL;
        accR = accR + sh_cut_weight(i, rows, sR, varR, col_bias) * invR;
        accB = accB + sh_cut_weight(i, rows, sB, asym, back_bias) * invB;
        let c = accB * (1.0 - col_free) + mix(accL, accR, uu) * col_free;
        if (y < c) {
            return vec3<f32>(f32(i), prev, c);
        }
        prev = c;
    }
    return vec3<f32>(f32(max(rows - 1, 0)), prev, 1.0);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let aspect = p_aspect();

    // Picture space -> 0..1 field space.
    var n = vec2<f32>(p.x / max(aspect, 0.01) + 0.5, p.y + 0.5);
    n = (n - vec2<f32>(0.5)) / max(f_scale(), 0.05) + vec2<f32>(0.5);
    n = p_rot(n - vec2<f32>(0.5), f_rotation() * PI) + vec2<f32>(0.5);
    n = n + vec2<f32>(f_offset_x(), f_offset_y()) * 0.5;

    let sp = f_speed();
    n = n + f_glide() * 0.15
        * vec2<f32>(sin(t * sp * 0.9 + 1.7), cos(t * sp * 0.7 + 0.3));
    n = n + f_wobble() * 0.035
        * vec2<f32>(sin(n.y * 9.0 + t * sp * 1.3), cos(n.x * 7.0 - t * sp * 1.1));

    let layout_id = i32(floor(clamp(f_layout(), 0.0, 2.0) + 0.5));
    let seed = floor(clamp(f_seed(), 0.0, 100.0) + 0.5);
    let density = clamp(f_density(), 0.0, 1.0);
    let asym = pow(clamp(f_cut_asymmetry(), 0.0, 1.0), 0.72);
    let coverage = clamp(f_coverage_distribution(), 0.0, 1.0);

    // Same layout_id regimes as the source tool.
    var cols = 2 + i32(floor(density * 10.0));
    var rows = cols;
    if (layout_id == 1) {
        cols = 4 + i32(floor(density * 18.0));
        rows = 2 + i32(floor(density * 4.0));
    } else if (layout_id == 2) {
        rows = 4 + i32(floor(density * 18.0));
        cols = 2 + i32(floor(density * 4.0));
    } else {
        if (aspect > 1.15) {
            cols = cols + i32(floor((aspect - 1.0) * 2.0));
        }
        if (aspect < 0.85) {
            rows = rows + i32(floor((1.0 / max(aspect, 0.2) - 1.0) * 2.0));
        }
    }
    cols = max(cols, 1);
    rows = max(rows, 1);

    let sGrid = seed * 137.731 + f32(layout_id) * 907.13;
    var xBias = 0.0;
    var colBias = 0.0;
    if (layout_id == 1) {
        xBias = 0.75;
        colBias = -0.12;
    } else if (layout_id == 2) {
        xBias = -0.15;
        colBias = 0.72;
    }
    let backBias = select(0.0, 0.55, layout_id == 2);

    let xc = sh_x_cell(n.x, cols, sGrid + 11.7, asym, xBias);
    let col = i32(xc.x);
    let xlo = xc.y;
    let xhi = xc.z;
    let u = clamp((n.x - xlo) / max(xhi - xlo, SH_EPS), 0.0, 1.0);

    let colFree = pow(asym, 1.18);
    let yc = sh_y_cell(n.y, u, col, rows, sGrid, asym, colBias, colFree, backBias);
    let row = i32(yc.x);
    let ytop = yc.y;
    let ybot = yc.z;
    let v = clamp((n.y - ytop) / max(ybot - ytop, SH_EPS), 0.0, 1.0);

    // Cell-centre metric: columns, rows, or a radial hash.
    let cx = (xlo + xhi) * 0.5;
    let cy = (ytop + ybot) * 0.5;
    let dx = cx - 0.5;
    let dy = cy - 0.5;
    let radial = length(vec2<f32>(dx, dy)) / 0.7071;
    let seedCell = seed + f32(row * cols + col + 1) * 0.121;
    var metric = sh_scalar_hash(
        seedCell * 17.317 + floor(cx * 91.0) * 3.1 + floor(cy * 113.0) * 5.7
            + radial * 11.3
    );
    if (layout_id == 1) {
        metric = cx;
    }
    if (layout_id == 2) {
        metric = cy;
    }
    var live_cell = metric <= coverage;
    if (coverage >= 0.999) {
        live_cell = true;
    }
    if (coverage <= 0.001) {
        live_cell = false;
    }

    // Shrink each shard toward the cell centre. Near full fill the shards are
    // expanded back by a seam blend, matching the source's pixel expansion.
    let fill = clamp(f_fragment_fill(), 0.0, 1.0);
    let scaleT = sqrt(fill);
    let du = abs(u - 0.5) - 0.5 * scaleT;
    let dv = abs(v - 0.5) - 0.5 * scaleT;
    let d = max(du, dv);
    let seamBlend = clamp((fill - 0.94) / 0.06, 0.0, 1.0);
    let frag = (1.0 - smoothstep(-0.004, 0.004, d + seamBlend * 0.008))
        * step(0.0001, scaleT);
    let fragMask = select(0.0, frag, live_cell);

    // Shard tone varies per cell; slot 0 is the shard, slot 1 the field.
    let cellTone = sh_scalar_hash(seedCell * 7.31 + f32(row * 3 + col) * 1.7);
    let bgCol = p_pal(1u, clamp(0.25 + 0.55 * cellTone, 0.0, 1.0));
    var fragCol = p_pal(0u, clamp(0.15 + 0.7 * cellTone, 0.0, 1.0));

    let inside = -d;
    let rim = (1.0 - smoothstep(0.0, 0.09, max(inside, 0.0))) * step(0.0, inside);
    fragCol = fragCol
        + p_pal(2u, clamp(0.25 + 0.5 * cellTone, 0.0, 1.0)) * f_sheen() * rim * 1.1;
    fragCol = fragCol
        + p_pal(2u, clamp(cellTone, 0.0, 1.0)) * f_sheen() * 0.12 * cellTone;

    let cw = clamp(f_crack_width(), 0.0, 0.5);
    let crack = select(
        0.0,
        1.0 - smoothstep(0.0, max(cw, 0.0001), d),
        d > 0.0
    ) * step(0.0001, cw);
    let crackMask = select(0.0, crack, live_cell);

    let opacity = clamp(f_fill_opacity(), 0.0, 1.0);
    var out_col = mix(bgCol, fragCol, fragMask * opacity);
    out_col = mix(out_col, p_pal(2u, clamp(0.2 + 0.6 * cellTone, 0.0, 1.0)), crackMask * opacity);

    let edge_a = 1.0 - smoothstep(0.47, 0.5, max(abs(uv.x - 0.5), abs(uv.y - 0.5)));
    return vec4<f32>(out_col, edge_a);
}
