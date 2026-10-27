// The post chain: light leaks composited first, camera shake applied as the
// blit that samples the (slightly larger) offscreen texture down to the
// final size. One pass, because both are the same "sample once, tint once"
// shape and every model goes through this identically.

// Wake's (`estela`) own params block, for a `fx.estela` chain step — shipped
// 2026-09-04, the ring this file's own note below used to call undone. See
// `shared::post::EstelaParams`'s own comment for the exact field mapping and
// for why `avance`/`tartamudeo`/`retencion`/`salto` (`models::estela::
// Stutter`) are not among them — the block-mosh proper never touched those
// four.
struct EstelaParams {
    blood: ColorSlot,
    // block (blocks across), freeze, refresh (seconds), drift
    a: vec4<f32>,
    // tear, bleed, exposure, contrast
    b: vec4<f32>,
    // saturation, r, g, b
    c: vec4<f32>,
};

// The advanced block, the Rust twin of `shared::post::GradeUniform` — same
// field names and order, so a change on one side is a change on the other.
// The grade components are exactly `metal_schema::GRADE_FLOATS`' own order,
// regrouped into 16-byte units (a uniform array's stride is 16 bytes whatever
// the element holds); the linear `GRADE_FLOATS` index of each component is
// with it. `fx` is `(fx_texture, fx_clarity, fx_dehaze, active)` where
// `active` is 1 only when some grade float is off its declared default — see
// `effect_grade`'s own note for why the gate exists.
struct GradeUniform {
    // 0 temperature, 1 tint, 2 exposure, 3 contrast
    tone_a: vec4<f32>,
    // 4 highlights, 5 shadows, 6 whites, 7 blacks
    tone_b: vec4<f32>,
    // 8 vibrance, 9 saturation, 10 curve_highlights, 11 curve_lights
    color_a: vec4<f32>,
    // 12 curve_darks, 13 curve_shadows, 14 split_shadows, 15 split_mid
    curve_a: vec4<f32>,
    // 16 split_highlights, 17 refine_saturation, 54 blending, 55 balance
    mix_a: vec4<f32>,
    // 18..41: band b is (hue, sat, lum, spare) at 18 + 3b
    mixer: array<vec4<f32>, 8>,
    // 42..53: wheel z is (hue, sat, lum, spare) at 42 + 3z
    grading: array<vec4<f32>, 4>,
    // advanced 56 fx_texture, 57 fx_clarity, 58 fx_dehaze, active gate
    fx: vec4<f32>,
};

// The extra params of the chain steps that have more than one float, the Rust
// twin of `shared::post::ChainFx`. One block per effect, appended here when an
// effect is added (an effect's first float is the chain step's own amount and
// is not repeated in it). `fm` is `fx.fm`:
//   fm_a = (carrier_freq, modulator_freq, index, phase)
//   fm_b = (angle in radians, wave_shape 0..3, x_tilt, y_tilt)
//   fm_c = (colour_tilt, feedback, spare, spare)
// `wavewarp` is `fx.wavewarp`:
//   wavewarp_a = (kind 0..7, length, height, angle in radians)
//   wavewarp_b = (speed, phase in radians, pin 0..6, pin_x)
//   wavewarp_c = (pin_y, asymmetry, layer2, layer2_length)
//   wavewarp_d = (layer2_angle in radians, crossing, edge 0..2, spare)
// `ascii` is `fx.ascii`:
//   ascii_a = (cell px, ramp 0..4, contrast, colour_mode 0..2)
//   ascii_b = (colour_mix, background, invert, noise)
//   ascii_c = (gamma, flicker, aspect ratio, tint)
//   ascii_d = (edges, drift, thickness, spare)
struct ChainFx {
    fm_a: vec4<f32>,
    fm_b: vec4<f32>,
    fm_c: vec4<f32>,
    wavewarp_a: vec4<f32>,
    wavewarp_b: vec4<f32>,
    wavewarp_c: vec4<f32>,
    wavewarp_d: vec4<f32>,
    ascii_a: vec4<f32>,
    ascii_b: vec4<f32>,
    ascii_c: vec4<f32>,
    ascii_d: vec4<f32>,
    mosaic_a: vec4<f32>,
    faceted_a: vec4<f32>,
    thermograph_a: vec4<f32>,
    depthfield_a: vec4<f32>,
    refract_a: vec4<f32>,
    relamp_a: vec4<f32>,
    letterform_a: vec4<f32>,
    datafield_a: vec4<f32>,
};

struct PostUniform {
    // x: shake 0..1, y: light_leak 0..1, z: time (s), w: output aspect (w/h)
    params: vec4<f32>,
    // xy: final_size / offscreen_size (the crop factor toward the centre)
    crop: vec4<f32>,
    // x: kaleidoscope mirrors, below 2 is off. yzw spare.
    extra: vec4<f32>,
    // x: how many of `chain`'s 16 slots are populated, 0..=16. yzw spare —
    // a `vec4<u32>` rather than a bare `u32` so this sits at the same
    // 16-byte stride as everything else here and needs no manual padding
    // on either side of the WGSL/Rust boundary.
    chain_len: vec4<u32>,
    // The ten common effects below `kaleido`, plus Wake — `crt` through
    // `grain` and `fx.estela` — as an explicit ordered list instead of the
    // fixed sequence this used to be. Per step: x = effect index (the
    // position `metal-schema::EFFECTS_FLOATS` gives it for the ten, 4..=13, or
    // `metal_schema::CHAIN_KIND_ESTELA` for the footage promotion — see the
    // block comment above `effect_crt` for why nothing outside that set is a
    // chain step), y = amount, zw spare — `metal_schema::EffectStep::slot` is not
    // packed in here; see `estela`'s own comment below for why a footage
    // step's slot does not need to reach the GPU yet. 16 has to match
    // `metal_schema::MAX_CHAIN`. `array<vec4<f32>, 16>` rather than a tighter
    // packing because a uniform array's stride is 16 bytes regardless of how
    // few of the four components a step actually uses — the same reason
    // `ColorSlot` below is four `vec4`s instead of three packed `vec3`s.
    chain: array<vec4<f32>, 16>,
    leak_color: ColorSlot,
    // The one params block a `fx.estela` chain step reads, whichever slot
    // its own `EffectStep.slot` names — because there is only ever this one
    // to read regardless: v1 has exactly one params block per footage kind,
    // not one per chain step or per node, the same way there is one
    // `leak_color` above rather than one per model. See
    // `metal-render::shared::post::EstelaParams`'s own doc comment for why.
    wake: EstelaParams,
    // The advanced block — see `GradeUniform` above.
    grade: GradeUniform,
    // Multi-float chain steps' params — see `ChainFx` above.
    chain_fx: ChainFx,
};

@group(0) @binding(0) var<uniform> u: PostUniform;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;
// `fx.estela`'s ring: the last `ESTELA_TAPS` frames this generator actually
// drew, undecayed, newest first — `estela_ring0` is "0 taps back" (this
// frame's own copy, kept for uniform indexing — see `effect_estela`'s own
// comment), `estela_ring7` the oldest. `ESTELA_TAPS` separate bindings
// rather than one `texture_2d_array` (the shape `shared::footage::Frames`
// uses for its own eight taps) because a chain step's history lives in
// `shared::trail::EstelaRing` — N standalone textures with a write cursor,
// not one array texture — see that type's own doc for why, and
// `shared::post::PostChain::new`'s bind group layout for the Rust side of
// this same choice. A cold model, or `app.rs`'s window (which keeps no
// per-model history at all — see its own module doc), binds its own current
// frame to every one of these eight rather than leaving any unset — see
// `shared::trail::fallback_ring`.
@group(0) @binding(3) var estela_ring0: texture_2d<f32>;
@group(0) @binding(4) var estela_ring1: texture_2d<f32>;
@group(0) @binding(5) var estela_ring2: texture_2d<f32>;
@group(0) @binding(6) var estela_ring3: texture_2d<f32>;
@group(0) @binding(7) var estela_ring4: texture_2d<f32>;
@group(0) @binding(8) var estela_ring5: texture_2d<f32>;
@group(0) @binding(9) var estela_ring6: texture_2d<f32>;
@group(0) @binding(10) var estela_ring7: texture_2d<f32>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let xy = p[vi];
    var out: VOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}

// Fold a point of the frame into one mirrored wedge of `n`.
//
// The six lines the Kaleidoscope model is built on, here operating on whatever
// model just drew — which is why this is worth having in both places: the
// model chooses arms designed to be folded, and the effect folds Membrane,
// Turing and a room of wireframe for the same six lines.
//
// Corrected for the frame's aspect on the way in and back out, or a wedge is a
// sheared wedge on every output this suite draws to.
fn kaleidoscope(uv: vec2<f32>, n: f32, aspect: f32) -> vec2<f32> {
    var d = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);
    let r = length(d);
    var a = atan2(d.y, d.x);
    let wedge = 6.28318530718 / max(n, 2.0);
    a = a - floor(a / wedge) * wedge;
    if (a > wedge * 0.5) {
        a = wedge - a;
    }
    d = vec2<f32>(cos(a), sin(a)) * r;
    return d / vec2<f32>(aspect, 1.0) + vec2<f32>(0.5, 0.5);
}

