//! Runtime shaders on air: where a `forge` source's WGSL comes from, and
//! how `Paint::run` finds it.
//!
//! Two node kinds draw runtime WGSL:
//!
//! - **`gen.forge`** — a source. Its layer's [`crate::layers::Layer::shader`]
//!   (copied onto the node's config as `shader`) names the file; its row is
//!   the sixteen ingest columns as they arrive ([`ingest_row`]).
//! - **`fx.forge`** — an effect, wired after any source. Its node config's
//!   `shader` names an effect shader, run over the picture it is handed.
//!
//! The contract both are written against is `metal_render::models::forge`'s
//! module doc. The files live in `data/shaders/<id>.wgsl` — where
//! `save_shader` writes and `read_shader`/`raster` read — with the few-shot
//! examples in `data/agent/shaders/` as the fallback, the way venues fall
//! back to `data/agent/venues/`.

use std::path::{Path, PathBuf};

/// The effect node's kind.
pub const FX_KIND: &str = "fx.forge";

/// The model id of the generator.
pub const MODEL: &str = "forge";

/// Where saved shaders live.
pub fn dir(root: &Path) -> PathBuf {
    root.join("shaders")
}

/// Where the example shaders live.
pub fn examples_dir(root: &Path) -> PathBuf {
    root.join("agent").join("shaders")
}

/// A shader id is a file name, never a path.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A shader reference a node's `shader` config may hold: a plain id, or one
/// inside `presets/` (`presets/kaleido` → `data/shaders/presets/kaleido.wgsl`).
/// Only that one subfolder — the reference is still a file name, never a path.
pub fn valid_ref(id: &str) -> bool {
    valid_id(id) || id.strip_prefix("presets/").is_some_and(valid_id)
}

/// The WGSL a shader id names: `data/shaders/<id>.wgsl`, else the example of
/// that name.
pub fn source(root: &Path, id: &str) -> Result<String, String> {
    if !valid_ref(id) {
        return Err(format!("{id:?} is not a shader id (letters, digits, `-`, `_`)"));
    }
    let file = format!("{id}.wgsl");
    for path in [dir(root).join(&file), examples_dir(root).join(&file)] {
        if let Ok(text) = metal_vfs::read_to_string(&path) {
            return Ok(text);
        }
    }
    Err(format!("no shader {id:?} in {} or the examples", dir(root).display()))
}

/// Every shader id on disk, saved ones first, examples after, each once.
pub fn list(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in [dir(root), examples_dir(root)] {
        let Ok(entries) = metal_vfs::read_dir(&d) else { continue };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()? != "wgsl" {
                    return None;
                }
                Some(p.file_stem()?.to_string_lossy().to_string())
            })
            .filter(|id| !out.contains(id))
            .collect();
        ids.sort();
        out.extend(ids);
    }
    out
}

/// Whether a shader is an **effect**: it declares the picture an `fx.forge`
/// works on, a sampled 2D texture at `@group(0) @binding(1)` and a sampler at
/// `@binding(2)` (the rule `metal_forge::check` applies with naga; this is the
/// text form, for listings built without the `gpu` feature).
pub fn is_effect(wgsl: &str) -> bool {
    let texture = wgsl.lines().any(|l| {
        let l: String = l.chars().filter(|c| !c.is_whitespace()).collect();
        l.contains("@group(0)@binding(1)") && l.contains("texture_2d<")
    });
    let sampler = wgsl.lines().any(|l| {
        let l: String = l.chars().filter(|c| !c.is_whitespace()).collect();
        l.contains("@group(0)@binding(2)") && l.contains("sampler")
    });
    texture && sampler
}

/// The shader ids a `gen.forge` source can use: [`list`] without the effects.
/// Each is read the way `source` reads it (saved file first, then example).
pub fn generators(root: &Path) -> Vec<String> {
    list(root)
        .into_iter()
        .filter(|id| source(root, id).is_ok_and(|wgsl| !is_effect(&wgsl)))
        .collect()
}

