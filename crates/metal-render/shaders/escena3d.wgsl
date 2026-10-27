// Model — Scene 3D (`escena3d`), the template a future AI harness reads before
// writing its own scene (the design notes §H.1). Raymarched SDFs, one
// object instanced across a finite grid of tiles, so `create_node` can later
// swap just the SDF and keep the camera/grid/background machinery that
// surrounds it. Every float below is commented with *what it drives* and *how
// it is used*, because that comment is the first-pass way an AI reads a shader
// — see H.4's `read_shader` tool.

// One uniform, one bind group, like every fullscreen model in this suite —
// `shared::fullscreen::Fullscreen` prepends `VERTEX` (gives us `FsVOut`) and
// `shared::PRELUDE` (gives us `ColorSlot`, hashes, etc) ahead of this file.
struct Escena3dUniform {
    // aspect, time (seconds), fov (radians, half-angle already folded into
    // the tangent on the CPU side is NOT done — this is the raw field of
    // view in radians, tan() happens in the shader), unused.
    f0: vec4<f32>,
    // Camera eye position in world space, already orbited around the scene
    // and offset by cam.track (the design notes §13.2) on the CPU side.
    eye: vec4<f32>,
    // Camera forward/right/up, an orthonormal basis built on the CPU with
    // glam's look-at — cheaper and less error-prone than redoing it here.
    fwd: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    // rot_x, rot_y, rot_z — angular *speed* in radians/second around each
    // axis of the object every tile instances. Multiplied by `f0.y` (time)
    // in `map()` below, so a slider sets a spin rate, not a fixed pose.
    rot: vec4<f32>,
    // grid_cols, grid_rows, grid_gap, grid_scale — the tiling. `grid_cols` /
    // `grid_rows` are how many tiles the finite grid has along x and z;
    // `grid_scale` is each tile's object half-size; `grid_gap` is the extra
    // space between tiles on top of `grid_scale * 2`. This is a property of
    // the *model*, not the scene — every tile instances the same SDF, which
    // is what lets "a different mask per tile" mean anything later.
    grid: vec4<f32>,
    // bg_a, bg_b, bg_mix, fondo_alpha (0.0/1.0). bg_a/bg_b are the two tones
    // of the backdrop the grid floats over; bg_mix walks a vertical gradient
    // between them. `fondo_alpha` mirrors `Ctx::fondo_alpha` — when it is 1,
    // background pixels write alpha 0 instead of a colour, because
    // `Model::backdrop() == true` here (the design notes PARTE IV §17.3).
    bg: vec4<f32>,
    // metal, rough, light_dir, unused. `metal` blends the surface from a
    // matte diffuse look towards a mirror-ish specular one; `rough` widens
    // or narrows the specular highlight; `light_dir` is a single float
    // azimuth (0..1 turns) the light orbits the scene at, at a fixed
    // elevation — one float because a full light vector is three sliders for
    // one knob's worth of "where the light is coming from".
    mat: vec4<f32>,
    // The `body` part's picture and `mat_*`, one `MaterialU` — see
    // material.wgsl, prepended ahead of this file.
    mat_body: MaterialU,
};

@group(0) @binding(0) var<uniform> u: Escena3dUniform;
@group(0) @binding(1) var surf_body_tex: texture_2d<f32>;
@group(0) @binding(2) var surf_body_samp: sampler;

const TAU: f32 = 6.28318530718;
const MAX_STEPS: i32 = 96;
const MAX_DIST: f32 = 40.0;
const SURF_EPS: f32 = 0.0015;

// --- rotation ------------------------------------------------------------

fn rot_x(p: vec3<f32>, a: f32) -> vec3<f32> {
    let s = sin(a); let c = cos(a);
    return vec3<f32>(p.x, c * p.y - s * p.z, s * p.y + c * p.z);
}
fn rot_y(p: vec3<f32>, a: f32) -> vec3<f32> {
    let s = sin(a); let c = cos(a);
    return vec3<f32>(c * p.x + s * p.z, p.y, -s * p.x + c * p.z);
}
fn rot_z(p: vec3<f32>, a: f32) -> vec3<f32> {
    let s = sin(a); let c = cos(a);
    return vec3<f32>(c * p.x - s * p.y, s * p.x + c * p.y, p.z);
}

// The one object every tile instances. A box, on purpose — the simplest SDF
// that already reads as "a 3D object" and the cheapest to keep readable next
// to the grid/camera plumbing around it. This is the block a `create_node`
// with its own shader replaces; nothing else in this file needs to know it
// changed.
fn sd_object(p: vec3<f32>, half_extent: f32) -> f32 {
    let q = abs(p) - vec3<f32>(half_extent);
    return length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0);
}

