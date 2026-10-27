// The three systems every model includes: the 4-colour gradient slot, a
// camera block for the 3D models, and small hash/noise utilities that
// several models and the post chain both need. String-concatenated ahead of
// every model's own WGSL at pipeline build time — see shared::PRELUDE.

struct ColorSlot {
    c0: vec4<f32>,
    c1: vec4<f32>,
    c2: vec4<f32>,
    c3: vec4<f32>,
};

// Piecewise-linear walk through the 4 control points. Four equal values is a
// solid colour, four close values is a soft gradient, four different values
// is a combination — no branching on which of the three it is, because the
// distinction was never a mode, only how far apart the inputs are.
fn grad4(slot: ColorSlot, t: f32) -> vec3<f32> {
    let tt = clamp(t, 0.0, 1.0) * 3.0;
    let seg = clamp(floor(tt), 0.0, 2.0);
    let f = tt - seg;
    var a: vec3<f32>;
    var b: vec3<f32>;
    if (seg < 0.5) {
        a = slot.c0.rgb; b = slot.c1.rgb;
    } else if (seg < 1.5) {
        a = slot.c1.rgb; b = slot.c2.rgb;
    } else {
        a = slot.c2.rgb; b = slot.c3.rgb;
    }
    return mix(a, b, f);
}

// Where a point sits along a background gradient, 0..1.
//
// `p` is centred and aspect-corrected — x in ±aspect, y in ±1, y up — and
// `dir` is the direction the slot is walked along. The `reach` term is the
// frame rectangle's support function in that direction: without it a diagonal
// gradient runs out before the corners it points at and leaves two flat
// wedges of the first and last stop.
//
// Shared because two things compute it: the backdrop pass (`shared/backdrop.rs`,
// for the models that draw objects over a background) and the models whose own
// fullscreen quad writes every pixel and so has to composite its background
// itself. Two copies would be two backgrounds that drift apart under one name.
fn backdrop_t(p: vec2<f32>, dir: vec2<f32>, aspect: f32) -> f32 {
    let reach = max(aspect * abs(dir.x) + abs(dir.y), 0.001);
    return clamp(dot(p, dir) / (2.0 * reach) + 0.5, 0.0, 1.0);
}

// The same, for a fullscreen model that has `uv` (0..1, y down) rather than
// clip space, walked top to bottom — which is what all three of them want.
fn backdrop_from_uv(slot: ColorSlot, uv: vec2<f32>, aspect: f32) -> vec3<f32> {
    let p = vec2<f32>((uv.x - 0.5) * 2.0 * aspect, (0.5 - uv.y) * 2.0);
    return grad4(slot, backdrop_t(p, vec2<f32>(0.0, -1.0), aspect));
}

// The 4 3D models each carry a `view_proj: mat4x4<f32>` in their own
// uniform instead of a shared struct here — `CameraGpu::view_proj` builds it
// once on the CPU with glam, which is simpler and less error-prone than
// reimplementing look-at/perspective in WGSL for four different models.

// Simple one-light Lambertian shading, shared so the 3D models read as one
// consistent lighting rig rather than four different guesses at "looks lit".
fn shade(n: vec3<f32>, base: vec3<f32>) -> vec3<f32> {
    let light_direction = normalize(vec3<f32>(0.45, 0.8, 0.35));
    let ambient = 0.35;
    let diffuse = max(dot(normalize(n), light_direction), 0.0) * 0.65;
    return base * (ambient + diffuse);
}

fn hash11(x: f32) -> f32 {
    return fract(sin(x * 127.1) * 43758.5453123);
}

fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453123);
}

fn hash31(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3<f32>(127.1, 311.7, 74.7))) * 43758.5453123);
}

