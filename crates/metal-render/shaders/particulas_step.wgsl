// Particulas, the step pass: a fixed-step symplectic leapfrog for up to 65536
// bodies, written from the algorithm (rebound's WHFast/leapfrog family and its
// shearing-sheet boundary); no code was taken.
//
// State is two ping-pong storage buffers per quantity: position xyz and the
// half-step velocity xyz. One step is the merged kick-drift form
//     v' = v + a(x) dt;   x' = x + v' dt
// which is the velocity-Verlet leapfrog with the two half kicks fused.
// Everything a body feels is computed from the *read* buffers only, so the
// result does not depend on thread scheduling:
//   * a central softened mass, force law  a = -GM x / (|x|^2 + eps^2)^(p/2)
//     (p = 3 is Newtonian; the exponent is the rotation curve shear),
//   * an optional attractor (a point mass on a prescribed orbit),
//   * self gravity from a tile subset: every workgroup draws 64 source bodies
//     by hash and each of its 64 bodies sums them (a Monte-Carlo estimate of
//     the disc's field),
//   * soft collisions against one representative body per grid cell (the cell
//     keeps the highest index: atomicMax is order independent, so it is
//     deterministic): a penalty spring plus a restitution impulse,
//   * in sheet mode the local Hill frame instead of the central mass:
//     x'' = 2 Om y' + 2 q Om^2 x,  y'' = -2 Om x',  z'' = -Om^2 z, with the
//     Coriolis rotation applied exactly and the sheared periodic boundary.
// Safety: speed and acceleration are capped, bodies that leave the kill
// radius, fall into the softening core or go non-finite are respawned on the
// disc, so no input can send the state to NaN or infinity.

struct SU {
    // dt, GM, eps (central), force exponent p
    a: vec4<f32>,
    // q (shear), Om, mode (0 disc, 1 sheet), R
    b: vec4<f32>,
    // thickness, spin, caos, vref
    c: vec4<f32>,
    // attractor xyz, attractor GM
    d: vec4<f32>,
    // collision stiffness, restitution, pair weight (GM_disc / 64), cell size
    e: vec4<f32>,
    // vmax, kill radius, box half (sheet), shear offset (sheet)
    f: vec4<f32>,
    // grid extent, attractor eps, eps (pair), accel cap
    g: vec4<f32>,
    // collision strength, 0, 0, 0
    h: vec4<f32>,
    // n, seed, step counter, 0
    ints: vec4<u32>,
};

@group(0) @binding(0) var<uniform> u: SU;
@group(0) @binding(1) var<storage, read> pos_in: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> vel_in: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> pos_out: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> vel_out: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> grid: array<atomic<u32>>;

const GRID: i32 = 256;
const TILE: u32 = 64u;
const TAU: f32 = 6.2831853;

var<workgroup> tile: array<vec4<f32>, 64>;

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

fn unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

fn rng(i: u32, k: u32, salt: u32) -> f32 {
    return unit(pcg(pcg(i * 0x9E3779B1u + salt) ^ (k * 0x85EBCA6Bu + 0x27D4EB2Fu)));
}

// Three uniform draws summed and centred: a bell with sigma about 1.
fn bell(i: u32, k: u32, salt: u32) -> f32 {
    return (rng(i, k, salt) + rng(i, k + 1u, salt) + rng(i, k + 2u, salt) - 1.5) * 2.0;
}

fn central_accel(p: vec3<f32>) -> vec3<f32> {
    let r2 = dot(p, p) + u.a.z * u.a.z;
    return -u.a.y * p * pow(r2, -0.5 * u.a.w);
}

fn attractor_accel(p: vec3<f32>) -> vec3<f32> {
    let d = u.d.xyz - p;
    let d2 = dot(d, d) + u.g.y * u.g.y;
    return u.d.w * d * pow(d2, -1.5);
}

fn cap_speed(v: vec3<f32>) -> vec3<f32> {
    let sp = length(v);
    if (sp > u.f.x) {
        return v * (u.f.x / sp);
    }
    return v;
}

struct Body {
    p: vec3<f32>,
    v: vec3<f32>,
};