/// The shader a node's config names, if any.
pub fn node_shader(node: &metal_graph::Node) -> Option<String> {
    if node.config.is_empty() {
        return None;
    }
    let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
    cfg.get("shader")?.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// What an `fx.forge` shader is handed as its ingest: the digest as it
/// arrives, with each of `metal_graph::params::FORGE_FX_PARAMS` the node's
/// config holds (a loop's per-frame value, loops T11) pinned over its column.
pub fn fx_ingest(node: &metal_graph::Node, raw: &[f32]) -> Vec<f32> {
    let mut out = raw.to_vec();
    if out.len() < metal_graph::params::FORGE_FX_PARAMS.len() {
        out.resize(metal_graph::params::FORGE_FX_PARAMS.len(), 0.0);
    }
    let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).unwrap_or_default();
    for (i, key) in metal_graph::params::FORGE_FX_PARAMS.iter().enumerate() {
        if let Some(v) = cfg.get(*key).and_then(serde_json::Value::as_f64) {
            out[i] = (v as f32).clamp(0.0, 1.0);
        }
    }
    out
}

/// A `forge` row's own floats: the ingest, as it arrives. Every other model
/// gets `drive::row`'s modulation around the operator's setting; a shader
/// somebody wrote against "rms in `floats2.x`" gets rms.
pub fn ingest_row(row: &mut [f32], frame: &crate::digest::Frame) {
    for (dst, src) in row.iter_mut().zip(frame.f.iter()) {
        *dst = src.clamp(0.0, 1.0);
    }
}

/// Says once, on stderr, why a shader did not draw — a frame is drawn many
/// times a second and a broken shader would otherwise fill the log.
pub fn report(id: &str, why: &str) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = (id.to_string(), why.to_string());
    if !seen.contains(&entry) {
        eprintln!("  forge: shader {id} does not draw: {why}");
        seen.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    #[test]
    fn the_examples_are_found_by_id_and_ids_are_names_not_paths() {
        assert!(source(&root(), "turtle").unwrap().contains("fs_main"));
        assert!(source(&root(), "cup-caustics").unwrap().contains("texture_2d"));
        assert!(source(&root(), "../schema").is_err());
        assert!(source(&root(), "no-such-shader").is_err());
        let ids = list(&root());
        assert!(ids.contains(&"turtle".to_string()) && ids.contains(&"cup-caustics".to_string()));
    }

    /// `metal-render`'s forge schema names its sixteen floats after the digest
    /// columns, in digest order; the manifest is how this crate sees it.
    #[test]
    fn the_forge_rows_are_the_digest_columns() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../data/schema.json")).unwrap();
        let forge = manifest["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == MODEL)
            .expect("the manifest lists forge");
        let text = forge.to_string();
        // The forge model's floats are the music prefix, as they always were;
        // the voice block is not an ingest float there.
        for column in crate::digest::COLUMNS[..crate::digest::MUSIC].iter() {
            let ident = column.to_lowercase().replace(' ', "_").replace('#', "_sharp");
            assert!(text.contains(&format!("\"{ident}\"")), "forge names no float {ident:?}");
        }
    }

    #[test]
    fn an_effect_is_told_apart_from_a_generator() {
        let effect = include_str!("../../../data/agent/shaders/cup-caustics.wgsl");
        let generator = include_str!("../../../data/agent/shaders/water.wgsl");
        assert!(is_effect(effect), "samples src and its sampler");
        assert!(!is_effect(generator), "declares no input picture");
    }

    #[test]
    fn generators_lists_only_the_sources() {
        let root = std::env::temp_dir().join(format!("forge-generators-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(dir(&root)).unwrap();
        let effect = include_str!("../../../data/agent/shaders/cup-caustics.wgsl");
        let generator = include_str!("../../../data/agent/shaders/water.wgsl");
        std::fs::write(dir(&root).join("cup.wgsl"), effect).unwrap();
        std::fs::write(dir(&root).join("pool.wgsl"), generator).unwrap();
        let all = list(&root);
        assert!(all.contains(&"cup".to_string()) && all.contains(&"pool".to_string()));
        let gens = generators(&root);
        assert!(gens.contains(&"pool".to_string()), "a generator is offered");
        assert!(!gens.contains(&"cup".to_string()), "an effect is not offered as a source");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_node_names_its_shader_in_its_config() {
        let mut n = metal_graph::Node::new("fx-1", FX_KIND, [0.0, 0.0]);
        assert_eq!(node_shader(&n), None);
        n.config = metal_graph::Config::from_json_str(r#"{"shader":"cup-caustics"}"#).unwrap();
        assert_eq!(node_shader(&n).as_deref(), Some("cup-caustics"));
    }
}
