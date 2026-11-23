// Trafico, the display pass: the torus of cars drawn as a field of cells
// through a 2D camera, an optional floor tilt, a mirror fold and a domain warp.
// The grid state is read with textureLoad; the camera wraps, so a zoomed-out or
// panned view shows the torus tiled rather than a hole.

struct TraficoU {
    red: ColorSlot,
    blue: ColorSlot,
    empty: ColorSlot,
    background: ColorSlot,
    // aspect, time, grid w, grid h
    a: vec4<f32>,
    // densidad, p_rojo, tercer_tipo, estela
    b: vec4<f32>,
    // forma, celda_gap, celda_redondez, glow
    c: vec4<f32>,
    // brillo_rojo, brillo_azul, perspectiva, fondo_alpha
    d: vec4<f32>,
    // zoom, pan_x, pan_y, giro
    e: vec4<f32>,
    // deriva, brillo, contraste, opacidad
    f: vec4<f32>,
    // fondo_nivel, tono, saturacion, 0
    g: vec4<f32>,
    // warp_cant, warp_freq, espejo, pulso
    h: vec4<f32>,
    // The `body` part's picture and `mat_*`, one `MaterialU` — see
    // material.wgsl, prepended ahead of this file.
    mat_body: MaterialU,
};

@group(0) @binding(0) var<uniform> u: TraficoU;
@group(0) @binding(1) var st: texture_2d<f32>;
@group(0) @binding(2) var st_smp: sampler;
// The road/cars material, the second picture pair (`Fullscreen` numbers it 3/4).
@group(0) @binding(3) var surf_body_tex: texture_2d<f32>;
@group(0) @binding(4) var surf_body_samp: sampler;

fn tr_pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}
fn tr_rnd(a: u32, b: u32) -> f32 {
    return f32(tr_pcg(a ^ tr_pcg(b ^ tr_pcg(0u))) >> 8u) / 16777216.0;
}

fn tr_type(rank: f32) -> i32 {
    if (rank >= u.b.x) { return 0; }
    let bits = bitcast<u32>(rank);
    // Same hash as trafico_step.wgsl: (bits, 7, k, 0) folded left to right.
    let h1 = tr_pcg(bits ^ tr_pcg(7u ^ tr_pcg(1u ^ tr_pcg(0u))));
    if (f32(h1 >> 8u) / 16777216.0 < u.b.z * 0.5) { return 3; }
    let h2 = tr_pcg(bits ^ tr_pcg(7u ^ tr_pcg(2u ^ tr_pcg(0u))));
    if (f32(h2 >> 8u) / 16777216.0 < u.b.y) { return 1; }
    return 2;
}

fn tr_cell(c: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(i32(u.a.z), i32(u.a.w));
    return textureLoad(st, ((c % size) + size) % size, 0);
}

fn tr_slot_col(t: i32, rank: f32) -> vec3<f32> {
    if (t == 1) { return grad4(u.red, rank / max(u.b.x, 0.001)) * u.d.x; }
    if (t == 2) { return grad4(u.blue, rank / max(u.b.x, 0.001)) * u.d.y; }
    // Diagonal cars: the red slot a third of the way round the hue wheel.
    return kit_hue(grad4(u.red, rank / max(u.b.x, 0.001)), 0.5) * u.d.x;
}