// The deterministic initial / respawn state of body `i`. `salt` is 0 at the
// seed and the step counter on a respawn, so a respawned body lands somewhere
// new but reproducibly.
fn spawn(i: u32, salt: u32) -> Body {
    let s = u.ints.y * 7919u + salt * 104729u;
    let R = u.b.w;
    let thick = u.c.x;
    let spin = u.c.y;
    let chaos = u.c.z;
    var o: Body;
    if (u.b.z < 0.5) {
        let rr = R * (0.5 + 0.6 * rng(i, 1u, s));
        var th = TAU * rng(i, 2u, s);
        // Two trailing arms: a modulation of the angle that the differential
        // rotation then winds into a swirl.
        th = th + 0.5 * sin(2.0 * th + 4.0 * log(rr / R + 0.3));
        let z = thick * R * 0.07 * bell(i, 3u, s);
        let r2 = rr * rr + u.a.z * u.a.z;
        let vc = sqrt(max(u.a.y * rr * rr * pow(r2, -0.5 * u.a.w), 0.0));
        let tang = vec3<f32>(-sin(th), cos(th), 0.0);
        let rad = vec3<f32>(cos(th), sin(th), 0.0);
        o.p = vec3<f32>(rr * cos(th), rr * sin(th), z);
        o.v = spin * vc * tang
            + chaos * vc * 0.22 * (rad * bell(i, 6u, s) + tang * bell(i, 9u, s))
            + vec3<f32>(0.0, 0.0, chaos * vc * 0.12 * bell(i, 12u, s));
    } else {
        let H = u.f.z;
        let om = u.b.y;
        let x = (2.0 * rng(i, 1u, s) - 1.0) * H;
        let y = (2.0 * rng(i, 2u, s) - 1.0) * H;
        let z = thick * R * 0.07 * bell(i, 3u, s);
        let sig = chaos * om * H * 0.12;
        o.p = vec3<f32>(x, y, z);
        o.v = vec3<f32>(sig * bell(i, 6u, s), -spin * u.b.x * om * x + sig * bell(i, 9u, s), sig * 0.5 * bell(i, 12u, s));
    }
    o.v = cap_speed(o.v);
    return o;
}

fn cell_of(p: vec3<f32>) -> i32 {
    let ext = u.g.x;
    let cs = u.e.w;
    let cx = i32(floor((p.x + ext) / cs));
    let cy = i32(floor((p.y + ext) / cs));
    if (cx < 0 || cy < 0 || cx >= GRID || cy >= GRID) {
        return -1;
    }
    return cy * GRID + cx;
}

@compute @workgroup_size(64)
fn cs_seed(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= u.ints.x) {
        return;
    }
    var b = spawn(i, 0u);
    // Start the half-step velocity half a kick behind, so the first step lands
    // on the synchronised velocity.
    var a = vec3<f32>(0.0);
    if (u.b.z < 0.5) {
        a = central_accel(b.p) + attractor_accel(b.p);
    } else {
        let om = u.b.y;
        a = vec3<f32>(2.0 * u.b.x * om * om * b.p.x, 0.0, -om * om * b.p.z) + attractor_accel(b.p);
    }
    pos_out[i] = vec4<f32>(b.p, 0.0);
    vel_out[i] = vec4<f32>(cap_speed(b.v - 0.5 * u.a.x * a), 0.0);
}

@compute @workgroup_size(64)
fn cs_clear(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (i32(gid.x) < GRID * GRID) {
        atomicStore(&grid[gid.x], 0u);
    }
}

@compute @workgroup_size(64)
fn cs_build(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= u.ints.x) {
        return;
    }
    let c = cell_of(pos_in[i].xyz);
    if (c >= 0) {
        atomicMax(&grid[c], i + 1u);
    }
}

