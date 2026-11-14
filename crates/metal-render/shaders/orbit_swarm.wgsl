// orbit-swarm: orbiting particle field. Behaviour and controls follow the
// built-in "particles" tool's sphere/cube orbit and vibration/wave modes; this
// implementation is original.
//
// Each particle owns a radius, an angular slot on a golden-angle spiral, a
// Keplerian orbital rate and a depth phase. The radius is a blend of a
// jittered ring band and a uniform disc (`ring_mix`), squashed into an ellipse,
// rotated as a whole, then deformed by organic wobble, turbulence, swirl,
// vibration inside a radial zone, and radial wave kicks. A particle draws a
// soft trail behind its head plus a shaped core and a halo. A separate sparse
// dust lattice fills the gaps. Coverage is faded at the frame rim, so the
// `fondo` slot and `background_level` always show through.

const OS_GOLD: f32 = 2.39996323;
const OS_MAX: i32 = 120;

fn os_rot(v: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

fn os_seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.000001), 0.0, 1.0);
    return length(pa - ba * h);
}

fn os_core(d: vec2<f32>, rad: f32, kind: f32) -> f32 {
    let a = abs(d);
    let ang = atan2(d.y, d.x + 0.000001);
    var q = length(d) / max(rad, 0.0001);
    if (kind > 0.5 && kind < 1.5) {
        q = (a.x + a.y) * 0.7071 / max(rad, 0.0001);
    } else if (kind >= 1.5) {
        q = q * (0.72 + 0.28 * abs(sin(ang * 5.0)));
    }
    return 1.0 - smoothstep(0.72, 1.0, q);
}

