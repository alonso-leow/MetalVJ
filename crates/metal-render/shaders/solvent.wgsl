// Model 10 — Solvent. Drops accumulated into a field, blurred, resolved
// through two palettes and grained. `models/solvent.rs` has the argument for
// why the model is made of drops at all; this file is the three passes.
//
//   cs_accumulate  drops   -> (wa, wa·ta, wb, wb·tb)
//   cs_blur  x2    field   -> the same, separably gaussian-blurred
//   fs_main        field   -> the frame
//
// The field carries **weighted sums, not colours**, and that is the one idea
// the whole thing turns on. A texel does not hold "material A, tone 0.7"; it
// holds "0.9 of material A, whose tone-sum is 0.63". The tone is the quotient,
// taken at the very end. Two consequences:
//
//   * Where drops of one material overlap, the tone is the average of theirs
//     weighted by how much of each is here — so an overlap is a *gradient
//     between* two colours instead of whichever drop was drawn last.
//   * Blurring is linear, so blurring the two sums and dividing afterwards is
//     the same as blurring the average. The blur passes do not have to know
//     that they are moving a colour around.

struct Drop {
    // x, y, radius, weight
    place: vec4<f32>,
    // tone 0..1, material (0 = a, 1 = b), unused, unused
    tint: vec4<f32>,
};

struct SolUniform {
    liquid_a: ColorSlot,
    liquid_b: ColorSlot,
    background: ColorSlot,
    // aspect, time, field size, blur sigma in texels
    misc: vec4<f32>,
    // agitacion, golpe, viscosidad, granulado
    look: vec4<f32>,
    // blur direction xy, sigma scale on that axis, unused
    sweep: vec4<f32>,
    drops: array<Drop, 64>,
};

@group(0) @binding(0) var<uniform> u: SolUniform;

// Two statements of one number — `MAX_DROPS` in solvent.rs is the other.
const DROPS: i32 = 64;
// Half the blur kernel. `BLUR_TAPS` in solvent.rs is the other statement.
const TAPS: i32 = 8;

// Where a surface is, in coverage. Under this the liquid is thinning out and
// the background shows; over it there is a body of liquid. `viscosidad` sets
// how wide the crossing is, and that width is the whole difference between a
// haze and a lava lamp.
//
// The band is clamped at zero on the way in, and that is not tidiness. A
// `smoothstep` whose lower bound is negative is already partway up at an input
// of nothing, so at low viscosity an empty frame came out 20% liquid — a
// navy wash over the whole background, from the first stop of `liquido_a`,
// with no drop anywhere near it.
const SURFACE: f32 = 0.22;

// ---------------------------------------------------------------------------
// where a texel is
// ---------------------------------------------------------------------------

// The field is square and the frame usually is not, so a texel maps to the
// frame, not the other way round: x runs to ±aspect, y to ±1, y up. Drops are
// dealt in this same space by `solvent::drops`, which is what makes a drop
// round on screen instead of round in the texture.
fn frame_point(uv: vec2<f32>, aspect: f32) -> vec2<f32> {
    return vec2<f32>((uv.x - 0.5) * 2.0 * aspect, (0.5 - uv.y) * 2.0);
}

