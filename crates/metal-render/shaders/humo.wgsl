// Model 28 — Smoke (`humo`). Smoke rising off a source, lit from a direction.
//
// Not a raymarch. A few layers of fbm at different scales, each rising at its
// own rate and warped by the one below it, summed into a density and then cut
// with a threshold. The suite had nothing soft in it — everything is either a
// hard-edged object or a field with structure, and smoke is the one thing that
// is neither.
//
// # Why layers and not a march
//
// A real march through a 3D noise field is 24 to 64 samples of a 3D fbm per
// pixel, and a 3D fbm is four to six lattice reads per octave. That is the
// single most expensive renderer in the suite by a wide margin, in a project
// that has no way to measure what a renderer costs (`NEXT.md`), on a laptop
// that is also driving a venue.
//
// Layered 2D at four octaves is about a tenth of that, and the thing it gives
// up — genuine self-occlusion — is not what makes smoke read as smoke. What
// does is the *shape of the edge*: soft, curling, torn. That comes from the
// warp and the threshold, both of which are here in full.
//
// # The lighting is a gradient, and it is the whole trick
//
// Density alone is a grey cloud. What makes it a volume is that the light
// arriving anywhere depends on how much smoke is between there and the light,
// and the cheap stand-in for "how much is in the way" is the derivative of the
// density along the light direction. One extra sample of the field, offset
// toward `light_direction`, and the difference is the shading. It is not correct and
// it is not trying to be — it is the term that turns a flat blob into
// something with a lit side.

struct HumoUniform {
    smoke: ColorSlot,
    background: ColorSlot,
    // aspect, time, capas, subida
    a: vec4<f32>,
    // escala, detalle, remolino, densidad
    b: vec4<f32>,
    // corte, suavidad, luz_dir, luz
    c: vec4<f32>,
    // deriva, origen_y, alcance, unused
    d: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: HumoUniform;

const MAX_CAPAS: i32 = 5;

/// The density of smoke at a point. Everything in this model is this function
/// sampled two or three times.
fn density(p: vec2<f32>, t: f32, layers: i32, rise: f32, scale: f32,
           detail: i32, swirl: f32, drift: f32) -> f32 {
    var total = 0.0;
    var weight = 0.0;

    for (var i = 0; i < MAX_CAPAS; i = i + 1) {
        if (i >= layers) { break; }
        let f = f32(i);
        // Each layer bigger, slower and fainter than the last, so the near
        // wisps move over the far body rather than with it. Parallax without
        // a camera.
        let scale = scale * (1.0 + f * 0.85);
        let speed = rise / (1.0 + f * 0.6);
        let amp = 1.0 / (1.0 + f * 0.8);

        var q = p * scale;
        // Rising: the field scrolls down past the point, which is the same
        // thing as the smoke going up, and costs a subtraction.
        q.y = q.y + t * speed;
        q.x = q.x - t * drift * scale;

        if (swirl > 0.001) {
            // Warped by a coarser copy of itself. This is what curls the
            // edges; without it the layers are clouds and never smoke.
            let w = vec2<f32>(
                lattice_fbm(q * 0.5 + vec2<f32>(3.1, 7.7), 2),
                lattice_fbm(q * 0.5 - vec2<f32>(5.3, 1.9), 2)
            ) - 0.5;
            q = q + w * swirl * 2.0;
        }

        total = total + lattice_fbm(q, detail) * amp;
        weight = weight + amp;
    }

    return total / max(weight, 1e-4);
}

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let layers = i32(clamp(u.a.z, 1.0, f32(MAX_CAPAS)));
    let rise = u.a.w;
    let scale = max(u.b.x, 0.05);
    let detail = i32(clamp(u.b.y, 1.0, 6.0));
    let swirl = max(u.b.z, 0.0);
    let densidad = max(u.b.w, 0.0);
    let cut = clamp(u.c.x, 0.0, 1.0);
    let softness = max(u.c.y, 0.005);
    let light_direction = u.c.z;
    let light = max(u.c.w, 0.0);
    let drift = u.d.x;
    let origin_y = u.d.y;
    let reach = max(u.d.z, 0.05);

    let p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    var col = backdrop_from_uv(u.background, in.uv, aspect);

    // How much smoke there can be here at all: none below the source, full
    // just above it, dissipating over `alcance`. Without this the frame is
    // uniformly cloudy and the smoke has no origin — the difference between
    // smoke and fog.
    let up = p.y - origin_y;
    if (up < -0.1) {
        return vec4<f32>(col, 1.0);
    }
    let column = smoothstep(-0.1, 0.15, up) * (1.0 - smoothstep(0.0, reach, max(up, 0.0)));
    if (column <= 0.002) {
        return vec4<f32>(col, 1.0);
    }

    let d = density(p, t, layers, rise, scale, detail, swirl, drift);

    // The cut. Everything below `corte` is clear air; the transition over
    // `softness` is the edge, and it is the edge that carries the whole read.
    var a = smoothstep(cut, cut + softness, d) * column * densidad;
    a = clamp(a, 0.0, 1.0);
    if (a <= 0.002) {
        return vec4<f32>(col, 1.0);
    }

    // The lit side. One more sample, a step toward the light — see the header.
    let ldir = vec2<f32>(cos(light_direction), sin(light_direction));
    let ahead = density(p + ldir * 0.09, t, layers, rise, scale, detail, swirl, drift);
    // Positive where the smoke thins toward the light, so that face is bright
    // and the far side of the same puff is not.
    let facing = clamp((d - ahead) * 4.0 + 0.5, 0.0, 1.0);

    // The gradient is walked by how lit a piece of smoke is, not by how dense
    // it is: the four stops are the shadowed and lit sides of one material,
    // which is what makes them read as one substance in different light.
    let shade = clamp(facing * light + (1.0 - light) * 0.5, 0.0, 1.0);
    let tint = grad4(u.smoke, shade);

    col = mix(col, tint, a);
    return vec4<f32>(col, 1.0);
}
