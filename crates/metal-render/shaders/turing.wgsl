// Model 12 — Turing. Gray-Scott reaction-diffusion running on a torus,
// carried along by a flow field, read back as the height of a 3D sheet.
//
// The pattern is where the creases come from: v is high along the ridges the
// reaction grows, so displacing the mesh by v turns a chemical pattern into
// folded cloth. The flow is what makes them *move* — the field is advected
// every step, so a ridge travels rather than sitting still, and its speed and
// heading are two floats (`flujo_fuerza`, `flujo_dir`) with a curl term
// (`flujo_giro`) so the travel swirls instead of sliding rigidly.
//
// Unlike the other eleven this one carries state between frames: a reaction
// is an integrator, and re-deriving one from t=0 every frame would cost
// thousands of dispatches per frame. What it keeps instead is the invariant
// that matters — asking for the same instant twice advances nothing, so a
// preset still reproduces. See `models/turing.rs` for the stepping rule.

struct TurUniform {
    patron: ColorSlot,
    fold: ColorSlot,
    view_proj: mat4x4<f32>,
    f0: vec4<f32>, // feed, mata, dif_u, dif_v
    f1: vec4<f32>, // flujo_fuerza, flujo_dir, flujo_giro, escala
    f2: vec4<f32>, // pliegue, filo, ola_alto, ola_frec
    f3: vec4<f32>, // ola_vel, luz, brillo, time
    f4: vec4<f32>, // semilla, field_size, mesh_n, unused
};

@group(0) @binding(0) var<uniform> u: TurUniform;
//@compute
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
//@end-compute
@group(0) @binding(2) var src: texture_2d<f32>;

const EXTENT: f32 = 8.0;   // half-width of the sheet in world units
const PLANE_Y: f32 = 1.0;  // sits at the shared camera block's default target
const DT: f32 = 1.0;

// --- the field ------------------------------------------------------------

// Wrapped fetch. The domain is a torus so the flow can carry the pattern for
// ever without it piling up against an edge.
fn load_uv(c: vec2<i32>, n: i32) -> vec2<f32> {
    let w = (c + vec2<i32>(n, n)) % vec2<i32>(n, n);
    return textureLoad(src, w, 0).xy;
}

// Bilinear, because advection lands between texels and nearest-neighbour
// there turns a smooth drift into visible stair-stepping.
fn sample_uv(p: vec2<f32>, n: i32) -> vec2<f32> {
    let q = p - vec2<f32>(0.5, 0.5);
    let i = floor(q);
    let f = q - i;
    let c = vec2<i32>(i);
    let a = load_uv(c, n);
    let b = load_uv(c + vec2<i32>(1, 0), n);
    let d = load_uv(c + vec2<i32>(0, 1), n);
    let e = load_uv(c + vec2<i32>(1, 1), n);
    return mix(mix(a, b, f.x), mix(d, e, f.x), f.y);
}

// A steady heading plus the curl of a noise field. Curl rather than the noise
// itself because curl has no sources or sinks: the pattern gets stirred, not
// squeezed into and out of existence.
fn flow_at(uv: vec2<f32>, time: f32) -> vec2<f32> {
    let ang = u.f1.y * 6.2831853;
    let steady = vec2<f32>(cos(ang), sin(ang));

    let e = 0.01;
    let s = 2.6;
    let t = time * 0.05;
    let px = fbm((uv + vec2<f32>(e, 0.0)) * s + t, 3);
    let nx = fbm((uv - vec2<f32>(e, 0.0)) * s + t, 3);
    let py = fbm((uv + vec2<f32>(0.0, e)) * s + t, 3);
    let ny = fbm((uv - vec2<f32>(0.0, e)) * s + t, 3);
    let curl = vec2<f32>(py - ny, nx - px) / (2.0 * e);

    return steady + curl * clamp(u.f1.z, 0.0, 1.0) * 0.06;
}

