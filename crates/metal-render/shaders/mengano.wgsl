// Model 19 — Mengano. One fullscreen fragment pass: a travelling background,
// a squashed ball on top of it, and two eyes on the ball.
//
// The eyes are Oracle's, deliberately. `saccade_target` and the ease below are
// the same lines `oracle.wgsl` uses — snap to a new hashed target on a tick,
// then hold — because the thing that makes an eye read as alive is that it
// moves in jumps, and that was already solved once here. What is not Oracle's
// is the geometry: Oracle's iris and lids are angular zones on a sphere, and
// Mengano's are flat discs on a face, which is the difference between an eye
// that is being looked at and an eye that belongs to somebody.
//
// The lid is drawn in the *body's* colour, which is the one trick worth
// copying outright: a blink then closes an eye in a face instead of opening a
// black hole in one.

struct MenganoUniform {
    body: ColorSlot,
    eye: ColorSlot,
    background: ColorSlot,
    // aspect, time, how far between the two backgrounds, background scale
    misc: vec4<f32>,
    // x, y, radius, arrival pulse 0..1
    ball: vec4<f32>,
    // stretch axis xy, stretch along it, squash across it
    squish: vec4<f32>,
    // gaze bias xy, how open the lids are 0..1, eye size
    eyes: vec4<f32>,
    // where the world has travelled to, and the two scenes' seeds
    world: vec4<f32>,
    // saccade rate, unused, unused, unused
    misc2: vec4<f32>,
    // The `body` part's picture and `mat_*`, one `MaterialU` — see
    // material.wgsl, prepended ahead of this file.
    // x/y scale of the dance squash (world axes), tilt of the face (rad), jelly ripple
    dance: vec4<f32>,
    // eye lag xy (world), unused, light angle (rad, world space)
    dance2: vec4<f32>,
    // aura, halo, rim boost, emissive
    glow1: vec4<f32>,
    // rays, glow hue, glow saturation, beat pulse factor
    glow2: vec4<f32>,
    // background brightness, tint amount, tint hue, vertical gradient
    bg1: vec4<f32>,
    // vignette, beat pulse factor, unused, unused
    bg2: vec4<f32>,
    mat_body: MaterialU,
};