// The flow the whole body of liquid is swept by.
//
// **`lattice_fbm`, not `fbm`.** The first version used the shared `fbm` and
// the frame came out with flat axis-aligned rectangles in it, tens of pixels
// across, with hard straight edges — in a model that has no straight line
// anywhere in its design. `fbm` is built on `hash21`, which is
// `fract(sin(dot(p, k)) * big)`; by the fourth octave of a coordinate a few
// units wide, `sin`'s argument is in the thousands, its range reduction in f32
// has about three digits left, and runs of neighbouring cells come back
// **identical**. A plateau in a value field is a smudge. A plateau in a *warp*
// field is worse than that: every screen point in the plateau is sent to the
// same place, so a whole rectangle of the frame samples one drop colour and
// the picture gets an edge no drop drew.
//
// Advected around a circle rather than by adding time to the coordinate, for
// the neighbouring reason: a set runs for hours and `time * 0.13` does not
// stay small. Two incommensurate frequencies keep it bounded and still never
// repeat.
fn slosh(p: vec2<f32>, time: f32, agitation: f32, hit: f32) -> vec2<f32> {
    if (agitation <= 0.0 && hit <= 0.0) {
        return p;
    }
    let drift = vec2<f32>(cos(time * 0.11), sin(time * 0.09)) * 2.0;
    // The warp has to be worth a good fraction of a drop's radius or it only
    // slides the blobs about; at a wavelength near the drop size and this
    // amplitude it deforms them, which is the difference between bokeh and
    // something with surface tension.
    let a = lattice_fbm(p * 1.30 + drift, 4) - 0.5;
    let b = lattice_fbm(p * 1.30 + drift + vec2<f32>(41.7, 17.3), 4) - 0.5;
    var q = p + vec2<f32>(a, b) * agitation * 1.60;

    // A periodic shove rather than a steady force — "they get hit" reads as
    // an impact, so it has to arrive and decay, not blow.
    let phase = fract(time * 0.4);
    let punch = hit * exp(-phase * 6.0) * sin(phase * 40.0);
    return q + vec2<f32>(punch, -punch) * 0.22;
}

// ---------------------------------------------------------------------------
// pass 1 — the drops
// ---------------------------------------------------------------------------

@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_accumulate(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = u32(u.misc.z);
    if (gid.x >= size || gid.y >= size) {
        return;
    }
    let aspect = max(u.misc.x, 0.01);
    let uv = (vec2<f32>(gid.xy) + 0.5) / f32(size);
    let p = slosh(frame_point(uv, aspect), u.misc.y, u.look.x, u.look.y);

    var wa = 0.0;
    var ta = 0.0;
    var wb = 0.0;
    var tb = 0.0;
    for (var i = 0; i < DROPS; i = i + 1) {
        let d = u.drops[i];
        let r = max(d.place.z, 1e-4);
        let dp = p - d.place.xy;
        // Compact support, flat on top, zero with zero slope at the rim, and
        // no `exp` in the inner loop. The rim is what makes it a drop rather
        // than a disc; the flat top is what makes it *fat*, and that matters
        // more than it sounds. Averaged over its own area this kernel is 0.5,
        // where `(1 - q)³` — the obvious choice, and the first one here — is
        // 0.25. Half the coverage for the same drops meant twice as many
        // drops were needed before the frame stopped being separate circles
        // on black, and by then every colour had averaged away.
        let x = max(0.0, 1.0 - dot(dp, dp) / (r * r));
        let w = x * x * (3.0 - 2.0 * x) * d.place.w;
        // Branchless: 64 drops per texel, and half of them taking the other
        // side of an `if` is 64 divergent iterations for nothing.
        let is_b = d.tint.y;
        let is_a = 1.0 - is_b;
        wa = wa + w * is_a;
        ta = ta + w * is_a * d.tint.x;
        wb = wb + w * is_b;
        tb = tb + w * is_b * d.tint.x;
    }

    textureStore(dst, vec2<i32>(gid.xy), vec4<f32>(wa, ta, wb, tb));
}

// ---------------------------------------------------------------------------
// pass 2 and 3 — the blur
// ---------------------------------------------------------------------------

// One entry point run twice, differing only in `u.sweep`. Separable, because
// a 17×17 kernel is 289 taps and two 17-tap passes are 34 for the same result.
//
// The edge clamps rather than fading to zero: a wash that reaches the frame
// edge should stay full-bleed on a projector, not develop a dark border no
// parameter asked for.
@compute @workgroup_size(8, 8, 1)
fn cs_blur(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = i32(u.misc.z);
    if (i32(gid.x) >= size || i32(gid.y) >= size) {
        return;
    }
    let dir = vec2<i32>(u.sweep.xy);
    let sigma = max(u.misc.w * max(u.sweep.z, 1e-3), 0.05);
    let inv = -0.5 / (sigma * sigma);

    var sum = vec4<f32>(0.0);
    var norm = 0.0;
    for (var k = -TAPS; k <= TAPS; k = k + 1) {
        let w = exp(f32(k * k) * inv);
        let c = clamp(vec2<i32>(gid.xy) + dir * k, vec2<i32>(0), vec2<i32>(size - 1));
        sum = sum + textureLoad(src, c, 0) * w;
        norm = norm + w;
    }
    textureStore(dst, vec2<i32>(gid.xy), sum / norm);
}