fn scaffold_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let t = p_time();
    let aspect = p_aspect();
    let sed = f_seed();
    // Authored count, scaled down for small tiles (p_detail: 0.25..1.0).
    let count = floor(mix(20.0, 120.0, clamp(f_density() / 100.0, 0.0, 1.0)) * p_detail());
    let bands = max(floor(f_bands() + 0.5), 1.0);
    let rot = f_rotation() * TAU;
    let ell = 1.0 - clamp(f_orbit_ellipse(), 0.0, 1.0) * 0.6;
    let rad_base = 0.0030 + 0.0045 * f_size();
    let shape = f_shape();
    let trail_len = f_trail() * 1.1;
    let trail_on = smoothstep(0.0, 0.02, f_trail());

    var acc = vec3<f32>(0.0);
    var cov = 0.0;

    for (var i: i32 = 0; i < OS_MAX; i = i + 1) {
        let fi = f32(i);
        if (fi >= count) { break; }

        let h1 = p_hash(vec2<f32>(fi * 1.7, 1.3 + sed * 3.1));
        let h2 = p_hash(vec2<f32>(fi * 1.31 + 4.0, 8.3 + sed * 1.9));
        let h3 = p_hash(vec2<f32>(fi * 0.73 + 9.0, 17.1 + sed * 2.2));
        let h4 = p_hash(vec2<f32>(fi * 1.97 + 2.0, 29.3 + sed * 0.7));

        // Radius: a jittered ring band blended with a uniform disc.
        let band = floor(h1 * bands);
        let band_t = (band + 0.5) / bands;
        let r_ring = f_orbit_radius() * (0.30 + 0.70 * band_t);
        let r_disc = f_orbit_radius() * sqrt(clamp(h1, 0.0, 1.0));
        var r = mix(r_disc, r_ring, clamp(f_ring_mix(), 0.0, 1.0));
        r = r + (h2 - 0.5) * (0.010 + f_thickness() * 0.075);

        // Organic wobble and angular spread.
        let wph = f_frequency() * h3 * TAU + t * 0.9 + fi * 0.37;
        r = r + (h3 - 0.5) * f_spread() * 0.045 + f_wobble() * 0.055 * sin(wph);
        var th = fi * OS_GOLD + (h3 - 0.5) * f_spread() * 3.0;

        // Keplerian drift: inner particles orbit faster.
        th = th + f_speed() * t * (0.35 + 1.1 * h4) / (0.18 + r);
        // Turbulence bends the orbital phase.
        th = th + f_turbulence() * 0.55 * sin(th * 2.0 + t * 0.6 + h2 * TAU) * (0.30 + r);
        // Swirl adds a radial-dependent tangential shear.
        th = th + f_swirl() * 0.70 * sin(r * 9.0 - t * 0.8 + h1 * TAU);

        let dep = 0.5 + 0.5 * sin(th + h2 * TAU);
        let dscale = mix(1.0, 0.55 + 0.90 * dep, clamp(f_depth(), 0.0, 1.0));
        let rad = rad_base * dscale;

        let head0 = vec2<f32>(r * cos(th), r * sin(th) * ell);
        let th_tail = th - trail_len;
        let tail0 = vec2<f32>(r * cos(th_tail), r * sin(th_tail) * ell);
        let pos_h = os_rot(head0, rot);
        let pos_t = os_rot(tail0, rot);

        // Radial wave kick (the tool's travelling displacement bumps).
        let wph2 = r * 12.0 - t * (1.0 + f_wave_speed() * 2.0) + h2 * TAU;
        let wdisp = f_wave_amp() * 0.030 * sin(wph2) * (1.0 - smoothstep(0.0, f_orbit_radius() * 1.2, r) * 0.5);
        let dirv = normalize(vec2<f32>(cos(th), sin(th) * ell + 0.0001));

        // Vibration inside a radial zone, phased per particle.
        let zone = 0.25 + 0.75 * clamp(f_vibration_area(), 0.0, 1.0);
        let vmask = 1.0 - smoothstep(zone - 0.12, zone + 0.12, r / max(f_orbit_radius(), 0.001));
        let vamp = f_vibration() * 0.011 * vmask;
        let phx = fract(fi * 0.381966) * TAU;
        let phy = fract(fi * 0.618034) * TAU;
        let om = 2.0 + f_frequency() * 0.45;
        let vx = vamp * (sin(om * t + phx) + 0.35 * sin(2.0 * om * t + phy) + 0.15 * sin(3.0 * om * t + phx * 1.3));
        let vy = vamp * (sin(om * t + phy) + 0.35 * sin(2.0 * om * t + phx) + 0.15 * sin(3.0 * om * t + phy * 1.3));

        let head = pos_h + dirv * wdisp + vec2<f32>(vx, vy);
        let tail = pos_t + dirv * wdisp * 0.8 + vec2<f32>(vx, vy) * 0.6;

        let d_head = length(p - head);
        let core_a = os_core(p - head, rad, shape) * (0.65 + 0.35 * dep);
        let d_seg = os_seg(p, head, tail);
        let ba = tail - head;
        let along = clamp(dot(p - head, ba) / max(dot(ba, ba), 0.000001), 0.0, 1.0);
        let fade = pow(max(1.0 - along, 0.0), 1.0 + f_trail_fade() * 4.0);
        let trail_a = (1.0 - smoothstep(rad * 0.25, rad * 0.65, d_seg)) * fade * trail_on;

        let gg = d_head / max(rad, 0.0001);
        let halo = exp(-gg * gg * 0.35) * f_glow() * 0.35;

        let hue = fract(h1 * 0.73 + f_hue_shift() + dep * 0.18 + sed * 0.01);
        let c0 = p_pal(0u, hue);
        let c1 = p_pal(1u, fract(hue + 0.35));
        let c2 = p_pal(2u, fract(h1 + dep * 0.40));
        let body = mix(c0, c1, clamp(f_color_mix(), 0.0, 1.0));
        let tw = 1.0 - f_twinkle() * 0.65 * (0.5 + 0.5 * sin(t * f_twinkle_speed() * (0.5 + h4 * 2.0) + h1 * TAU));
        let bri = (0.55 + 0.45 * dep) * tw;

        acc = acc + (body * trail_a * 0.85 + c2 * core_a * (0.5 + 1.5 * f_core()) + c2 * halo) * bri;
        cov = max(cov, clamp(core_a + trail_a * 0.50 + halo * 0.25, 0.0, 1.0));
    }

    // Sparse background dust, on its own lattice.
    let cell = floor(p * 14.0);
    let fp = fract(p * 14.0) - vec2<f32>(0.5);
    let dh1 = p_hash(cell + vec2<f32>(sed * 3.0, 1.0));
    let dh2 = p_hash(cell + vec2<f32>(7.0, sed * 2.0));
    let dotp = vec2<f32>(dh1 - 0.5, dh2 - 0.5) * 0.60;
    let dd = length(fp - dotp);
    let da = (1.0 - smoothstep(0.075, 0.180, dd)) * f_dust() * 0.55;
    let dcol = p_pal(2u, fract(dh1 + f_hue_shift()));
    acc = acc + dcol * da * 0.60;
    cov = max(cov, da * 0.50);

    // Rim fade keeps the frame edge transparent for `background_level`.
    let e = length(p / vec2<f32>(max(aspect * 0.5, 0.01), 0.5));
    let edge = 1.0 - smoothstep(0.86, 1.02, e);
    let vig = 1.0 - f_vignette() * 0.55 * smoothstep(0.25, 1.0, e);
    cov = clamp(cov * edge * vig, 0.0, 1.0);
    acc = min(acc * vig, vec3<f32>(4.0));

    return vec4<f32>(acc, cov);
}