// The whole scene as one distance field: find the nearest tile centre to
// `p`, clamp it to the finite `grid_cols` x `grid_rows` extent, recentre `p`
// on that tile, spin it by `rot`, and evaluate the shared object SDF there.
fn map(p: vec3<f32>) -> f32 {
    let cols = max(round(u.grid.x), 1.0);
    let rows = max(round(u.grid.y), 1.0);
    let half_extent = max(u.grid.w, 0.02);
    let spacing = half_extent * 2.0 + max(u.grid.z, 0.0);

    // Nearest tile index along each axis, clamped to the finite grid so the
    // field is a grid of `cols x rows` objects, not an infinite tiling.
    let ix = clamp(round(p.x / spacing), -(cols - 1.0) * 0.5, (cols - 1.0) * 0.5);
    let iz = clamp(round(p.z / spacing), -(rows - 1.0) * 0.5, (rows - 1.0) * 0.5);

    var local = p - vec3<f32>(ix * spacing, 0.0, iz * spacing);

    // Every tile spins together — same SDF, same rotation, per H.1: the grid
    // is one object repeated, not one object per tile.
    let t = u.f0.y;
    local = rot_x(local, u.rot.x * t);
    local = rot_y(local, u.rot.y * t);
    local = rot_z(local, u.rot.z * t);

    return sd_object(local, half_extent);
}

// Central-difference normal off `map()` — six taps, cheap enough at this
// step count and the plain way to shade an implicit surface.
fn calc_normal(p: vec3<f32>) -> vec3<f32> {
    let e = vec2<f32>(0.001, 0.0);
    return normalize(vec3<f32>(
        map(p + e.xyy) - map(p - e.xyy),
        map(p + e.yxy) - map(p - e.yxy),
        map(p + e.yyx) - map(p - e.yyx),
    ));
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.f0.x, 0.01);
    let fov = clamp(u.f0.z, 0.1, 3.0);

    // uv is 0..1 with y down (from `shared::fullscreen::VERTEX`); recentre
    // to -1..1 with y up before building the ray.
    let px = (in.uv.x * 2.0 - 1.0);
    let py = 1.0 - in.uv.y * 2.0;
    let half_h = tan(fov * 0.5);
    let half_w = half_h * aspect;

    let eye = u.eye.xyz;
    let dir = normalize(u.fwd.xyz + u.right.xyz * (px * half_w) + u.up.xyz * (py * half_h));

    // The background every miss falls back to: a two-tone vertical gradient,
    // `bg_mix` sliding which tone dominates. Grayscale on purpose — this is
    // a template's placeholder sky, coloured for real by the effects chain
    // (`EFFECTS_COLOR_BLOCK`) that every model has appended after it.
    let sky_t = clamp(u.bg.z + py * 0.35, 0.0, 1.0);
    let sky = mix(vec3<f32>(u.bg.x), vec3<f32>(u.bg.y), sky_t);
    let fondo_alpha = u.bg.w > 0.5;

    var t = 0.0;
    var hit = false;
    for (var i = 0; i < MAX_STEPS; i = i + 1) {
        let p = eye + dir * t;
        let d = map(p);
        if (d < SURF_EPS) {
            hit = true;
            break;
        }
        t = t + d;
        if (t > MAX_DIST) {
            break;
        }
    }

    if (!hit) {
        // Backdrop() == true means this model owns its own background, so
        // it is the one that has to answer `fondo_alpha` itself rather than
        // relying on `shared::backdrop::Backdrop` — see `Ctx::fondo_alpha`.
        let a = select(1.0, 0.0, fondo_alpha);
        return vec4<f32>(sky, a);
    }

    let p = eye + dir * t;
    let view = normalize(eye - p);

    // The shared material, off at mix 0 (a migrated row): the picture and the
    // `mat_*` kind replace the template's plain grey, its bump and glitch
    // perturb the normal, and a kind overrides `metal`/`rough` when it is on.
    let shaded = material_apply(
        u.mat_body, surf_body_tex, surf_body_samp, vec3<f32>(0.72), p, calc_normal(p), in.uv, view, u.f0.y,
    );
    let n = shaded.normal;
    let base = shaded.albedo;

    // Light orbits the scene at a fixed elevation; `light_dir` (0..1 turns)
    // is the only free axis, so it reads as one dial instead of three.
    let light_turn = u.mat.z * TAU;
    let light = normalize(vec3<f32>(cos(light_turn), 0.65, sin(light_turn)));
    let half_vec = normalize(light + view);

    var metal = clamp(u.mat.x, 0.0, 1.0);
    var rough = clamp(u.mat.y, 0.02, 1.0);
    if (u.mat_body.b.x > 0.0) {
        metal = clamp(shaded.metalness, 0.0, 1.0);
        rough = clamp(shaded.roughness, 0.02, 1.0);
    }

    // Plain Blinn-ish split: `metal` trades diffuse response for a tinted
    // specular, `rough` narrows or widens the highlight. A base tint of
    // light gray keeps the object legible against either background tone
    // without a colour slot of its own — the template has none to spend.
    let ndotl = max(dot(n, light), 0.0);
    let diffuse = base * ndotl * mix(1.0, 0.15, metal);
    let spec_power = mix(4.0, 160.0, 1.0 - rough);
    let spec_strength = mix(0.08, 1.0, metal);
    let specular = vec3<f32>(pow(max(dot(n, half_vec), 0.0), spec_power)) * spec_strength;
    let ambient = base * 0.18;

    let color = ambient + diffuse + specular;
    return vec4<f32>(color, 1.0);
}
