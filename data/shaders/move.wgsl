// `fx.move` — translate the source's picture relative to its mask.
//
// The mask lives downstream of this node, so moving the picture here is
// exactly "the content moves, the mask stays". The uniform contract is
// `metal_render::models::forge`'s effect shape; this node writes its own five
// parameters into the ingest slots, in order:
//
//   floats2.x  x     offset across the frame, -1..1
//   floats2.y  y     offset down the frame, -1..1
//   floats2.z  vx    drift across, per beat
//   floats2.w  vy    drift down, per beat
//   floats3.x  wrap  0 clamp (edges smear), 1 tile (the picture repeats)
//
// `u.floats.x` is the generator's own accumulated time, which is also its beat
// clock here: `speed` (an `fx.speed` upstream) scales it, so a drift written
// per beat advances on the music and stalls when the music does.
struct Floats {
    floats: vec4<f32>,   // x time (s), y aspect (w/h)
    floats2: vec4<f32>,  // x x, y y, z vx, w vy
    floats3: vec4<f32>,  // x wrap, y/z/w spare
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let t = u.floats.x;
    let off = vec2<f32>(u.floats2.x, u.floats2.y)
        + vec2<f32>(u.floats2.z, u.floats2.w) * t;
    var uv = in.uv - off;
    if (u.floats3.x > 0.5) {
        uv = fract(uv);
    } else {
        uv = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
    }
    return textureSample(src, samp, uv);
}
