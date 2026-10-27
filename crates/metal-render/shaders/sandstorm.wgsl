// Model 23 — Sandstorm. Grains of sand falling from a source, on a 2D canvas.
//
// One fullscreen fragment pass, and no CPU side to it at all: the whole model
// is the inversion below. A pixel does not ask "which of the thousands of
// grains am I near" — it asks "which grain *would have to* have been emitted
// for it to be here now", and then checks the handful of neighbours of that
// answer. That is why the grain count is not a parameter and there is no
// buffer of particles: the field is indexed, not simulated.
//
// # The inversion
//
// A grain is emitted at time `e` from lane `k` of the source and then falls:
//
//     y(a) = origen_y - (caida*a + gravedad*a^2/2)
//     x(a) = origen_x + lane_off*(1 + abanico*a) + deriva*a + viento*a^2/2
//
// with `a = t - e` its age and `lane_off` its lane's offset from the middle of
// the mouth. `y` depends on the age alone, so it inverts — solve the quadratic
// for `a` and a pixel's height *is* an age. Knowing the age, the horizontal
// terms are a constant to subtract and a constant to divide by, so the pixel's
// own x gives the lane the grain was poured from. So a point of the frame maps
// to a point of `(lane, emission time)`, and grains live on a lattice in *that*
// space, evenly spaced, never moving. The frame is the picture of that lattice
// pulled through the fall.
//
// `abanico` — the spray widening as it falls — is why the horizontal half is a
// division and not just a subtraction, and why it is *linear* in the age. A fan
// that grew with any curve in it, or one that pushed each grain out by its own
// random amount, would be a perfectly good picture and would not invert: the
// lane would appear on both sides of the equation. Linear in `a` keeps it one
// divide, and a cone is what a mouth pouring sand actually opens into.
//
// Two things fall out of this that a particle buffer would have had to be
// argued into:
//
//   - It is a pure function of `(floats, time)`. Ask for the same instant
//     twice, or come back to it after automix parked the model for a minute,
//     and it is the same frame. Nothing accumulates because nothing is stored.
//   - Grains crowd near the source and stretch apart lower down without being
//     told to. Even spacing in emission time is uneven spacing in `y` exactly
//     when there is a `gravedad`, which is what falling sand looks like.
//
// # The one place it is approximate, and why it is not a lie
//
// A pixel checks 3 lanes x 5 emissions around its own. That is every grain
// that can reach it *while the vertical spacing between emissions is bigger
// than a grain*. Near the source, at high `densidad`, it is not: hundreds of
// grains overlap there and five of them are sampled. Rather than iterate into
// a region whose answer is known — a stream that dense is solid — the coverage
// is scaled by how many emissions fit in a grain (`overlap` below). It is an
// approximation of an opaque thing by an opaque thing.
//
// `turbulencia` is applied as a warp of the sample point rather than as a
// force on the grains, for the same reason: a force would have to be
// integrated per grain and would break the inversion above. Displacing the
// point we ask about, before asking, gives a field that swirls identically and
// stays invertible.

struct SandUniform {
    sand: ColorSlot,
    background: ColorSlot,
    // aspect, time, origen_x, origen_y
    a: vec4<f32>,
    // ancho, caida, gravedad, deriva
    b: vec4<f32>,
    // viento, turbulencia, remolino, carriles
    c: vec4<f32>,
    // cell_t (seconds between emissions in a lane), tamano, vida, estela
    d: vec4<f32>,
    // brillo, matiz, abanico, unused
    e: vec4<f32>,
    // flurry, depth_range, gust, gust_rate
    f: vec4<f32>,
    // streak, grit, shafts, fog
    g: vec4<f32>,
    // tint, visibility, pulse, pulse_rate
    h: vec4<f32>,
    // curtain, slant, unused, unused
    i: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: SandUniform;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let xy = p[vi];
    var out: VOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}

/// How old a grain must be to have fallen `depth` below the source.
///
/// `depth = v0*a + g*a^2/2`, solved for `a`. The `g -> 0` branch is not a
/// nicety: without gravity the quadratic degenerates and the closed form is
/// `0/0`, which is a frame of NaN rather than a frame of sand falling at a
/// constant speed.
fn age_at_depth(depth: f32, v0: f32, g: f32) -> f32 {
    if (g < 1e-3) {
        return depth / max(v0, 1e-3);
    }
    return (sqrt(max(v0 * v0 + 2.0 * g * depth, 0.0)) - v0) / g;
}


