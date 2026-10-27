//! One ONNX Runtime wrapper for every model the repo loads.
//!
//! D6 makes this the single place the ONNX Runtime decision lives: Basic Pitch
//! is the first user, the Colourist models the next, and a swap to a pure-Rust
//! engine would be a change to this crate alone rather than to every caller.
//!
//! The surface is deliberately small — load a session (optionally against a
//! recorded hash), run one named input, hand back every output as a plain
//! [`Tensor`]. That is everything a frame-posterior model and a future
//! classifier both need, and nothing they do not.

use std::path::Path;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

pub mod fetch;
pub mod registry;

/// A dense `f32` tensor: the shape in row-major order and the data flat.
///
/// The plan's callers (Basic Pitch framing, the tone reducer) want raw floats
/// keyed by shape, not an `ndarray` view — converting once here keeps
/// `ndarray` an implementation detail of `ort` rather than a type the rest of
/// the workspace has to know.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// Every way loading or running a model can fail.
#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("onnx runtime error: {0}")]
    Ort(String),
    #[error("sha256 mismatch: expected {expected}, got {got}")]
    HashMismatch { expected: String, got: String },
    #[error("bad shape: {0}")]
    BadShape(String),
}

/// A loaded inference session, remembering the input name and output names so
/// a caller can run it without re-reading the graph metadata each time.
///
/// `ort`'s `Session::run` takes `&mut self`; a `Mutex` lets [`Model::run`]
/// stay `&self` (one shared model across the digest's inference thread) and
/// keeps the handle `Sync` for whatever thread the caller puts it on.
pub struct Model {
    session: Mutex<ort::session::Session>,
    input: String,
    outputs: Vec<String>,
}

impl Model {
    /// Load a session from `path`, reading off its first input name and its
    /// output names for the runs that follow.
    pub fn load(path: &Path) -> Result<Model, MlError> {
        Self::open(path, false)
    }

    /// [`Model::load`] for a small model that runs next to the renderer (Basic
    /// Pitch, 230 KB): one thread per session, no memory-pattern cache and no
    /// CPU memory arena. The default session keeps a per-session arena that
    /// grows to the largest window it has ever run and never gives it back,
    /// and ORT's thread pool competes with the UI for cores.
    pub fn load_light(path: &Path) -> Result<Model, MlError> {
        Self::open(path, true)
    }

    fn open(path: &Path, light: bool) -> Result<Model, MlError> {
        // `commit` answers `false` when an environment already exists (a
        // second detector, a test) — the normal case, not an error, exactly
        // as metal-sense's MediaPipe detector does it.
        ort::init().commit();
        let mut builder = ort::session::Session::builder().map_err(|e| MlError::Ort(e.to_string()))?;
        let mut builder = if light {
            let ort_err = |e: ort::Error<ort::session::builder::SessionBuilder>| MlError::Ort(e.to_string());
            let cpu = ort::ep::CPU::default().with_arena_allocator(false).build();
            builder
                .with_intra_threads(1)
                .map_err(ort_err)?
                .with_inter_threads(1)
                .map_err(ort_err)?
                .with_memory_pattern(false)
                .map_err(ort_err)?
                .with_execution_providers([cpu])
                .map_err(ort_err)?
        } else {
            builder
        };
        let session = builder
            .commit_from_file(path)
            .map_err(|e| MlError::Ort(format!("{}: {e}", path.display())))?;
        let input = session
            .inputs()
            .first()
            .map(|o| o.name().to_string())
            .ok_or_else(|| MlError::BadShape(format!("{} has no inputs", path.display())))?;
        let outputs = session
            .outputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect();
        Ok(Model {
            session: Mutex::new(session),
            input,
            outputs,
        })
    }

    /// [`Model::load`] guarded by a recorded sha256.
    ///
    /// An **empty** `sha256` means "hash check disabled" — the placeholder a
    /// `model.toml` carries until the real file has been fetched and hashed,
    /// and the only way to load a model whose hash is not known yet. A
    /// non-empty hash is compared case-insensitively so a TOML written in
    /// upper case still matches.
    pub fn load_checked(path: &Path, sha256: &str) -> Result<Model, MlError> {
        if !sha256.is_empty() {
            let got = sha256_file(path)?;
            if !got.eq_ignore_ascii_case(sha256) {
                return Err(MlError::HashMismatch {
                    expected: sha256.to_string(),
                    got,
                });
            }
        }
        Model::load(path)
    }

    /// The input name a caller must not have to know: the model's own first
    /// input, captured at load time.
    pub fn input_name(&self) -> &str {
        &self.input
    }

    /// The output names, in graph order. Informational; [`Model::run`] already
    /// returns every output positionally.
    pub fn output_names(&self) -> &[String] {
        &self.outputs
    }

    /// Run one input through the session and return every output.
    ///
    /// `shape` is the tensor shape of `input` in row-major order (for Basic
    /// Pitch's audio window, `[1, samples]`). Each output is copied out of the
    /// runtime's buffer into owned [`Tensor`]s because the `SessionOutputs`
    /// borrow the session, which the `Mutex` releases on return.
    pub fn run(&self, input: &[f32], shape: &[usize]) -> Result<Vec<Tensor>, MlError> {
        let tensor = ort::value::Tensor::from_array((shape, input.to_vec()))
            .map_err(|e| MlError::BadShape(e.to_string()))?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| MlError::Ort("session mutex poisoned".into()))?;
        let outputs = session
            .run(ort::inputs![self.input.as_str() => tensor])
            .map_err(|e| MlError::Ort(e.to_string()))?;
        let mut out = Vec::with_capacity(outputs.len());
        for i in 0..outputs.len() {
            let (shape, data) = outputs[i]
                .try_extract_tensor::<f32>()
                .map_err(|e| MlError::Ort(e.to_string()))?;
            out.push(Tensor {
                shape: shape.iter().map(|d| *d as usize).collect(),
                data: data.to_vec(),
            });
        }
        Ok(out)
    }
}

/// The lowercase hex sha256 of a file's bytes.
///
/// Exposed so `fetch` and tests can verify a download without loading it into
/// an ONNX session first — a hash check must happen before any `ort` call.
pub fn sha256_file(path: &Path) -> Result<String, MlError> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}