// ---------------------------------------------------------------------------
// Six picks ported in from radiance (2026-08-25), plus four more added the
// same way just after (vignette/blur/sharpen/grain) — see
// `import/shader-picks/IMPORT.md` for where the first six came from.
//
// E5 (the design notes PARTE IV §18.11). Through 2026-08-30, all ten read `src_tex`
// fresh at `in.uv`, independently of the fold/crop/shake chain above and of
// each other, composited one at a time via `mix(color, effect(...),
// amount)`. That mix is exact at `amount == 0` (`mix(a, b, 0.0) == a`, bit
// for bit, whatever `b` computed) — which is what kept "0 is off" true for
// each of them individually — but it also meant they did not compose with
// each other or with the kaleidoscope fold: turning on `crt` and `zoom`
// together cross-faded one over the other rather than running one through
// the other.
//
// Fixed by threading one running `(chain_uv, chain_color)` pair through
// whichever of the ten below actually run — the way `folded` already
// threads a single UV through kaleidoscope/crop/shake above. `chain_uv`
// starts at `sampled_uv`, so these ten finally see the fold/crop/shake too,
// and is only ever relocated by the two effects with one well-defined new
// sample position — `zoom` (via `zoom_uv`) and `lorenz` (via
// `lorenz_warp_uv`). `crt` and `glitch` read `chain_uv` but don't relocate
// it: a three-way channel split and a noisy block substitution don't reduce
// to a single "new position" for whoever comes next, so they contribute to
// `chain_color` only. `chain_color` starts at the base sample and is
// updated after every step that runs, so each effect's "current pixel" is
// whatever ran before it *in whatever order the operator chose*, not a
// fresh read of the original frame.
//
// Through 2026-09-03, "whatever order the operator chose" was not actually
// a choice: the ten always ran crt, glitch, posterize, zoom, fire, lorenz,
// vignette, blur, sharpen, grain, because `fs_main` was ten lines of
// `chain_color = mix(chain_color, effect_x(...), ...)` in that literal
// source order, whatever a node graph upstream said. That was fine while an
// effect was a float on a model's row, and wrong the moment effects are
// nodes an operator arranges on a canvas, because "vignette then zoom" and
// "zoom then vignette" are different images and a fixed source order cannot
// express the difference. Fixed by replacing those ten lines with the loop
// below, which walks `u.chain` — populated from
// `metal_schema::Post::resolved_chain` on the Rust side, see its doc comment
// for the empty-chain case (no graph driving this row) that reproduces the
// old fixed order exactly, zero-amount steps and all, which is what keeps
// every preset saved before `chain` existed rendering the same picture it
// always did. Each populated entry names one of the ten by its
// `metal-schema::EFFECTS_FLOATS` position (4..=13 — `crt` through `grain`), or
// — Conjunto F ("B7") — Wake by `metal_schema::CHAIN_KIND_ESTELA`, and how much
// of it to apply; the `switch` dispatches on that position instead of a
// fixed line of source, so the *order the entries appear in `u.chain`* is
// now the composition order, not the order they are written below.
//
// Only these eleven are chain steps. `shake`, `light_leak` and `kaleido`
// (positions 0, 1, 3) stay exactly where they have always been — set up
// above, before `chain_uv`/`chain_color` even exist — because they are not
// "a pixel effect on the current pixel" the way these eleven are: `shake`
// and `kaleido` change *which* pixel `chain_uv` starts the chain at, and
// `light_leak` composites a lens phenomenon deliberately keyed to
// screen-locked `in.uv` rather than the chain's own warped coordinate (see
// where it composites, below). `ghost` (position 2) never reaches this
// shader at all: it is a separate history pass over a second offscreen
// texture, run in `headless.rs`/`trail.wgsl` *before* whichever texture
// `src_tex` binds to here, because it needs a frame of the past and this
// pass keeps none.
//
// ## The multi-tap question, decided — and shipped
//
// `metal_schema::CHAIN_KIND_ESTELA` (15) took the longest to land
// because it was an open question whether `fx.estela` was worth a real
// history ring or whether the chain should just never offer it. Decided:
// **it was worth it**, and the reason was not aesthetic. Wake was being
// retired as a *model* — it was always meant to be an effect, and
// `models::estela`'s own header says so in the first line ("a model that
// does not draw a picture, it modifies one"). With that retirement,
// `fx.estela` is the only Wake there is, and an effect that fell through
// to `default` and did nothing was not an effect, it was a name in a menu.
//
// Why the model could afford no history and the effect could not, which is
// the whole asymmetry: `models::estela` is a *transmission*. Every frame it
// can ever show already exists on disk at a known address, so "the frame a
// block froze on" is `playhead(t - Δ)` — arithmetic, not memory. A chain
// step runs over a *generated* model, whose past frames exist nowhere unless
// something kept them. There was no address to evaluate.
//
// What it cost, so the next person does not have to re-derive it:
// `shared::trail` gained a second history type, [`EstelaRing`]/[`EstelaRings`]
// — a ring of `ESTELA_TAPS` frames with a write cursor, undecayed, sitting
// *alongside* `Trail`'s own two-texture ping-pong rather than replacing it
// (see that module's doc for why one struct could not honestly serve both:
// ghost's history is a decayed blend, Wake's is specific, unblended
// frames). The ring itself needed no new pass, just a
// `copy_texture_to_texture` per advance — the cheap half. The expensive half
// was `PostUniform`'s bind group growing `ESTELA_TAPS` texture bindings so
// this switch could sample them: every builder of that bind group
// (`headless.rs`, `app.rs`) had to bind `ESTELA_TAPS` views, plus a defined
// fallback for the frames before a ring has filled (`fallback_ring`: every
// slot gets the current frame until real history exists).
//
// A `switch` with no matching `case` for a position outside 4..=15 still
// falls through to `default` and does nothing, so a chain that happens to
// name shake/light_leak/ghost/kaleido — which structurally do not belong
// here at all — or a position no schema declares wastes a slot rather than
// double-applying or corrupting anything, the same "drop rather than crash"
// posture `resolved_chain` already takes with an index no schema declares.
//
// Effects' own internal geometry (crt's scanlines, vignette's radius, fire's
// flame band, grain's noise seed, glitch's block grid) keys off `chain_uv`
// too, uniformly, the same choice `folded` already made for kaleidoscope
// rather than keeping a second, screen-locked coordinate alive in parallel
// just for these ten.
//
// The outer `mix(chain_color, effect(...), clamp(amount, 0, 1))` at every
// step is unchanged and still load-bearing: it is what guarantees "0 is
// exactly off" per effect, including cases like `glitch` whose own internal
// step()-based math is not provably identity at amount 0 by itself.
//
// Scoped, disclosed exception: `blur` and `sharpen` each read several
// *neighbouring* texels of `src_tex` inside their own function body (a ring
// of eight for blur, a cross of four for sharpen), not just the one pixel
// `chain_uv` points at. Fully chaining those neighbours — blur seeing what
// a prior effect actually drew next door, not just at its own centre —
// needs a real multi-pass ping-pong between two offscreen textures (each
// effect's output becomes the next effect's input texture), the same shape
// `the design notes PARTE IV` §18.6 (datamosh) and F3 (estela) already need and don't
// have. That is a bigger change than this fix takes on, so it is not
// attempted here. What blur/sharpen get instead: their *centre* tap is
// `chain_color` (the one pixel they're centred on does reflect every effect
// before them — real chaining, not fake), their *neighbour* taps are still
// `src_tex` (pre-chain — there is no other texture to read them from), and
// both are positioned at `chain_uv` so the neighbourhood at least moves
// correctly with any zoom/lorenz warp already applied. A blur after a zoom
// blurs the zoomed-in region, not the original framing — with some
// pre-zoom-scale neighbour texels, until a real ping-pong pass exists.
//
// `stand_in_pulse` replaces radiance's beat-phase `defaultPulse` — this chain
// has no beat clock, so it is wall-clock time against a fixed assumed tempo
// rather than the real one. Wiring an actual beat phase into `PostUniform`
// is the honest fix; until then every call below takes this in its place.
// ── Ported Sketch filters (2026-10-21) ──────────────────────────────────────
// Each takes the chain's amount as `amt` and its own params in one vec4 `p`.

fn effect_mosaic(uv: vec2<f32>, aspect: f32, amt: f32, p: vec4<f32>, base: vec3<f32>) -> vec3<f32> {
    let cell = max(p.x, 4.0);
    let gap = clamp(p.y, 0.0, 1.0);
    let shape = u32(clamp(p.z, 0.0, 3.0) + 0.5);
    let grid = vec2<f32>(aspect, 1.0) * cell;
    let g = uv * grid;
    let id = floor(g);
    let f = fract(g) - 0.5;
    let centre = (id + 0.5) / grid;
    var d = max(abs(f.x), abs(f.y));
    if (shape == 1u) { d = length(f); }
    if (shape == 2u) { d = abs(f.x) + abs(f.y); }
    if (shape == 3u) { d = max(abs(f.x), abs(f.y) * 0.5); }
    let cov = 1.0 - smoothstep(0.5 - gap - 0.01, 0.5 - gap, d);
    let c = textureSample(src_tex, src_sampler, centre).rgb;
    return mix(base, c * mix(1.0, 1.0 - gap * 0.8, 1.0 - cov), amt);
}

fn effect_faceted(uv: vec2<f32>, aspect: f32, amt: f32, p: vec4<f32>, base: vec3<f32>) -> vec3<f32> {
    let count = max(p.x, 3.0);
    let edge = clamp(p.y, 0.0, 1.0);
    let jitter = clamp(p.z, 0.0, 1.0);
    let q = uv * vec2<f32>(aspect, 1.0) * count;
    let id = floor(q);
    let f = fract(q) - 0.5;
    let h = hash21(id);
    let off = (vec2<f32>(hash21(id + 13.1), hash21(id + 41.7)) - 0.5) * jitter * 0.6;
    let d = length(f - off);
    let border = min(min(fract(q).x, 1.0 - fract(q).x), min(fract(q).y, 1.0 - fract(q).y));
    let centre = (id + 0.5 + off) / (vec2<f32>(aspect, 1.0) * count);
    let c = textureSample(src_tex, src_sampler, centre).rgb;
    let seam = smoothstep(0.0, 0.06 + edge * 0.12, border);
    return mix(base, c * mix(1.0 - edge * 0.7, 1.0, seam), amt);
}

fn thermal_palette(t: f32) -> vec3<f32> {
    let q = clamp(t, 0.0, 1.0);
    let cold = vec3<f32>(0.02, 0.05, 0.35);
    let mid = vec3<f32>(0.85, 0.25, 0.05);
    let hot = vec3<f32>(1.0, 0.95, 0.75);
    if (q < 0.5) { return mix(cold, mid, q * 2.0); }
    return mix(mid, hot, (q - 0.5) * 2.0);
}

fn effect_thermograph(uv: vec2<f32>, amt: f32, p: vec4<f32>, base: vec3<f32>) -> vec3<f32> {
    let levels = max(p.x, 2.0);
    let contrast = clamp(p.y, 0.0, 1.0);
    let tint = clamp(p.z, 0.0, 1.0);
    let c = textureSample(src_tex, src_sampler, uv).rgb;
    var l = dot(c, vec3<f32>(0.299, 0.587, 0.114));
    l = clamp((l - 0.5) * mix(1.0, 2.4, contrast) + 0.5, 0.0, 1.0);
    let q = floor(l * levels) / max(levels - 1.0, 1.0);
    let heat = thermal_palette(q);
    return mix(base, mix(heat, c * heat, tint * 0.35), amt);
}

fn effect_depthfield(uv: vec2<f32>, aspect: f32, amt: f32, p: vec4<f32>, base: vec3<f32>) -> vec3<f32> {
    let range_v = clamp(p.x, 0.0, 1.0);
    let parallax = clamp(p.y, 0.0, 1.0);
    let invert = p.z;
    let c = textureSample(src_tex, src_sampler, uv).rgb;
    var depth = dot(c, vec3<f32>(0.299, 0.587, 0.114));
    depth = select(depth, 1.0 - depth, invert > 0.5);
    let offset = (uv - 0.5) * parallax * range_v * (depth - 0.5) * 2.0;
    let shifted = textureSample(src_tex, src_sampler, clamp(uv + offset, vec2<f32>(0.0), vec2<f32>(1.0))).rgb;
    return mix(base, shifted * (0.65 + 0.7 * depth), amt);
}

