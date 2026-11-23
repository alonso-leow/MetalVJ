// Model 13 — Kaleidoscope. Geoterrain's spiral arms, folded into a wedge and
// mirrored around the frame.
//
// The arms are the same parametrisation Geoterrain draws with lines —
// `r = radius * s`, `theta = twist * s + arm`, turning at `angular_vel` — but
// evaluated as a field per pixel instead of tessellated into strips, because a
// fold is an operation on the *frame* and there is nothing to fold a triangle
// strip against.
//
// The fold itself is six lines and they are the whole model: polar, wrap the
// angle into one wedge of `2*pi/n`, mirror alternate wedges, evaluate. The
// same six lines run in `post.wgsl` as a common effect, where they fold every
// other model in the suite — see `docs/NEXT.md` §3. This one exists because
// what a kaleidoscope folds matters, and arms designed to be folded are not
// the same picture as any other model reflected.

struct KaleidoUniform {
    color: ColorSlot,
    floats: vec4<f32>,  // wedges, arms, twist, sharpness
    floats2: vec4<f32>, // radius, rings, spin, time
    floats3: vec4<f32>, // aspect, unused, unused, unused
};

@group(0) @binding(0) var<uniform> u: KaleidoUniform;

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

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let wedges = max(u.floats.x, 1.0);
    let arms = max(u.floats.y, 1.0);
    let twist = u.floats.z;
    let sharpness = max(u.floats.w, 0.5);
    let radius = max(u.floats2.x, 0.05);
    let rings = u.floats2.y;
    let spin = u.floats2.z;
    let time = u.floats2.w;
    let aspect = max(u.floats3.x, 0.01);

    // Centred, and corrected for the frame so a wedge is a wedge and not a
    // sheared one.
    let p = (in.uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(2.0 * aspect, 2.0);
    var r = length(p);
    var a = atan2(p.y, p.x) + time * spin;

    // --- the fold -----------------------------------------------------------
    // Wrap into one wedge, then mirror every other one so the seams meet as
    // reflections rather than as repeats — the difference between a
    // kaleidoscope and a pinwheel.
    let wedge = 6.28318530718 / wedges;
    a = a - floor(a / wedge) * wedge;
    if (a > wedge * 0.5) {
        a = wedge - a;
    }

    // The radial fold, which is what fills a wide frame's corners: without it
    // the field runs out at `radius` and everything past it is one flat
    // colour. `rings` at 0 leaves the plain single disc.
    if (rings > 0.5) {
        let period = radius / max(rings, 1.0);
        let band = floor(r / period);
        var rr = r - band * period;
        // Mirror alternate bands for the same reason the wedges mirror.
        if (fract(band * 0.5) > 0.25) {
            rr = period - rr;
        }
        r = rr * max(rings, 1.0);
    }

    // --- the arms -----------------------------------------------------------
    // `s` runs 0..1 out along an arm, exactly as it does in geoterrain.rs.
    let s = clamp(r / radius, 0.0, 1.0);
    let phase = (a - s * twist) * arms;
    // A smooth petal rather than a hairline: at any real output size a
    // one-pixel spiral folded twelve ways is a moiré, and the sharpness float
    // is what walks it back from a wash to an edge.
    var band = 0.5 + 0.5 * cos(phase);
    band = pow(band, sharpness);

    // Toward the rim the arms thin out the way Geoterrain's do (its half-width
    // is scaled by `s`), and a soft floor keeps the field from being pure
    // black between them — a kaleidoscope is a lit thing seen through glass.
    let falloff = 1.0 - 0.55 * s;
    let t = clamp(band * falloff + 0.10 * (1.0 - s), 0.0, 1.0);

    let col = grad4(u.color, t);
    return vec4<f32>(col * (0.12 + 0.88 * t), 1.0);
}
