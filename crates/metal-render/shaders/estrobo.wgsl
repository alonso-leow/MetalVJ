// Model 24 — Strobe (`estrobo`). The frame is the light.
//
// There is no subject here and that is the design. Every other model in the
// suite draws a thing and lights it; this one skips the thing. What the room
// sees is not a picture on a wall, it is the wall going off, which is the only
// effect in the set that reaches people who are not looking at the screen.
//
// The whole model is one number: `phase`, where the clock is inside the current
// flash. Everything else shapes it — how long it stays up (`duty`), how it
// leaves (`caida`), how hard the edge is (`borde`), and whether the frame fires
// at once or in a chase (`bandas`, `barrido`).
//
// # It is a square wave, and the two places it must not be
//
// `caida` exists because a square flash reads as a light *switch* and a decayed
// one reads as a light *hit*. Same duty, entirely different event.
//
// `borde` exists because a hard edge in a fragment shader is a hard edge in
// time as well as space: at 60 fps a flash shorter than a frame either lands on
// a frame or does not, and the strobe skips beats at random. Softening the
// window widens the phase range that shows *something*, so more frames catch
// part of a short flash instead of missing it whole.
//
// It does not cure the problem, and it is worth being exact about that rather
// than leaving the paragraph above sounding like it does. A flash genuinely
// shorter than a frame cannot be drawn correctly without temporal
// supersampling, which this suite does not have. What handles the rest is the
// floor on `duty` in `models::estrobo` — the range simply stops before the
// picture stops being the one that was asked for.

struct EstroboUniform {
    light: ColorSlot,
    background: ColorSlot,
    // aspect, time, periodo, duty
    a: vec4<f32>,
    // borde, bandas, barrido, alterna
    b: vec4<f32>,
    // caida, resplandor, vineta, unused
    c: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: EstroboUniform;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let period = max(u.a.z, 0.01);
    // The same floor `models::estrobo` declares, restated here because a
    // uniform can arrive from anywhere and the two must not disagree about
    // where the shortest legal flash is.
    let duty = clamp(u.a.w, 0.02, 1.0);
    let edge = clamp(u.b.x, 0.0, 1.0);
    let bands = max(floor(u.b.y + 0.5), 1.0);
    let sweep = clamp(u.b.z, 0.0, 1.0);
    let alternate = clamp(u.b.w, 0.0, 1.0);
    let fall = clamp(u.c.x, 0.0, 1.0);
    let glare = clamp(u.c.y, 0.0, 1.0);
    let vignette = clamp(u.c.z, 0.0, 1.0);

    let p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    var col = backdrop_from_uv(u.background, in.uv, aspect);

    // Which band of the frame this pixel is in, top to bottom. One band is the
    // whole frame, which is the ordinary strobe.
    let band = floor(clamp(in.uv.y, 0.0, 0.9999) * bands);
    // A chase: each band lags the one above it by a share of the period. At
    // `barrido` 0 they all fire together, so `bandas` costs nothing until it
    // is asked to.
    let lag = band / bands * sweep * period;

    // The flash this pixel is in, and how far through it. `floor` and `fract`
    // of the same quotient, so the index and the phase can never disagree
    // about which flash is on.
    let k = (t - lag) / period;
    let flash = floor(k);
    let phase = k - flash;

    // The window. Soft on both sides — see the header on why the leading edge
    // in particular cannot be hard.
    let soft = max(edge * duty * 0.5, 1e-4);
    let up = smoothstep(0.0, soft, phase);
    let down = 1.0 - smoothstep(duty - soft, duty, phase);
    var level = up * down;

    // Inside the window, either a plateau or something that falls away from
    // the strike. `caida` blends the two rather than switching, because a
    // half-decayed flash is a legal and useful thing to ask for.
    let decay = exp(-phase / duty * 3.2);
    level = level * mix(1.0, decay, fall);

    // Which colour this flash is: **one of the four stops, never between
    // them.** This is the one place in the suite that does not walk its
    // gradient continuously, and the reason is specific to a strobe. The four
    // stops are four lamps; halfway between an ice blue and an amber is a
    // muddy olive that nobody chose and that no lamp in a room produces.
    // Every other model wants the in-betweens — that is what a gradient across
    // a shape is for — and this one wants the four colours it was given.
    //
    // So the hash snaps to a stop, and `alterna` decides how often a flash
    // deviates from the first one rather than how far it deviates. At 0 every
    // flash is stop 0; at 1 every flash is a fresh draw. Consecutive flashes
    // are unrelated rather than cycling in order, because a strobe you can
    // anticipate is not doing its job.
    let stop = floor(hash11(flash * 12.9898 + 3.7) * 4.0) / 3.0;
    let deviates = step(hash11(flash * 78.233 - 11.1), alternate);
    let tint = grad4(u.light, stop * deviates);

    // The room is never quite black between flashes unless it is asked to be.
    let lit = level + glare * 0.12;

    // Corners darker than the middle, so a full-frame flash reads as a source
    // in the room rather than as the monitor turning white.
    let r = length(p) / max(aspect, 1.0);
    let vig = mix(1.0, clamp(1.0 - r * 0.55, 0.0, 1.0), vignette);

    col = col + tint * lit * vig;
    return vec4<f32>(col, 1.0);
}