// The seed and the step are plain functions of a texel, so the compute entry
// points and the fragment ping-pong (turing_pp.wgsl, for a device without
// compute) run the same code and cannot drift apart.
fn seed_value(c: vec2<i32>) -> vec4<f32> {
    let n = i32(u.f4.y);
    let uv = (vec2<f32>(c) + 0.5) / f32(n);
    let s = u.f4.x;

    // Scattered patches rather than one central blob: the reaction spreads
    // from every seed at once, so a few hundred burn-in steps is enough to
    // have a full pattern the first time you look at it.
    let patches = fbm(uv * 6.0 + s * 37.0, 3);
    let grain = hash21(uv * 512.0 + s * 13.0);
    let v = select(0.0, 0.26 + 0.10 * grain, patches > 0.56);
    return vec4<f32>(clamp(1.0 - 2.0 * v, 0.0, 1.0), v, 0.0, 1.0);
}

//@compute
@compute @workgroup_size(8, 8, 1)
fn cs_seed(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = i32(u.f4.y);
    if (i32(gid.x) >= n || i32(gid.y) >= n) {
        return;
    }
    textureStore(dst, vec2<i32>(gid.xy), seed_value(vec2<i32>(gid.xy)));
}
//@end-compute

fn step_value(cell: vec2<i32>) -> vec4<f32> {
    let n = i32(u.f4.y);
    let uv = (vec2<f32>(cell) + 0.5) / f32(n);

    // Semi-Lagrangian: read the field from where it came from. The whole
    // stencil is offset by the same velocity, which treats the flow as locally
    // constant — true at these speeds, and it keeps the read count at nine.
    let speed = flow_at(uv, u.f3.w) * clamp(u.f1.x, 0.0, 1.0) * 0.25;
    let base = vec2<f32>(cell) + vec2<f32>(0.5, 0.5) - speed;

    let c  = sample_uv(base, n);
    let l  = sample_uv(base + vec2<f32>(-1.0,  0.0), n);
    let r  = sample_uv(base + vec2<f32>( 1.0,  0.0), n);
    let d  = sample_uv(base + vec2<f32>( 0.0, -1.0), n);
    let up = sample_uv(base + vec2<f32>( 0.0,  1.0), n);
    let dl = sample_uv(base + vec2<f32>(-1.0, -1.0), n);
    let dr = sample_uv(base + vec2<f32>( 1.0, -1.0), n);
    let ul = sample_uv(base + vec2<f32>(-1.0,  1.0), n);
    let ur = sample_uv(base + vec2<f32>( 1.0,  1.0), n);

    // The nine-point stencil, not five: five is anisotropic and grows ridges
    // that line up with the texel axes, which on a sheet reads as a grid.
    let lap = (l + r + d + up) * 0.2 + (dl + dr + ul + ur) * 0.05 - c;

    let feed = u.f0.x;
    let kill = u.f0.y;
    let du = u.f0.z;
    let dv = u.f0.w;
    let reaction = c.x * c.y * c.y;

    let nu = c.x + (du * lap.x - reaction + feed * (1.0 - c.x)) * DT;
    let nv = c.y + (dv * lap.y + reaction - (feed + kill) * c.y) * DT;

    return vec4<f32>(clamp(nu, 0.0, 1.0), clamp(nv, 0.0, 1.0), 0.0, 1.0);
}

//@compute
@compute @workgroup_size(8, 8, 1)
fn cs_step(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = i32(u.f4.y);
    if (i32(gid.x) >= n || i32(gid.y) >= n) {
        return;
    }
    textureStore(dst, vec2<i32>(gid.xy), step_value(vec2<i32>(gid.xy)));
}
//@end-compute

// --- the sheet ------------------------------------------------------------

