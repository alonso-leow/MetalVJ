// turtle — one flat 2D sea turtle, swimming, driven by the ingest.
//
// A `forge` generator (see `crates/metal-render/src/models/forge.rs` for the
// contract). It draws on black, so it layers over the water with
// `blend = "screen"`: black adds nothing, the turtle shows.
//
// What the ingest does to it:
//   rms       — louder swims faster and a little bigger;
//   onset     — a hit is a flipper stroke and a puff of bubbles;
//   centroid  — a brighter sound lightens the shell;
//   chroma    — the loudest pitch class tints the shell's plates.

struct Floats {
    floats: vec4<f32>,   // x time (s), y aspect (w/h)
    floats2: vec4<f32>,  // rms, bandwidth, onset, centroid
    floats3: vec4<f32>,  // chroma C  C#  D   D#
    floats4: vec4<f32>,  // chroma E  F   F#  G
    floats5: vec4<f32>,  // chroma G# A   A#  B
};
@group(0) @binding(0) var<uniform> u: Floats;

fn rot(p: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * p.x - s * p.y, s * p.x + c * p.y);
}

// Distance to an ellipse, approximate but monotone: negative inside.
fn ellipse(p: vec2<f32>, r: vec2<f32>) -> f32 {
    return (length(p / r) - 1.0) * min(r.x, r.y);
}

fn fill(d: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(-aa, aa, d);
}

// Where the turtle is at swim-time `s`: a slow figure eight, in frame-height
// units (the caller stretches x by the aspect).
fn path(s: f32) -> vec2<f32> {
    return vec2<f32>(0.34 * sin(s * 0.23), 0.17 * sin(s * 0.46));
}

// A flipper hinged at `root`, reaching outward (+y), swept by `angle`
// (negative sweeps it back toward the tail).
fn flipper(q: vec2<f32>, root: vec2<f32>, angle: f32, r: vec2<f32>) -> f32 {
    let local = rot(q - root, -angle);
    return ellipse(local - vec2<f32>(0.0, r.y * 0.85), r);
}

// The pitch class the ingest hears loudest, as a hue in 0..1.
fn chroma_hue() -> f32 {
    let c = array<f32, 12>(
        u.floats3.x, u.floats3.y, u.floats3.z, u.floats3.w,
        u.floats4.x, u.floats4.y, u.floats4.z, u.floats4.w,
        u.floats5.x, u.floats5.y, u.floats5.z, u.floats5.w,
    );
    var best = 0;
    var loud = -1.0;
    for (var i: i32 = 0; i < 12; i = i + 1) {
        if (c[i] > loud) {
            loud = c[i];
            best = i;
        }
    }
    return f32(best) / 12.0;
}

fn hue(h: f32) -> vec3<f32> {
    return clamp(abs(fract(h + vec3<f32>(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let t = u.floats.x;
    let aspect = max(u.floats.y, 0.01);
    let rms = clamp(u.floats2.x, 0.0, 1.0);
    let onset = clamp(u.floats2.z, 0.0, 1.0);
    let centroid = clamp(u.floats2.w, 0.0, 1.0);

    // Frame space: y up, height 1, x across the aspect.
    let p = (in.uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, -1.0);

    // Louder swims faster; the heading follows the path.
    let s = t * (0.55 + 1.1 * rms);
    let stretch = vec2<f32>(aspect * 0.9, 1.0);
    let here = path(s) * stretch;
    let ahead = path(s + 0.08) * stretch;
    let heading = atan2(ahead.y - here.y, ahead.x - here.x);
    let size = 0.13 * (1.0 + 0.25 * rms);

    // Turtle space: +x forward, one unit is `size`.
    let q = rot(p - here, -heading) / size;
    let aa = max(fwidth(q.x), 0.002);

    // Flippers: the front pair long and swept back, the rear pair short. A
    // hit adds a stroke on top of the steady paddle.
    let stroke = 0.45 * sin(s * 5.0) + 0.5 * onset;
    let m = vec2<f32>(q.x, -q.y);
    let front = min(
        flipper(q, vec2<f32>(0.35, 0.42), -0.9 + stroke, vec2<f32>(0.16, 0.55)),
        flipper(m, vec2<f32>(0.35, 0.42), -0.9 + stroke, vec2<f32>(0.16, 0.55)),
    );
    let rear = min(
        flipper(q, vec2<f32>(-0.55, 0.30), -2.2 - 0.4 * stroke, vec2<f32>(0.10, 0.26)),
        flipper(m, vec2<f32>(-0.55, 0.30), -2.2 - 0.4 * stroke, vec2<f32>(0.10, 0.26)),
    );
    let head = ellipse(q - vec2<f32>(0.98 + 0.03 * sin(s * 2.0), 0.0), vec2<f32>(0.24, 0.19));
    let tail = ellipse(q - vec2<f32>(-0.95, 0.0), vec2<f32>(0.14, 0.05));
    let skin_d = min(min(front, rear), min(head, tail));
    let shell = ellipse(q, vec2<f32>(0.78, 0.58));

    // Flat colours: an outline, the skin, the shell and its plates.
    let outline = vec3<f32>(0.05, 0.09, 0.07);
    let skin = vec3<f32>(0.60, 0.72, 0.44);
    let shell_base = mix(vec3<f32>(0.30, 0.40, 0.18), vec3<f32>(0.55, 0.60, 0.30), centroid);
    let plate_tint = mix(shell_base, hue(chroma_hue()) * 0.6 + shell_base * 0.4, 0.35);

    var col = vec3<f32>(0.0);
    col = mix(col, outline, fill(skin_d - 0.035, aa));
    col = mix(col, skin, fill(skin_d, aa));
    let eyes = min(length(q - vec2<f32>(1.08, 0.09)), length(q - vec2<f32>(1.08, -0.09))) - 0.035;
    col = mix(col, outline, fill(eyes, aa));

    // Plates: a brick of rounded cells, darker seams, a darker rim.
    let g = q * vec2<f32>(3.2, 3.6);
    let cell = abs(fract(g + vec2<f32>(0.5 * floor(g.y), 0.0)) - vec2<f32>(0.5, 0.5));
    let seam = smoothstep(0.40, 0.47, max(cell.x, cell.y));
    let rim = smoothstep(-0.14, -0.05, shell);
    let shell_col = plate_tint * (1.0 - 0.35 * seam) * (1.0 - 0.3 * rim);
    col = mix(col, outline, fill(shell - 0.035, aa));
    col = mix(col, shell_col, fill(shell, aa));

    // Bubbles: three rings rising from the head, brighter on a hit.
    let nose = here + rot(vec2<f32>(1.1 * size, 0.0), heading);
    for (var i: i32 = 0; i < 3; i = i + 1) {
        let fi = f32(i);
        let phase = fract(t * 0.6 + fi / 3.0);
        let at = nose + vec2<f32>(0.015 * sin(t * 3.0 + fi * 2.0), phase * 0.28);
        let r = 0.008 + 0.012 * onset + 0.006 * fi;
        let ring = abs(length(p - at) - r) - 0.0025;
        let a = (1.0 - phase) * (0.35 + 0.65 * onset);
        col = col + vec3<f32>(0.55, 0.85, 1.0) * a * (1.0 - smoothstep(0.0, 0.003, ring));
    }

    return vec4<f32>(col, 1.0);
}
