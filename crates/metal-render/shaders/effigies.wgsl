// effigies: stylised, procedurally shaded head-and-bust silhouettes.
// The parameter list follows the "Humans" tool (pose, materials, light,
// floppy physics, grid views, perspective); all geometry and shading here is
// original. No meshes, textures or upstream shader code are used.
//
// A single smooth height field builds the cranium, jaw, neck and shoulders.
// Its gradient is the surface normal, so one directional key light gives the
// 3D read. Carved eyes, brows, nose and lips are height bumps plus colour
// masks. The grid block repeats the bust (single / ring / wall / tunnel), and
// the physics block bends it along a held spine point.

struct EffCell {
    q: vec2<f32>,
    id: vec2<f32>,
};

fn eff_dir(i: f32) -> vec2<f32> {
    let k = floor(i + 0.5);
    if (k < 0.5) { return vec2<f32>(-1.0, 0.0); }
    if (k < 1.5) { return vec2<f32>(0.0, -1.0); }
    if (k < 2.5) { return vec2<f32>(0.0, 1.0); }
    return vec2<f32>(1.0, 0.0);
}

fn eff_light(i: f32) -> vec3<f32> {
    let k = floor(i + 0.5);
    if (k < 0.5) { return normalize(vec3<f32>(-0.45, -0.62, 0.64)); }
    if (k < 1.5) { return normalize(vec3<f32>(0.08, -0.92, 0.38)); }
    if (k < 2.5) { return normalize(vec3<f32>(-0.92, -0.12, 0.38)); }
    return normalize(vec3<f32>(0.42, 0.48, 0.76));
}

// The bust's height above the picture plane. Higher = closer to the viewer.
fn eff_height(q: vec2<f32>, ya: f32, pit: f32) -> f32 {
    let cy = -0.18;
    let down = clamp((q.y - cy) / 0.30, 0.0, 1.0);
    let hw = 0.205 * (1.0 - 0.36 * down * down);
    let hh = 0.255;
    let hr = length(vec2<f32>(q.x / max(hw, 0.04), (q.y - cy) / hh));
    var h = sqrt(max(0.0, 1.0 - hr * hr)) * 0.62;

    // Neck column.
    let nr = length(vec2<f32>(q.x / 0.082, (q.y - 0.01) / 0.19));
    h = max(h, sqrt(max(0.0, 1.0 - nr * nr)) * 0.30);

    // Shoulders and chest.
    let sr = length(vec2<f32>(q.x / 0.42, (q.y - 0.40) / 0.20));
    h = max(h, sqrt(max(0.0, 1.0 - sr * sr)) * 0.38);

    // Nose ridge, turning with yaw and pitch.
    let fx = sin(ya) * 0.07;
    let fy = sin(pit) * 0.03;
    let nx = (q.x - fx * 1.15) / 0.022;
    let ny = (q.y - (cy + 0.10 - fy)) / 0.085;
    let nose = exp(-nx * nx) * exp(-ny * ny);
    h = h + nose * 0.105;

    // Brow ridge.
    let bx = 0.070;
    let by = cy + 0.005 - fy;
    let byy = (q.y - (by - 0.048)) / 0.026;
    let bl = (q.x - fx + bx) / 0.060;
    let br = (q.x - fx - bx) / 0.060;
    let brow = exp(-byy * byy) * (exp(-bl * bl) + exp(-br * br));
    h = h + brow * 0.038;

    // Ears, slightly forward of the silhouette.
    let ear_y = cy + 0.035;
    let el = length(vec2<f32>((q.x + 0.202) / 0.042, (q.y - ear_y) / 0.078));
    let er = length(vec2<f32>((q.x - 0.202) / 0.042, (q.y - ear_y) / 0.078));
    h = max(h, sqrt(max(0.0, 1.0 - el * el)) * 0.30);
    h = max(h, sqrt(max(0.0, 1.0 - er * er)) * 0.30);

    // A little chiselled grain so the surface never reads as plastic.
    let hgrain = (p_noise(q * 26.0 + vec2<f32>(f_seed() * 3.0)) - 0.5) * 0.0028;
    h = h + hgrain * smoothstep(0.0, 0.03, h);
    return h;
}

