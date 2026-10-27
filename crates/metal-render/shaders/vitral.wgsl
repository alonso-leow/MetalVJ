// Model 27 — Stained Glass (`vitral`). The frame as a leaded window: cells of
// glass with lead between them, drifting, and lighting up a few at a time.
//
// A Voronoi diagram over a jittered lattice — the same 3x3 neighbourhood trick
// Sandstorm indexes its grains with, put to the other use it is good for. Each
// cell of the lattice holds one seed; a pixel finds the nearest seed (F1) and
// the next nearest (F2) among its nine neighbours. F1 says which pane it is in
// and F2 - F1 says how close it is to the join, which is the lead.
//
// # Why F2 - F1 and not a distance to a line
//
// The lead is the *boundary* of the cells, and the boundary is exactly where
// two seeds are equidistant. `F2 - F1` is zero there and grows going inwards,
// so one `smoothstep` on it gives every join at once, at a consistent width,
// with no edges to enumerate and nothing to tessellate. Drawing the joins as
// geometry would mean actually building the diagram — which is a real
// computation and a real data structure, for a picture that is one subtraction
// away.
//
// It also means the lead automatically stays put when a pane lights up or the
// seeds drift, because it is a property of the seeds and not a thing laid over
// them.
//
// # The pane that lights is chosen once per cell, not per pixel
//
// `periodo` is a round like Rings': `floor(time / periodo)` picks which panes
// are lit this round. Both are derived from the cell's own integer coordinates
// and the round index, so every pixel of one pane agrees about whether it is
// lit without anything being shared between them. That is what keeps this a
// pure function of `(floats, time)` while still looking like it is being
// driven by something.

struct VitralUniform {
    glass: ColorSlot,
    lead: ColorSlot,
    background: ColorSlot,
    // aspect, time, celdas, irregular
    a: vec4<f32>,
    // deriva, ancho_plomo, bisel, periodo
    b: vec4<f32>,
    // encendidas, destello, caida, estallido
    c: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: VitralUniform;

@fragment
fn fs_main(in: FsVOut) -> @location(0) vec4<f32> {
    let aspect = max(u.a.x, 0.01);
    let t = u.a.y;
    let cells = max(u.a.z, 1.0);
    let irregular = clamp(u.a.w, 0.0, 1.0);
    let drift = clamp(u.b.x, 0.0, 1.0);
    let ancho_plomo = max(u.b.y, 0.0);
    let bisel = clamp(u.b.z, 0.0, 1.0);
    let period = max(u.b.w, 0.05);
    let lit = clamp(u.c.x, 0.0, 1.0);
    let flash = clamp(u.c.y, 0.0, 1.0);
    let fall = clamp(u.c.z, 0.0, 1.0);
    let burst = max(u.c.w, 0.0);

    let p = vec2<f32>((in.uv.x - 0.5) * 2.0 * aspect, (0.5 - in.uv.y) * 2.0);
    var col = backdrop_from_uv(u.background, in.uv, aspect);

    // Blown apart from the middle. Applied to the sample point rather than to
    // the seeds, for the reason Sandstorm warps rather than pushes: moving the
    // seeds means the lattice a pixel searches is no longer the lattice its own
    // position implies, and panes start disappearing at the seams.
    var q = p;
    if (burst > 0.001) {
        let r = length(q);
        q = q / (1.0 + burst * (1.0 - exp(-r * 1.5)));
    }

    // Lattice space. The frame is `celdas` panes across its short axis.
    let g = q * cells * 0.5;
    let base = floor(g);

    // F1 and F2, plus which cell won.
    var f1 = 1e9;
    var f2 = 1e9;
    var won = vec2<f32>(0.0);

    for (var i = -1; i <= 1; i = i + 1) {
        for (var j = -1; j <= 1; j = j + 1) {
            let cell = base + vec2<f32>(f32(i), f32(j));
            let hx = lattice(cell);
            let hy = lattice(cell + vec2<f32>(51.0, 17.0));
            // Seeds sit at the centre of their cell at `irregular` 0 — a
            // regular grid of squares, which is a legal and useful window — and
            // wander to anywhere inside it at 1.
            var seed = cell + vec2<f32>(0.5, 0.5)
                + (vec2<f32>(hx, hy) - 0.5) * irregular;
            if (drift > 0.001) {
                // Each seed on its own small circular path, so the panes
                // breathe against each other instead of the whole window
                // sliding. Kept under half a cell so the lattice search stays
                // correct: a seed that left its own cell would be missed by
                // pixels that ought to belong to it.
                let ph = (hx + hy) * 6.28318 + t * (0.3 + 0.7 * hy);
                seed = seed + vec2<f32>(cos(ph), sin(ph)) * drift * 0.35;
            }

            let d = length(seed - g);
            if (d < f1) {
                f2 = f1;
                f1 = d;
                won = cell;
            } else if (d < f2) {
                f2 = d;
            }
        }
    }

    // The join. `f2 - f1` is 0 exactly on the boundary between two panes.
    let join = f2 - f1;
    let px = max(fwidth(join), 1e-4);
    let lead = 1.0 - smoothstep(ancho_plomo * cells * 0.5 - px,
                                ancho_plomo * cells * 0.5 + px, join);

    // The pane's own number, and what colour of glass it was cut from.
    let mine = lattice(won + vec2<f32>(7.0, 23.0));
    var glass = grad4(u.glass, mine);

    // Lit panes. The round picks a fresh set; a pane is in it if its own hash,
    // rotated by the round, falls under `encendidas`.
    let round = floor(t / period);
    let local = (t / period) - round;
    let pick = lattice(won + vec2<f32>(round * 3.0 + 91.0, round * 5.0 - 13.0));
    if (pick < lit) {
        // Comes up fast and leaves at `caida`'s pace — a pane catching light,
        // not a pane fading in.
        let up = smoothstep(0.0, 0.06, local);
        let out = 1.0 - smoothstep(0.0, 1.0, local / max(1.0 - fall * 0.85, 0.05));
        glass = glass + grad4(u.glass, fract(mine + 0.5)) * flash * up * max(out, 0.0);
    }

    // A bevel: darker toward the lead, so a pane reads as a piece of glass in a
    // frame rather than as a flat colour region. Driven by the same `join`,
    // which is the distance to the nearest edge whichever edge that is.
    let bevel = mix(1.0, clamp(join * cells * 0.35, 0.25, 1.0), bisel);
    glass = glass * bevel;

    col = mix(glass, grad4(u.lead, f1 / max(f2, 1e-4)), lead);
    return vec4<f32>(col, 1.0);
}
