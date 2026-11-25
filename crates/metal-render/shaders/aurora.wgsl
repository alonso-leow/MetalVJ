// Aurora — a 3D kaleido mesh of vibrating strings (2026-11-25 rework).
//
// The seven band floats are still the seven strings' vibration signals, and
// `palette` still walks warm (string 0) to cool (string 6). What changed:
//
// - Kaleido: the plane is folded into `sectors` mirrored wedges, the same
//   centred symmetry the kaleido fx uses, so the two stack cleanly. Each
//   string is a segment from one wedge edge to the other, at its own radius
//   on each edge, so every string runs in its own direction inside the wedge,
//   and its mirror image meets it exactly on the edge: same-colour strings
//   touch, and together they close into star rings.
// - Physics: a string is fixed at both ends. It vibrates as a standing wave
//   (`sin(pi * s * mode)`) whose amplitude is its band and whose mode and
//   frequency rise with `tension`; two modes beat against each other, so it
//   never looks like a single sine.
// - Depth: `depth_rows` layers of the mesh stand at increasing depth and fly
//   toward the camera; a layer that passes the camera is reborn at the back,
//   so the tunnel repeats forever. Each layer turns by `twist`, and fog plus
//   the haze sky swallow the far ones.
// - Camera: it sways (`cam_orbit`), lifts (`cam_height`) and dollies
//   (`cam_dolly`), always looking down the tunnel's axis.

