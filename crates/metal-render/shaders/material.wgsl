// Material — what a 3D model's surface is made of, beside `surface.wgsl`.
//
// The brief's step M3: one shared block every model concatenates after its
// surface sampling rule, so a host replaces its own `mix(albedo, tex.rgb, m)`
// with one call and every 3D model wears the same seven kinds. This file holds
// no textures and no model: the analytic kinds are functions of object space,
// the normal and the clock, the one picture a part may carry, and nothing else.
//
// **One uniform per part.** [`MaterialU`] is appended to the model's own UBO,
// one entry per declared part, exactly where that part's old `s_<part>` vec4
// sat. It carries both sides of the part: the picture `surface_sample` needs
// (`a.xyz`) and the material floats the row's `material` group resolved for
// that part (`a.w` and `b`/`c`/`d`). A later per-part override (a `mat.*` node
// on one part) only changes what fills the struct.
//
// **Off is mix 0.** `mat_mix` at 0 — the migrated default row — makes
// [`material_apply`] return its input untouched: the same albedo, the same
// normal, no colour written. That is what keeps an all-zero material group
// byte-identical to a row saved before the group existed.
//
// `material_apply` returns [`Shaded`], not a colour, because a raymarched host
// shades at the hit: the perturbed normal, the roughness, the metalness and the
// alpha are as much the surface as the albedo is. A host that only has the one
// `mix` call to replace — monstruo's species files — takes `.albedo` and leaves
// the rest for the models that can use it.

// (image mix, image mapping 0 object / 1 projector, image scale, mat_kind)
// (mat_mix, mat_scale, mat_rough, mat_metal)
// (mat_bump, mat_env, mat_tint, mat_tint_hue)
// (mat_glitch, mat_glitch_rate, mat_glitch_seed, _)
struct MaterialU {
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
    d: vec4<f32>,
};

struct Shaded {
    albedo: vec3<f32>,
    normal: vec3<f32>,
    roughness: f32,
    metalness: f32,
    alpha: f32,
};

// What one analytic kind contributes, before the operator's overrides fold in.
struct MatLayer {
    albedo: vec3<f32>,
    bump: f32,
    rough: f32,
    metal: f32,
    alpha: f32,
};

// A cosine palette, so `mat_tint_hue`/the glitch need no hue/sat/lum conversion.
fn mat_hue(h: f32) -> vec3<f32> {
    let t = h * 6.2831853;
    return 0.5 + 0.5 * cos(vec3<f32>(t, t + 2.0943951, t + 4.1887902));
}

// Matte: holds the host's colour and kills the gloss. The one kind that is a
// finish rather than a new substance, so it keeps `base` and presses it flat.
fn mat_layer_matte(base: vec3<f32>) -> MatLayer {
    let flat = mix(base, vec3<f32>(dot(base, vec3<f32>(0.299, 0.587, 0.114))), 0.25);
    return MatLayer(flat * 0.9, 0.0, 0.92, 0.0, 1.0);
}

// Concrete: grey mottle over fbm at three scales, with a rare bright speck.
fn mat_layer_concrete(p: vec3<f32>, sc: f32) -> MatLayer {
    let q = p * sc;
    let g = lattice_fbm(vec2<f32>(q.x * 6.0 + q.y, q.z * 6.0 - q.y), 4);
    let speck = step(0.82, hash21(floor(vec2<f32>(q.x, q.z) * 48.0)));
    let a = mix(vec3<f32>(0.16, 0.16, 0.17), vec3<f32>(0.44, 0.44, 0.42), g) + speck * 0.08;
    return MatLayer(a, g, 0.95, 0.0, 1.0);
}

// Wood: growth rings around the part's own z axis, warped so no two are round.
fn mat_layer_wood(p: vec3<f32>, sc: f32) -> MatLayer {
    let warp = lattice_fbm(vec2<f32>(p.x, p.y) * sc * 2.0, 3);
    let r = length(vec2<f32>(p.x, p.z)) * sc * 5.0 + warp * 3.0;
    let rings = 0.5 + 0.5 * sin(r * 6.2831853);
    let a = mix(vec3<f32>(0.14, 0.07, 0.03), vec3<f32>(0.46, 0.26, 0.10), rings);
    return MatLayer(a, rings, 0.55, 0.0, 1.0);
}

