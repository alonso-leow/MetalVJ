// Lattice: one ray-marched object repeated on a 3D grid of columns (x),
// rows (y) and depth rows (z, receding from the default camera at +z), each
// copy spun on three axes. 2026-11-25 review: a single bulb/object read as
// "too weird"; a 3D field of them, dithered in phase, reads as a pattern.
//
// lat  = (columns, rows, depth rows, spacing in object radii)
// spin = (spin_x, spin_y, spin_z in turns/s, dither 0..1 = per-copy phase spread)
// Counts are rounded; columns and rows are centred (odd counts sit on the
// axis), depth rows run from 0 back to -(depth-1).

fn lat_hash3(c: vec3<f32>) -> vec3<f32> {
    let q = vec3<f32>(dot(c, vec3<f32>(127.1, 311.7, 74.7)),
                      dot(c, vec3<f32>(269.5, 183.3, 246.1)),
                      dot(c, vec3<f32>(113.5, 271.9, 124.6)));
    return fract(sin(q) * 43758.5453);
}

fn lat_half(lat: vec4<f32>) -> vec3<f32> {
    let n = max(round(lat.xyz), vec3<f32>(1.0));
    return vec3<f32>((n.x - 1.0) * 0.5, (n.y - 1.0) * 0.5, n.z - 1.0);
}

fn lat_cell(p: vec3<f32>, lat: vec4<f32>) -> vec3<f32> {
    let s = max(lat.w, 2.3);
    let h = lat_half(lat);
    let c = round(p / s);
    return vec3<f32>(clamp(c.x, -h.x, h.x), clamp(c.y, -h.y, h.y), clamp(c.z, -h.z, 0.0));
}

fn lat_rx(v: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a); let s = sin(a);
    return vec3<f32>(v.x, c * v.y - s * v.z, s * v.y + c * v.z);
}
fn lat_ry(v: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a); let s = sin(a);
    return vec3<f32>(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}
fn lat_rz(v: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a); let s = sin(a);
    return vec3<f32>(c * v.x - s * v.y, s * v.x + c * v.y, v.z);
}

// World point -> the local point of the copy that owns it (rigid motion, so
// the object's distance estimate stays valid).
fn lat_local(p: vec3<f32>, lat: vec4<f32>, spin: vec4<f32>, time: f32) -> vec3<f32> {
    let s = max(lat.w, 2.3);
    let c = lat_cell(p, lat);
    var q = p - c * s;
    let ph = (lat_hash3(c) - vec3<f32>(0.5)) * 6.2832 * clamp(spin.w, 0.0, 1.0);
    let a = spin.xyz * time * 6.2832 + ph;
    q = lat_rz(q, -a.z);
    q = lat_ry(q, -a.y);
    q = lat_rx(q, -a.x);
    return q;
}

// Ray against the lattice's bounding box (copies of radius `r`): (t near, t far).
fn lat_box(ro: vec3<f32>, rd: vec3<f32>, lat: vec4<f32>, r: f32) -> vec2<f32> {
    let s = max(lat.w, 2.3);
    let h = lat_half(lat);
    let lo = vec3<f32>(-h.x * s - r, -h.y * s - r, -h.z * s - r);
    let hi = vec3<f32>(h.x * s + r, h.y * s + r, r);
    let inv = 1.0 / select(rd, vec3<f32>(1e-6), abs(rd) < vec3<f32>(1e-6));
    let t1 = (lo - ro) * inv;
    let t2 = (hi - ro) * inv;
    let tn = max(max(min(t1.x, t2.x), min(t1.y, t2.y)), min(t1.z, t2.z));
    let tf = min(min(max(t1.x, t2.x), max(t1.y, t2.y)), max(t1.z, t2.z));
    return vec2<f32>(tn, tf);
}
