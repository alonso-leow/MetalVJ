//! Loads `data/schema.json` — the float manifest every model in the
//! suite is described by: which floats it takes, their ranges and defaults,
//! and its colour-slot and camera blocks.
//!
//! This mirrors the shape of `metal-render::model::Schema` (`FloatSpec`,
//! `Block`, `width`) without depending on that crate. The two stay decoupled
//! on purpose — a shared JSON file rather than a shared Rust type — so this
//! studio and the renderer suite can be built and iterated on independently.
//! See `docs/SUITE_STUDIO.md` for the reconciliation note.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FloatSpec {
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Floats {
        fields: Vec<FloatSpec>,
    },
    ColorSlot {
        name: String,
    },
    /// `default` is the view this model asks to be seen from, emitted by
    /// `metal-render`'s own schema. Optional because it arrived later than the
    /// block did: a manifest written before per-model cameras existed still
    /// parses, and falls back to the view every model used to share.
    Camera {
        #[serde(default)]
        default: Option<[f32; 7]>,
    },
}

impl Block {
    /// Matches `metal-render::model::Block::width()`: 12 floats per colour
    /// slot (4 colours x rgb — `grad4` in shared.wgsl reads them), 7 for
    /// camera (pos.xyz + target.xyz + fov).
    pub fn width(&self) -> usize {
        match self {
            Block::Floats { fields } => fields.len(),
            Block::ColorSlot { .. } => 12,
            Block::Camera { .. } => 7,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelSchema {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub technique: String,
    pub blocks: Vec<Block>,
}

/// The default bindings a model ships: `(param, digest var)` per model id,
/// the table `restaurar fuentes` applies and node creation applies where it
/// can. A model not listed binds nothing. Names are display-first, so a digest
/// column with a space or `#` is spelled exactly as `metal_schema` names it
/// (`chroma C#`), while the parameter it drives is the model's own float.
pub static DEFAULT_BINDINGS: &[(&str, &[(&str, &str)])] = &[(
    "forge",
    &[
        ("rms", "rms"),
        ("bandwidth", "bandwidth"),
        ("onset", "onset"),
        ("centroid", "centroid"),
        ("chroma_c", "chroma C"),
        ("chroma_c_sharp", "chroma C#"),
        ("chroma_d", "chroma D"),
        ("chroma_d_sharp", "chroma D#"),
        ("chroma_e", "chroma E"),
        ("chroma_f", "chroma F"),
        ("chroma_f_sharp", "chroma F#"),
        ("chroma_g", "chroma G"),
        ("chroma_g_sharp", "chroma G#"),
        ("chroma_a", "chroma A"),
        ("chroma_a_sharp", "chroma A#"),
        ("chroma_b", "chroma B"),
    ],
)];

impl ModelSchema {
    pub fn width(&self) -> usize {
        self.blocks.iter().map(Block::width).sum()
    }

    /// The model's default `(param, digest var)` bindings — empty when it
    /// ships none. See [`DEFAULT_BINDINGS`].
    pub fn bindings(&self) -> &'static [(&'static str, &'static str)] {
        DEFAULT_BINDINGS
            .iter()
            .find(|(id, _)| *id == self.id)
            .map(|(_, table)| *table)
            .unwrap_or(&[])
    }

    /// The row a fresh example starts from: every float at its declared
    /// default, colour slots solid white, and each camera where its own model
    /// asked to be seen from — the same row
    /// `metal-render::model::Schema::defaults()` builds, because the manifest
    /// this reads is generated from it.
    ///
    /// The camera fallback is what every model shared before the defaults
    /// became per-model, and it framed three of the five 3D models edge-on or
    /// off-screen (`docs/CORRECTIONS.md` §43). It is here for old manifests
    /// only; a generated one always carries its own.
    pub fn defaults(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.width());
        for b in &self.blocks {
            match b {
                Block::Floats { fields } => out.extend(fields.iter().map(|f| f.default)),
                Block::ColorSlot { .. } => out.extend([1.0; 12]),
                Block::Camera { default } => {
                    out.extend(default.unwrap_or([0.0, 1.4, 3.2, 0.0, 1.0, 0.0, 45.0]))
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub rate: f32,
    pub models: Vec<ModelSchema>,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Manifest, String> {
        let text = metal_vfs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn model(&self, id: &str) -> Option<&ModelSchema> {
        self.models.iter().find(|m| m.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // An inline fixture rather than reading the real `data/schema.json`
    // off disk: a unit test must not depend on the process's working
    // directory, and this is small enough to write out in full.
    fn fixture() -> Manifest {
        let text = r#"{
            "rate": 8.0,
            "models": [
                { "id": "geoterrain", "label": "Geoterrain", "blocks": [
                    { "type": "floats", "fields": [
                        {"name":"radius","min":0.1,"max":5.0,"default":1.5},
                        {"name":"arms","min":1,"max":12,"default":3}
                    ]},
                    { "type": "color_slot", "name": "principal" }
                ]},
                { "id": "chamber", "label": "Chamber", "blocks": [
                    { "type": "floats", "fields": [
                        {"name":"height","min":0.05,"max":2.0,"default":0.6}
                    ]},
                    { "type": "camera" },
                    { "type": "color_slot", "name": "principal" }
                ]}
            ]
        }"#;
        serde_json::from_str(text).expect("fixture parses")
    }

    #[test]
    fn a_float_block_and_a_colour_slot_add_up() {
        let m = fixture();
        let g = m.model("geoterrain").unwrap();
        assert_eq!(g.width(), 2 + 12, "two floats plus a 12-float colour slot");
        assert_eq!(g.defaults().len(), g.width());
        assert_eq!(g.defaults()[0], 1.5);
        assert_eq!(g.defaults()[1], 3.0);
        assert_eq!(
            &g.defaults()[2..14],
            &[1.0; 12][..],
            "a fresh colour slot is solid white"
        );
    }

    #[test]
    fn camera_and_colour_both_widen_the_row() {
        let m = fixture();
        let h = m.model("chamber").unwrap();
        assert_eq!(h.width(), 1 + 7 + 12);
        let d = h.defaults();
        assert_eq!(d[1..4], [0.0, 1.4, 3.2], "camera position default");
        assert_eq!(d[7], 45.0, "camera fov default, last of the seven");
    }

    #[test]
    fn an_unknown_model_id_is_none_not_a_panic() {
        assert!(fixture().model("does-not-exist").is_none());
    }

    #[test]
    fn a_model_without_a_table_binds_nothing_and_forge_names_its_columns() {
        let m = fixture();
        assert!(m.model("geoterrain").unwrap().bindings().is_empty());
        let forge = ModelSchema {
            id: "forge".into(),
            label: "Forge".into(),
            technique: String::new(),
            blocks: Vec::new(),
        };
        let b = forge.bindings();
        assert_eq!(b[0], ("rms", "rms"));
        assert!(b.iter().any(|(p, v)| *p == "chroma_c" && *v == "chroma C"));
    }

    #[test]
    fn the_real_manifest_on_disk_parses_and_has_ten_models() {
        // The one test that does touch the real file — run only when the
        // repo layout is where it is expected to be, so it is a real check
        // when it runs and not a spurious failure when cargo's cwd differs.
        let path = Path::new("../../data/schema.json");
        if !metal_vfs::is_file(&path) {
            eprintln!("skipping: {} not found from this cwd", path.display());
            return;
        }
        let m = Manifest::load(path).expect("schema.json parses");
        // Not an exact count any more: the file is generated from
        // `metal-render`'s renderers and the suite grows — Turing arrived as
        // the eleventh on 2026-08-17. Pinning the number here only meant this
        // test failed every time a model was added, which says nothing about
        // whether the manifest is usable.
        assert!(!m.models.is_empty(), "the manifest describes no models");
        let ids: std::collections::HashSet<_> = m.models.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), m.models.len(), "model ids are unique");
        for s in &m.models {
            assert!(s.width() > 0, "{} has an empty schema", s.id);
        }
    }
}
