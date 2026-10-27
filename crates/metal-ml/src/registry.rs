//! The model registry under `data/models/<id>/`.
//!
//! Each model is a folder holding the binary plus a `model.toml` that records
//! where it came from and what its outputs mean (D6/D9). Keeping the metadata
//! beside the file — and the file itself uncommitted and git-ignored — is what
//! lets the same checkout run with or without a fetched model: the folder is
//! absent until someone fetches it, and the digest simply falls back to chroma.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{MlError, Model};

/// One model's manifest, read from `data/models/<id>/model.toml`.
///
/// `id` comes from the directory name rather than the file so the folder and
/// the manifest can never disagree about which model they describe.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    #[serde(default)]
    pub id: String,
    /// The expected lowercase hex sha256 of `file`. Empty means "check
    /// disabled" — see [`Model::load_checked`].
    #[serde(default)]
    pub sha256: String,
    /// The binary's filename inside the folder; Basic Pitch ships `model.onnx`
    /// but a folder may hold more than one file, or name it differently.
    #[serde(default = "default_file")]
    pub file: String,
    /// Where `fetch` downloads the binary from, when the manifest names one.
    /// Empty means "no canonical source recorded": fetch with an explicit URL.
    #[serde(default)]
    pub url: String,
    /// The graph's input names, for documentation and for callers that wire
    /// more than one. The runtime uses the session's own first input.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// The graph's output names. Callers select outputs by name: the session's
    /// order is not the manifest's order (Basic Pitch lists `:1`/`:2` but the
    /// graph returns `:2, :1, :0`).
    #[serde(default)]
    pub outputs: Vec<String>,
    /// The licence the model is distributed under, so the attribution lives
    /// with the model and not only in a root the licences that can drift.
    #[serde(default)]
    pub licence: String,
}

fn default_file() -> String {
    "model.onnx".to_string()
}

/// Read `data/models/<id>/model.toml` into a [`ModelEntry`].
pub fn load(dir: &Path, id: &str) -> Result<ModelEntry, MlError> {
    let path = dir.join(id).join("model.toml");
    let text = std::fs::read_to_string(&path)?;
    let mut entry: ModelEntry = toml::from_str(&text)
        .map_err(|e| MlError::BadShape(format!("{}: {e}", path.display())))?;
    entry.id = id.to_string();
    Ok(entry)
}

/// The binary's full path: `<dir>/<id>/<file>`.
pub fn path_for(dir: &Path, entry: &ModelEntry) -> PathBuf {
    dir.join(&entry.id).join(&entry.file)
}

/// Load an entry and its session in one step, hash-checked against the
/// manifest. This is the call a caller who just wants "the model, if it is
/// here" makes.
pub fn open(dir: &Path, id: &str) -> Result<(ModelEntry, Model), MlError> {
    let entry = load(dir, id)?;
    let path = path_for(dir, &entry);
    let model = Model::load_checked(&path, &entry.sha256)?;
    Ok((entry, model))
}