fn hue_rgb(h: f32, s: f32) -> vec3<f32> {
    let k = abs(fract(vec3<f32>(h) + vec3<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0);
    return mix(vec3<f32>(1.0), clamp(k - 1.0, vec3<f32>(0.0), vec3<f32>(1.0)), s);
}

fn rot2(v: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

@group(0) @binding(0) var<uniform> u: MenganoUniform;
@group(0) @binding(1) var surf_body_tex: texture_2d<f32>;
@group(0) @binding(2) var surf_body_samp: sampler;

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

// One background: soft blobs crossed with a band, at a hashed angle and phase.
// Returns a 0..1 place along the `fondo` slot rather than a colour, so the two
// scenes can be crossfaded before the palette is applied and a travelling shot
// never passes through a colour neither background has.
// The background needed its own noise, and the reason is worth writing down
// because it is invisible until it is not.
//
// `shared.wgsl`'s `noise2` interpolates with the cubic smoothstep, which is C1:
// the value and its slope are continuous across a cell boundary, but the
// curvature jumps. That is invisible in a field that is already busy and a
// visible crease in this one — a large, smooth, low-contrast field walked
// through a steep four-stop ramp. The frame came out looking like nested
// squares, from a hash that plateaued and an interpolation that could not hide
// it.
//
// **Both halves of the fix now live in `shared.wgsl`** — `lattice` and
// `quintic_noise`, with the whole argument, because Solvent hit the same
// plateau from the other direction and two copies of the cure is exactly what
// that file exists to prevent. What stayed here is the third change, which is
// Mengano's alone: rotating every octave, including the first, so that
// whatever crease is left is not aligned with the frame.
fn warped_fbm(p: vec2<f32>) -> f32 {
    var total = 0.0;
    var amp = 0.5;
    // Turned off the axes before the first sample, not after it.
    var q = vec2<f32>(p.x * 0.80 + p.y * 0.60, p.y * 0.80 - p.x * 0.60);
    for (var o = 0; o < 5; o = o + 1) {
        total = total + quintic_noise(q) * amp;
        // 2.03 rather than 2, so the octaves' grids never share a line.
        q = vec2<f32>(q.x * 0.80 + q.y * 0.60, q.y * 0.80 - q.x * 0.60) * 2.03;
        amp = amp * 0.5;
    }
    return total;
}

fn scene(p: vec2<f32>, seed: f32, scale: f32) -> f32 {
    let a = hash11(seed * 3.7) * 6.2831853;
    let dir = vec2<f32>(cos(a), sin(a));
    let q = p * scale + vec2<f32>(seed * 11.3, seed * 7.1);
    let blobs = warped_fbm(q);
    let bands = 0.5 + 0.5 * sin(dot(p, dir) * scale * 1.7 + seed * 5.0);
    return clamp(mix(blobs, bands, 0.35 + 0.4 * hash11(seed * 9.1)), 0.0, 1.0);
}

fn hash2(id: f32) -> vec2<f32> {
    return vec2<f32>(hash11(id * 17.3), hash11(id * 91.7 + 4.0));
}

// Oracle's, unchanged.
fn saccade_target(tick: f32, bias: vec2<f32>) -> vec2<f32> {
    return bias + (hash2(tick) * 2.0 - 1.0) * 0.6;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let aspect = max(u.misc.x, 0.01);
    let time = u.misc.y;
    let blend = clamp(u.misc.z, 0.0, 1.0);
    let scale = max(u.misc.w, 0.05);

    let p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);

    // ---- the background, travelling -------------------------------------
    // Two scenes at once and a crossfade between them, so "a new background
    // with new parameters" arrives while the old one is still leaving. The
    // world offset is what makes it a travelling shot rather than a dissolve.
    let w = p + u.world.xy;
    let field = mix(scene(w, u.world.z, scale), scene(w, u.world.w, scale), blend);
    var col = grad4(u.background, field);

    // ---- the background's own controls (identity at the defaults) -------
    col = col * u.bg1.x * (1.0 + u.bg2.y);
    col = mix(col, col * hue_rgb(u.bg1.z, 1.0) * 1.4, clamp(u.bg1.y, 0.0, 1.0));
    col = col * clamp(1.0 + u.bg1.w * p.y * 0.6, 0.0, 3.0);
    col = col * (1.0 - clamp(u.bg2.x, 0.0, 1.0) * smoothstep(0.35, 1.7, length(p / vec2<f32>(aspect, 1.0))));

    // ---- the ball --------------------------------------------------------
    let radius = max(u.ball.z, 1e-3);
    let ax = normalize(u.squish.xy + vec2<f32>(1e-6, 0.0));
    let ay = vec2<f32>(-ax.y, ax.x);
    let rel_w = p - u.ball.xy;
    // The dance squash is in world axes, before the travel squash.
    let rel = rel_w / vec2<f32>(max(u.dance.x, 1e-3), max(u.dance.y, 1e-3));
    // Local frame: along the direction it is travelling, and across it. A ball
    // that is moving is longer that way and thinner this way, which is the
    // whole of "squishy" and costs two divisions.
    let local = vec2<f32>(dot(rel, ax) / max(u.squish.z, 1e-3), dot(rel, ay) / max(u.squish.w, 1e-3));
    let rr = length(local);
    // Jelly: a slow ripple round the rim, only while the body is being shaken.
    let ripple = u.dance.w * radius * 0.05 * sin(atan2(local.y, local.x) * 3.0 + time * 9.0);
    let d = rr - radius - ripple;
    let aa = max(fwidth(d), 1e-5);
    let inside = 1.0 - smoothstep(-aa, aa, d);

    // Lit from above and to the left, so it reads as a body rather than a
    // disc. The four stops run from the lit side to the shaded one.
    let nrm = local / max(rr, 1e-4);
    // The light lives in WORLD space. `nrm` is in the body's stretch frame,
    // which turns with the direction of travel, so it is carried back to
    // world axes before it meets the light; the face and the material turn
    // with the body (`lr`), the lighting does not.
    let nrm_w = nrm.x * ax + nrm.y * ay;
    let light = vec2<f32>(cos(u.dance2.w), sin(u.dance2.w));
    let lit = clamp(0.5 - 0.52 * dot(nrm_w, light) + 0.35 * rr / radius, 0.0, 1.0);
    let lr = rot2(local, -u.dance.z);
    let gcol = hue_rgb(u.glow2.y, u.glow2.z);
    let gp = u.glow2.w;
    let body = grad4(u.body, lit);

    // A soft contact shadow under it, so it sits on the background.
    let shade = exp(-length((p - u.ball.xy - vec2<f32>(0.0, -radius * 0.85)) * vec2<f32>(1.0, 2.4)) * 3.2 / radius);
    col = mix(col, col * 0.45, clamp(shade, 0.0, 1.0) * 0.55);

    // ---- radiated light: aura, halo, rays (additive, behind the body) ----
    {
        let dist = length(rel_w) / radius;
        let out_d = max(dist - 1.0, 0.0);
        var g = u.glow1.x * exp(-out_d * 1.5) * 0.7;
        g = g + u.glow1.y * exp(-pow((dist - 1.7) * 5.0, 2.0)) * 1.1;
        let ang = atan2(rel_w.y, rel_w.x);
        let streak = pow(max(0.0, sin(ang * 9.0 + time * 0.5 + 2.0 * sin(ang * 3.0 - time * 0.3))), 3.0);
        g = g + u.glow2.x * streak * exp(-out_d * 0.55) * 0.8;
        col = col + gcol * g * gp;
    }

    if (inside > 0.001) {
        // The shared material, off at mix 0 (a migrated row): the ball's body
        // colour is what a kind or a `tex.body` picture replaces. The disc's
        // radial normal is lifted into 3D so bump and glitch have a surface.
        let shaded = material_apply(
            u.mat_body, surf_body_tex, surf_body_samp, body,
            vec3<f32>(lr, 0.0), normalize(vec3<f32>(nrm, 0.5)), in.uv,
            vec3<f32>(0.0, 0.0, 1.0), time,
        );
        let body_m = shaded.albedo;
        var skin = body_m;

        // ---- the eyes ----------------------------------------------------
        // Placed in the ball's own local frame, so they squash with it.
        let eye_r = radius * clamp(u.eyes.w, 0.05, 1.0) * 0.44;
        let sep = radius * 0.42;
        let lift = radius * 0.17;

        // Oracle's saccade, plus wherever the body is heading. `mirada`
        // decided the mix on the CPU and arrives as the bias.
        let speed = max(u.misc2.x, 0.02);
        let tick = floor(time * speed);
        let frac = fract(time * speed);
        let bias = u.eyes.xy;
        let ease = 1.0 - exp(-frac * 16.0);
        let gaze = mix(saccade_target(tick - 1.0, bias), saccade_target(tick, bias), ease) * 0.5;

        let open = clamp(u.eyes.z, 0.0, 1.0);
        for (var e = 0; e < 2; e = e + 1) {
            let lag_l = rot2(vec2<f32>(dot(u.dance2.xy, ax), dot(u.dance2.xy, ay)), -u.dance.z);
            let centre = vec2<f32>(select(-sep, sep, e == 1), lift) + lag_l * radius;
            let q = lr - centre;
            let ed = length(q) - eye_r;
            let eaa = max(fwidth(ed), 1e-5);
            let in_eye = 1.0 - smoothstep(-eaa, eaa, ed);
            if (in_eye <= 0.001) { continue; }

            // The lid. Same colour as the skin around it — Oracle's rule —
            // so closing one closes an eye rather than punching a hole.
            let lid = eye_r * (open * 2.0 - 1.0);
            if (q.y > lid) {
                skin = mix(skin, body_m * 0.88, in_eye);
                continue;
            }

            var eye = grad4(u.eye, 0.06);
            let pupil_r = eye_r * 0.56;
            let pupil_at = gaze * eye_r * 0.34;
            let pd = length(q - pupil_at) - pupil_r;
            eye = mix(eye, grad4(u.eye, 0.94), 1.0 - smoothstep(-eaa, eaa, pd));
            // The catchlight, which is the entire difference between an eye
            // and a hole.
            let hl = length(q - pupil_at - vec2<f32>(-pupil_r * 0.36, pupil_r * 0.40)) - pupil_r * 0.30;
            eye = mix(eye, grad4(u.eye, 0.0) + vec3<f32>(0.25), 1.0 - smoothstep(-eaa, eaa, hl));

            skin = mix(skin, eye, in_eye);
        }

        // A rim light, brightest on the arrival pulse, so a landing reads.
        let rim = smoothstep(radius * 0.86, radius, rr) * (0.10 + 0.5 * clamp(u.ball.w, 0.0, 1.0));
        skin = skin + grad4(u.body, 0.0) * rim;
        // Emissive body and a stronger rim, both in the glow colour.
        let rim_w = smoothstep(radius * 0.7, radius, rr);
        skin = skin + gcol * (u.glow1.w * 0.6 + u.glow1.z * rim_w * 1.2) * gp;

        col = mix(col, skin, inside);
    }

    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