// Arrange the picture into one bust cell: single, ring, wall or tunnel.
fn eff_cell(world: vec2<f32>, t: f32) -> EffCell {
    let mode = floor(f_grid() + 0.5);
    let amount = max(f_grid_amount(), 1.0);
    let rad = clamp(f_grid_radius() / 100.0, 0.0, 1.0);
    let gspd = f_grid_speed();
    let gdir = f_grid_dir();
    var q = world;
    var cell = vec2<f32>(0.0);

    if (mode < 0.5) {
        // One effigy, centred.
        q = world;
    } else if (mode < 1.5) {
        // A ring of effigies around the centre.
        let n = max(round(amount), 1.0);
        let sect = TAU / n;
        let centre = vec2<f32>(0.0, -0.04);
        let d = world - centre;
        let ang = atan2(d.y, d.x);
        let sgn = select(-1.0, 1.0, gdir > 1.5);
        let phase = t * gspd * 0.55 * sgn;
        let idx = floor((ang - phase) / sect + 0.5);
        let ca = centre + vec2<f32>(cos(phase + idx * sect), sin(phase + idx * sect))
            * (0.16 + rad * 0.72);
        q = world - ca;
        cell = vec2<f32>(idx, 1.0);
    } else if (mode < 2.5) {
        // A wall of effigies, optionally brick-offset and scrolled.
        let cols = max(round(f_wall_cols() + amount * 0.5), 1.0);
        let rows = max(round(f_wall_rows()), 1.0);
        let zoom = max(f_wall_zoom(), 0.1);
        let sp = vec2<f32>(1.10, 1.10) * zoom / vec2<f32>(cols, rows)
            * (0.80 + 0.40 * rad);
        let ww = world - eff_dir(gdir) * t * gspd * 0.25;
        let row = floor(ww.y / sp.y + 0.5);
        let parity = row - 2.0 * floor(row * 0.5);
        let off = f_wall_brick() * 0.5 * sp.x * parity;
        let tint = floor((ww.x - off) / sp.x + 0.5);
        let centre = vec2<f32>(tint * sp.x + off, row * sp.y);
        let bscale = max(min(sp.y / 1.15, sp.x / 0.55), 0.04);
        q = (ww - centre) / bscale;
        cell = vec2<f32>(tint, row);
    } else {
        // Concentric bands down a tunnel, each band a turned effigy.
        let centre = vec2<f32>(0.0, -0.02);
        let d = world - centre;
        let r = max(length(d), 0.001);
        let ratio = 1.30 + 0.05 * amount + 0.28 * rad;
        let sgn = select(-1.0, 1.0, gdir > 1.5);
        let flow = t * f_tunnel_flow() * 0.35 * sgn;
        let bandf = log(r) / log(ratio) + flow;
        let band = clamp(floor(bandf), -18.0, 18.0);
        let sc = pow(ratio, band) * (0.55 + rad * 0.5);
        let turn = band * f_tunnel_twist() * 0.16
            + f_tunnel_angle() * PI / 180.0 * 0.5;
        q = p_rot(d / max(sc, 0.0001), turn);
        cell = vec2<f32>(band, 0.0);
    }
    return EffCell(q, cell);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let seedv = f_seed();

    // Object transform: size, offset, in-plane roll.
    var world = p - vec2<f32>(f_offset_x(), f_offset_y()) / 100.0;
    world = world / max(f_size(), 0.05);
    world = p_rot(world, f_roll() * PI / 180.0);

    let ec = eff_cell(world, t);
    var q = ec.q;
    let cellseed = p_hash2(ec.id * 1.7 + vec2<f32>(seedv * 3.1, seedv * 7.7));

    // Universal tunnel terms: they also give the other views a slow drift.
    let drift = f_tunnel_twist() * 0.22 * sin(t * 0.9 + cellseed.x * TAU);
    q = p_rot(q, drift + f_tunnel_angle() * PI / 180.0 * 0.35);
    q = q + vec2<f32>(
        f_tunnel_flow() * 0.05 * sin(t * 1.4 + cellseed.y * TAU),
        f_tunnel_flow() * 0.02 * cos(t * 1.1 + cellseed.x * TAU)
    );

    // Perspective: nearer copies read larger, with a small vertical parallax.
    let persp = clamp(f_perspective() / 100.0, 0.0, 1.0);
    q = q * (0.90 + 0.25 * persp);
    q.y = q.y + (cellseed.x - 0.5) * 0.06 * persp;

    // Floppy spine: bend everything above/below the held point.
    let holdv = floor(f_hold() + 0.5);
    var anchor = 0.38;
    if (holdv < 0.5) {
        anchor = -0.22;
    } else if (holdv < 1.5) {
        anchor = 0.06;
    }
    let lever = q.y - anchor;
    let stiff = clamp(f_stiffness() / 100.0, 0.0, 1.0);
    let bend = f_floppy() * (1.60 - 1.20 * stiff);
    let gstr = f_gravity() * clamp(f_gravity_strength() / 100.0, 0.0, 1.0) * 2.5;
    let gx = f_gravity_x() / 100.0;
    let gy = f_gravity_y() / 100.0;
    let sway = f_floppy() * (f_swing() / 100.0) * 0.08
        * sin(t * 2.2 + seedv + q.y * 2.5);
    q.x = q.x + lever * lever * (bend * gx * 3.0 + gstr * gx * 2.0) + sway;
    q.y = q.y + lever * lever * (bend * gy * 1.6 + gstr * gy * 1.2);

    // Spin and pose.
    let spin = t * f_speed() * 0.9;
    let dirv = eff_dir(f_spin_dir());
    let ya = f_yaw() * PI / 180.0 + spin * dirv.x * 2.2;
    let pit = f_pitch() * PI / 180.0 + spin * dirv.y * 0.35;
    let cy = -0.18;
    let fs = sin(ya);
    let fc = cos(ya);
    let fx = fs * 0.075;
    let fy = sin(pit) * 0.035;

    // Height field and its gradient normal.
    let e = 0.0035;
    let h0 = eff_height(q, ya, pit);
    let hx = eff_height(q + vec2<f32>(e, 0.0), ya, pit);
    let hy = eff_height(q + vec2<f32>(0.0, e), ya, pit);
    let nrm = normalize(vec3<f32>(
        -(hx - h0) / e * 0.22,
        -(hy - h0) / e * 0.22,
        1.0
    ));
    let cov = smoothstep(0.0, 0.008, h0);

    // Material parameters.
    let mati = floor(f_material() + 0.5);
    var rough = 0.80;
    var spec_k = 0.10;
    var metal_k = 0.0;
    var sheen_k = 0.10;
    var irid_k = 0.0;
    var glass_k = 0.0;
    var clear_k = 0.0;
    var toon_k = 0.0;
    if (mati > 0.5 && mati < 1.5) {
        rough = 0.28; spec_k = 0.85; clear_k = 0.8;
    } else if (mati > 1.5 && mati < 2.5) {
        rough = 0.20; spec_k = 0.90; metal_k = 1.0;
    } else if (mati > 2.5 && mati < 3.5) {
        rough = 0.30; spec_k = 1.00; metal_k = 0.5; clear_k = 0.9;
    } else if (mati > 3.5 && mati < 4.5) {
        rough = 0.95; spec_k = 0.04; sheen_k = 1.0;
    } else if (mati > 4.5 && mati < 5.5) {
        rough = 0.25; spec_k = 0.60; metal_k = 0.6; irid_k = 1.0;
    } else if (mati > 5.5 && mati < 6.5) {
        rough = 0.08; spec_k = 1.00; metal_k = 0.2; glass_k = 1.0;
    } else if (mati > 6.5 && mati < 7.5) {
        rough = 1.00; spec_k = 0.0; sheen_k = 0.0; toon_k = 1.0;
    } else if (mati > 7.5) {
        rough = 0.55; spec_k = 0.25;
    }

    // Face-local coordinates: yaw slides the features, pitch lifts them.
    let face = q - vec2<f32>(fx, fy);
    let ex = 0.068 * (0.62 + 0.38 * abs(fc));
    let ey = cy + 0.035;

    // Colours before carving.
    var skin = p_pal(0u, clamp(0.30 + 1.7 * (q.y - cy + 0.22) + 0.10 * cellseed.y, 0.0, 1.0));
    let rimc = p_pal(1u, clamp(0.42 + 0.34 * cellseed.y + 0.18 * (q.y + 0.4), 0.0, 1.0));
    let cloth = p_pal(2u, clamp(0.30 + 0.55 * (q.y + 0.2) + 0.12 * cellseed.x, 0.0, 1.0));
    let grain = p_noise(face * 44.0 + vec2<f32>(seedv * 5.0, 0.0));
    skin = skin * (0.92 + 0.16 * grain);

    // Marble veining is a colour stage before the light.
    if (mati > 7.5) {
        let vn = p_fbm(q * 7.0 + vec2<f32>(seedv * 2.0, seedv * 0.7), 4);
        let vein = 1.0 - smoothstep(0.0, 0.16, abs(vn - 0.5));
        skin = mix(skin, rimc, vein * 0.55);
    }
    let ids = cellseed;

    // Carved features.
    let eye_l = 1.0 - smoothstep(0.80, 1.05,
        length(vec2<f32>((face.x + ex) / 0.030, (face.y - ey) / 0.017)));
    let eye_r = 1.0 - smoothstep(0.80, 1.05,
        length(vec2<f32>((face.x - ex) / 0.030, (face.y - ey) / 0.017)));
    let eye = max(eye_l, eye_r);
    let pupil_l = 1.0 - smoothstep(0.65, 1.0,
        length(vec2<f32>((face.x + ex + fs * 0.006) / 0.012, (face.y - ey) / 0.012)));
    let pupil_r = 1.0 - smoothstep(0.65, 1.0,
        length(vec2<f32>((face.x - ex + fs * 0.006) / 0.012, (face.y - ey) / 0.012)));
    let pupil = max(pupil_l, pupil_r);
    let glint_l = 1.0 - smoothstep(0.4, 1.0,
        length(vec2<f32>((face.x + ex - 0.006) / 0.005, (face.y - ey + 0.006) / 0.005)));
    let glint_r = 1.0 - smoothstep(0.4, 1.0,
        length(vec2<f32>((face.x - ex - 0.006) / 0.005, (face.y - ey + 0.006) / 0.005)));
    let glint = max(glint_l, glint_r);
    let lip = 1.0 - smoothstep(0.78, 1.0,
        length(vec2<f32>(face.x / 0.050, (face.y - (cy + 0.155)) / 0.016)));
    let lip_shadow = 1.0 - smoothstep(0.70, 1.0,
        length(vec2<f32>(face.x / 0.043, (face.y - (cy + 0.172)) / 0.010)));

    skin = mix(skin, cloth * 0.60, eye * 0.88);
    skin = mix(skin, vec3<f32>(0.006, 0.006, 0.008), pupil * 0.90);
    skin = skin + rimc * glint * 1.3;
    skin = mix(skin, skin * 0.55 + vec3<f32>(0.10, 0.02, 0.015), lip * 0.65);
    skin = mix(skin, skin * 0.40, lip_shadow * 0.45);

    // Hair cap and clothing.
    let hairline = cy - 0.105 + 0.012 * sin(q.x * 40.0 + seedv * 3.0);
    var hair = 1.0 - smoothstep(hairline - 0.006, hairline + 0.006, q.y);
    hair = hair * (1.0 - smoothstep(cy + 0.10, cy + 0.16, q.y));
    hair = hair * smoothstep(0.02, 0.20, h0 + 0.02);
    skin = mix(skin, cloth, hair * 0.90);
    let clothes = smoothstep(0.15 + 0.05 * abs(q.x), 0.22 + 0.05 * abs(q.x), q.y);
    skin = mix(skin, cloth, clothes * 0.92);

    // Lighting.
    let L = eff_light(f_light());
    let V = vec3<f32>(0.0, 0.0, 1.0);
    let Hv = normalize(L + V);
    let ndl_raw = clamp(dot(nrm, L), 0.0, 1.0);
    let ndh = clamp(dot(nrm, Hv), 0.0, 1.0);
    let fres = pow(1.0 - clamp(nrm.z, 0.0, 1.0), 3.0);

    var diff = ndl_raw;
    if (toon_k > 0.5) {
        diff = 0.30 + 0.32 * smoothstep(0.34, 0.44, ndl_raw)
            + 0.38 * smoothstep(0.72, 0.82, ndl_raw);
    }
    var lit = 0.30 + 0.82 * diff;
    if (metal_k > 0.9) {
        lit = 0.22 + 0.95 * diff;
    }
    if (glass_k > 0.5) {
        lit = 0.55 + 0.45 * diff;
    }
    if (sheen_k > 0.9) {
        lit = 0.34 + 0.56 * diff;
    }

    var irid = vec3<f32>(0.5) + 0.5 * cos(TAU * (
        fres * 1.35 + vec3<f32>(0.0, 0.33, 0.67) + 0.17 * ids.y
    ));
    irid = clamp(irid, vec3<f32>(0.0), vec3<f32>(1.0));

    var tint = skin * lit;
    tint = tint + rimc * spec_k * pow(ndh, mix(130.0, 9.0, rough))
        * (0.20 + 0.80 * clear_k) * (0.6 + 1.5 * metal_k);
    tint = tint + rimc * fres * (0.08 + 0.38 * metal_k + 0.40 * glass_k + 0.55 * sheen_k);
    tint = tint + irid * irid_k * fres * 0.75;
    if (sheen_k > 0.9) {
        tint = mix(tint, rimc * 0.55, (1.0 - ndl_raw) * 0.22);
    }
    if (glass_k > 0.5) {
        tint = mix(tint, skin * 0.55 + rimc * 0.35, 0.30);
        tint = tint + rimc * fres * 0.35;
    }
    if (toon_k > 0.5) {
        tint = tint * (0.62 + 0.38 * smoothstep(0.0, 0.10, h0));
    }

    // Toon ink at the silhouette edge.
    if (toon_k > 0.5) {
        tint = mix(cloth * 0.20, tint, smoothstep(0.0, 0.06, cov));
    }

    tint = max(tint, vec3<f32>(0.0));
    var cover = cov;
    if (glass_k > 0.5) {
        cover = cov * 0.88;
    }
    return vec4<f32>(tint, cover);
}
