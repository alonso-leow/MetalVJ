//! Save/load the float rows you set by hand. `data/presets/<model>/
//! <name>.json` — this is what "set the values for the examples" means
//! in practice: move the sliders, save, and get the same image back later.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn presets_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/presets")
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// The shared effects, re-exported from `metal-schema`.
///
/// It lives there rather than here for the same reason `Schema` does: it is
/// plain data with no graphics in it, and `metal-dash` needs it on hosts with no
/// renderers to link against. Saving and loading a preset is still this
/// module's job — only the type moved.
pub use metal_schema::Post;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Preset {
    pub model: String,
    pub floats: Vec<f32>,
    /// Absent in files written before the effects were saved; those load with
    /// both effects off, which is what they were rendered with anyway.
    #[serde(default)]
    pub post: Post,
}

fn to_io_err(e: serde_json::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
}

pub fn save(model_name: &str, preset_name: &str, floats: &[f32], post: &Post) -> std::io::Result<()> {
    let dir = presets_root().join(sanitize(model_name));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", sanitize(preset_name)));
    let file = Preset {
        model: model_name.to_string(),
        floats: floats.to_vec(),
        post: post.clone(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(to_io_err)?;
    std::fs::write(path, json)
}

pub fn load(model_name: &str, preset_name: &str) -> std::io::Result<Preset> {
    let path = presets_root()
        .join(sanitize(model_name))
        .join(format!("{}.json", sanitize(preset_name)));
    let json = std::fs::read_to_string(path)?;
    serde_json::from_str(&json).map_err(to_io_err)
}

/// Names only (no extension), sorted, for the load dropdown.
pub fn list(model_name: &str) -> Vec<String> {
    let dir = presets_root().join(sanitize(model_name));
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Some(stem) = entry.path().file_stem() {
                out.push(stem.to_string_lossy().into_owned());
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_preset_loads_back_identical() {
        let model = "test-model-roundtrip";
        let floats = vec![0.1, 0.2, 0.3, 1.0, 1.0, 1.0];
        let post = Post {
            version: metal_schema::EFFECTS_VERSION,
            shake: 0.42,
            light_leak: 0.17,
            leak_color: vec![0.5; 12],
            ghost: 0.6,
            kaleido: 6.0,
            crt: 0.3,
            glitch: 0.2,
            posterize: 0.1,
            zoom: -0.4,
            fire: 0.5,
            lorenz: 0.7,
            vignette: 0.8,
            blur: 0.25,
            sharpen: 0.35,
            grain: 0.15,
            // Non-empty on purpose: the graph order is part of the image the
            // same way the amounts above are, so a preset that drops it on
            // the way to disk would be exactly the silent corruption this
            // test exists to catch. Reuses zoom's and shake's own amounts
            // above rather than inventing unrelated numbers.
            chain: vec![
                metal_schema::EffectStep { effect: 7, amount: -0.4, slot: 0 },
                metal_schema::EffectStep { effect: 0, amount: 0.42, slot: 0 },
            ],
            advanced: metal_schema::Post::default().advanced,
        };
        save(model, "example", &floats, &post).expect("save");
        let loaded = load(model, "example").expect("load");
        assert_eq!(floats, loaded.floats);
        assert_eq!(post, loaded.post, "the shared effects are part of the image");
        assert!(list(model).contains(&"example".to_string()));

        // A file written before `post` existed still has to load.
        let dir = presets_root().join(sanitize(model));
        std::fs::write(
            dir.join("old.json"),
            r#"{"model":"test-model-roundtrip","floats":[0.5,0.5]}"#,
        )
        .expect("write legacy file");
        let old = load(model, "old").expect("legacy preset still loads");
        assert_eq!(old.floats, vec![0.5, 0.5]);
        assert_eq!(old.post, Post::default());

        // Clean up so the repo tree does not accumulate test fixtures.
        let _ = std::fs::remove_dir_all(dir);
    }
}
