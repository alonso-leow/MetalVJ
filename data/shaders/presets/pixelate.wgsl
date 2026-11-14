// `presets/pixelate` — the picture in square blocks. The block count follows
// the music: loud drops coarsen it, quiet passages resolve back to the picture.

struct Floats {
    floats: vec4<f32>,   // x time (s), y aspect (w/h)
    floats2: vec4<f32>,  // rms, bandwidth, onset, centroid
    floats3: vec4<f32>,
    floats4: vec4<f32>,
    floats5: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.floats.y, 0.01);
    let rms = clamp(u.floats2.x, 0.0, 1.0);
    let cells_y = mix(72.0, 10.0, rms);
    let cells = vec2<f32>(cells_y * aspect, cells_y);
    let uv = (floor(in.uv * cells) + vec2<f32>(0.5, 0.5)) / cells;
    return textureSample(src, samp, uv);
}

