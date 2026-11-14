// Aurora — horizontal flowing curtain bands, one per frequency band. A
// spectrogram disguised as northern lights.
//
// Ported from fosfora's assets/shaders/aurora.wgsl. That version read seven
// raw audio globals (u.sub_bass..u.brilliance, u.rms, u.onset, u.beat_phase)
// off a uniform every model in fosfora shared. MetalVJ has no such
// uniform — a model only knows what a live signal did to it through its own
// FloatSpec row (see metal-render/src/model.rs's doc comment: "a model never
// knows whether the row came from a slider or the digest"). So the seven
// bands, the RMS term and the onset term are now this model's own floats
// (`banda_0`..`banda_6`, `energia`, `impact`), driven from the digest the
// same way `metal-dash` drives anything else — see IMPORT.md.
//
// Two things fosfora did not have: `paleta`, a 4-stop gradient replacing the
// seven hardcoded band colours (grad4 at fi/6.0 walks warm to cool exactly
// like the original array did, but it is now a colour slot a person can
// repaint), and `emphasis_band`/`emphasis_force`, which brighten and widen
// whichever single band is dialled in rather than treating all seven equally.
//
// The beat-locked pulse fosfora multiplied over the whole frame
// (`1 + sin(beat_phase*TAU)*0.1*beat_strength`) is dropped here: a phase that
// must advance deterministically between digest rows is a clock, the way
// Strobe's `periodo` is a clock, and computing one honestly is a second
// model's worth of work this port does not need to also do. `impact` (the
// old `onset`) still gives every curtain a transient brighten on a hit.

struct AuroraUniform {
    palette: ColorSlot,
    // aspect, time, velocidad, separacion
    a: vec4<f32>,
    // ancho_brillo, energia, impacto, enfasis_banda
    b: vec4<f32>,
    // enfasis_fuerza, banda_0, banda_1, banda_2
    c: vec4<f32>,
    // banda_3, banda_4, banda_5, banda_6
    d: vec4<f32>,
    // haze, sharpness, _, _
    e: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: AuroraUniform;

const NUM_BANDAS: i32 = 7;

fn banda_val(i: i32, c: vec4<f32>, d: vec4<f32>) -> f32 {
    // c.yzw holds bandas 0..2, d holds bandas 3..6 — one branch, not seven.
    if (i == 0) { return c.y; }
    if (i == 1) { return c.z; }
    if (i == 2) { return c.w; }
    if (i == 3) { return d.x; }
    if (i == 4) { return d.y; }
    if (i == 5) { return d.z; }
    return d.w;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let speed = u.a.z;
    let separation = u.a.w;
    let width_shine = u.b.x;
    let energy = u.b.y;
    let impact = u.b.z;
    let emphasis_band = u.b.w;
    let emphasis_force = u.c.x;
    let haze = u.e.x;
    let sharpness = u.e.y;

    let uv = in.uv;
    let p = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);

    var col = vec3<f32>(0.0);

    for (var i = 0; i < NUM_BANDAS; i = i + 1) {
        let fi = f32(i);
        let banda_amp = banda_val(i, u.c, u.d);

        let banda_y = (fi - 3.0) / 7.0 * separation;

        let wave_freq = 2.0 + fi * 0.5;
        let wave_phase = t * speed * (0.8 + fi * 0.1);
        let wave = sin(p.x * wave_freq + wave_phase) * 0.05
                 + sin(p.x * wave_freq * 2.3 + wave_phase * 0.7) * 0.03;

        let noise_p = vec2<f32>(p.x * 3.0 + wave_phase * 0.5, fi * 2.0 + t * 0.1);
        // `quintic_noise`, not `noise2`: `noise_p` walks with song time, and
        // `noise2`'s sin hash goes flat in runs of cells once it is out there
        // (vertical tears in the curtains), while its cubic ease creased the
        // ripple each time `t * 0.1` crossed a cell (a jump every 10 s).
        let ripple = (quintic_noise(noise_p) - 0.5) * 0.08;

        let dy = p.y - banda_y - wave - ripple;

        // The emphasised band gets both a brighter curtain and a tighter
        // glow — a soft Gaussian peak in index space centred on
        // `emphasis_band`, so dialling between two integers crossfades which
        // one stands out rather than snapping.
        let dist_i = fi - emphasis_band;
        let highlight = 1.0 + emphasis_force * exp(-dist_i * dist_i * 0.9);

        let width = width_shine * (1.0 + banda_amp * 1.5) / highlight;
        // `sharpness` raises the Gaussian's exponent: 0 is the soft original
        // bell, 1 a flatter-topped ribbon with crisper edges.
        let glow = exp(-pow(dy * dy / (width * width), mix(1.0, 1.6, sharpness)));

        let brightness = banda_amp * (1.5 + energy * 0.5) * highlight;

        let banda_col = grad4(u.palette, fi / f32(NUM_BANDAS - 1));
        col += banda_col * brightness * glow;
    }

    // Shimmer rides the curtains instead of washing the whole frame. It used
    // `noise2` at `y * 30`, whose hash goes flat in runs of cells: the dark
    // background came out as visible blocks. `quintic_noise` is smooth, and
    // the term is scaled by the light already there so black stays black.
    let lit = clamp(dot(col, vec3<f32>(0.3333)), 0.0, 1.0);
    let shimmer = (quintic_noise(vec2<f32>(p.x * 5.0 + t * 0.7, p.y * 9.0 - t * 0.3)) - 0.5) * 0.12 * energy * lit;
    col += vec3<f32>(shimmer);

    // Haze: a smooth, vertical night-sky gradient from the palette's cool end
    // at the zenith to its warm end at the horizon. Without it the background
    // is pure zero, which any 8-bit chain turns into a flat black with banded
    // falloff around the glow.
    let sky = clamp(uv.y, 0.0, 1.0);
    let sky_col = mix(grad4(u.palette, 1.0), grad4(u.palette, 0.0), sky * sky);
    col += sky_col * haze * (0.012 + 0.05 * pow(sky, 2.5)) * (0.6 + 0.4 * energy);

    col *= (1.0 + impact * 0.8);

    // Triangular-PDF dither, ~1 code value in a gamma-2.2 space, so the long
    // dark glow falloffs do not band when the frame is quantised to 8 bits.
    // Interleaved gradient noise (Jimenez) from the pixel position, two
    // decorrelated taps summed, then back to linear.
    let px = in.pos.xy;
    let n1 = fract(52.9829189 * fract(dot(px, vec2<f32>(0.06711056, 0.00583715))));
    let n2 = fract(52.9829189 * fract(dot(px + vec2<f32>(37.0, 17.0), vec2<f32>(0.06711056, 0.00583715))));
    let tri = (n1 + n2 - 1.0) / 255.0;
    let g = pow(max(col, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2)) + vec3<f32>(tri);
    col = pow(max(g, vec3<f32>(0.0)), vec3<f32>(2.2));

    return vec4<f32>(col, 1.0);
}