struct AuroraUniform {
    palette: ColorSlot,
    // aspect, time, speed, separation
    a: vec4<f32>,
    // width_shine, energy, impact, emphasis_band
    b: vec4<f32>,
    // emphasis_force, band_0, band_1, band_2
    c: vec4<f32>,
    // band_3, band_4, band_5, band_6
    d: vec4<f32>,
    // haze, sharpness, tension, _
    e: vec4<f32>,
    // sectors, depth_rows, depth_gap, fly
    f: vec4<f32>,
    // twist, sway, lift, dolly
    g: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: AuroraUniform;

const NUM_STRINGS: i32 = 7;
const MAX_ROWS: i32 = 8;
const PI: f32 = 3.14159265;

fn band_val(i: i32) -> f32 {
    if (i == 0) { return u.c.y; }
    if (i == 1) { return u.c.z; }
    if (i == 2) { return u.c.w; }
    if (i == 3) { return u.d.x; }
    if (i == 4) { return u.d.y; }
    if (i == 5) { return u.d.z; }
    return u.d.w;
}

fn h11(x: f32) -> f32 {
    return fract(sin(x * 127.1 + 31.7) * 43758.5453);
}

fn rot2(v: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a); let s = sin(a);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

// The light of every string at a point of one mesh layer (layer seed `ls`).
fn mesh(q: vec2<f32>, ls: f32, t: f32) -> vec3<f32> {
    let n = max(round(u.f.x), 2.0);
    let w = PI / n;
    // Mirror fold into one wedge [0, w].
    let r = length(q);
    var a = atan2(q.y, q.x);
    a = a - 2.0 * w * floor(a / (2.0 * w));
    if (a > w) { a = 2.0 * w - a; }
    let p = vec2<f32>(cos(a), sin(a)) * r;
    let edge_b = vec2<f32>(cos(w), sin(w));

    let speed = u.a.z;
    let separation = u.a.w;
    let width_shine = u.b.x;
    let energy = u.b.y;
    let emphasis_band = u.b.w;
    let emphasis_force = u.c.x;
    let sharpness = u.e.y;
    let tension = clamp(u.e.z, 0.0, 1.0);

    var col = vec3<f32>(0.0);
    for (var i = 0; i < NUM_STRINGS; i = i + 1) {
        let fi = f32(i);
        let amp_b = band_val(i);
        // Its own direction: the radius on each wedge edge differs per string
        // (and per layer), so no two strings in a wedge are parallel.
        let ra = (0.18 + fi * 0.1 * separation) * (0.85 + 0.3 * h11(fi + ls * 7.3));
        let rb = ra * (0.55 + 0.9 * h11(fi * 3.1 + 11.0 + ls * 2.9));
        let A = vec2<f32>(ra, 0.0);
        let B = edge_b * rb;
        let ab = B - A;
        let L = max(length(ab), 1e-3);
        let dir = ab / L;
        let sr = dot(p - A, dir) / L;
        let s = clamp(sr, 0.0, 1.0);
        let perp = dir.x * (p.y - A.y) - dir.y * (p.x - A.x);
        // Standing wave, fixed ends: two modes beating.
        let m1 = 1.0 + floor(tension * 3.0) + f32(i % 2);
        let f1 = (2.0 + tension * 6.0 + fi * 0.7) * speed;
        let vib = (0.006 + amp_b * 0.05) * L
                * (sin(PI * s * m1) * sin(t * f1 + fi * 1.3 + ls)
                   + 0.45 * sin(PI * s * (m1 + 1.0)) * sin(t * f1 * 1.618 + fi));
        let dy = perp - vib;
        // Outside the segment the distance runs to the end point.
        let along = (sr - s) * L;
        let dd = sqrt(dy * dy + along * along);

        let dist_i = fi - emphasis_band;
        let highlight = 1.0 + emphasis_force * exp(-dist_i * dist_i * 0.9);
        let width = width_shine * 0.6 * (1.0 + amp_b * 1.5) / highlight;
        let glow = exp(-pow(dd * dd / (width * width), mix(1.0, 1.6, sharpness)));
        let brightness = (0.25 + amp_b) * (1.2 + energy * 0.6) * highlight;
        col += grad4(u.palette, fi / f32(NUM_STRINGS - 1)) * brightness * glow;
    }
    return col;
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let energy = u.b.y;
    let impact = u.b.z;
    let haze = u.e.x;
    let rows = i32(clamp(round(u.f.y), 1.0, f32(MAX_ROWS)));
    let gap = max(u.f.z, 0.2);
    let fly = u.f.w;
    let twist = u.g.x;

    let uv = in.uv;
    let ndc = (uv - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0) * 2.0;

    // Camera: sway on a slow Lissajous, lifted and dollied, looking down -z
    // at a point deep in the tunnel so the axis stays near the centre.
    let co = u.g.y;
    let eye = vec3<f32>(0.35 * sin(t * co), 0.25 * sin(t * co * 0.7 + 1.0) + u.g.z * 0.8, u.g.w);
    let look = vec3<f32>(0.0, 0.0, -6.0);
    let fwd = normalize(look - eye);
    let right = normalize(cross(fwd, vec3<f32>(0.0, 1.0, 0.0)));
    let up = cross(right, fwd);
    let rd = normalize(fwd + right * ndc.x * 0.6 + up * ndc.y * 0.6);

    var col = vec3<f32>(0.0);
    let travel = t * fly;
    let base = floor(travel);
    let fr = travel - base;
    if (rd.z < -1e-4) {
        for (var k = 0; k < MAX_ROWS; k = k + 1) {
            if (k >= rows) { break; }
            // Layer k sits (k + 1 - fr) gaps in front of the dolly origin;
            // its seed is its global index, so a reborn layer keeps its look.
            let zk = (f32(k) + 1.0 - fr) * gap;
            let pz = -zk;
            let tt = (pz - eye.z) / rd.z;
            if (tt <= 0.0) { continue; }
            let P = eye + rd * tt;
            let kg = f32(k) - base;
            let q = rot2(P.xy, kg * twist * PI);
            // Fade in at the back, out as it reaches the camera.
            let front = smoothstep(0.0, 0.6 * gap, zk);
            let back = 1.0 - smoothstep((f32(rows) - 0.6) * gap, f32(rows) * gap, zk);
            let fog = exp(-zk * 0.18);
            col += mesh(q, kg, t) * front * back * fog;
        }
    }

    // Shimmer on the light already there.
    let lit = clamp(dot(col, vec3<f32>(0.3333)), 0.0, 1.0);
    let shimmer = (quintic_noise(vec2<f32>(ndc.x * 5.0 + t * 0.7, ndc.y * 9.0 - t * 0.3)) - 0.5) * 0.12 * energy * lit;
    col += vec3<f32>(shimmer);

    // Haze: the infinite tunnel's far glow, palette tinted, brightest at the
    // vanishing point, so the background is never a flat zero.
    let vp = length(ndc * 0.5);
    let sky_col = mix(grad4(u.palette, 1.0), grad4(u.palette, 0.0), clamp(vp, 0.0, 1.0));
    col += sky_col * haze * (0.015 + 0.06 * exp(-vp * 3.0)) * (0.6 + 0.4 * energy);

    col *= (1.0 + impact * 0.8);

    // Triangular-PDF dither against 8-bit banding in the glow falloffs.
    let px = in.pos.xy;
    let n1 = fract(52.9829189 * fract(dot(px, vec2<f32>(0.06711056, 0.00583715))));
    let n2 = fract(52.9829189 * fract(dot(px + vec2<f32>(37.0, 17.0), vec2<f32>(0.06711056, 0.00583715))));
    let tri = (n1 + n2 - 1.0) / 255.0;
    let gg = pow(max(col, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2)) + vec3<f32>(tri);
    col = pow(max(gg, vec3<f32>(0.0)), vec3<f32>(2.2));

    return vec4<f32>(col, 1.0);
}