// Lattice hashes with no `sin` in them, and the reason they exist.
//
// The three above are `fract(sin(dot(p, k)) * big)`, which is fine while `p`
// is small and silently stops being a hash when it is not. `dot` over an
// integer lattice walks an arithmetic progression; once its argument is in the
// thousands, f32 `sin` has almost no mantissa left and whole runs of
// neighbouring cells return the *same value*. The field goes flat in patches,
// and no amount of smarter interpolation helps: interpolation cannot smooth a
// plateau, because the plateau is the data.
//
// Two things in this suite walk out that far. Mengano's background is a
// five-octave fbm, and by the fifth octave the coordinate is in the thousands —
// the frame came out looking like nested squares (`CORRECTIONS.md` §46).
// Solvent's grain is indexed by pixel *and* by a frame counter that only goes
// up, so a long set walks it out to any range you like.
//
// Integer mixing has no argument range to run out of. Biased by 2^16 so the
// lattice is non-negative, which u32 wrapping arithmetic needs to be
// well-defined here. `lattice3` is the same mix with a third word; the two are
// deliberately one function apart rather than one function, because a 2-D
// caller should not pay to construct a `vec3`.
fn lattice(p: vec2<f32>) -> f32 {
    let x = u32(i32(p.x) + 65536);
    let y = u32(i32(p.y) + 65536);
    var n = x * 374761393u + y * 668265263u;
    n = (n ^ (n >> 13u)) * 1274126177u;
    n = n ^ (n >> 16u);
    return f32(n & 0x00FFFFFFu) / 16777216.0;
}

fn lattice3(p: vec3<f32>) -> f32 {
    let x = u32(i32(p.x) + 65536);
    let y = u32(i32(p.y) + 65536);
    let z = u32(i32(p.z) + 65536);
    var n = x * 374761393u + y * 668265263u + z * 2246822519u;
    n = (n ^ (n >> 13u)) * 1274126177u;
    n = n ^ (n >> 16u);
    return f32(n & 0x00FFFFFFu) / 16777216.0;
}

// Value noise and its octave sum. Here rather than per model because several
// models warp their fields with it, and two copies of a noise function is two
// chances for the same flow to look like two different flows.
//
// **These two are the legacy pair — reach for `quintic_noise` and
// `lattice_fbm` below in anything new.** `noise2` is built on `hash21`, which
// stops being a hash once its argument leaves the low hundreds (see
// `lattice`), and `fbm` gets there on the third or fourth octave of any
// coordinate bigger than about 3. What that looks like is not noise going
// slightly wrong: whole runs of cells return one value, and the field grows
// flat axis-aligned rectangles. Turing and Scratch keep these because their
// output is tuned against the exact numbers and neither shows the artefact —
// Turing's field is a reaction that averages it away, and Scratch only samples
// a line through it.
fn noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    let w = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

fn fbm(p: vec2<f32>, octaves: i32) -> f32 {
    var total = 0.0;
    var amp = 0.5;
    var freq = 1.0;
    for (var o = 0; o < octaves; o = o + 1) {
        total = total + noise2(p * freq) * amp;
        amp = amp * 0.5;
        freq = freq * 2.0;
    }
    return total;
}

// The same two, over `lattice`, and quintic instead of cubic. What a model
// should use unless it is one of the two that cannot change its output.
//
// Quintic interpolation is C2 where the cubic smoothstep is only C1: the cubic
// leaves a jump in curvature at every cell boundary, which is invisible in a
// busy field and a visible crease in a smooth one walked through a steep
// four-stop ramp. Mengano found that one; Solvent found the plateau. They are
// two different faults with the same cure, so the cure is one function.
fn quintic_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let a = lattice(i);
    let b = lattice(i + vec2<f32>(1.0, 0.0));
    let c = lattice(i + vec2<f32>(0.0, 1.0));
    let d = lattice(i + vec2<f32>(1.0, 1.0));
    let w = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

// 2.03 rather than 2, so no two octaves ever share a grid line.
fn lattice_fbm(p: vec2<f32>, octaves: i32) -> f32 {
    var total = 0.0;
    var amp = 0.5;
    var freq = 1.0;
    for (var o = 0; o < octaves; o = o + 1) {
        total = total + quintic_noise(p * freq) * amp;
        amp = amp * 0.5;
        freq = freq * 2.03;
    }
    return total;
}
