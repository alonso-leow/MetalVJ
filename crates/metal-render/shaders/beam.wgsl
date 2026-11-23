// Beam — vector-CRT oscilloscope trace. Ported from fosfora's
// assets/shaders/beam.wgsl, and the one port of the ten that is honestly
// incomplete rather than merely simplified — see the CAPABILITY GAP note
// below before wiring this in.
//
// CAPABILITY GAP: fosfora's beam sampled `waveform(x)`, a texture fosfora's
// own capture pass fills with the real per-column min/max of the audio
// signal — the thing that made it "draw the actual signal" rather than an
// audio-reactive pattern. MetalVJ's digest (`metal-listen`) hands models
// scalars, not a waveform texture; there is no equivalent capture path in
// this project today. `synth_wave` below is a procedural stand-in built from
// three of those scalars (`energia`, `transitorio`, `tono`) so the model
// renders something legible, but it is not the signal — it is a plausible
// forgery of one. A faithful port needs a real waveform-texture capture pass
// added to `metal-listen`/`metal-render` first; see IMPORT.md.
//
// What is new relative to fosfora, i.e. the "more personalisation" this port
// was asked for: `haz` (a repaintable colour gradient, replacing the fixed
// warm/cool `audioPalette` call), `amplitud` (the scope/radial excursion,
// one float instead of two separately-tuned constants), `symmetry` (radial
// mode repeats the trace in N wedges instead of drawing one ring — 1 is the
// original behaviour), and `gain_energy`/`gain_transient` (the
// gain fosfora hardcoded to 1.4/1.5, now dials an operator can reach).
//
// Also dropped: the phosphor-persistence feedback pass (`prev = feedback(uv)`
// in fosfora). This file draws one frame's fresh energy only — see
// IMPORT.md for why persistence needs its own pass rather than living here.

struct BeamUniform {
    beam: ColorSlot,
    // aspect, time, modo, enfoque_base
    a: vec4<f32>,
    // intensidad, amplitud, simetria, ganancia_energia
    b: vec4<f32>,
    // ganancia_transitorio, deriva_color, energia, transitorio
    c: vec4<f32>,
    // enfoque_audio, pulso, tono, decaimiento (reserved, unused here)
    d: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: BeamUniform;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318530;
const SAMPLES: i32 = 5;

fn sd_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn beam_glow(d: f32, sigma: f32) -> f32 {
    return exp(-(d * d) / (sigma * sigma));
}

fn beam_band(v: f32, lo: f32, hi: f32, sigma: f32) -> f32 {
    let d = max(max(lo - v, v - hi), 0.0);
    let w = hi - lo;
    let line_w = sigma / (w + sigma);
    return beam_glow(d, sigma) * (0.35 + 0.65 * line_w);
}

// The stand-in for fosfora's captured waveform — see the header. Returns
// (min, max) the way fosfora's `waveform()` did, so the two call sites below
// did not have to change shape.
fn synth_wave(x: f32, energy: f32, transient: f32, tone: f32, t: f32) -> vec2<f32> {
    let h1 = sin(x * 40.0 + t * 6.0);
    let h2 = sin(x * 13.0 - t * 3.1) * 0.5;
    let h3 = (noise2(vec2<f32>(x * 30.0, t * 4.0)) - 0.5) * 0.6;
    let mixed = tone * h2 + (1.0 - tone) * h1 * 0.6;
    let sig = (h1 * 0.5 + mixed + h3) * (0.25 + energy * 1.1) * (1.0 + transient * 1.4);
    let spread = 0.08 + energy * 0.1;
    return vec2<f32>(sig - spread, sig + spread);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let mode = u.a.z;
    let focus_base = u.a.w;
    let intensity = u.b.x;
    let amplitude = u.b.y;
    let symmetry = max(u.b.z, 1.0);
    let gain_energy = u.b.w;
    let gain_transient = u.c.x;
    let drift_color = u.c.y;
    let energia = u.c.z;
    let transient = u.c.w;
    let focus_audio = u.d.x;
    let pulse = u.d.y;
    let tone = u.d.z;

    let uv = in.uv;
    var p = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);

    let sigma = mix(0.004, 0.020, focus_base) * (1.0 + focus_audio * 3.0);
    let gain = intensity * (0.55 + energia * gain_energy) * (1.0 + transient * gain_transient);

    var energy = 0.0;

    if (mode < 0.5) {
        // ---------------- SCOPE ----------------
        let resolution_x = 640.0; // column density; a real port reads ctx.width.
        let du = 1.0 / resolution_x;
        let spacing = du * aspect;

        var top: array<vec2<f32>, 5>;
        var bot: array<vec2<f32>, 5>;
        for (var i = 0; i < SAMPLES; i = i + 1) {
            let sx = uv.x + f32(i - SAMPLES / 2) * du;
            let mm = synth_wave(clamp(sx, 0.0, 1.0), energia, transient, tone, t);
            let scr_x = (sx - 0.5) * aspect;
            top[i] = vec2<f32>(scr_x, -clamp(mm.y, -1.2, 1.2) * amplitude);
            bot[i] = vec2<f32>(scr_x, -clamp(mm.x, -1.2, 1.2) * amplitude);
        }

        for (var i = 0; i < SAMPLES - 1; i = i + 1) {
            let dt = sd_segment(p, top[i], top[i + 1]);
            let db = sd_segment(p, bot[i], bot[i + 1]);
            let lt = length(top[i + 1] - top[i]) + 1e-4;
            let lb = length(bot[i + 1] - bot[i]) + 1e-4;
            let dwell_t = 0.25 + 0.75 * spacing / lt;
            let dwell_b = 0.25 + 0.75 * spacing / lb;
            energy = energy + beam_glow(dt, sigma) * dwell_t;
            energy = energy + beam_glow(db, sigma) * dwell_b;
        }

        let cy_top = top[SAMPLES / 2].y;
        let cy_bot = bot[SAMPLES / 2].y;
        energy = energy + beam_band(p.y, cy_top, cy_bot, sigma * 1.6) * 0.5;
    } else {
        // ---------------- RADIAL ----------------
        // `symmetry` folds the ring into N wedges before sampling — 1 is
        // fosfora's original single-ring behaviour.
        let ang_raw = atan2(p.y, p.x);
        let wedge = TAU / symmetry;
        var ang = ang_raw - floor(ang_raw / wedge) * wedge;
        if (ang > wedge * 0.5) {
            ang = wedge - ang;
        }
        let tx = fract(ang / wedge);
        let rad = length(p);
        let r0 = 0.16 + energia * 0.18;
        let mm = synth_wave(tx, energia, transient, tone, t);
        let ramp = amplitude * 0.9;
        let r_in = r0 + clamp(mm.x, -1.2, 1.2) * ramp;
        let r_out = r0 + clamp(mm.y, -1.2, 1.2) * ramp;
        energy = energy + beam_band(rad, r_in, r_out, sigma);
        energy = energy + beam_glow(abs(rad - r_out), sigma) * 0.6;
    }

    energy = min(energy * (1.0 + pulse * 0.15), 2.5);

    let col_t = fract(0.5 + drift_color * sin(t * 0.15) + p.x * 0.05);
    let beam_col = grad4(u.beam, col_t);
    let col = beam_col * energy * gain;

    return vec4<f32>(min(col, vec3<f32>(1.6)), clamp(max(col.r, max(col.g, col.b)) * 2.0, 0.0, 1.0));
}
