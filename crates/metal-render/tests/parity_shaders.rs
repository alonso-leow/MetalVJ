//! Parity P-A: the built-in effect shaders `fx.chroma_key`, `fx.level`,
//! `fx.math` and `fx.projection` draw through the forge effect pass, so each
//! must pass the same `metal_forge::check` (naga) a runtime shader does.

#[test]
fn every_parity_shader_validates() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/shaders");
    for id in ["chroma_key", "level", "math", "projection"] {
        let text = std::fs::read_to_string(dir.join(format!("{id}.wgsl"))).expect("built-in shader on disk");
        if let Err(e) = metal_forge::check(&text) {
            panic!("{id}.wgsl does not validate: {e:?}");
        }
    }
}
