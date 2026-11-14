// Model 16 — ASCII Waveform. A wave drawn in characters on a fixed grid of
// cells, one fullscreen fragment pass and no geometry.
//
// **This is the only model in the suite that draws a letter**, and the whole
// cost of it is that capability. The font is a 32-glyph 8x8 bitmap set baked
// into the binary and handed here as `array<vec4<u32>, 32>` — two u32 per
// glyph, four rows in each. No texture, no sampler, no atlas, no font file and
// no licence, and the 8x8 grid *is* the aesthetic: a smoothly antialiased
// vector glyph is not what "ASCII waveform" means. `docs/NEXT.md` §7 records
// why the font stack already in the lock file is the wrong answer anyway — it
// rasterises into egui's atlas and never hands a model a view it can sample.
//
// The ramp is picked on the CPU, so only the glyphs in use arrive here and the
// shader never needs to know which set it is drawing.

struct GlyphUniform {
    color: ColorSlot,
    background: ColorSlot,
    floats: vec4<f32>,  // cols, rows, ramp_len, thickness
    floats2: vec4<f32>, // wavelength, amplitude, speed, time
    floats3: vec4<f32>, // waves, aspect, unused, unused
    glyphs: array<vec4<u32>, 32>,
};

@group(0) @binding(0) var<uniform> u: GlyphUniform;

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

// Where the wave sits at `x`, in 0..1 down the frame. Several sine components
// at halving amplitude and rising frequency: one sine is a test pattern, and
// three is a waveform.
fn wave_at(x: f32) -> f32 {
    let wavelength = max(u.floats2.x, 0.02);
    let amplitude = u.floats2.y;
    let speed = u.floats2.z;
    let time = u.floats2.w;
    let waves = i32(clamp(u.floats3.x, 1.0, 4.0));

    var y = 0.0;
    var amp = 1.0;
    var freq = 1.0;
    var total = 0.0;
    for (var k = 0; k < waves; k = k + 1) {
        // Each component drifts at its own rate, so they beat against each
        // other instead of travelling as one rigid shape.
        let phase = (x / wavelength) * freq * 6.28318530718 - time * speed * (1.0 + 0.31 * f32(k));
        y = y + sin(phase) * amp;
        total = total + amp;
        amp = amp * 0.5;
        freq = freq * 2.13;
    }
    return 0.5 + amplitude * (y / max(total, 0.001));
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let cols = max(u.floats.x, 2.0);
    let rows = max(u.floats.y, 2.0);
    let ramp_len = max(u.floats.z, 1.0);
    let thickness = max(u.floats.w, 0.001);

    // Which cell, and where inside it.
    let cell = floor(in.uv * vec2<f32>(cols, rows));
    let inside = fract(in.uv * vec2<f32>(cols, rows));

    // The wave is sampled at the *cell's* centre, not at the pixel: every
    // pixel of a character has to agree about which character it is in, or the
    // glyph tears along the boundary where the density crosses a step.
    let x = (cell.x + 0.5) / cols;
    let y = (cell.y + 0.5) / rows;
    let here = wave_at(x);

    // Density falls off with distance from the wave, so the band is a few
    // characters thick and fades out rather than ending on a hard row.
    let d = abs(y - here) / thickness;
    let density = clamp(1.0 - d, 0.0, 1.0);

    // Index into the ramp. `ramp_len - 1` is the densest glyph and 0 is the
    // blank, which is why the blank is always the ramp's first entry.
    let index = u32(clamp(floor(density * ramp_len), 0.0, ramp_len - 1.0));
    let glyph = u.glyphs[index];

    // Unpack the 8x8: four rows per u32, most significant bit leftmost.
    let gx = u32(clamp(floor(inside.x * 8.0), 0.0, 7.0));
    let gy = u32(clamp(floor(inside.y * 8.0), 0.0, 7.0));
    var packed = glyph.x;
    var row_in_word = gy;
    if (gy >= 4u) {
        packed = glyph.y;
        row_in_word = gy - 4u;
    }
    let row_bits = (packed >> (row_in_word * 8u)) & 255u;
    let on = f32((row_bits >> (7u - gx)) & 1u);

    // Colour walks the slot across the frame, so a four-stop slot is a
    // gradient along the wave rather than four unrelated characters.
    let base = grad4(u.color, x);
    // The lit character carries the colour; the unlit cell keeps a trace of
    // it, which is what stops the frame being a black sheet with a thin line
    // of text across the middle.
    let glow = 0.06 * (1.0 - abs(y - here));
    // Over the background rather than over black. Added, not mixed: the
    // glyphs read as lit characters on a surface, and mixing would have a
    // bright background wash the text out instead of the text sitting on it.
    let bg = backdrop_from_uv(u.background, in.uv, max(u.floats3.y, 0.01));
    return vec4<f32>(bg + base * (on * (0.35 + 0.65 * density) + glow), 1.0);
}