// Leather: a cell pattern, each cell's colour from its own hash, its rim lit.
fn mat_layer_leather(p: vec3<f32>, sc: f32) -> MatLayer {
    let q = p * sc * 10.0;
    let f = fract(q.xy) - 0.5;
    let cell = 1.0 - smoothstep(0.0, 0.55, length(f));
    let h = hash21(floor(q.xy));
    let a = mix(vec3<f32>(0.05, 0.03, 0.035), vec3<f32>(0.24, 0.12, 0.10), 0.4 + 0.6 * h);
    return MatLayer(a * (0.8 + 0.4 * cell), cell, 0.7, 0.0, 1.0);
}

// Metal: a brushed fbm, conductive, with the fresnel that makes it read as one.
fn mat_layer_metal(p: vec3<f32>, n: vec3<f32>, v: vec3<f32>, sc: f32) -> MatLayer {
    let g = lattice_fbm(vec2<f32>(p.x, p.y) * sc * 6.0, 3);
    let fres = pow(1.0 - clamp(dot(normalize(n), normalize(v)), 0.0, 1.0), 3.0);
    let a = mix(vec3<f32>(0.45), vec3<f32>(0.92), g) * (0.55 + 0.45 * fres);
    return MatLayer(a, g, mix(0.12, 0.45, g), 1.0, 1.0);
}

// Glass: near-black conduction, a thin bright edge from the fresnel, low alpha.
fn mat_layer_glass(n: vec3<f32>, v: vec3<f32>) -> MatLayer {
    let fres = pow(1.0 - clamp(dot(normalize(n), normalize(v)), 0.0, 1.0), 3.0);
    let a = vec3<f32>(0.82, 0.90, 1.0) * (0.45 + 0.55 * fres);
    return MatLayer(a, 0.0, 0.05, 0.0, 0.35);
}

// The ramp the grid walks: a palette with no daylight anywhere in it, so
// nothing on the grid can be mistaken for the colour a thing *is*. Near-black
// violet, magenta, cyan, white-hot — the four stops `models::matrix` bakes
// into its one colour slot.
fn mat_malla(tt: f32) -> vec3<f32> {
    var slot: ColorSlot;
    slot.c0 = vec4<f32>(0.02, 0.01, 0.07, 1.0);
    slot.c1 = vec4<f32>(0.55, 0.05, 0.62, 1.0);
    slot.c2 = vec4<f32>(0.05, 0.76, 0.88, 1.0);
    slot.c3 = vec4<f32>(1.00, 0.97, 0.90, 1.0);
    return grad4(slot, tt);
}

