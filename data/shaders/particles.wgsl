// `fx.particles` — stateless dust, hashed from `(seed, i, t)`.
//
// No simulation buffer (decision 6): the picture at time `t` is a pure
// function of the ingest and the clock, so fan-out and scrubbing cannot put
// two screens out of step and the goldens have one frame to freeze. The CPU
// stand-in `metal_perform::fx::particles::dust` runs the same arithmetic in Rust.
//
// The uniform contract is `metal_render::models::forge`'s effect shape; this node
// writes its own nine parameters into the ingest slots, in order:
//
//   floats2.x  count     0..1, of 4096 points
//   floats2.y  size      point radius
//   floats2.z  drift     how fast the field travels
//   floats2.w  direction the angle it travels on, degrees
//   floats3.x  jitter    per-point wobble
//   floats3.y  twinkle   how much each point blinks
//   floats3.z  glow      additive strength
//   floats3.w  depth     parallax: near points are bigger
//   floats4.x  seed      the field's identity
//
// `u.floats.x` is the generator's own accumulated time (the same beat clock
// `fx.move` uses), so a drift written per beat stalls when the music does.
struct Floats {
    floats: vec4<f32>,   // x time (s), y aspect (w/h)
    floats2: vec4<f32>,  // x count, y size, z drift, w direction
    floats3: vec4<f32>,  // x jitter, y twinkle, z glow, w depth
    floats4: vec4<f32>,  // x seed, y/z/w spare
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

const MAX_POINTS: u32 = 4096u;

// The lowbias32 finalizer, the same mix the CPU stand-in uses.
fn mix32(n: u32) -> u32 {
    var h = n;
    h ^= h >> 16u;
    h = h * 0x7feb352du;
    h ^= h >> 15u;
    h = h * 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

fn unit(n: u32) -> f32 {
    return f32(mix32(n)) * 2.3283064365386963e-10; // 1 / 2^32
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let base = textureSample(src, samp, in.uv);
    let t = u.floats.x;
    let aspect = u.floats.y;
    let count = u32(clamp(u.floats2.x, 0.0, 1.0) * f32(MAX_POINTS));
    let size = u.floats2.y;
    let drift = u.floats2.z;
    let dir = u.floats2.w * 0.017453292;
    let jitter = u.floats3.x;
    let twinkle = u.floats3.y;
    let glow = u.floats3.z;
    let depth = u.floats3.w;
    let seed = u32(clamp(u.floats4.x, 0.0, 1000.0));

    var dust = 0.0;
    for (var i: u32 = 0u; i < MAX_POINTS; i = i + 1u) {
        if (i >= count) {
            break;
        }
        let s = seed ^ (i * 2654435761u);
        let p0 = vec2<f32>(unit(s), unit(s ^ 0x9e3779b9u));
        let phase = unit(s ^ 0x85ebca6bu);
        let sc = 0.5 + unit(s ^ 0xc2b2ae35u);
        var p = fract(p0 + vec2<f32>(cos(dir), sin(dir)) * drift * t * 0.5);
        p = p + vec2<f32>(sin(t * 3.0 + phase * 6.283185), cos(t * 2.3 + phase * 6.283185)) * jitter * 0.05;
        let d = (in.uv - p) * vec2<f32>(aspect, 1.0);
        let r = size * sc * (0.4 + 1.2 * depth) * 0.12;
        let fall = max(0.0, 1.0 - length(d) / max(r, 1e-4));
        let blink = 0.5 + 0.5 * sin(t * 6.0 + phase * 6.283185);
        let a = fall * (1.0 - twinkle * (1.0 - blink)) * (0.4 + glow);
        dust = dust + a;
    }

    let tint = vec3<f32>(1.0, 0.92, 0.78);
    return vec4<f32>(clamp(base.rgb + tint * clamp(dust, 0.0, 1.0), vec3<f32>(0.0), vec3<f32>(1.0)), base.a);
}