// ---------------------------------------------------------------------------
// The storm: the camera is inside it.
//
// The curtain below is one stream seen from outside, which is a shower. This
// is the other half: LAYERS of grains at stepped depths, each one a screen
// space lattice with one streak per cell (so, like the curtain, no buffer and
// a pure function of `(floats, time)`). A near layer has big sparse streaks
// that cross the frame fast; a far layer has fine dense grain that crawls.
// The cells are elongated along the fall and a streak sits wholly inside its
// own cell, so a pixel visits exactly one cell per layer.
// ---------------------------------------------------------------------------

const LAYERS: i32 = 9;

fn gust_vel(t: f32, rate: f32) -> f32 {
    return 0.6 * sin(t * rate * 1.9) + 0.4 * sin(t * rate * 3.3 + 2.0);
}

// The integral of `gust_vel`: how far the gust has pushed the air sideways.
fn gust_disp(t: f32, rate: f32) -> f32 {
    let r = max(rate, 1e-3);
    return -0.6 / (1.9 * r) * cos(t * r * 1.9) - 0.4 / (3.3 * r) * cos(t * r * 3.3 + 2.0);
}

fn storm_layer(p: vec2<f32>, q: vec2<f32>, l: i32, tilt: f32, haze: vec3<f32>, vis: f32, shaft: f32) -> vec4<f32> {
    let t = u.a.y;
    let flurry = clamp(u.f.x, 0.0, 1.0);
    let depth_range = max(u.f.y, 1.0);
    let gust = clamp(u.f.z, 0.0, 1.0);
    let grate = u.f.w;
    let streak = clamp(u.g.x, 0.0, 1.0);
    let grit = max(u.g.y, 0.05);
    let fog = clamp(u.g.w, 0.0, 1.0);
    let hue = clamp(u.e.y, 0.0, 1.0);

    let fl = f32(l);
    let z = (fl + 0.5) / f32(LAYERS);
    let sc = 1.0 / (1.0 + z * (depth_range - 1.0));

    // Near layers feel the swirl more than far ones.
    let pl = p + (q - p) * (1.3 - z);

    let n = sqrt(1.0 + tilt * tilt);
    let d = vec2<f32>(tilt, -1.0) / n;
    let perp = vec2<f32>(1.0, tilt) / n;
    let speed = (max(u.b.y, 0.0) + 0.6 * max(u.b.z, 0.0) + 0.3) * 1.3 * sc;
    let sway = gust * gust_disp(t, grate) * 0.55 * sc;

    let uu = dot(pl, perp) + sway + fl * 3.17;
    let ww = dot(pl, d) - speed * t + fl * 7.3;

    let cw = 0.5 * pow(sc, 0.9);
    let ch = cw * 3.0;
    let cu = floor(uu / cw);
    let cv = floor(ww / ch);
    let seed = vec2<f32>(cu + fl * 17.3, cv + fl * 5.1);
    let h1 = lattice(seed);
    if (h1 > min(flurry * 1.25, 1.0)) {
        return vec4<f32>(0.0);
    }
    let h3 = lattice(seed + vec2<f32>(41.0, 7.0));
    let h4 = lattice(seed + vec2<f32>(-19.0, 53.0));
    let h5 = lattice(seed + vec2<f32>(83.0, -29.0));
    let h6 = lattice(seed + vec2<f32>(-61.0, 13.0));

    let fu = (cu + 0.3 + 0.4 * h3) * cw;
    let fw = (cv + 0.5 + (h4 - 0.5) * 0.3) * ch;
    let len = min((0.03 + streak * 0.55) * pow(sc, 0.85) * (0.6 + 0.8 * h5) * 2.0, ch * 0.8);
    let half_len = len * 0.5;
    let rad = max(u.d.y, 0.0005) * grit * (0.55 + 0.9 * h6) * pow(sc, 0.8) * 3.0;

    let du = uu - fu;
    let dw = ww - fw;
    let dd = length(vec2<f32>(du, dw - clamp(dw, -half_len, half_len)));
    let cover = 1.0 - smoothstep(rad * 0.4, rad, dd);
    // The head leads and the tail fades; the fall is along +w.
    let tail = 0.35 + 0.65 * smoothstep(-half_len, half_len, dw);

    let fogz = 1.0 - exp(-fog * z * 2.8);
    var tone = grad4(u.sand, mix(h6, z, hue));
    tone = tone * (1.0 + shaft * 1.4) * (1.0 + u.e.x * 0.35);
    tone = mix(tone, haze, fogz * 0.8);
    let a = cover * tail * vis * (1.0 - fogz * 0.7) * (0.6 + 0.4 * (1.0 - z));
    return vec4<f32>(tone, clamp(a, 0.0, 1.0));
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let origen = vec2<f32>(u.a.z, u.a.w);
    let width = max(u.b.x, 0.02);
    let fall = max(u.b.y, 0.0);
    let grav = max(u.b.z, 0.0);
    let drift = u.b.w;
    let wind = u.c.x;
    let turb = clamp(u.c.y, 0.0, 1.0);
    let swirl = max(u.c.z, 0.05);
    let lanes = max(u.c.w, 1.0);
    let cell_t = max(u.d.x, 1e-4);
    let size = max(u.d.y, 0.0005);
    let life = max(u.d.z, 0.05);
    let wake = clamp(u.d.w, 0.0, 1.0);
    let shine = clamp(u.e.x, 0.0, 1.0);
    let hue = clamp(u.e.y, 0.0, 1.0);
    let fan = max(u.e.z, 0.0);

    // y up, x widened by the aspect, so a grain is round on a 16:9 wall.
    let p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    var col = backdrop_from_uv(u.background, in.uv, aspect);

    // The warp. Sideways more than up and down, because a gust moves sand
    // across the frame; a vertical gust reads as the fall rate changing, which
    // is what `caida` is for.
    var q = p;
    if (turb > 0.001) {
        let s = swirl;
        let n1 = lattice_fbm(vec2<f32>(q.x * s + 11.3, q.y * s - t * 0.31), 3) - 0.5;
        let n2 = lattice_fbm(vec2<f32>(q.x * s - 5.7, q.y * s + t * 0.23), 3) - 0.5;
        q = q + vec2<f32>(n1 * turb * 0.7, n2 * turb * 0.28);
    }

    // ---- the storm -------------------------------------------------------
    let gust = clamp(u.f.z, 0.0, 1.0);
    let wind_now = (u.c.x + u.b.w + gust * 1.6 * gust_vel(t, u.f.w)) * u.i.y;
    let tilt = clamp(wind_now / (max(u.b.y, 0.0) + 0.6 * max(u.b.z, 0.0) + 0.3) , -1.2, 1.2);
    let pulse_v = 0.5 + 0.5 * sin(t * 6.2831853 * max(u.h.w, 0.0));
    let vis = clamp(u.h.y * (1.0 - u.h.z * 0.6 * pulse_v) , 0.0, 1.0);
    let fog = clamp(u.g.w, 0.0, 1.0);
    let fog_k = clamp(fog * (1.0 + (1.0 - vis) * 1.2), 0.0, 1.0);
    let tint = clamp(u.h.x, 0.0, 1.0);
    let hay0 = grad4(u.sand, 0.4) * 0.75;
    let haze = mix(col, hay0, tint * 0.8 + 0.1);

    // Dust volume: drifting fbm over the whole frame, denser low down.
    let dd2 = vec2<f32>(q.x * 0.9 - t * 0.12 * (1.0 + wind_now), q.y * 1.3 + t * 0.05);
    let dust = lattice_fbm(dd2 * 1.4 + vec2<f32>(3.1, 9.7), 4);
    let low = 0.65 + 0.35 * clamp(0.5 - p.y * 0.5, 0.0, 1.0);
    col = mix(col, haze, clamp(fog_k * (0.18 + 0.5 * dust) * low, 0.0, 0.85));

    // Light shafts: slow stripes slanting down from the upper left.
    let sdir = normalize(vec2<f32>(0.55, 1.0));
    let sper = vec2<f32>(sdir.y, -sdir.x);
    let sm = smoothstep(0.42, 0.78, lattice_fbm(vec2<f32>(dot(p, sper) * 2.1 + t * 0.06, t * 0.05 + dot(p, sdir) * 0.25), 3));
    let shaft_f = clamp(u.g.z, 0.0, 1.0) * sm * (0.45 + 0.55 * clamp(p.y * 0.5 + 0.6, 0.0, 1.0));
    let shaft_lit = mix(hay0, vec3<f32>(1.0, 0.95, 0.8), 0.45) * 1.4;
    col = col + shaft_lit * shaft_f * (0.28 + 0.2 * (1.0 - vis));

    if (u.f.x > 0.001) {
        for (var l = LAYERS - 1; l >= 0; l = l - 1) {
            let r = storm_layer(p, q, l, tilt, haze, vis, shaft_f);
            col = mix(col, r.rgb, r.a);
        }
    }

    let curtain = clamp(u.i.x, 0.0, 1.0);
    if (curtain < 0.005) {
        return vec4<f32>(col, 1.0);
    }

    let depth = origen.y - q.y;
    if (depth <= 0.0) {
        return vec4<f32>(col, 1.0);
    }

    let age = age_at_depth(depth, fall, grav);
    if (age > life) {
        return vec4<f32>(col, 1.0);
    }

    // The lane this pixel's grain was poured from, in lane units. Lanes span
    // the source: `ancho` is the width of the mouth, and `carriles` is how
    // many grains it is wide, so widening the source pours more sand rather
    // than spreading the same sand thinner.
    let cell_w = width / lanes;
    let lane_off = (q.x - origen.x - (drift * age + 0.5 * wind * age * age))
        / (1.0 + fan * age);
    let uu = lane_off / cell_w + lanes * 0.5;
    let vv = (t - age) / cell_t;

    // How much of a grain's diameter one emission step covers here. Below 1
    // the stream is denser than we sample and the missing neighbours are
    // accounted for rather than visited — see the header.
    let dy = (fall + grav * age) * cell_t;
    let overlap = clamp(2.0 * size / max(dy, 1e-5), 1.0, 7.0);
    // The fan thins the curtain as it opens — the same grains over more width.
    let thinning = 1.0 / (1.0 + fan * age);

    var acc = vec3<f32>(0.0);
    var alpha = 0.0;
    var glow = vec3<f32>(0.0);

    let ui = floor(uu);
    let vi = floor(vv);
    for (var i = -1; i <= 1; i = i + 1) {
        let ci = ui + f32(i);
        if (ci < 0.0 || ci >= lanes) { continue; }
        for (var j = -2; j <= 2; j = j + 1) {
            let cj = vi + f32(j);

            let jitter_lane = lattice(vec2<f32>(ci, cj));
            let jitter_emit = lattice(vec2<f32>(ci + 37.0, cj - 91.0));
            // The grain's own number: its size, and where it sits in the
            // gradient. One hash so a big grain is reliably the same colour as
            // itself from frame to frame.
            let grain = lattice(vec2<f32>(ci - 13.0, cj + 57.0));

            let emit = (cj + jitter_emit) * cell_t;
            let ga = t - emit;
            if (ga < 0.0 || ga > life) { continue; }

            let off = (ci + jitter_lane - lanes * 0.5) * cell_w;
            let gx = origen.x + off * (1.0 + fan * ga)
                + drift * ga + 0.5 * wind * ga * ga;
            let gy = origen.y - (fall * ga + 0.5 * grav * ga * ga);
            let dvec = q - vec2<f32>(gx, gy);

            // Stretched along its own velocity, which is what turns a dot into
            // a fleck of falling sand at any shutter speed.
            let speed = vec2<f32>(drift + wind * ga + off * fan, -(fall + grav * ga));
            let dir = normalize(speed + vec2<f32>(0.0, -1e-4));
            let along = dot(dvec, dir);
            let across = length(dvec - dir * along);
            let stretch = 1.0 + wake * 7.0;
            let dd = length(vec2<f32>(along / stretch, across));

            let rad = size * (0.55 + 0.9 * grain);
            // Born and gone, both softened, so no grain ever appears or
            // vanishes as a hard dot.
            let born = smoothstep(0.0, min(0.08, life * 0.15), ga);
            let gone = 1.0 - smoothstep(life * 0.75, life, ga);
            let life = born * gone;

            let cover = (1.0 - smoothstep(rad * 0.55, rad, dd)) * life;
            let tone = grad4(u.sand, mix(grain, clamp(ga / life, 0.0, 1.0), hue));

            acc = acc + tone * cover;
            alpha = alpha + cover;
            if (shine > 0.0) {
                glow = glow + tone * exp(-max(dd - rad, 0.0) / max(rad, 1e-4) * 2.2) * life;
            }
        }
    }

    if (alpha > 1e-5) {
        let sand = acc / alpha;
        col = mix(col, sand, clamp(alpha * overlap * thinning, 0.0, 1.0) * curtain);
    }
    col = col + glow * shine * 0.14;
    return vec4<f32>(col, 1.0);
}
