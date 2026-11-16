// ringflow: concentric sliced rings from a Cassini-oval distance field.
// Behaviour and parameter list follow the sketch-tool "Cassini Flow";
// everything here is original, evaluated per pixel instead of by polygon
// clipping. The two focus points (core separation) give the oval family; the
// field is warped and drifted in time, then cut into `slices` radial bands
// whose width is `slice_width`. A seeded fraction of the bands uses the accent
// slot. Gaps between bands let the backdrop through.

fn rf_level(t: f32, core: f32, scale: f32) -> f32 {
    let lo = mix(0.035, 0.17, core * 0.6);
    let hi = mix(1.10, 3.10, scale * 0.55 + 0.45);
    let curve = mix(1.65, 0.76, core);
    return mix(lo, hi, pow(clamp(t, 0.0, 1.0), curve));
}

fn rf_accent(i: i32, seed: f32, amount: f32) -> bool {
    let h = p_hash(vec2<f32>(seed * 23.71 + f32(i + 1) * 11.93, 7.31));
    return h < amount;
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let aspect = p_aspect();

    let core = clamp(f_core_separation() / 100.0, 0.0, 1.0);
    let scale = clamp(f_scale() / 100.0, 0.0, 1.0);
    let warp = clamp(f_warp() / 100.0, 0.0, 1.0);
    let drift = clamp(f_drift_v() / 100.0, 0.0, 1.0);
    let sw = clamp(f_slice_width(), 5.0, 100.0) / 100.0;
    let accent = clamp(f_accent() / 100.0, 0.0, 1.0);
    let seed = f_seed();
    let mode = i32(clamp(floor(f_mode() + 0.5), 0.0, 5.0));
    let n = i32(clamp(floor(f_slices() + 0.5), 2.0, 36.0));
    let rot = f_rotation() * PI / 180.0;

    let tt = (t + f_time()) * f_speed() * (0.25 + drift) * 3.2 + seed * 0.173;
    let s = seed * 0.173;
    let fscale = mix(1.35, 6.85, scale);

    let q = p_rot(p, rot);
    var x = (q.x / aspect) * fscale;
    var y = q.y * fscale;

    if (mode == 1) {
        y = y * 1.42;
    } else if (mode == 2) {
        x = x * 0.78;
        y = y * 1.12;
    } else if (mode == 3) {
        x = x + sin(y * 1.25 + s + tt * 0.45) * 0.24;
    } else if (mode == 4) {
        y = y + sin(x * 1.15 - s + tt * 0.38) * 0.28;
    } else if (mode == 5) {
        let a = atan2(y, x);
        let k = 1.0 + 0.18 * sin(a * 5.0 + s + tt * 0.35);
        x = x * k;
        y = y * k;
    }

    let wa = warp * 0.38;
    if (wa > 0.0001) {
        let nx = wa * (0.55 * sin(y * 2.35 + s + tt * 0.60)
                     + 0.24 * sin((x + y) * 3.10 - s * 0.7 - tt * 0.42));
        let ny = wa * (0.55 * cos(x * 2.05 - s * 0.8 - tt * 0.51)
                     + 0.24 * cos((x - y) * 2.80 + s * 1.2 + tt * 0.33));
        x = x + nx;
        y = y + ny;
    }

    let dBase = mix(0.10, 1.28, core);
    let dPulse = 0.10 * warp * sin(tt * 0.55 + s);
    let d = max(0.025, dBase + dPulse);
    var ax = -d;
    var ay = 0.0;
    var bx = d;
    var by = 0.0;
    if (mode == 2) {
        ay = -d * 0.36;
        by = d * 0.36;
    } else if (mode == 4) {
        ax = 0.0;
        bx = 0.0;
        ay = -d;
        by = d;
    } else if (mode == 5) {
        ax = -d * 0.84;
        bx = d * 0.84;
        ay = sin(tt * 0.25 + s) * d * 0.20;
        by = -ay;
    }

    let r1 = length(vec2<f32>(x - ax, y - ay));
    let r2 = length(vec2<f32>(x - bx, y - by));
    var v = sqrt(max(r1 * r2, 0.0));
    if (mode == 3 || mode == 5) {
        let a = atan2(y, x);
        v = v + warp * 0.11 * sin(a * 6.0 + s + tt * 0.31);
    }

    let lo = rf_level(0.0, core, scale);
    let hi = rf_level(1.0, core, scale);
    let aa = max(fwidth(v) * 1.2, 0.002);

    var col = vec3<f32>(0.0);
    var cov = 0.0;
    for (var i = 0; i < n; i = i + 1) {
        let t0 = f32(i) / f32(n);
        let low = rf_level(t0, core, scale);
        let next = rf_level(f32(i + 1) / f32(n), core, scale);
        let high = mix(low, next, sw);
        let inside = p_edge(v, low, aa) * (1.0 - p_edge(v, high, aa));
        if (inside > 0.001) {
            let tc = clamp((v - lo) / max(hi - lo, 0.0001), 0.0, 1.0);
            let band = mix(tc, f32(i) / f32(max(n - 1, 1)), 0.35);
            if (rf_accent(i, seed, accent)) {
                col = p_pal(1u, clamp(band * 0.6 + 0.2, 0.0, 1.0));
            } else {
                col = p_pal(0u, clamp(band, 0.0, 1.0));
            }
            cov = inside;
            break;
        }
    }

    return vec4<f32>(col, cov);
}