@compute @workgroup_size(64)
fn cs_step(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wid: vec3<u32>,
) {
    let n = u.ints.x;
    let i = gid.x;
    // The tile: one hashed source body per thread, shared by the workgroup.
    let pick = pcg(wid.x * 2654435761u + u.ints.z * 40503u + lid.x * 97u + u.ints.y) % n;
    tile[lid.x] = pos_in[pick];
    workgroupBarrier();
    if (i >= n) {
        return;
    }

    let dt = u.a.x;
    let sheet = u.b.z > 0.5;
    var p = pos_in[i].xyz;
    var v = vel_in[i].xyz;
    let om = u.b.y;

    var a = attractor_accel(p);
    if (sheet) {
        a = a + vec3<f32>(2.0 * u.b.x * om * om * p.x, 0.0, -om * om * p.z);
    } else {
        a = a + central_accel(p);
    }

    // Self gravity from the tile subset.
    let eps2 = u.g.z * u.g.z;
    var g = vec3<f32>(0.0);
    for (var k = 0u; k < TILE; k = k + 1u) {
        let d = tile[k].xyz - p;
        let d2 = dot(d, d) + eps2;
        g = g + d * pow(d2, -1.5);
    }
    a = a + g * u.e.z;

    // Soft collisions against the representatives of the 3x3 cells around.
    let kcol = u.e.x;
    var dv = vec3<f32>(0.0);
    if (kcol > 0.0) {
        let d0 = 0.75 * u.e.w;
        let c = cell_of(p);
        if (c >= 0) {
            let cx = c % GRID;
            let cy = c / GRID;
            for (var oy = -1; oy <= 1; oy = oy + 1) {
                for (var ox = -1; ox <= 1; ox = ox + 1) {
                    let nx = cx + ox;
                    let ny = cy + oy;
                    if (nx < 0 || ny < 0 || nx >= GRID || ny >= GRID) {
                        continue;
                    }
                    let rep = atomicLoad(&grid[ny * GRID + nx]);
                    if (rep == 0u || rep - 1u == i) {
                        continue;
                    }
                    let j = rep - 1u;
                    let dd = pos_in[j].xyz - p;
                    let dist2 = dot(dd, dd);
                    if (dist2 < d0 * d0 && dist2 > 1e-10) {
                        let distance = sqrt(dist2);
                        let nrm = dd / distance;
                        let pen = d0 - distance;
                        a = a - nrm * (kcol * pen);
                        let vn = dot(vel_in[j].xyz - v, nrm);
                        if (vn < 0.0) {
                            dv = dv + nrm * (u.h.x * 0.5 * (1.0 + u.e.y) * vn * (pen / d0));
                        }
                    }
                }
            }
        }
    }

    // Cap the acceleration (a close pass through the core) and kick.
    let al = length(a);
    if (al > u.g.w) {
        a = a * (u.g.w / al);
    }
    v = v + a * dt + dv;
    if (sheet) {
        // The Coriolis term as an exact rotation of the velocity in the plane.
        let ang = -2.0 * om * dt;
        let cs = cos(ang);
        let sn = sin(ang);
        v = vec3<f32>(cs * v.x - sn * v.y, sn * v.x + cs * v.y, v.z);
    }
    v = cap_speed(v);
    p = p + v * dt;

    var respawn = false;
    if (sheet) {
        let H = u.f.z;
        let so = u.f.w;
        let sdv = u.b.x * om * 2.0 * H;
        if (p.x > H) {
            p.x = p.x - 2.0 * H;
            p.y = p.y + so;
            v.y = v.y + sdv;
        } else if (p.x < -H) {
            p.x = p.x + 2.0 * H;
            p.y = p.y - so;
            v.y = v.y - sdv;
        }
        p.y = p.y - 2.0 * H * floor((p.y + H) / (2.0 * H));
        respawn = abs(p.z) > 8.0 * H || abs(p.x) > H * 1.5;
    } else {
        let r2 = dot(p, p);
        respawn = r2 > u.f.y * u.f.y || r2 < 0.0004;
    }
    // Written so that a NaN on either side falls into the respawn branch.
    let sane = (abs(p.x) < 1.0e6) && (abs(p.y) < 1.0e6) && (abs(p.z) < 1.0e6)
        && (abs(v.x) < 1.0e6) && (abs(v.y) < 1.0e6) && (abs(v.z) < 1.0e6);
    if (respawn || !sane) {
        let b = spawn(i, u.ints.z + 1u);
        p = b.p;
        v = b.v;
    }
    pos_out[i] = vec4<f32>(p, 0.0);
    vel_out[i] = vec4<f32>(v, 0.0);
}
