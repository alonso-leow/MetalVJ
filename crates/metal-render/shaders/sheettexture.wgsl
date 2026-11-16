// sheettexture: a sheet of paper lit from the side, laid over an optional
// source picture; everything here is original.
//
// A height field is built from five layers: fibres (stretched, rotated noise in
// two directions plus a few long thin strands), roughness (fine isotropic
// noise), crumples (ridged low-frequency fbm: valleys where the noise crosses
// 0.5), folds (up to 12 straight creases, each a V groove with a soft facet
// beside it, limited to a segment) and drops (one water-stain dome per grid
// cell, with a darker rim). The normal comes from forward differences of that
// field and is lit by an orbiting light (diffuse + a satin highlight). The
// picture is treated as ink printed on the sheet: paper ramp (`papel` slot)
// times the picture, tinted by `tinta`, stained by `mancha`.

fn pt_off() -> vec2<f32> { return vec2<f32>(f_seed() * 7.31 + 3.0, f_seed() * 3.17 + 11.0); }

// Water drops: x = dome height, y = stain mask. One drop per cell of size ~gotas_tam.
fn pt_drops(q: vec2<f32>) -> vec2<f32> {
    let g = q / max(f_drops_size(), 0.03) + pt_off();
    let id = floor(g);
    let f = fract(g);
    let h = p_hash2(id);
    let pres = step(1.0 - f_drops() * 0.9, p_hash(id + vec2<f32>(7.0, 1.0)));
    let c = 0.3 + 0.4 * h;
    let r = 0.12 + 0.18 * p_hash(id + vec2<f32>(2.0, 5.0));
    let d = length(f - c) / r;
    let dome = (1.0 - smoothstep(0.0, 1.0, d)) * pres;
    let rim = exp(-pow((d - 0.95) * 6.0, 2.0)) * pres;
    return vec2<f32>(dome * 0.5 - rim * 0.25, clamp(dome * 0.6 + rim, 0.0, 1.0));
}

fn pt_folds(q: vec2<f32>, t: f32) -> f32 {
    var acc = 0.0;
    let n = i32(floor(f_n_folds() + 0.5));
    for (var i = 0; i < 12; i = i + 1) {
        if (i >= n) { break; }
        let fi = f32(i) + f_seed() * 0.37;
        let h = p_hash2(vec2<f32>(fi, 41.0));
        let a = h.x * PI + sin(t * f_folds_speed() + fi * 2.3) * 0.15;
        let c = (h - 0.5) * vec2<f32>(1.6, 1.0);
        let nrm = vec2<f32>(cos(a), sin(a));
        let tan = vec2<f32>(-nrm.y, nrm.x);
        let r = q - c;
        let d = dot(r, nrm);
        let al = dot(r, tan);
        let seg = 1.0 - smoothstep(0.25, 0.9, abs(al));
        let crease = -exp(-abs(d) * 55.0) + 0.35 * sign(d) * exp(-abs(d) * 5.0);
        acc = acc + crease * seg;
    }
    return acc * f_folds();
}

fn pt_height(q: vec2<f32>, t: f32) -> f32 {
    let off = pt_off();
    var h = 0.0;
    // Fibres: two orientations, streaks drift along their length.
    let fk = 8.0 / max(f_fiber_size(), 0.02);
    let fa = p_hash(vec2<f32>(f_seed(), 3.0)) * PI;
    let drift = t * f_wind() * 0.05;
    let u1 = p_rot(q, fa) + off;
    let u2 = p_rot(q, fa + 1.2) + off * 1.7;
    let n1 = p_noise(vec2<f32>(u1.x * fk / 14.0 + drift, u1.y * fk));
    let n2 = p_noise(vec2<f32>(u2.x * fk / 10.0 - drift, u2.y * fk * 0.7));
    let strand = smoothstep(0.72, 0.95, p_noise(vec2<f32>(u1.x * fk / 40.0, u1.y * fk * 0.5)));
    h = h + f_fiber() * ((n1 - 0.5) + (n2 - 0.5) * 0.7 + strand * 0.6) * 0.5;
    // Roughness: fine tooth.
    h = h + f_roughness() * (p_noise(q * 220.0 + off) - 0.5) * 0.45;
    // Crumples: ridged fbm, slowly breathing.
    let cs = 2.0 / max(f_wrinkles_size(), 0.05);
    let breath = 1.0 + f_breath() * 0.4 * sin(t * 0.8);
    let cn = p_fbm(q * cs + off + vec2<f32>(t * f_breath() * 0.08, 0.0), 3);
    h = h + f_wrinkles() * breath * (abs(cn - 0.5) * 2.0 - 0.4) * 0.9;
    h = h + pt_folds(q, t) * 0.5;
    h = h + pt_drops(q).x * f_drops() * 0.3;
    return h;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    // fade: texture strength falls off towards the frame edge.
    let rr = length((uv - 0.5) * vec2<f32>(1.0, 1.0)) * 1.8;
    let amt = 1.0 - f_fade() * smoothstep(0.15, 1.0, rr);

    let e = 0.003;
    let h0 = pt_height(p, t);
    let hx = pt_height(p + vec2<f32>(e, 0.0), t);
    let hy = pt_height(p + vec2<f32>(0.0, e), t);
    let k = f_relief() * amt * 0.02;
    let nrm = normalize(vec3<f32>(-(hx - h0) / e * k, -(hy - h0) / e * k, 1.0));

    let la = f_light_angle() * TAU + t * f_light_speed();
    let el = mix(0.15, 1.4, f_light_height());
    let L = normalize(vec3<f32>(cos(la) * cos(el), sin(la) * cos(el), sin(el)));
    let flat_d = L.z;
    let diff = dot(nrm, L) - flat_d;
    let Hv = normalize(L + vec3<f32>(0.0, 0.0, 1.0));
    let spec = pow(max(dot(nrm, Hv), 0.0), 40.0) * f_satin();

    // Contrast stretches the shading around mid-lightness.
    var lit = 0.72 + diff * (0.6 + f_contrast_paper() * 3.0);
    lit = clamp(lit, 0.0, 1.0);

    // Aged stains from the third slot.
    let ms = 1.5 / max(f_stain_size(), 0.05);
    let sn = p_fbm(p * ms + pt_off() * 0.5 + vec2<f32>(0.0, t * 0.01), 4);
    let stain = smoothstep(0.45, 0.8, sn) * f_age() * amt;
    let dr = pt_drops(p).y * f_drops() * amt;

    var col = p_pal(0u, lit);
    let ink = mix(vec3<f32>(1.0), p_src(p).rgb, f_mix());
    col = col * ink;
    col = mix(col, col * p_pal(1u, lit) * 1.6, f_tint());
    col = mix(col, col * p_pal(2u, 0.5 + dr * 0.3) * 1.3, clamp(stain + dr * 0.5, 0.0, 1.0));
    col = col + vec3<f32>(spec) * amt;
    col = col + vec3<f32>(p_grain(uv, t)) * f_grain() * 0.3;
    // fade also lets the background show through at the frame edge.
    let cov = 1.0 - f_fade() * 0.7 * smoothstep(0.75, 1.1, rr);
    return vec4<f32>(max(col, vec3<f32>(0.0)), cov);
}
