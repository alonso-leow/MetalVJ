// gradiente: radial gradient walking the palette from centre to edge.
// r is the distance from the (offset) centre in units of `radius`; `blur`
// eases the ramp (0 linear, 1 smoothstep) so the transition is softer.

fn paper_shade(p: vec2<f32>, uv: vec2<f32>) -> vec4<f32> {
    let q = p - vec2<f32>(f_off_x(), f_off_y());
    let t = clamp(length(q) / max(f_radius(), 0.05), 0.0, 1.0);
    let b = clamp(f_blur(), 0.0, 1.0);
    let k = mix(t, smoothstep(0.0, 1.0, t), b);
    // Outer ring fades into the background so the fondo slot shows at the edge.
    let cov = 1.0 - smoothstep(0.75, 1.0, t);
    return vec4<f32>(p_pal(0u, k), cov);
}