// A ridge in the raw field is two or three texels of near-full concentration
// with nothing either side, so a height taken straight from it steps from
// floor to ceiling in one vertex and the relief comes out as vertical towers
// rather than creases. Two things fix that together: the field is coarser
// than the mesh, so `sample_uv` interpolates between texels instead of
// stepping, and a five-tap box on top of that rounds the shoulders. Both wrap,
// so the sheet stays seamless across the torus.
fn field_smooth(g: vec2<f32>) -> f32 {
    let fs = i32(u.f4.y);
    let tile = max(1.0, round(u.f1.w));
    let p = fract(g * tile) * f32(fs);
    let c = sample_uv(p, fs).y;
    let a = sample_uv(p + vec2<f32>( 1.0,  1.0), fs).y;
    let b = sample_uv(p + vec2<f32>(-1.0,  1.0), fs).y;
    let d = sample_uv(p + vec2<f32>( 1.0, -1.0), fs).y;
    let e = sample_uv(p + vec2<f32>(-1.0, -1.0), fs).y;
    return c * 0.4 + (a + b + d + e) * 0.15;
}

// The slow swell under the creases: the sheet as a whole breathes, so the
// pattern travels over a surface that is itself moving.
fn swell(g: vec2<f32>, time: f32) -> f32 {
    let f = u.f2.w * 6.2831853;
    let s = time * u.f3.x;
    return u.f2.z * 0.35
        * (sin(g.x * f + s) * 0.6 + sin((g.x * 0.7 + g.y * 1.3) * f - s * 0.8) * 0.4);
}

fn ridge(g: vec2<f32>) -> f32 {
    let v = field_smooth(g);
    let t = clamp(v / 0.30, 0.0, 1.0);
    // `filo` slides between the raw concentration (soft dunes) and a
    // contrast-stretched one (hard folds), which is the same pattern read at
    // two sharpnesses rather than two different patterns.
    return mix(t, smoothstep(0.30, 0.72, t), clamp(u.f2.y, 0.0, 1.0));
}

fn height(g: vec2<f32>, time: f32) -> f32 {
    return u.f2.x * (ridge(g) - 0.5) * 0.40 + swell(g, time);
}

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) t: f32,
    @location(2) slope: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    let n = i32(u.f4.z);
    let ix = i32(vi) % n;
    let iz = i32(vi) / n;
    let g = vec2<f32>(f32(ix), f32(iz)) / f32(n - 1);
    let time = u.f3.w;

    let e = 1.0 / f32(n - 1);
    let h = height(g, time);
    let hx = height(g + vec2<f32>(e, 0.0), time);
    let hz = height(g + vec2<f32>(0.0, e), time);

    let step_world = e * 2.0 * EXTENT;
    let tangent_x = vec3<f32>(step_world, hx - h, 0.0);
    let tangent_z = vec3<f32>(0.0, hz - h, step_world);
    let nrm = normalize(cross(tangent_z, tangent_x));

    let world = vec3<f32>((g.x * 2.0 - 1.0) * EXTENT, PLANE_Y + h, (g.y * 2.0 - 1.0) * EXTENT);

    var out: VOut;
    out.clip = u.view_proj * vec4<f32>(world, 1.0);
    out.normal = nrm;
    out.t = ridge(g);
    out.slope = clamp(length(vec2<f32>(hx - h, hz - h)) / (step_world * 0.5), 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    // Two slots, one job each: the pattern colours the field, the second slot
    // paints only what is steep. Set them to the same four colours and the
    // creases stop reading as a separate material.
    let base = grad4(u.patron, in.t);
    let edge = grad4(u.fold, in.slope);
    var col = mix(base, edge, in.slope * 0.85);

    col = shade(in.normal, col) * (0.55 + clamp(u.f3.y, 0.0, 1.0));

    let light = normalize(vec3<f32>(0.45, 0.8, 0.35));
    let spec = pow(max(dot(normalize(in.normal), light), 0.0), 24.0) * clamp(u.f3.z, 0.0, 1.0);
    return vec4<f32>(col + vec3<f32>(spec, spec, spec), 1.0);
}
