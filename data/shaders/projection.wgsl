// `fx.projection` (parity P-A, the "Cube Map") — equirect ↔ cubemap cross
// (4×3) ↔ fisheye (dome), with yaw/pitch/roll and FOV. The CPU stand-in is
// `metal_perform::fx::parity::projection`; keep the two in step.
//
//   floats2  from, to (0 equirect, 1 cross, 2 fisheye), yaw, pitch (degrees)
//   floats3  roll (degrees), fov (degrees)
struct Floats {
    floats: vec4<f32>,
    floats2: vec4<f32>,
    floats3: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Floats;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;

// A direction for `uv` in layout `mode`; w = 0 outside the picture.
fn to_dir(mode: i32, uv: vec2<f32>, fov: f32) -> vec4<f32> {
    if (mode == 0) {
        let lon = (uv.x - 0.5) * TAU;
        let lat = (0.5 - uv.y) * PI;
        return vec4<f32>(cos(lat) * sin(lon), sin(lat), cos(lat) * cos(lon), 1.0);
    }
    if (mode == 1) {
        let cx = i32(clamp(floor(uv.x * 4.0), 0.0, 3.0));
        let cy = i32(clamp(floor(uv.y * 3.0), 0.0, 2.0));
        let s = (uv.x * 4.0 - f32(cx)) * 2.0 - 1.0;
        let t = (uv.y * 3.0 - f32(cy)) * 2.0 - 1.0;
        if (cy == 1) {
            if (cx == 1) { return vec4<f32>(s, -t, 1.0, 1.0); }
            if (cx == 2) { return vec4<f32>(1.0, -t, -s, 1.0); }
            if (cx == 3) { return vec4<f32>(-s, -t, -1.0, 1.0); }
            return vec4<f32>(-1.0, -t, s, 1.0);
        }
        if (cx == 1 && cy == 0) { return vec4<f32>(s, 1.0, t, 1.0); }
        if (cx == 1 && cy == 2) { return vec4<f32>(s, -1.0, -t, 1.0); }
        return vec4<f32>(0.0);
    }
    let p = vec2<f32>((uv.x - 0.5) * 2.0, (0.5 - uv.y) * 2.0);
    let r = length(p);
    if (r > 1.0) { return vec4<f32>(0.0); }
    let theta = r * radians(fov) * 0.5;
    let phi = atan2(p.y, p.x);
    return vec4<f32>(sin(theta) * cos(phi), sin(theta) * sin(phi), cos(theta), 1.0);
}

// Where direction `d` lands in layout `mode`; z = 0 outside.
fn to_uv(mode: i32, dir: vec3<f32>, fov: f32) -> vec3<f32> {
    let d = normalize(dir);
    if (mode == 0) {
        return vec3<f32>(atan2(d.x, d.z) / TAU + 0.5, 0.5 - asin(clamp(d.y, -1.0, 1.0)) / PI, 1.0);
    }
    if (mode == 1) {
        let a = abs(d);
        var cell = vec2<f32>(1.0, 1.0);
        var st = vec2<f32>(0.0);
        if (a.z >= a.x && a.z >= a.y) {
            if (d.z > 0.0) { cell = vec2<f32>(1.0, 1.0); st = vec2<f32>(d.x, -d.y) / a.z; }
            else { cell = vec2<f32>(3.0, 1.0); st = vec2<f32>(-d.x, -d.y) / a.z; }
        } else if (a.x >= a.y) {
            if (d.x > 0.0) { cell = vec2<f32>(2.0, 1.0); st = vec2<f32>(-d.z, -d.y) / a.x; }
            else { cell = vec2<f32>(0.0, 1.0); st = vec2<f32>(d.z, -d.y) / a.x; }
        } else if (d.y > 0.0) {
            cell = vec2<f32>(1.0, 0.0); st = vec2<f32>(d.x, d.z) / a.y;
        } else {
            cell = vec2<f32>(1.0, 2.0); st = vec2<f32>(d.x, -d.z) / a.y;
        }
        let f = clamp((st + vec2<f32>(1.0)) * 0.5, vec2<f32>(0.0), vec2<f32>(0.9999));
        return vec3<f32>((cell.x + f.x) / 4.0, (cell.y + f.y) / 3.0, 1.0);
    }
    let theta = acos(clamp(d.z, -1.0, 1.0));
    let r = theta / max(radians(fov) * 0.5, 0.0001);
    if (r > 1.0) { return vec3<f32>(0.0); }
    let phi = atan2(d.y, d.x);
    return vec3<f32>(0.5 + r * cos(phi) * 0.5, 0.5 - r * sin(phi) * 0.5, 1.0);
}

fn rotate(d0: vec3<f32>, yaw: f32, pitch: f32, roll: f32) -> vec3<f32> {
    let sr = sin(radians(roll)); let cr = cos(radians(roll));
    let sp = sin(radians(pitch)); let cp = cos(radians(pitch));
    let sy = sin(radians(yaw)); let cy = cos(radians(yaw));
    var d = vec3<f32>(d0.x * cr - d0.y * sr, d0.x * sr + d0.y * cr, d0.z);
    d = vec3<f32>(d.x, d.y * cp - d.z * sp, d.y * sp + d.z * cp);
    return vec3<f32>(d.x * cy + d.z * sy, d.y, -d.x * sy + d.z * cy);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let src_mode = i32(round(u.floats2.x));
    let dst_mode = i32(round(u.floats2.y));
    let fov = u.floats3.y;
    let d = to_dir(dst_mode, in.uv, fov);
    let s = to_uv(src_mode, rotate(d.xyz, u.floats2.z, u.floats2.w, u.floats3.x), fov);
    // Sampled unconditionally (uniform control flow), masked after.
    let c = textureSampleLevel(src, samp, s.xy, 0.0);
    if (d.w < 0.5 || s.z < 0.5) {
        return vec4<f32>(0.0);
    }
    return c;
}