fn effect_refract(uv: vec2<f32>, amt: f32, p: vec4<f32>, base: vec3<f32>) -> vec3<f32> {
    let strength = clamp(p.x, 0.0, 1.0);
    let dispersion = clamp(p.y, 0.0, 1.0);
    let a = p.z * 0.0174532925;
    let dir = vec2<f32>(cos(a), sin(a));
    let shift = dir * strength * 0.06;
    let ca = dir * strength * dispersion * 0.02;
    let cr = textureSample(src_tex, src_sampler, clamp(uv + shift + ca, vec2<f32>(0.0), vec2<f32>(1.0))).r;
    let cg = textureSample(src_tex, src_sampler, clamp(uv + shift, vec2<f32>(0.0), vec2<f32>(1.0))).g;
    let cb = textureSample(src_tex, src_sampler, clamp(uv + shift - ca, vec2<f32>(0.0), vec2<f32>(1.0))).b;
    return mix(base, vec3<f32>(cr, cg, cb), amt);
}

fn effect_relamp(uv: vec2<f32>, aspect: f32, amt: f32, p: vec4<f32>, base: vec3<f32>) -> vec3<f32> {
    let lx = clamp(p.x, 0.0, 1.0);
    let ly = clamp(p.y, 0.0, 1.0);
    let intensity = clamp(p.z, 0.0, 1.0);
    let texel = vec2<f32>(1.0 / max(aspect, 0.001), 1.0) * 0.004;
    let l0 = dot(textureSample(src_tex, src_sampler, uv).rgb, vec3<f32>(0.299, 0.587, 0.114));
    let lx1 = dot(textureSample(src_tex, src_sampler, uv + vec2<f32>(texel.x, 0.0)).rgb, vec3<f32>(0.299, 0.587, 0.114));
    let ly1 = dot(textureSample(src_tex, src_sampler, uv + vec2<f32>(0.0, texel.y)).rgb, vec3<f32>(0.299, 0.587, 0.114));
    let n = normalize(vec3<f32>((l0 - lx1) * 8.0, (l0 - ly1) * 8.0, 0.35));
    let ld = normalize(vec3<f32>((lx - 0.5) * 2.0, (ly - 0.5) * 2.0, 0.8));
    let lit = clamp(dot(n, ld), 0.0, 1.0) * intensity;
    let c = textureSample(src_tex, src_sampler, uv).rgb;
    return mix(base, c * (0.35 + 1.35 * lit) + vec3<f32>(0.08, 0.09, 0.11) * lit, amt);
}

fn glyph_mask(bits: u32, cellp: vec2<f32>, threshold: f32) -> f32 {
    let gx = u32(clamp(floor(cellp.x * 3.0), 0.0, 2.0));
    let gy = u32(clamp(floor(cellp.y * 5.0), 0.0, 4.0));
    let bit = u32(gy * 3u + gx);
    let on = ((bits >> bit) & 1u) == 1u;
    return select(0.0, 1.0, on);
}

fn effect_letterform(uv: vec2<f32>, aspect: f32, amt: f32, p: vec4<f32>, base: vec3<f32>) -> vec3<f32> {
    let cell = max(p.x, 4.0);
    let threshold = clamp(p.y, 0.0, 1.0);
    let invert = p.z;
    let grid = vec2<f32>(aspect, 1.0) * cell;
    let q = uv * grid;
    let id = floor(q);
    let centre = (id + 0.5) / grid;
    var l = dot(textureSample(src_tex, src_sampler, centre).rgb, vec3<f32>(0.299, 0.587, 0.114));
    l = select(l, 1.0 - l, invert > 0.5);
    let bits = u32(hash21(id) * 32767.0) ^ (u32(clamp(l, 0.0, 1.0) * 15.0) << 10u);
    let inked = glyph_mask(bits, fract(q), threshold);
    let paper = vec3<f32>(0.02, 0.02, 0.025);
    let ink = vec3<f32>(0.95, 0.93, 0.86);
    let glyph = mix(paper, ink, inked * step(threshold * 0.05, l));
    return mix(base, glyph, amt);
}

fn effect_datafield(uv: vec2<f32>, amt: f32, p: vec4<f32>, base: vec3<f32>) -> vec3<f32> {
    let cell = max(p.x, 4.0);
    let gain = clamp(p.y, 0.0, 1.0);
    let mode = u32(clamp(p.z, 0.0, 2.0) + 0.5);
    let q = uv * cell;
    let id = floor(q);
    let f = fract(q);
    let centre = (id + 0.5) / cell;
    let v = dot(textureSample(src_tex, src_sampler, centre).rgb, vec3<f32>(0.299, 0.587, 0.114)) * gain;
    let bar = step(f.y, v);
    let dotm = 1.0 - step(0.28, length(f - 0.5));
    let heat = thermal_palette(v);
    var out = vec3<f32>(v) * bar;
    if (mode == 1u) { out = vec3<f32>(0.8, 0.95, 0.85) * dotm * v; }
    if (mode == 2u) { out = heat * bar; }
    return mix(base, out, amt);
}

fn stand_in_pulse(t: f32) -> f32 {
    let bpm = 120.0;
    let beat_len = 60.0 / bpm;
    let phase = fract(t / beat_len);
    return pow(clamp(1.0 - phase, 0.0, 1.0), 3.0) * 0.85 + 0.1;
}

fn effect_crt(uv: vec2<f32>, aspect: f32, intensity: f32, pulse: f32) -> vec3<f32> {
    let p = pow(pulse, 0.5);
    let separate = intensity * vec2<f32>(0.005, 0.0) * p;
    let nc = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);
    let r_off = nc - separate;
    let g_off = nc;
    let b_off = nc + separate;

    let r = textureSample(src_tex, src_sampler, r_off / vec2<f32>(aspect, 1.0) + 0.5).r;
    let g = textureSample(src_tex, src_sampler, g_off / vec2<f32>(aspect, 1.0) + 0.5).g;
    let b = textureSample(src_tex, src_sampler, b_off / vec2<f32>(aspect, 1.0) + 0.5).b;

    var rgb = vec3<f32>(r, g, b);
    rgb = rgb * mix(1.0, 1.0 - pow(abs(sin(g_off.y * 160.0)), 16.0), intensity * p / 3.0);
    let rr = rgb.r * mix(1.0, 1.0 - pow(abs(sin(r_off.x * 200.0)), 6.0), intensity * p);
    let gg = rgb.g * mix(1.0, 1.0 - pow(abs(sin(g_off.x * 200.0)), 6.0), intensity * p);
    let bb = rgb.b * mix(1.0, 1.0 - pow(abs(sin(b_off.x * 200.0)), 6.0), intensity * p);
    return vec3<f32>(rr, gg, bb);
}

// radiance's fifth sub-glitch ("freeze") mixes in a previous frame from a
// second buffer. Dropped here for the reason `scramble.wgsl` explains at
// length: it needs a frame of history, which does not fit a function meant
// to sit in this stateless chain.
//
// `base` is `chain_color` — whatever the chain has produced at this pixel so
// far — rather than a fresh sample, which is what lets glitch's noise/
// invert/solid sub-mixes actually apply on top of crt instead of the
// original frame. Its one *positional* extra sample (the "shift" sub-glitch)
// still has to come from `src_tex`, same disclosed reason as blur/sharpen.
fn effect_glitch(uv: vec2<f32>, base: vec3<f32>, t: f32, intensity: f32, pulse: f32) -> vec3<f32> {
    let c = base;

    var block_sizes: array<vec2<f32>, 3> = array<vec2<f32>, 3>(
        vec2<f32>(0.3, 0.5), vec2<f32>(1.0, 0.15), vec2<f32>(1.0, 1.0));

    var n1x = 1.0; var n1y = 1.0; var n1z = 1.0; var n1w = 1.0;
    for (var i = 0; i < 3; i = i + 1) {
        let block = floor(uv / block_sizes[i]) * block_sizes[i];
        // Named `seed`, not `base` — this function now also takes a `base`
        // parameter (the chained color), and shadowing it with this unrelated
        // per-block hash offset inside the loop is exactly the kind of thing
        // worth just not doing.
        let seed = f32(i) * 13.7 + floor(t * 8.0);
        n1x = n1x * hash21(block + seed);
        n1y = n1y * hash21(block + seed + 7.0);
        n1z = n1z * hash21(block + seed + 31.0);
        n1w = n1w * hash21(block + seed + 53.0);
    }
    n1x = pow(n1x, 0.1); n1y = pow(n1y, 0.1); n1z = pow(n1z, 0.1); n1w = pow(n1w, 0.1);

    let parameter = intensity * (0.5 + 0.5 * pow(pulse, 2.0));

    let noise_glitch = step(1.0 - 0.2 * parameter, n1x);
    let white_noise = vec3<f32>(hash21(uv * 37.0 + t));
    let c2 = mix(c, white_noise, noise_glitch);

    let invert_glitch = step(1.0 - 0.2 * parameter, n1y);
    let c3 = mix(c2, 1.0 - c2, invert_glitch);

    let solid_glitch = step(1.0 - 0.2 * parameter, n1z);
    let c4 = mix(c3, vec3<f32>(0.0, 1.0, 0.0), solid_glitch);

    let shift_glitch = step(1.0 - 0.2 * parameter, n1w);
    let c5 = mix(c4, textureSample(src_tex, src_sampler, uv - vec2<f32>(0.2, 0.0)).rgb, shift_glitch);

    return c5;
}

// Operates on `chain_color` directly — posterize has never needed a texture
// read of its own, only the pixel value, so chaining it is just: quantize
// whatever the chain handed it instead of a fresh sample of the original.
fn effect_posterize(base: vec3<f32>, intensity: f32, pulse: f32) -> vec3<f32> {
    let i = max(intensity, 0.02);
    let bins = min(256.0, 1.0 / i);
    let c2 = clamp(round(base * bins) / bins, vec3<f32>(0.0), vec3<f32>(1.0));
    return mix(base, c2, pow(pulse, 2.0));
}

// The coordinate half of `effect_zoom`, split out so `fs_main` can advance
// `chain_uv` by the same warp the color sample below uses — one source of
// truth for the transform, rather than a second copy of this formula living
// next to it. Identity at `intensity == 0.0` (`factor == 1.0` exactly), so
// relocating `chain_uv` unconditionally is safe: a zero amount leaves later
// effects reading from the same position they would have without zoom in
// the chain at all.
fn zoom_uv(uv: vec2<f32>, intensity: f32, pulse: f32, invert: bool) -> vec2<f32> {
    let factor = clamp(1.0 - abs(intensity) * pow(pulse, 2.0), 0.05, 2.0);
    if (!invert) {
        return (uv - 0.5) * factor + 0.5;
    }
    return (uv - 0.5) / factor + 0.5;
}

// One function for radiance's `zoomin`/`zoomout` — the same six lines with
// the transform inverted, so the sign of `intensity` (via `invert`) is what
// they differed by, not two shaders.
fn effect_zoom(uv: vec2<f32>, intensity: f32, pulse: f32, invert: bool) -> vec3<f32> {
    let tc = zoom_uv(uv, intensity, pulse, invert);
    if (!invert) {
        return textureSample(src_tex, src_sampler, tc).rgb;
    }
    let in_bounds = f32(tc.x >= 0.0 && tc.x <= 1.0 && tc.y >= 0.0 && tc.y <= 1.0);
    return textureSample(src_tex, src_sampler, tc).rgb * in_bounds;
}