// ---------------------------------------------------------------------------
// pass 4 — the frame
// ---------------------------------------------------------------------------

@group(0) @binding(3) var samp: sampler;

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

// Film grain, and a dither.
//
// It is in the ask, and it also fixes something the ask did not mention: a
// wide smooth ramp quantised to 8 bits bands, and this model is nothing but
// wide smooth ramps. So the amplitude has a floor of about one and a half code
// values even at `granulado = 0` — enough to turn a step into a dither,
// too little to see as texture.
//
// Triangular rather than uniform (`h1 + h2 - 1`): it is the right dither
// distribution, it has no flat top, and it looks like grain instead of static.
// Strongest in the midtones, because that is where emulsion grain lives and
// where banding is worst — both arguments point the same way.
fn grain(col: vec3<f32>, px: vec2<f32>, time: f32, amount: f32) -> vec3<f32> {
    // 24 a second: grain that updates every frame crawls at high frame rates,
    // and grain that holds still reads as a dirty lens.
    let tick = floor(time * 24.0);
    let n1 = lattice3(vec3<f32>(px, tick));
    let n2 = lattice3(vec3<f32>(px + vec2<f32>(37.0, 91.0), tick + 7.0));
    let tri = n1 + n2 - 1.0;
    let luma = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
    let bite = 0.35 + 0.65 * clamp(4.0 * luma * (1.0 - luma), 0.0, 1.0);
    let amp = 1.5 / 255.0 + amount * 0.13;
    return col + tri * amp * bite;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let aspect = max(u.misc.x, 0.01);
    let v = textureSampleLevel(src, samp, in.uv, 0.0);
    let wa = v.x;
    let wb = v.z;
    let eps = 1e-4;

    // Each material's own tone, then the two mixed by how much of each is
    // here. `mix` and not `+`: two liquids meeting make a third colour, they
    // do not make a brighter one. The first Solvent added them and clipped.
    //
    // The quotient is taken raw. There was a gain here for one revision, on
    // the argument that a mean of n uniforms is narrow and would only ever
    // show the middle of a four-stop palette. True where drops pile up, and
    // exactly wrong where they do not: an isolated drop carries its own tone,
    // and a gain pushes half of those past the ends, so most of the frame came
    // out as the first or last stop and the palette between them never
    // appeared. The averaging is the model working, not an error to undo.
    let ta = clamp(v.y / max(wa, eps), 0.0, 1.0);
    let tb = clamp(v.w / max(wb, eps), 0.0, 1.0);
    let total = wa + wb;
    let visc = clamp(u.look.z, 0.0, 1.0);

    // How readily the two materials give up being themselves. A straight
    // `wb / total` is what mixing paint does and it is also mud: with the
    // drops crowded enough to read as liquid at all, most of the frame sits
    // near 50/50, and 50/50 of a blue and an orange is grey. Every colour in
    // the middle of the picture went milky.
    //
    // So the crossing gets a width, and `viscosidad` is what sets it, which is
    // the physical reading as well as the better-looking one: a thin liquid
    // blends into the one beside it, a thick one holds its own body and meets
    // the other along an edge. The 50/50 texel is still grey — it has to be —
    // but there are far fewer of them.
    let sep = mix(0.50, 0.10, visc);
    let m = smoothstep(0.5 - sep, 0.5 + sep, wb / max(total, eps));
    let liquid = mix(grad4(u.liquid_a, ta), grad4(u.liquid_b, tb), m);

    let edge = mix(0.55, 0.04, visc);
    let alpha = smoothstep(max(SURFACE - edge, 0.0), SURFACE + edge, total);

    let bg = backdrop_from_uv(u.background, in.uv, aspect);
    let col = mix(bg, liquid, alpha);
    return vec4<f32>(grain(col, floor(in.pos.xy), u.misc.y, clamp(u.look.w, 0.0, 1.0)), 1.0);
}
