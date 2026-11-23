// Terrarium's seed and Margolus step on a device with no compute shaders
// (WebGL2): one fragment per cell recomputes its 2x2 block with the same
// `make_block` / `cell_out` the compute entry point uses, and keeps its own
// slot. Drawn by the model's fullscreen triangle (`vs_main`).

@fragment
fn fs_seed(in: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}

@fragment
fn fs_step(in: VOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let off = i32(st.s.z);
    let q = p + vec2<i32>(off, off);
    // Floor-halved, so the shifted step's block at -1 covers column 0.
    let base = vec2<i32>(q.x / 2 * 2 - off, q.y / 2 * 2 - off);
    let k = (p.x - base.x) + 2 * (p.y - base.y);
    return cell_out(make_block(base), k);
}