// Needs `noise3` — shared.wgsl only has `hash31` (the 3D hash this builds
// on), not the value-noise wrapper.
fn post_noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let f2 = f * f * (3.0 - 2.0 * f);
    let n000 = hash31(i + vec3<f32>(0.0, 0.0, 0.0));
    let n100 = hash31(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = hash31(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = hash31(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = hash31(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = hash31(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = hash31(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = hash31(i + vec3<f32>(1.0, 1.0, 1.0));
    let nx00 = mix(n000, n100, f2.x); let nx10 = mix(n010, n110, f2.x);
    let nx01 = mix(n001, n101, f2.x); let nx11 = mix(n011, n111, f2.x);
    let nxy0 = mix(nx00, nx10, f2.y); let nxy1 = mix(nx01, nx11, f2.y);
    return mix(nxy0, nxy1, f2.z);
}

// `base` is `chain_color` — the flame is a synthetic gradient (`flame`,
// below) blended over whatever the chain has drawn so far, never a second
// texture read, so fire never needed its own sample to begin with.
fn effect_fire(uv: vec2<f32>, aspect: f32, t: f32, intensity: f32, pulse: f32, base: vec3<f32>) -> vec3<f32> {
    let nc = (uv - 0.5) * vec2<f32>(aspect, 1.0);
    let ni = vec3<f32>(nc * 3.0 + vec2<f32>(0.0, t * 0.5), t * 0.25);
    var shift = vec2<f32>(post_noise3(ni), post_noise3(ni + 100.0)) - 0.5;
    shift = shift + (vec2<f32>(post_noise3(2.0 * ni), post_noise3(2.0 * ni + 100.0)) - 0.5) * 0.5;
    shift = shift + (vec2<f32>(post_noise3(4.0 * ni), post_noise3(4.0 * ni + 100.0)) - 0.5) * 0.25;
    shift = (intensity * 0.5 + 0.5) * (0.7 * pow(pulse, 2.0) + 0.3) * shift
        + vec2<f32>(0.0, -0.5 + 0.5 * intensity);
    let uv2 = uv + shift / vec2<f32>(aspect, 1.0);
    let flame = vec3<f32>(1.0, (1.0 - uv2.y) * 0.6, 0.0);
    let m = clamp(smoothstep(0.1, 0.3, uv2.y) * smoothstep(0.0, 0.2, intensity), 0.0, 1.0);
    return mix(base, flame, m);
}

fn lorenz_deriv(xyz: vec3<f32>, srb: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        srb.x * (xyz.y - xyz.x),
        xyz.x * (srb.y - xyz.z) - xyz.y,
        xyz.x * xyz.y - srb.z * xyz.z);
}

// The coordinate half of `effect_lorenz`, split out for the same reason
// `zoom_uv` is: `fs_main` advances `chain_uv` by exactly this warp, from one
// source of truth. Identity at `intensity == 0.0` (`duv * 0.0 == 0.0` for
// any finite `duv`, so `new_uv == uv` exactly) — the chaotic flow still
// runs, but multiplying its delta by a zero intensity discards it before it
// ever reaches `uv`, same as the zoom case.
fn lorenz_warp_uv(uv: vec2<f32>, t: f32, intensity: f32) -> vec2<f32> {
    let srb = vec3<f32>(10.0, 28.0, 8.0 / 3.0);
    var xyz = vec3<f32>(uv - 0.5, sin(t * 0.6));
    xyz = xyz * mix(vec3<f32>(1.0), vec3<f32>(90.0, 120.0, 20.0), smoothstep(0.0, 0.7, intensity));
    xyz = xyz + vec3<f32>(10.0, 0.0, 30.0);
    let start = xyz;
    for (var i = 0; i < 20; i = i + 1) {
        xyz = xyz + lorenz_deriv(xyz, srb) * 0.01;
    }
    let dxyz = xyz - start;
    let duv = (dxyz.xy + 0.5 * dxyz.z) * 0.01;
    return uv + duv * intensity;
}

fn effect_lorenz(uv: vec2<f32>, t: f32, intensity: f32) -> vec3<f32> {
    let new_uv = lorenz_warp_uv(uv, t, intensity);
    let clamped = clamp(new_uv, vec2<f32>(0.0), vec2<f32>(1.0));
    let in_bounds = f32(new_uv.x >= 0.0 && new_uv.x <= 1.0 && new_uv.y >= 0.0 && new_uv.y <= 1.0);
    return textureSample(src_tex, src_sampler, clamped).rgb * in_bounds;
}

// Four more, added the same way as the six radiance picks above, and
// composited via the same `mix(chain_color, ..., amt)` in `fs_main`, so none
// of them need their own off-case. `vignette` and `grain` never needed a
// texture read at all — both are a pure function of the pixel value they're
// handed (`base` = `chain_color`) plus position/time — so chaining them is
// just: stop re-deriving `base` from a fresh sample.
fn effect_vignette(uv: vec2<f32>, aspect: f32, intensity: f32, base: vec3<f32>) -> vec3<f32> {
    let d = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);
    let r = length(d);
    let falloff = smoothstep(0.2, 0.85 - 0.5 * intensity, r);
    return base * (1.0 - falloff * intensity);
}

// `base` (= `chain_color`) stands in for the effect's own centre tap, so the
// one pixel this is centred on reflects every effect before it in the
// chain. The eight ring taps are still `src_tex` — see the block comment
// above `effect_crt` for why (a real multi-pass ping-pong is what full
// neighbour-chaining needs, not attempted here) — positioned at `uv`
// (`chain_uv`) so the ring at least sits over the right part of the frame
// after a zoom/lorenz warp, even though its *values* are pre-chain.
fn effect_blur(uv: vec2<f32>, aspect: f32, intensity: f32, base: vec3<f32>) -> vec3<f32> {
    let radius = intensity * 0.02;
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var i = 0; i < 8; i = i + 1) {
        let a = f32(i) * 0.78539816340;
        let off = vec2<f32>(cos(a), sin(a)) * radius / vec2<f32>(aspect, 1.0);
        sum = sum + textureSample(src_tex, src_sampler, uv + off).rgb;
        total = total + 1.0;
    }
    sum = sum + base;
    total = total + 1.0;
    return sum / total;
}

// Same shape as `effect_blur`: centre tap is `base` (`chain_color`, real
// chaining), the four cross taps are the same disclosed `src_tex`-neighbour
// exception, positioned at `uv`.
fn effect_sharpen(uv: vec2<f32>, aspect: f32, intensity: f32, base: vec3<f32>) -> vec3<f32> {
    let texel = vec2<f32>(0.005, 0.005) / vec2<f32>(aspect, 1.0);
    let up = textureSample(src_tex, src_sampler, uv + vec2<f32>(0.0, texel.y)).rgb;
    let down = textureSample(src_tex, src_sampler, uv - vec2<f32>(0.0, texel.y)).rgb;
    let left = textureSample(src_tex, src_sampler, uv - vec2<f32>(texel.x, 0.0)).rgb;
    let right = textureSample(src_tex, src_sampler, uv + vec2<f32>(texel.x, 0.0)).rgb;
    let avg = (up + down + left + right) * 0.25;
    return clamp(base + (base - avg) * intensity * 3.0, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn effect_grain(uv: vec2<f32>, t: f32, intensity: f32, base: vec3<f32>) -> vec3<f32> {
    let n = hash21(uv * 800.0 + hash11(t * 60.0) * 100.0) - 0.5;
    return clamp(base + vec3<f32>(n) * intensity, vec3<f32>(0.0), vec3<f32>(1.0));
}

// Luma (Rec. 709) — the same formula as `luma709` in
// `shared::footage::PRELUDE`, duplicated rather than shared, because that
// prelude is not part of this shader: `shared::build_shader` only prepends
// `shared::PRELUDE` (`shaders/shared.wgsl`), and `footage::PRELUDE` is
// deliberately kept out of that one — see its own header — because most of
// what this pass draws over is not footage.
fn luma709(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// ---------------------------------------------------------------------------
// The advanced block: the Lightroom-style colour grade plus texture, clarity
// and dehaze. The maths is a straight port of `metal-perform::grade`'s CPU
// mirror — that module is the authority and this is its GPU twin, so the two
// sides of the same sliders move a picture the same way. Neutral params (the
// declared defaults, which include split 25/50/75, refine 100 and blending 50)
// are the identity; `effect_grade` itself is gated on `u.grade.fx.w` so a row
// that never carried an advanced block does not even round-trip through HSL,
// which is what keeps the pre-existing 14-effect output byte-identical.
//
// The three helpers below are `metal-perform::grade`'s `rgb_to_hsl`,
// `hsl_to_rgb` and `hue_weight`, unchanged; WGSL has no closures, so the HSL
// channel curve is `grade_hsl_channel` rather than a local lambda.
fn grade_hsl_channel(p: f32, q: f32, t: f32) -> f32 {
    let tt = fract(t);
    if (tt < 1.0 / 6.0) {
        return p + (q - p) * 6.0 * tt;
    }
    if (tt < 0.5) {
        return q;
    }
    if (tt < 2.0 / 3.0) {
        return p + (q - p) * (2.0 / 3.0 - tt) * 6.0;
    }
    return p;
}

fn grade_rgb_to_hsl(c: vec3<f32>) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let l = (mx + mn) * 0.5;
    let d = mx - mn;
    if (d <= 1e-6) {
        return vec3<f32>(0.0, 0.0, l);
    }
    let s = select(d / (mx + mn), d / (2.0 - mx - mn), l > 0.5);
    var h = (c.r - c.g) / d + 4.0;
    if (mx == c.r) {
        h = (c.g - c.b) / d;
    } else if (mx == c.g) {
        h = (c.b - c.r) / d + 2.0;
    }
    return vec3<f32>(fract(h / 6.0), s, l);
}

fn grade_hsl_to_rgb(hsl: vec3<f32>) -> vec3<f32> {
    let h = fract(hsl.x);
    let s = clamp(hsl.y, 0.0, 1.0);
    let l = clamp(hsl.z, 0.0, 1.0);
    if (s <= 1e-6) {
        return vec3<f32>(l);
    }
    let q = select(l * (1.0 + s), l + s - l * s, l >= 0.5);
    let p = 2.0 * l - q;
    return vec3<f32>(
        grade_hsl_channel(p, q, h + 1.0 / 3.0),
        grade_hsl_channel(p, q, h),
        grade_hsl_channel(p, q, h - 1.0 / 3.0));
}

fn grade_hue_weight(hue_deg: f32, centre: f32, spread: f32) -> f32 {
    let d = fract((hue_deg - centre + 180.0) / 360.0) * 360.0 - 180.0;
    return clamp(1.0 - abs(d) / spread, 0.0, 1.0);
}

fn grade_white_balance(c: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    let t = p.tone_a.x / 100.0;
    let ti = p.tone_a.y / 100.0;
    return c + vec3<f32>(0.18 * t + 0.06 * ti, -0.12 * ti, -0.18 * t - 0.06 * ti);
}

fn grade_exposure(c: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    // ±2 stops over the slider's whole range.
    return c * exp2(p.tone_a.z / 100.0 * 2.0);
}

fn grade_contrast(c: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    let k = 1.0 + p.tone_a.w / 100.0;
    return (c - vec3<f32>(0.5)) * k + vec3<f32>(0.5);
}

// The four tone zones, each an additive luminance nudge — `metal-perform::grade`'s
// `tone_zones`, unchanged.
fn grade_tone_zones(c: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    let l = clamp(luma709(c), 0.0, 1.0);
    let w_hi = smoothstep(0.5, 1.0, l);
    let w_sh = 1.0 - smoothstep(0.0, 0.5, l);
    let w_wh = smoothstep(0.7, 1.0, l);
    let w_bl = 1.0 - smoothstep(0.0, 0.3, l);
    let d = 0.25 * (p.tone_b.x / 100.0 * w_hi
        + p.tone_b.y / 100.0 * w_sh
        + p.tone_b.z / 100.0 * w_wh
        + p.tone_b.w / 100.0 * w_bl);
    return c + vec3<f32>(d);
}

// One band of `parametric_curve`'s four regions: 1 inside `lo..hi`, falling to
// 0 a little past each edge.
fn grade_region(lo: f32, hi: f32, l: f32) -> f32 {
    let in_lo = smoothstep(lo - 0.15, lo, l);
    let in_hi = 1.0 - smoothstep(hi, hi + 0.15, l);
    return in_lo * in_hi;
}

fn grade_parametric_curve(c: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    let l = clamp(luma709(c), 0.0, 1.0);
    let s0 = clamp(p.curve_a.z / 100.0, 0.0, 1.0);
    let s1 = max(clamp(p.curve_a.w / 100.0, 0.0, 1.0), s0);
    let s2 = max(clamp(p.mix_a.x / 100.0, 0.0, 1.0), s1);
    let d = 0.2 * (p.curve_a.y / 100.0 * grade_region(0.0, s0, l)
        + p.curve_a.x / 100.0 * grade_region(s0, s1, l)
        + p.color_a.w / 100.0 * grade_region(s1, s2, l)
        + p.color_a.z / 100.0 * grade_region(s2, 1.0, l));
    return c + vec3<f32>(d);
}

fn grade_vibrance_saturation(c: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    let l = luma709(c);
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let sat = select(0.0, (mx - mn) / mx, mx > 1e-6);
    let vib = p.color_a.x / 100.0 * (1.0 - sat);
    var amount = p.color_a.y / 100.0 + vib * 0.5;
    if (p.mix_a.y > 0.0 && p.mix_a.y < 100.0) {
        amount = amount * p.mix_a.y / 100.0;
    }
    let a = clamp(amount, -1.0, 1.0);
    return c + (c - vec3<f32>(l)) * a;
}

// The eight band centres in wheel order — `metal-perform::grade::MIX_BANDS`.
const GRADE_MIX_CENTRES: array<f32, 8> = array<f32, 8>(
    0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 280.0, 320.0);

fn grade_hsl_mixer(c: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    var hsl = grade_rgb_to_hsl(c);
    let hue_deg = hsl.x * 360.0;
    var dh = 0.0;
    var ds = 0.0;
    var dl = 0.0;
    for (var b = 0; b < 8; b = b + 1) {
        let w = grade_hue_weight(hue_deg, GRADE_MIX_CENTRES[b], 60.0);
        if (w <= 0.0) {
            continue;
        }
        dh = dh + p.mixer[b].x / 100.0 * w * 0.15;
        ds = ds + p.mixer[b].y / 100.0 * w;
        dl = dl + p.mixer[b].z / 100.0 * w * 0.25;
    }
    hsl.x = fract(hsl.x + dh);
    hsl.y = clamp(hsl.y * (1.0 + ds), 0.0, 1.0);
    hsl.z = clamp(hsl.z + dl, 0.0, 1.0);
    return grade_hsl_to_rgb(hsl);
}

// One grading wheel as a channel offset — `metal-perform::grade::wheel_offset`.
fn grade_wheel_offset(wheel: vec4<f32>) -> vec3<f32> {
    let s = clamp(wheel.y / 100.0, 0.0, 1.0);
    if (s <= 0.0) {
        return vec3<f32>(0.0);
    }
    let rgb = grade_hsl_to_rgb(vec3<f32>(wheel.x / 360.0, s, 0.5));
    let lum = wheel.z / 100.0 * 0.2;
    let mean = (rgb.r + rgb.g + rgb.b) / 3.0;
    return (rgb - vec3<f32>(mean)) * s * 0.25 + vec3<f32>(lum);
}

fn grade_colour_grade(c: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    let l = clamp(luma709(c), 0.0, 1.0);
    let bal = p.mix_a.w / 100.0 * 0.15;
    let w_sh = clamp(1.0 - smoothstep(0.0, 0.5 - bal, l), 0.0, 1.0);
    let w_hi = smoothstep(0.5 + bal, 1.0, l);
    let w_mid = max(1.0 - w_sh - w_hi, 0.0);
    let split = clamp(p.mix_a.z / 100.0, 0.0, 1.0) * 2.0;
    var off = vec3<f32>(0.0);
    off = off + grade_wheel_offset(p.grading[0]) * (w_sh * split);
    off = off + grade_wheel_offset(p.grading[1]) * (w_mid * split);
    off = off + grade_wheel_offset(p.grading[2]) * (w_hi * split);
    return c + off + grade_wheel_offset(p.grading[3]);
}

fn grade_refine_saturation(c: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    if (p.mix_a.y <= 0.0 || p.mix_a.y >= 100.0) {
        return c;
    }
    let k = clamp(p.mix_a.y / 100.0, 0.0, 1.0);
    let l = luma709(c);
    return vec3<f32>(l) + (c - vec3<f32>(l)) * k;
}

// The whole grade, in `metal-perform::grade::apply_grade`'s order.
fn effect_grade(color: vec3<f32>, p: GradeUniform) -> vec3<f32> {
    var c = color;
    c = grade_white_balance(c, p);
    c = grade_exposure(c, p);
    c = grade_contrast(c, p);
    c = grade_tone_zones(c, p);
    c = grade_parametric_curve(c, p);
    c = grade_vibrance_saturation(c, p);
    c = grade_hsl_mixer(c, p);
    c = grade_colour_grade(c, p);
    c = grade_refine_saturation(c, p);
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

// ---------------------------------------------------------------------------
// `fx.fm` — frequency-modulation warp. A carrier wave displaces the picture
// along an axis at `angle`; the carrier's phase is bent by a modulator wave
// (`index` deep), as in FM synthesis: wave(fc*s + I/(2pi) * wave(fm*s + phi)).
// `feedback` feeds the luma of the already-displaced sample back into the
// modulator's phase and resamples (in-frame recursion: this pass has no
// previous-frame texture). Waves are in cycles, -1..1, all sin-phased.
// ---------------------------------------------------------------------------
fn fm_wave(x: f32, shape: f32) -> f32 {
    let k = i32(round(shape));
    if (k == 1) {
        return 1.0 - 4.0 * abs(fract(x + 0.25) - 0.5);
    }
    if (k == 2) {
        return 2.0 * fract(x + 0.5) - 1.0;
    }
    if (k == 3) {
        return select(-1.0, 1.0, fract(x) < 0.5);
    }
    return sin(6.2831853 * x);
}

// The displacement (in uv) at position `uv`; `push` is the feedback phase
// push on the modulator and `off` a carrier phase offset (per channel).
fn fm_disp(uv: vec2<f32>, aspect: f32, amt: f32, p: ChainFx, push: f32, off: f32) -> vec2<f32> {
    let dir = vec2<f32>(cos(p.fm_b.x), sin(p.fm_b.x));
    let s = dot((uv - 0.5) * vec2<f32>(aspect, 1.0), dir);
    let m = fm_wave(p.fm_a.y * s + p.fm_a.w + push, p.fm_b.y);
    let car = fm_wave(p.fm_a.x * s + p.fm_a.z * 0.15915494 * m + off, p.fm_b.y);
    return vec2<f32>(p.fm_b.z / aspect, p.fm_b.w) * car * 0.12 * amt;
}

// The coordinate half, split out so `fs_main` advances `chain_uv` by the
// same warp the colour uses (the zoom pattern). Identity at amt == 0.
fn fm_uv(uv: vec2<f32>, aspect: f32, amt: f32, p: ChainFx) -> vec2<f32> {
    var d = fm_disp(uv, aspect, amt, p, 0.0, 0.0);
    if (p.fm_c.y > 0.0) {
        let c1 = textureSample(src_tex, src_sampler, clamp(uv + d, vec2<f32>(0.0), vec2<f32>(1.0))).rgb;
        d = fm_disp(uv, aspect, amt, p, p.fm_c.y * luma709(c1), 0.0);
    }
    return clamp(uv + d, vec2<f32>(0.0), vec2<f32>(1.0));
}

fn effect_fm(uv: vec2<f32>, aspect: f32, amt: f32, p: ChainFx) -> vec3<f32> {
    let cc = p.fm_c.x;
    let tc = fm_uv(uv, aspect, amt, p);
    var c = textureSample(src_tex, src_sampler, tc).rgb;
    if (cc > 0.0) {
        // Colour FM: red and blue ride the carrier at shifted phases (a
        // chroma split along the wave) and the wave also gains the brightness.
        let push = select(0.0, p.fm_c.y * luma709(c), p.fm_c.y > 0.0);
        let dr = fm_disp(uv, aspect, amt, p, push, -0.15 * cc);
        let db = fm_disp(uv, aspect, amt, p, push, 0.15 * cc);
        let r = textureSample(src_tex, src_sampler, clamp(uv + dr, vec2<f32>(0.0), vec2<f32>(1.0))).r;
        let b = textureSample(src_tex, src_sampler, clamp(uv + db, vec2<f32>(0.0), vec2<f32>(1.0))).b;
        let car = fm_wave(p.fm_a.x * dot((uv - 0.5) * vec2<f32>(aspect, 1.0), vec2<f32>(cos(p.fm_b.x), sin(p.fm_b.x))), p.fm_b.y);
        c = vec3<f32>(r, c.g, b) * (1.0 + 0.35 * cc * car);
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

// ---------------------------------------------------------------------------
// `fx.wavewarp` — After Effects' Wave Warp. A wave travels along `angle`;
// each pixel is displaced perpendicular to it by the wave's value, `height`
// (a fraction of the frame height) tall, `length` (also of the height) long.
// Shapes (cycles in, -1..1 out): sine, triangle, square, saw, circle,
// semicircle, uncircle, noise. `pin` fades the warp to nothing toward the
// chosen edges / centre. Written from the AE behaviour; no code shared.
// ---------------------------------------------------------------------------
fn ww_half(u: f32) -> f32 {
    let q = 2.0 * u - 1.0;
    return sqrt(max(1.0 - q * q, 0.0));
}

fn ww_hash(i: f32) -> f32 {
    return fract(sin(i * 127.1 + 311.7) * 43758.5453) * 2.0 - 1.0;
}

fn ww_wave(x: f32, shape: f32) -> f32 {
    let k = i32(round(shape));
    let ph = fract(x);
    if (k == 1) {
        return 1.0 - 4.0 * abs(fract(x + 0.25) - 0.5);
    }
    if (k == 2) {
        return select(-1.0, 1.0, ph < 0.5);
    }
    if (k == 3) {
        return 2.0 * ph - 1.0;
    }
    if (k == 4) {
        return select(-ww_half(ph * 2.0 - 1.0), ww_half(ph * 2.0), ph < 0.5);
    }
    if (k == 5) {
        return 2.0 * ww_half(fract(ph * 2.0)) - 1.0;
    }
    if (k == 6) {
        return 1.0 - 2.0 * ww_half(fract(ph * 2.0));
    }
    if (k == 7) {
        return ww_hash(floor(x * 2.0));
    }
    return sin(6.2831853 * x);
}

// 0 where the warp is pinned, 1 where it is free. u, v in 0..1.
fn ww_pin(uv: vec2<f32>, p: ChainFx) -> f32 {
    let k = i32(round(p.wavewarp_b.z));
    let rx = max(p.wavewarp_b.w, 0.05);
    let ry = max(p.wavewarp_c.x, 0.05);
    let u = clamp(uv.x, 0.0, 1.0);
    let v = clamp(uv.y, 0.0, 1.0);
    var w = 1.0;
    if (k == 1) {
        w = min(min(u, 1.0 - u) / rx, min(v, 1.0 - v) / ry);
    } else if (k == 2) {
        w = u / rx;
    } else if (k == 3) {
        w = (1.0 - u) / rx;
    } else if (k == 4) {
        w = v / ry;
    } else if (k == 5) {
        w = (1.0 - v) / ry;
    } else if (k == 6) {
        w = length(vec2<f32>((u - 0.5) / rx, (v - 0.5) / ry));
    }
    return clamp(w, 0.0, 1.0);
}

// One wave layer at `pos` (height units): direction angle `ang`, wavelength `len`.
fn ww_layer(pos: vec2<f32>, ang: f32, len: f32, phase: f32, p: ChainFx) -> f32 {
    let d = vec2<f32>(cos(ang), sin(ang));
    var w = ww_wave(dot(pos, d) / len + phase, p.wavewarp_a.x);
    // Asymmetry: crests grow, troughs shrink (or the reverse).
    w = w * (1.0 + p.wavewarp_c.y * select(-1.0, 1.0, w > 0.0));
    return w;
}

fn ww_edge(t: vec2<f32>, mode: f32) -> vec2<f32> {
    let k = i32(round(mode));
    if (k == 1) {
        return 1.0 - abs(1.0 - (t - 2.0 * floor(t * 0.5)));
    }
    if (k == 2) {
        return fract(t);
    }
    return clamp(t, vec2<f32>(0.0), vec2<f32>(1.0));
}

// The coordinate half, split out so `fs_main` advances `chain_uv` by the
// same warp (the zoom/fm pattern). Identity at amt == 0.
fn wavewarp_uv(uv: vec2<f32>, aspect: f32, time: f32, amt: f32, p: ChainFx) -> vec2<f32> {
    let pos = (uv - 0.5) * vec2<f32>(aspect, 1.0);
    let phase = p.wavewarp_b.y * 0.15915494 + time * p.wavewarp_b.x;
    let ang = p.wavewarp_a.w;
    let len = max(p.wavewarp_a.y, 0.02);
    var disp = ww_layer(pos, ang, len, phase, p);
    if (p.wavewarp_c.z > 0.0) {
        let a2 = ang + p.wavewarp_d.x;
        // The second layer travels the other way so the two beat against each other.
        let d2 = ww_layer(pos, a2, len * p.wavewarp_c.w, -phase * 1.3, p);
        disp = disp + p.wavewarp_c.z * d2;
    }
    let amount = disp * p.wavewarp_a.z * amt * ww_pin(uv, p);
    // Displaced perpendicular to the travel direction; `crossing` also pushes along it.
    let d = vec2<f32>(cos(ang), sin(ang));
    let off = vec2<f32>(-d.y, d.x) * amount + d * amount * p.wavewarp_d.y;
    return ww_edge(uv + off / vec2<f32>(aspect, 1.0), p.wavewarp_d.z);
}

fn effect_wavewarp(uv: vec2<f32>, aspect: f32, time: f32, amt: f32, p: ChainFx) -> vec3<f32> {
    return textureSample(src_tex, src_sampler, wavewarp_uv(uv, aspect, time, amt, p)).rgb;
}

// `fx.ascii` — the frame redrawn as characters. The picture is cut into a grid
// of cells (`cell` px wide, `aspect ratio` times as tall); each cell's
// luminance (contrast, gamma, edges, noise, flicker, inversion applied) picks
// a glyph from one of four ramps and the glyph is drawn from the 8x8 bitmap
// font of the `ascii` model, tinted from the frame. Cells are read from
// `src_tex` at the cell centre (5 taps), like the other resampling effects.
// ASCII_GLYPHS is the four ramps of `models/ascii.rs` back to back (lengths
// 10, 9, 8, 7), each glyph two words, four rows per word, low byte first, most
// significant bit leftmost; a Rust test keeps it equal to that source.
// ---------------------------------------------------------------------------
const ASCII_GLYPHS = array<vec2<u32>, 34>(
    vec2<u32>(0u, 0u),
    vec2<u32>(0u, 1579008u),
    vec2<u32>(1579008u, 1579008u),
    vec2<u32>(2113929216u, 126u),
    vec2<u32>(8257536u, 126u),
    vec2<u32>(2115508224u, 1579134u),
    vec2<u32>(1012537344u, 1595964u),
    vec2<u32>(612246564u, 606371364u),
    vec2<u32>(268985442u, 8799776u),
    vec2<u32>(2862236220u, 8159406u),
    vec2<u32>(0u, 0u),
    vec2<u32>(0u, 4278190080u),
    vec2<u32>(0u, 4294901760u),
    vec2<u32>(0u, 4294967040u),
    vec2<u32>(0u, 4294967295u),
    vec2<u32>(4278190080u, 4294967295u),
    vec2<u32>(4294901760u, 4294967295u),
    vec2<u32>(4294967040u, 4294967295u),
    vec2<u32>(4294967295u, 4294967295u),
    vec2<u32>(0u, 0u),
    vec2<u32>(0u, 4278190080u),
    vec2<u32>(2113929216u, 126u),
    vec2<u32>(268960770u, 8405024u),
    vec2<u32>(404232216u, 404232216u),
    vec2<u32>(270549120u, 132104u),
    vec2<u32>(2115508224u, 1579134u),
    vec2<u32>(612246564u, 606371364u),
    vec2<u32>(0u, 0u),
    vec2<u32>(402653184u, 24u),
    vec2<u32>(0u, 1579008u),
    vec2<u32>(1111228416u, 3949122u),
    vec2<u32>(1111638588u, 3949122u),
    vec2<u32>(1380599356u, 3949154u),
    vec2<u32>(4294934076u, 1014956031u),
);

fn ascii_bit(idx: u32, x: i32, y: i32) -> f32 {
    if (x < 0 || x > 7 || y < 0 || y > 7) {
        return 0.0;
    }
    let g = ASCII_GLYPHS[idx];
    let word = select(g.x, g.y, y >= 4);
    let bits = (word >> (u32(y & 3) * 8u)) & 255u;
    return f32((bits >> u32(7 - x)) & 1u);
}

fn ascii_hash(p: vec2<f32>, s: f32) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7)) + s * 74.7) * 43758.5453);
}

fn ascii_hue(h: f32) -> vec3<f32> {
    let k = clamp(abs(fract(h + vec3<f32>(0.0, 0.6666667, 0.3333333)) * 6.0 - 3.0) - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
    return mix(vec3<f32>(1.0), k, 0.8);
}

// The frame colour of the cell whose centre is `c` (uv): centre plus four
// taps a quarter cell away, so a thin line does not alias out of the cell.
fn ascii_cell_rgb(c: vec2<f32>, q: vec2<f32>) -> vec3<f32> {
    var s = textureSampleLevel(src_tex, src_sampler, c, 0.0).rgb * 2.0;
    s = s + textureSampleLevel(src_tex, src_sampler, c + vec2<f32>(q.x, q.y), 0.0).rgb;
    s = s + textureSampleLevel(src_tex, src_sampler, c + vec2<f32>(-q.x, q.y), 0.0).rgb;
    s = s + textureSampleLevel(src_tex, src_sampler, c + vec2<f32>(q.x, -q.y), 0.0).rgb;
    s = s + textureSampleLevel(src_tex, src_sampler, c + vec2<f32>(-q.x, -q.y), 0.0).rgb;
    return s / 6.0;
}

fn effect_ascii(uv: vec2<f32>, time: f32, p: ChainFx) -> vec3<f32> {
    let dims = vec2<f32>(textureDimensions(src_tex));
    let cell = vec2<f32>(max(p.ascii_a.x, 1.0), max(p.ascii_a.x * p.ascii_c.z, 1.0));
    let scroll = vec2<f32>(0.0, p.ascii_d.y * time);
    let g = uv * dims / cell + scroll;
    let id = floor(g);
    let f = fract(g);
    let cuv = clamp((id - scroll + 0.5) * cell / dims, vec2<f32>(0.0), vec2<f32>(1.0));
    let q = cell * 0.25 / dims;

    let rgb = ascii_cell_rgb(cuv, q);
    var l = luma709(rgb);
    // Edge boost: the luminance step to the four neighbouring cells adds ink.
    if (p.ascii_d.x > 0.0) {
        let dx = vec2<f32>(cell.x / dims.x, 0.0);
        let dy = vec2<f32>(0.0, cell.y / dims.y);
        let lr = luma709(textureSampleLevel(src_tex, src_sampler, clamp(cuv + dx, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb);
        let ll = luma709(textureSampleLevel(src_tex, src_sampler, clamp(cuv - dx, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb);
        let lu = luma709(textureSampleLevel(src_tex, src_sampler, clamp(cuv + dy, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb);
        let ld = luma709(textureSampleLevel(src_tex, src_sampler, clamp(cuv - dy, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb);
        l = l + p.ascii_d.x * 2.5 * (abs(lr - ll) + abs(lu - ld));
    }
    l = clamp((clamp(l, 0.0, 1.0) - 0.5) * p.ascii_a.z + 0.5, 0.0, 1.0);
    l = pow(l, max(p.ascii_c.x, 0.05));
    l = l + (ascii_hash(id, 1.0) - 0.5) * p.ascii_b.w;
    // Flicker: ten times a second a random half of the cells re-roll their level.
    let tick = floor(time * 10.0);
    let hit = step(0.5, ascii_hash(id, 2.0 + tick));
    l = l + hit * (ascii_hash(id, 5.0 + tick) - 0.5) * p.ascii_c.y * 1.4;
    l = clamp(l, 0.0, 1.0);
    l = mix(l, 1.0 - l, p.ascii_b.z);

    // Which ramp: 0..3 one set, 4 a different set in every cell.
    var r = u32(clamp(round(p.ascii_a.y), 0.0, 4.0));
    if (r == 4u) {
        r = u32(ascii_hash(id, 3.0) * 4.0) & 3u;
    }
    var n = 10u;
    var off = 0u;
    if (r == 1u) { n = 9u; off = 10u; }
    if (r == 2u) { n = 8u; off = 19u; }
    if (r == 3u) { n = 7u; off = 27u; }
    let idx = off + min(u32(l * f32(n)), n - 1u);

    let gx = i32(clamp(f.x * 8.0, 0.0, 7.99));
    let gy = i32(clamp(f.y * 8.0, 0.0, 7.99));
    var ink = ascii_bit(idx, gx, gy);
    // Weight: a fraction of the lit neighbour bits fattens the strokes.
    if (p.ascii_d.z > 0.0) {
        let nb = max(max(ascii_bit(idx, gx - 1, gy), ascii_bit(idx, gx + 1, gy)),
                     max(ascii_bit(idx, gx, gy - 1), ascii_bit(idx, gx, gy + 1)));
        ink = max(ink, p.ascii_d.z * nb);
    }

    // Colour of the ink: 0 mono tint, 1 frame hue, 2 palette walking with
    // luminance; `colour_mix` pulls it to the cell's own (unnormalised) colour.
    let mode = u32(clamp(round(p.ascii_a.w), 0.0, 2.0));
    let tint = ascii_hue(p.ascii_c.w);
    let hue_rgb = rgb / max(max(max(rgb.r, rgb.g), rgb.b), 0.15);
    var col = tint;
    if (mode == 1u) { col = clamp(hue_rgb, vec3<f32>(0.0), vec3<f32>(1.0)); }
    if (mode == 2u) { col = ascii_hue(p.ascii_c.w + l * 0.4); }
    col = mix(col, rgb, p.ascii_b.x);
    // The ground glows half in the cell's colour, half in the tint, whatever the ink mode.
    let bg = mix(rgb, tint * (0.3 + l), 0.5) * p.ascii_b.y;
    return clamp(mix(bg, col, ink), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Texture, clarity and dehaze — `metal-perform::grade`'s `apply_texture`,
// `apply_clarity` and `apply_dehaze`, per-pixel as they are there (a real
// neighbour pass is a separate, disclosed project). Each is exactly identity
// at amount 0.
fn effect_texture(color: vec3<f32>, amount: f32) -> vec3<f32> {
    let a = clamp(amount / 100.0, -1.0, 1.0);
    if (a == 0.0) {
        return color;
    }
    let mean = (color.r + color.g + color.b) / 3.0;
    return clamp(color + (color - vec3<f32>(mean)) * a, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn effect_clarity(color: vec3<f32>, amount: f32) -> vec3<f32> {
    let a = clamp(amount / 100.0, -1.0, 1.0);
    if (a == 0.0) {
        return color;
    }
    let l = clamp(luma709(color), 0.0, 1.0);
    let mask = 1.0 - abs(2.0 * l - 1.0);
    let mean = (color.r + color.g + color.b) / 3.0;
    return clamp(color + (color - vec3<f32>(mean)) * a * mask * 1.5, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn effect_dehaze(color: vec3<f32>, amount: f32) -> vec3<f32> {
    let a = clamp(amount / 100.0, -1.0, 1.0);
    if (a == 0.0) {
        return color;
    }
    let dark = min(color.r, min(color.g, color.b));
    return clamp(color - vec3<f32>(a * dark * 0.6) + vec3<f32>(a * 0.02), vec3<f32>(0.0), vec3<f32>(1.0));
}

// ---------------------------------------------------------------------------
// Wake as a chain step (`metal_schema::CHAIN_KIND_ESTELA`, position 15) —
// shipped 2026-09-04, the ring the block comment above `effect_crt` used to
// call undone.
//
// Ported from `shaders/estela.wgsl`'s `fs_main`, which stays the authority on
// this math — the block grid, the clumped per-region freeze, the drift and
// tear, the hue-drift on stale blocks — and is not repeated here. What the
// port changes:
//
// - **No `Stutter`.** The model's own header explains at length why the taps
//   it reads are resolved on the CPU (`playhead`, against a `Cue`/`Library`):
//   a chain step has neither, so there is nothing to hold or jump *within* —
//   see `EstelaParams`'s own comment (`shared::post`) for the full argument.
//   What is left is exactly the block-mosh, unchanged, reading a ring of
//   this generator's own last `ESTELA_TAPS` frames instead of resolved
//   footage rungs.
// - **No `cover()`, no slate.** The model fits footage of a foreign aspect
//   into the frame and draws a test card with nothing loaded; a ring slot is
//   always this frame's own size, and always holds *something* once
//   allocated (`fallback_ring`), so neither case exists here.
// - **`footage::PRELUDE`'s `hash` is not in scope.** `post.wgsl` is built
//   with `shared::PRELUDE` only — the same gap `luma709`'s own
//   comment discloses — so `estela_hash` stands in, built on
//   `lattice` (`shared.wgsl`) rather than duplicating that formula a third
//   time.
// - **`ESTELA_TAPS` separate texture bindings, not a `texture_2d_array`.**
//   `estela_ring_sample` is the `switch` that stands in for the dynamic
//   index an array texture would have given for free — see the block
//   comment above `estela_ring0`'s own binding for why.
fn estela_hash(cell: vec2<i32>, salt: u32) -> f32 {
    return lattice(vec2<f32>(cell) + vec2<f32>(f32(salt) * 15485.863, f32(salt) * 86028.121));
}

// `estela.wgsl::clumped`, unchanged down to the bilinear blend — only the
// hash underneath it differs (`estela_hash`, above).
fn estela_clumped(p: vec2<f32>, salt: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let s = f * f * (3.0 - 2.0 * f);
    let c = vec2<i32>(i);
    return mix(
        mix(estela_hash(c, salt), estela_hash(c + vec2<i32>(1, 0), salt), s.x),
        mix(estela_hash(c + vec2<i32>(0, 1), salt), estela_hash(c + vec2<i32>(1, 1), salt), s.x),
        s.y,
    );
}

// `estela.wgsl::grade`, unchanged: intensity, contrast, saturation, then the
// three channel gains, in that order — a channel gain applied before a
// contrast pivot means something else.
fn estela_grade(c: vec3<f32>, p: EstelaParams) -> vec3<f32> {
    var x = c * p.b.z; // exposure
    x = (x - vec3<f32>(0.5)) * p.b.w + vec3<f32>(0.5); // contrast
    x = mix(vec3<f32>(luma709(x)), x, p.c.x); // saturation
    return x * p.c.yzw; // r, g, b
}

const ESTELA_TAPS: i32 = 8;
// The cadence `shared::trail::EstelaRing::advance` gates real advances on
// (`models::estela::TAP_DT`) — the two sides of one agreement, the same way
// `shared::footage::MAX_TAPS` and the shader's own `array<Tap, 8>` are one
// number twice. If this drifts from the Rust constant, "how many taps back a
// freeze reaches" stops meaning the same thing on the two sides — it does
// not go out of range either way, so nothing here would catch that; the
// number is trusted, not verified, the same trust `models::estela`'s own
// module doc places in `TAP_DT`/`MAX_HOLD`.
const ESTELA_TAP_DT: f32 = 0.083333336;
const ESTELA_CLUMP: f32 = 3.0; // `estela.wgsl::CLUMP` — same reason, unchanged.

// The `switch` a `texture_2d_array` would not have needed — see the block
// comment above `estela_ring0`'s own binding.
fn estela_ring_sample(generation: i32, uv: vec2<f32>) -> vec3<f32> {
    let uvc = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
    switch (generation) {
        case 0: { return textureSample(estela_ring0, src_sampler, uvc).rgb; }
        case 1: { return textureSample(estela_ring1, src_sampler, uvc).rgb; }
        case 2: { return textureSample(estela_ring2, src_sampler, uvc).rgb; }
        case 3: { return textureSample(estela_ring3, src_sampler, uvc).rgb; }
        case 4: { return textureSample(estela_ring4, src_sampler, uvc).rgb; }
        case 5: { return textureSample(estela_ring5, src_sampler, uvc).rgb; }
        case 6: { return textureSample(estela_ring6, src_sampler, uvc).rgb; }
        default: { return textureSample(estela_ring7, src_sampler, uvc).rgb; }
    }
}

fn effect_estela(uv: vec2<f32>, aspect: f32, time: f32, p: EstelaParams) -> vec3<f32> {
    let frozen = p.a.y;
    let refresh = max(p.a.z, 0.05);

    // Square blocks on the wall, not in uv — `estela.wgsl`'s own reason,
    // unchanged: an oblong macroblock is the one thing a codec never
    // produces.
    let across = max(p.a.x, 1.0);
    let grid = vec2<f32>(across, max(round(across / max(aspect, 1e-3)), 1.0));
    let cell = vec2<i32>(floor(uv * grid));

    // The region this block belongs to — `estela.wgsl`'s own reason for
    // deciding the freeze per region rather than per block: independent
    // coins scatter into confetti, and a codec loses a contiguous patch, not
    // isolated squares.
    let where_ = vec2<f32>(cell) / ESTELA_CLUMP;
    let region = vec2<i32>(floor(where_));

    // Each region keeps its own keyframe clock, offset by its own hash, so
    // regions do not all reset on the same frame.
    let phase = estela_hash(region, 7u);
    let epoch = floor(time / refresh + phase);
    let froze = (epoch - phase) * refresh;

    var generation = 0.0;
    var drift = vec2<f32>(0.0);
    if (estela_clumped(where_, u32(i32(epoch) & 0xFFFF) * 2654435761u + 11u) < frozen) {
        // How many taps back the frame this block froze on now is — walks
        // deeper every tap the region stays frozen, clamped to the oldest
        // tap the ring actually keeps.
        generation = clamp(floor((time - froze) / ESTELA_TAP_DT), 0.0, f32(ESTELA_TAPS - 1));
        // The vectors that kept arriving, one direction per region so a
        // frozen patch marches instead of dissolving.
        let angle = estela_hash(region, 23u) * 6.2831853;
        drift = vec2<f32>(cos(angle), sin(angle)) * p.a.w * generation; // drift
        // The block's own tear, the only part of the drift that is per
        // block rather than per region.
        drift = drift + vec2<f32>(estela_hash(cell, 41u) - 0.5, estela_hash(cell, 53u) - 0.5) * p.b.x; // tear
    }

    let tap = clamp(i32(generation), 0, ESTELA_TAPS - 1);
    var c = estela_grade(estela_ring_sample(tap, uv + drift), p);

    // Chroma drifts on a stale block — `estela.wgsl`'s own reason: a
    // residual carries colour computed against pixels it never met, so the
    // error is a hue rotation, not a fade.
    let stale = generation / f32(ESTELA_TAPS - 1);
    c = c * mix(vec3<f32>(1.0), grad4(p.blood, stale), p.b.y * step(0.5, generation)); // bleed

    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let shake = u.params.x;
    let leak = u.params.y;
    let time = u.params.z;
    let mirrors = u.extra.x;

    // A new jitter target every ~24th of a second, held for that whole
    // window rather than drifting continuously — a held offset reads as a
    // shake; a smoothly varying one reads as a slow pan.
    let tick = floor(time * 24.0);
    // Divided in x by the aspect so the jitter is the same number of *pixels*
    // in both axes; a raw UV offset shakes wider than it shakes tall.
    let aspect = max(u.params.w, 0.01);
    let jitter = (vec2<f32>(hash11(tick), hash11(tick + 91.7)) * 2.0 - 1.0)
        * shake * 0.5 * (1.0 - u.crop.x) / vec2<f32>(aspect, 1.0);

    // The fold happens first, on the frame's own coordinates: fold, then crop
    // and shake what came out of it. The other order shakes each wedge
    // independently, which reads as the mirrors coming loose.
    var folded = in.uv;
    if (mirrors >= 2.0) {
        folded = kaleidoscope(folded, mirrors, aspect);
    }

    // `chain_uv`/`chain_color` are the running pair the ten effects below
    // thread through — see the block comment above `effect_crt`. They start
    // exactly where the fold/crop/shake chain above left off, which is half
    // of the E5 fix: before it, none of the ten ever saw the kaleidoscope
    // fold either, only the raw, unwarped `in.uv`.
    var chain_uv = clamp((folded - 0.5) * u.crop.xy + 0.5 + jitter, vec2<f32>(0.0), vec2<f32>(1.0));
    var chain_color = textureSample(src_tex, src_sampler, chain_uv).rgb;

    // A burst is a time-slot that rolled active; how many slots roll active
    // is the one float. Inside an active slot the envelope decays across it
    // — the "frames flashing past" of a tape change, not a steady tint.
    let slot = floor(time * 2.0);
    let roll = hash11(slot + 511.0);
    let burst_on = step(1.0 - leak * 0.7, roll);
    let envelope = burst_on * (1.0 - fract(time * 2.0)) * leak;
    // Screen-locked on purpose: a light leak is a lens/camera phenomenon,
    // not part of the frame's own content, so it stays keyed to `in.uv`
    // rather than `chain_uv` even once the chain below starts warping it.
    let band = smoothstep(0.0, 0.4, in.uv.x) * smoothstep(1.0, 0.6, in.uv.x);
    let leak_col = grad4(u.leak_color, fract(in.uv.x + roll));
    chain_color = chain_color + leak_col * envelope * band;

    // The ten common effects, in whatever order `u.chain` names — see the
    // block comment above `effect_crt` for what "chained" means here, why
    // only these ten (positions 4..=13) are chain steps at all, and the one
    // disclosed exception (blur/sharpen's neighbour taps). `mix` at amount
    // 0.0 returns `chain_color` unchanged regardless of what the effect
    // computed, which is what keeps every one of them exactly off at its
    // default — still true per step, just no longer tied to a fixed line of
    // source for which step runs when.
    let pulse = stand_in_pulse(time);
    for (var i = 0u; i < u.chain_len.x; i = i + 1u) {
        let step = u.chain[i];
        let kind = u32(step.x);
        let amt = step.y;
        switch kind {
            case 4u: {
                chain_color = mix(chain_color, effect_crt(chain_uv, aspect, amt, pulse), clamp(amt, 0.0, 1.0));
            }
            case 5u: {
                chain_color = mix(chain_color, effect_glitch(chain_uv, chain_color, time, amt, pulse), clamp(amt, 0.0, 1.0));
            }
            case 6u: {
                chain_color = mix(chain_color, effect_posterize(chain_color, amt, pulse), clamp(amt, 0.0, 1.0));
            }
            case 7u: {
                // Signed, so "off" is `amt == 0`, not `amt` at its minimum —
                // `abs` is what the mix clamps on, the same test
                // `Post::resolved_chain` already applies to this float on
                // the Rust side. Relocates `chain_uv` after the mix, since
                // the mix needs the pre-zoom position as its own input;
                // whichever step reads `chain_uv` next — in chain order, not
                // source order — sees the zoomed frame.
                let invert = amt < 0.0;
                chain_color = mix(chain_color, effect_zoom(chain_uv, amt, pulse, invert), clamp(abs(amt), 0.0, 1.0));
                chain_uv = zoom_uv(chain_uv, amt, pulse, invert);
            }
            case 8u: {
                chain_color = mix(chain_color, effect_fire(chain_uv, aspect, time, amt, pulse, chain_color), clamp(amt, 0.0, 1.0));
            }
            case 9u: {
                // Relocates `chain_uv` after the mix, same reason as zoom.
                chain_color = mix(chain_color, effect_lorenz(chain_uv, time, amt), clamp(amt, 0.0, 1.0));
                chain_uv = lorenz_warp_uv(chain_uv, time, amt);
            }
            case 10u: {
                chain_color = mix(chain_color, effect_vignette(chain_uv, aspect, amt, chain_color), clamp(amt, 0.0, 1.0));
            }
            case 11u: {
                chain_color = mix(chain_color, effect_blur(chain_uv, aspect, amt, chain_color), clamp(amt, 0.0, 1.0));
            }
            case 12u: {
                chain_color = mix(chain_color, effect_sharpen(chain_uv, aspect, amt, chain_color), clamp(amt, 0.0, 1.0));
            }
            case 13u: {
                chain_color = mix(chain_color, effect_grain(chain_uv, time, amt, chain_color), clamp(amt, 0.0, 1.0));
            }
            case 169u: { chain_color = mix(chain_color, effect_mosaic(chain_uv, aspect, amt, u.chain_fx.mosaic_a, chain_color), clamp(amt, 0.0, 1.0)); }
            case 173u: { chain_color = mix(chain_color, effect_faceted(chain_uv, aspect, amt, u.chain_fx.faceted_a, chain_color), clamp(amt, 0.0, 1.0)); }
            case 177u: { chain_color = mix(chain_color, effect_thermograph(chain_uv, amt, u.chain_fx.thermograph_a, chain_color), clamp(amt, 0.0, 1.0)); }
            case 181u: { chain_color = mix(chain_color, effect_depthfield(chain_uv, aspect, amt, u.chain_fx.depthfield_a, chain_color), clamp(amt, 0.0, 1.0)); }
            case 185u: { chain_color = mix(chain_color, effect_refract(chain_uv, amt, u.chain_fx.refract_a, chain_color), clamp(amt, 0.0, 1.0)); }
            case 189u: { chain_color = mix(chain_color, effect_relamp(chain_uv, aspect, amt, u.chain_fx.relamp_a, chain_color), clamp(amt, 0.0, 1.0)); }
            case 193u: { chain_color = mix(chain_color, effect_letterform(chain_uv, aspect, amt, u.chain_fx.letterform_a, chain_color), clamp(amt, 0.0, 1.0)); }
            case 197u: { chain_color = mix(chain_color, effect_datafield(chain_uv, amt, u.chain_fx.datafield_a, chain_color), clamp(amt, 0.0, 1.0)); }
            case 120u: {
                // `fx.ascii` — a pure colour step (the glyph grid is drawn at
                // the pixel); its other fifteen floats are `u.chain_fx`.
                if (amt > 0.0) {
                    chain_color = mix(chain_color, effect_ascii(chain_uv, time, u.chain_fx), clamp(amt, 0.0, 1.0));
                }
            }
            case 93u: {
                // `fx.fm` — relocates `chain_uv` like zoom; its other ten
                // floats are `u.chain_fx`. Off (amt == 0) is untouched. The
                // blend fades in fast so a tiny amount is not a hard cut to
                // the resampled source.
                if (amt > 0.0) {
                    chain_color = mix(chain_color, effect_fm(chain_uv, aspect, amt, u.chain_fx), smoothstep(0.0, 0.05, amt));
                    chain_uv = fm_uv(chain_uv, aspect, amt, u.chain_fx);
                }
            }
            case 104u: {
                // `fx.wavewarp` — relocates `chain_uv` like zoom; its other
                // floats are `u.chain_fx`. Off (amt == 0) is untouched.
                if (amt > 0.0) {
                    chain_color = mix(chain_color, effect_wavewarp(chain_uv, aspect, time, amt, u.chain_fx), smoothstep(0.0, 0.05, amt));
                    chain_uv = wavewarp_uv(chain_uv, aspect, time, amt, u.chain_fx);
                }
            }
            case 136u: {
                // `fx.brush` -- stateful: it repaints the model's frame before
                // this pass (`shared::brush`), so there is nothing to do here.
                // The case exists so the step is a declared one, not a spent
                // slot.
            }
            case 156u: {
                // `fx.hardstutter` -- stateful: it freezes this pass's output
                // in its own pass afterwards (`shared::stutter`), so there is
                // nothing to do here. The case exists so the step is a
                // declared one, not a spent slot.
            }
            case 15u: {
                // Wake — see the block comment above `effect_crt` for the
                // history this needed and `EstelaParams` for why `u.estela`
                // is the one look every `fx.estela` step reads. `amt` only
                // gates the outer `mix` — the look's own
                // `congelado`/`refresco`/etc. (the WGSL names) are what actually
                // shape the mosh, not a second scaling by `amt` inside
                // `effect_estela` itself.
                chain_color = mix(chain_color, effect_estela(chain_uv, aspect, time, u.wake), clamp(amt, 0.0, 1.0));
            }
            default: {
                // Position 0..3 (shake/light_leak/ghost/kaleido — applied
                // above, or in `ghost`'s case entirely outside this shader;
                // never chain steps), or >= 16 (a schema this build does not
                // declare). Either way: a slot spent doing nothing, not a
                // crash — same block comment.
            }
        }
    }

    // The advanced block, once per frame, after the common chain: texture,
    // clarity and dehaze (each exactly identity at 0), then the colour grade.
    // The grade is gated on `fx.w` — the CPU side sets it only when some
    // `grade_*` float is off its declared default — so a post with no advanced
    // block leaves the 14-effect output bit-for-bit where it always was; see
    // `GradeUniform`'s own note. Order matches `metal-perform::grade`, texture/
    // clarity/dehaze before the grade.
    chain_color = effect_texture(chain_color, u.grade.fx.x);
    chain_color = effect_clarity(chain_color, u.grade.fx.y);
    chain_color = effect_dehaze(chain_color, u.grade.fx.z);
    if (u.grade.fx.w > 0.5) {
        chain_color = effect_grade(chain_color, u.grade);
    }

    return vec4<f32>(chain_color, 1.0);
}
