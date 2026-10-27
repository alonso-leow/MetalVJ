// Model 11 — Oracle. One UV-sphere mesh, reused per eye instance;
// iris/pupil/eyelid are angular zones around a gaze direction computed in
// the fragment shader, not separate geometry. The gaze itself is a scripted
// saccade: snap to a new hashed target every tick, then hold — "moving
// like scanning the audience" rather than smoothly drifting.
//
// The lid is drawn in the same skin the pass is cleared to (see
// `oracle.rs`), so a blink closes an eye in a face rather than opening a
// black hole in one. The only thing the lid adds over flat skin is a crease
// shadow that fades out within a few degrees of the margin — at a full close
// that shadow is a single line and everything else is exactly the background.

struct OjoUniform {
    iris_color: ColorSlot,
    fondo_color: ColorSlot,
    brillo_color: ColorSlot,
    pupil_color: ColorSlot,
    piel_color: ColorSlot,
    floats: vec4<f32>,  // mirada_x, mirada_y, vel_escaneo, parpado
    floats2: vec4<f32>, // pupila, iris, n_ojos, time
    floats3: vec4<f32>, // pos_x, pos_y, tamano, giro
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: OjoUniform;

// Where the flat skin is sampled. Must match SKIN_T in oracle.rs, which is
// what the frame is cleared to.
const SKIN_T: f32 = 0.5;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) t: f32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local_dir: vec3<f32>,
};

@vertex
fn vs_main(in: VIn, @builtin(instance_index) ii: u32) -> VOut {
    let n = max(round(u.floats2.z), 1.0);
    let i = f32(ii);
    let spacing = 2.3 / n;
    let scale = clamp(1.5 / n, 0.22, 1.0) * max(u.floats3.z, 0.02);
    let offset_x = (i - (n - 1.0) * 0.5) * spacing;

    // The eye's own placement, on top of the row's spacing. Range either side
    // is a little over one frame width so an eye can leave and come back.
    let move_xy = (u.floats3.xy - vec2<f32>(0.5, 0.5)) * 3.0;

    // Roll about the view axis. Applied to the *geometry* and to the normal
    // alike, so the lids — which are zones on |dir.y| — close on the diagonal
    // the roll implies instead of staying level while the sphere turns.
    let a = u.floats3.w * 3.14159265;
    let ca = cos(a);
    let sa = sin(a);
    let rolled = vec3<f32>(in.pos.x * ca - in.pos.y * sa, in.pos.x * sa + in.pos.y * ca, in.pos.z);

    let world_pos = vec3<f32>(
        rolled.x * scale + offset_x + move_xy.x,
        rolled.y * scale + move_xy.y,
        rolled.z * scale,
    );

    var out: VOut;
    out.clip = u.view_proj * vec4<f32>(world_pos, 1.0);
    out.local_dir = in.normal;
    return out;
}

fn hash2(id: f32) -> vec2<f32> {
    return vec2<f32>(hash11(id * 17.3), hash11(id * 91.7 + 4.0));
}

fn saccade_target(tick: f32, bias: vec2<f32>) -> vec2<f32> {
    return bias + (hash2(tick) * 2.0 - 1.0) * 0.6;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let dir = normalize(in.local_dir);
    if (dir.z < 0.0) {
        discard;
    }

    let skin = grad4(u.piel_color, SKIN_T);

    let speed = max(u.floats.z, 0.05);
    let time = u.floats2.w;
    let tick = floor(time * speed);
    let frac = fract(time * speed);
    let bias = vec2<f32>(u.floats.x * 2.0 - 1.0, u.floats.y * 2.0 - 1.0) * 0.5;
    let prev_target = saccade_target(tick - 1.0, bias);
    let this_target = saccade_target(tick, bias);
    let ease = 1.0 - exp(-frac * 16.0);
    let gaze_xy = mix(prev_target, this_target, ease);
    let gaze_dir = normalize(vec3<f32>(gaze_xy.x, gaze_xy.y, 1.4));

    let eyelid = clamp(u.floats.w, 0.0, 1.0);
    let open_half = mix(0.03, 1.0, eyelid);
    if (abs(dir.y) > open_half) {
        // Flat skin, with a crease that dies out 0.06 rad past the margin.
        // `smoothstep` reaching 1.0 is what makes "the same as the
        // background" literal rather than approximate.
        let past = abs(dir.y) - open_half;
        let crease = mix(0.82, 1.0, smoothstep(0.0, 0.06, past));
        return vec4<f32>(skin * crease, 1.0);
    }

    let angle_cos = dot(dir, gaze_dir);
    let iris_r = mix(0.997, 0.90, clamp(u.floats2.y, 0.0, 1.0));
    let pupil_r = mix(0.9995, iris_r + (0.9995 - iris_r) * 0.05, 1.0 - clamp(u.floats2.x, 0.0, 1.0));

    let highlight_dir = normalize(gaze_dir + vec3<f32>(-0.22, 0.30, 0.05));
    let highlight = smoothstep(0.975, 0.995, dot(dir, highlight_dir));

    var col: vec3<f32>;
    if (angle_cos > pupil_r) {
        // The black of the eye. Its own slot now, walked from the centre out,
        // so "black" is a choice this model makes rather than one it imposes.
        let t = clamp((angle_cos - pupil_r) / max(1.0 - pupil_r, 1e-4), 0.0, 1.0);
        col = grad4(u.pupil_color, 1.0 - t);
    } else if (angle_cos > iris_r) {
        let t = clamp((angle_cos - iris_r) / max(pupil_r - iris_r, 1e-4), 0.0, 1.0);
        col = grad4(u.iris_color, t);
    } else {
        col = grad4(u.fondo_color, clamp(dir.y * 0.5 + 0.5, 0.0, 1.0));
    }
    col = mix(col, grad4(u.brillo_color, 0.5), highlight);

    return vec4<f32>(shade(dir, col), 1.0);
}