// Matrix: the grid as a material — the look `models::matrix` bakes into line
// geometry, evaluated here in object space so **any** 3D model can wear it.
// Three rules do all of it: the lamp is the only light, colour is depth, and a
// role is a rate rather than a tint. `p` is the surface point in object space,
// `v` the direction to the eye, `t` the clock, `sc` the operator's scale.
fn mat_layer_matrix(p: vec3<f32>, v: vec3<f32>, t: f32, sc: f32) -> MatLayer {
    // A role is a rate. A coarse cell of object space picks one of five grid
    // rates and one of five light behaviours — a television is not "the blue
    // thing", it is the grid running at a different rate, and a curtain is
    // the grid with a slow wave along it. None of them is a tint.
    let coarse = floor(p * 1.5 + 0.5);
    let role = floor(hash31(coarse + 0.5) * 5.0);
    let rate = 1.0 + role;
    let f = p * (3.0 + sc * 5.0) * rate;
    let g = abs(fract(f) - 0.5);
    let edge = min(min(g.x, g.y), g.z);
    let line = 1.0 - smoothstep(0.03, 0.09, edge);

    // Flicker: a per-cell hash held for a slot of time, so the weave switches
    // on and off rather than shimmering.
    let slot = floor(t * (0.5 + role * 0.6));
    let flick = 0.55 + 0.45 * hash31(floor(f) + vec3<f32>(slot, slot * 1.7, role));

    // Rule 1 — the lamp is the only light, so distance from it *is* the
    // shading. The same bulb the room hangs, in object space.
    let reach = p - vec3<f32>(0.05, 0.24, -0.05);
    let lit = 2.2 / (1.0 + dot(reach, reach) * 2.4);

    // Rule 2 — colour is depth, not material: the ramp is walked by how far
    // the point sits from the eye along the view, mixed with brightness.
    let depth = clamp(0.5 - 0.5 * dot(p, v), 0.0, 1.0);
    let by_light = clamp(lit * 0.55, 0.0, 1.0);
    let by_depth = 1.0 - depth;
    var base = mat_malla(mix(by_light, by_depth, 0.5));

    // Rule 3 — a role is a rate. Ordered low to high and each branch closed at
    // the top, so adding a role never silently falls into the one below it.
    var lum = lit * flick;
    if (role < 0.5) {
        lum = lum * 0.78;
    } else if (role < 1.5) {
    } else if (role < 2.5) {
        let scan = fract(p.y * 22.0 - t * 1.7);
        lum = lum + 0.55 + 0.75 * scan * scan;
    } else if (role < 3.5) {
        lum = lum * (0.72 + 0.5 * sin(p.x * 11.0 + t * 0.55));
    } else {
        lum = lum + 0.9;
    }

    // The sweep: a plane travelling through the object along its diagonal, the
    // one thing that crosses every region as if they were the same object.
    let front = fract(t * 0.12) * 3.0 - 1.5;
    let along = p.x * 0.5 + p.y * 0.5 + p.z;
    let sweep = exp(-pow(along - front, 2.0) * 2.2);
    lum = clamp(lum + sweep * 1.2, 0.0, 2.4);

    let a = base * (line * lum + 0.06);
    return MatLayer(a, line, 0.4, 0.0, 1.0);
}

// 0 matte · 1 concrete · 2 wood · 3 leather · 4 metal · 5 glass · 6 matrix.
// Any other value is "no kind": the host's own surface is returned.
fn mat_layer(
    kind: u32,
    base: vec3<f32>,
    p: vec3<f32>,
    n: vec3<f32>,
    v: vec3<f32>,
    t: f32,
    sc: f32,
) -> MatLayer {
    if (kind == 0u) { return mat_layer_matte(base); }
    if (kind == 1u) { return mat_layer_concrete(p, sc); }
    if (kind == 2u) { return mat_layer_wood(p, sc); }
    if (kind == 3u) { return mat_layer_leather(p, sc); }
    if (kind == 4u) { return mat_layer_metal(p, n, v, sc); }
    if (kind == 5u) { return mat_layer_glass(n, v); }
    if (kind == 6u) { return mat_layer_matrix(p, v, t, sc); }
    return MatLayer(base, 0.0, 1.0, 0.0, 1.0);
}