// Signed distance of the shape inside a cell, q in -1..1 (cell centre 0).
fn tr_shape(q: vec2<f32>, t: i32, shape: f32, hs: f32, rad: f32) -> f32 {
    let k = i32(floor(shape + 0.5));
    if (k == 1) { return length(q) - hs; }
    if (k == 2) { return (abs(q.x) + abs(q.y)) - hs * 1.25; }
    var he = vec2<f32>(hs);
    if (k == 3) {
        // A car: a bar along its direction of travel.
        he = select(vec2<f32>(hs, hs * 0.55), vec2<f32>(hs * 0.55, hs), t == 2);
    }
    let r = min(rad * min(he.x, he.y), min(he.x, he.y));
    let d = abs(q) - he + vec2<f32>(r);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let tilt = clamp(u.d.z, 0.0, 1.0);

    // Floor tilt: rows near the top of the frame are far away.
    let qy = in.uv.y - 0.5;
    let fz = max(1.0 + tilt * 0.9 * qy * 2.0, 0.08);
    var uv = vec2<f32>(0.5 + (in.uv.x - 0.5) / fz, 0.5 + qy / (fz * fz));
    let pulse = clamp(u.h.w, 0.0, 1.0);
    uv = (uv - 0.5) / (1.0 + pulse * 0.12) + 0.5;

    var p = kit_cam2d(uv, aspect, u.e.x, vec2<f32>(u.e.y, u.e.z), u.e.w, u.f.x, t);
    p = kit_warp(p, u.h.x, u.h.y, t);
    p = kit_mirror(p, u.h.z);

    // Cells are square on screen: the grid is gridH cells tall per unit of p.
    let cells_y = u.a.w;
    let g = p * cells_y + vec2<f32>(u.a.z, u.a.w) * 0.5;
    let gc = vec2<i32>(floor(g));
    let fr = fract(g);
    let aa = max(fwidth(g.x), fwidth(g.y));
    let hs = clamp(1.0 - u.c.y, 0.05, 1.0);

    let bg_t = backdrop_from_uv(u.background, in.uv, aspect) * u.g.x;
    var col = bg_t;
    var cover = 0.0;

    // Own cell plus the eight around it, so glow can spill across edges.
    var glow = vec3<f32>(0.0);
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let off = vec2<i32>(i, j);
            let s = tr_cell(gc + off);
            let ty = tr_type(s.x);
            let q = (fr - vec2<f32>(0.5) - vec2<f32>(f32(i), f32(j))) * 2.0;
            var ccol = vec3<f32>(0.0);
            var present = 0.0;
            if (ty > 0) {
                ccol = tr_slot_col(ty, s.x);
                present = 1.0;
            } else if (s.y > 0.0 && u.b.w > 0.0) {
                let tt = i32(s.z + 0.5);
                ccol = tr_slot_col(max(tt, 1), 0.5) * s.y * u.b.w;
                present = 0.6 * s.y * u.b.w;
            }
            if (present > 0.0) {
                let sd = tr_shape(q, ty, u.c.x, hs, u.c.z);
                if (i == 0 && j == 0) {
                    let m = 1.0 - smoothstep(-aa * 2.0, aa * 2.0, sd * 0.5);
                    // The shared material, off at mix 0 (a migrated row): a
                    // kind or a `tex.body` picture seasons the car's slot too.
                    let car_shaded = material_apply(
                        u.mat_body, surf_body_tex, surf_body_samp, ccol,
                        vec3<f32>(g.x, g.y, 0.0), vec3<f32>(0.0, 0.0, 1.0), in.uv,
                        vec3<f32>(0.0, 0.0, 1.0), t,
                    );
                    // sd is in cell-half units; aa is in cells.
                    col = mix(col, car_shaded.albedo, clamp(m * present, 0.0, 1.0));
                    cover = max(cover, m * present);
                }
                if (u.c.w > 0.0 && ty > 0) {
                    glow = glow + ccol * exp(-max(sd, 0.0) * 2.2) * 0.35;
                }
            }
        }
    }
    // The empty road under everything: the vacio slot, shown where there is
    // no car and no trail, in the interior of the cell only.
    let me = tr_cell(gc);
    if (tr_type(me.x) == 0) {
        let sd_cell = tr_shape((fr - vec2<f32>(0.5)) * 2.0, 1, 0.0, hs, u.c.z);
        let m = 1.0 - smoothstep(-aa * 2.0, aa * 2.0, sd_cell * 0.5);
        let road = grad4(u.empty, f32(((gc.y % i32(u.a.w)) + i32(u.a.w)) % i32(u.a.w)) / u.a.w);
        // The same material on the road the cars run on; off is the empty slot.
        let road_shaded = material_apply(
            u.mat_body, surf_body_tex, surf_body_samp, road,
            vec3<f32>(g.x, g.y, 0.0), vec3<f32>(0.0, 0.0, 1.0), in.uv,
            vec3<f32>(0.0, 0.0, 1.0), t,
        );
        col = mix(col, road_shaded.albedo, m * (1.0 - cover));
    }
    col = col + glow * u.c.w * (1.0 - cover);

    // Distance haze for the floor tilt.
    col = mix(bg_t, col, clamp(mix(1.0, fz * 1.3, tilt), 0.0, 1.0));
    col = col * (1.0 + pulse * 0.5);
    col = kit_look(col, u.f.y, u.f.z, u.g.y, u.g.z);

    var alpha = u.f.w;
    if (u.d.w > 0.5) {
        alpha = alpha * max(cover, u.g.x);
    }
    return vec4<f32>(col, alpha);
}
