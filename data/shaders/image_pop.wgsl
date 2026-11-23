// `fx.image-pop` — reel media drawn as N instances.
//
// **Not wired yet.** The live path is `metal_perform::fx::image_pop::compose`, in
// Rust, because `metal_render::models::forge`'s effect contract hands an effect
// shader exactly one texture — the picture it is applied to — and there is no
// second binding a reel atlas could arrive on. This file is the shader that
// contract would run the day it grows one; it takes the atlas as `src` and
// draws the pop layer over transparent black, and the caller composites that
// layer over the upstream picture with `blend`. See `crate::fx`'s module doc.
//
// The atlas is a grid of `cols × rows` equally sized frames, cell `k` at
// `(k % cols, k / cols)`. `floats5.x`/`floats5.y` carry the grid.
//
// The uniform contract (the same `Floats` shape every forge effect uses):
//
//   floats.x   time (s)            floats.y   aspect (w/h)
//   floats2.x  rate                floats2.y  concurrent
//   floats2.z  life                floats2.w  fade
//   floats3.x  scale               floats3.y  scale_jitter
//   floats3.z  spread              floats3.w  position_x
//   floats4.x  position_y          floats4.y  seed
//   floats4.z  blend               floats4.w  crop_zoom
//   floats5.x  atlas cols          floats5.y  atlas rows
struct Floats {
    floats: vec4<f32>,
    floats2: vec4<f32>,
    floats3: vec4<f32>,
    floats4: vec4<f32>,
    floats5: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

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
    return f32(mix32(n)) * 2.3283064365386963e-10;
}

fn cell_uv(frame: u32, cols: u32, rows: u32, uv: vec2<f32>) -> vec2<f32> {
    let c = frame % cols;
    let r = frame / cols;
    return (vec2<f32>(f32(c), f32(r)) + uv) / vec2<f32>(f32(cols), f32(rows));
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let t = u.floats.x;
    let aspect = u.floats.y;
    let rate = u.floats2.x;
    let concurrent = u32(clamp(u.floats2.y, 1.0, 32.0));
    let life = max(u.floats2.z, 0.1);
    let fade = clamp(u.floats2.w, 0.0, 0.5);
    let scale = u.floats3.x;
    let scale_jitter = u.floats3.y;
    let spread = u.floats3.z;
    let pos = vec2<f32>(u.floats3.w, u.floats4.x);
    let seed = u32(clamp(u.floats4.y, 0.0, 1000.0));
    // `u.floats4.z` is `blend`; the caller applies it when it composites this
    // layer, so the shader itself does not read it.
    let crop = clamp(u.floats4.w, 0.0, 1.0);
    let cols = max(u32(u.floats5.x), 1u);
    let rows = max(u32(u.floats5.y), 1u);
    let frames = cols * rows;

    if (rate <= 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let last = i32(floor(t * rate));
    let first = max(i32(ceil(t * rate - life * rate)), 0);
    var acc = vec3<f32>(0.0);
    var cover = 0.0;
    var drawn = 0u;
    var i = last;
    loop {
        if (i < first || drawn >= concurrent) {
            break;
        }
        let age = t - f32(i) / rate;
        if (age >= 0.0 && age < life) {
            let u_i = u32(i);
            let f = fade * life;
            var alpha = 1.0;
            if (f > 1e-6) {
                alpha = clamp(min(age / f, (life - age) / f), 0.0, 1.0);
            }
            let ox = unit(seed ^ mix32(u_i * 2654435761u + 10u)) * 2.0 - 1.0;
            let oy = unit(seed ^ mix32(u_i * 2654435761u + 11u)) * 2.0 - 1.0;
            let centre = clamp(pos + vec2<f32>(ox, oy) * spread * 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
            let jit = unit(seed ^ mix32(u_i * 2654435761u + 12u)) * 2.0 - 1.0;
            let sc = max(scale * (1.0 + scale_jitter * jit), 0.0);
            let frame = (i % i32(frames) + i32(frames)) % i32(frames);
            // Media aspect comes from the atlas cell, which is square here;
            // the CPU compositor uses the media's own aspect. Kept square so
            // the shader stays a reference, not a second implementation.
            let half = vec2<f32>(sc * 0.5, sc * 0.5 * aspect);
            let local = (in.uv - centre + half) / (half * 2.0);
            if (local.x >= 0.0 && local.x <= 1.0 && local.y >= 0.0 && local.y <= 1.0) {
                let suv = vec2<f32>(mix(crop * 0.5, 1.0 - crop * 0.5, local.x),
                                    mix(crop * 0.5, 1.0 - crop * 0.5, local.y));
                let tex = textureSample(src, samp, cell_uv(frame, cols, rows, suv));
                acc = acc + tex.rgb * alpha;
                cover = max(cover, alpha);
                drawn = drawn + 1u;
            }
        }
        i = i - 1;
    }
    // The pop layer, not the composited frame: `blend_mode` is applied by the
    // caller as it composites this over the upstream picture.
    let _ = blend_mode;
    return vec4<f32>(clamp(acc, vec3<f32>(0.0), vec3<f32>(1.0)), cover);
}