// The one entry a host calls. `m` is this part's uniform; `tex`/`samp` its
// picture slot (the fallback texel when nothing is wired); `obj_pos`/`normal`
// the surface point and normal in the space `surface_sample` and the kinds
// share; `screen_uv` the fragment's uv for projector mapping; `view` the
// direction from the point to the eye; `time` the model's own clock.
//
// A `tex.<part>` source replaces the albedo and keeps everything else; a
// material kind seasons (mix) or replaces it. Both off returns the input.
fn material_apply(
    m: MaterialU,
    tex: texture_2d<f32>,
    samp: sampler,
    albedo_in: vec3<f32>,
    obj_pos: vec3<f32>,
    normal: vec3<f32>,
    screen_uv: vec2<f32>,
    view: vec3<f32>,
    time: f32,
) -> Shaded {
    var albedo = albedo_in;
    var n = normal;
    var roughness = 1.0;
    var metalness = 0.0;
    var alpha = 1.0;

    // The glitch is a per-frame hash gate clocked by `mat_glitch_rate`, and it
    // is **the same beat as the strobe**: the row drives `mat_glitch` from the
    // onset column (`metal_schema::MATERIAL_COLUMNS`, the column
    // `models::estrobo`'s flash rides), so a hit opens this gate on the flash
    // and nowhere else. One gate breaks the UV, the normal and the tint —
    // a broken surface, not a filter over a clean one.
    let gl = clamp(m.d.x, 0.0, 1.0);
    let frame = floor(time * max(m.d.y, 0.0) + m.d.z * 97.0);
    let cell = floor(obj_pos * 16.0);
    let h = hash21(vec2<f32>(cell.x + cell.y * 7.0 + frame, cell.z));
    let gate = step(1.0 - gl, h) * step(1e-4, gl);

    // UV: where the gate is open, a block drags the picture's own sample — the
    // object mapping and the projector mapping both tear.
    var p = obj_pos;
    var suv = screen_uv;
    if (gate > 0.0) {
        let off = vec2<f32>(hash21(vec2<f32>(frame, 1.0)), hash21(vec2<f32>(frame, 2.0))) - vec2<f32>(0.5);
        p = obj_pos + vec3<f32>(off * 0.06, 0.0);
        suv = screen_uv + off * 0.06;
    }

    let imix = clamp(m.a.x, 0.0, 1.0);
    if (imix > 0.0) {
        let t = surface_sample(tex, samp, p, normal, suv, u32(m.a.y + 0.5), max(m.a.z, 1e-3));
        albedo = mix(albedo, t.rgb, imix);
    }

    let mmix = clamp(m.b.x, 0.0, 1.0);
    if (mmix > 0.0) {
        let sc = max(m.b.y, 1e-3);
        let kind = u32(m.a.w + 0.5);
        let layer = mat_layer(kind, albedo, p, normal, view, time, sc);
        var base = layer.albedo;

        let tint = clamp(m.c.z, 0.0, 1.0);
        if (tint > 0.0) {
            let tc = mat_hue(m.c.w);
            base = mix(base, base * (0.35 + 0.65 * tc) + tc * 0.15, tint);
        }

        let env = clamp(m.c.y, 0.0, 1.0);
        if (env > 0.0) {
            let r = reflect(-normalize(view), normalize(normal));
            let sky = mix(
                vec3<f32>(0.04, 0.06, 0.10),
                vec3<f32>(0.65, 0.78, 1.0),
                clamp(r.y * 0.5 + 0.5, 0.0, 1.0),
            );
            base = base + sky * env * (0.15 + 0.85 * layer.metal);
        }

        // Normal and tint: the same gate tilts the surface and burns the cell's
        // own hue through the colour.
        if (gate > 0.0) {
            let jitter = vec3<f32>(
                hash21(vec2<f32>(frame, 1.0)),
                hash21(vec2<f32>(frame, 2.0)),
                hash21(vec2<f32>(frame, 3.0)),
            ) - vec3<f32>(0.5);
            n = normalize(n + jitter * gate * 0.35);
            base = mix(base, mat_hue(h), gate * 0.6);
        }

        let bump = clamp(m.c.x, 0.0, 1.0);
        if (bump > 0.0) {
            let g = vec3<f32>(
                hash21(obj_pos.xy * 12.0),
                hash21(obj_pos.yz * 12.0),
                hash21(obj_pos.zx * 12.0),
            ) - vec3<f32>(0.5);
            n = normalize(n + g * (0.25 + 0.75 * layer.bump) * bump);
        }

        albedo = mix(albedo, base, mmix);
        roughness = mix(1.0, layer.rough, mmix);
        if (m.b.z > 0.0) { roughness = m.b.z; }
        metalness = mix(0.0, layer.metal, mmix);
        if (m.b.w > 0.0) { metalness = m.b.w; }
        alpha = mix(1.0, layer.alpha, mmix);
    }

    return Shaded(albedo, n, roughness, metalness, alpha);
}
