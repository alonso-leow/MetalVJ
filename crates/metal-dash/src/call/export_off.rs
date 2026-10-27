//! `export.video`'s routes on a build without the `gpu` feature. The exporter
//! renders through `metal_render::headless::Exporter`, and `metal-render` is a
//! `gpu`-gated dependency (see `metal-dash`'s own feature doc), so on a
//! no-adapter build there is no renderer to export with. The routes exist and
//! say so. See `call::export` for the real handlers, which this file mirrors
//! by name.

use super::{CallError, Ctx, Params};
use serde_json::Value;

fn refuse() -> Result<Value, CallError> {
    Err(CallError::Internal("video export needs the gpu feature".into()))
}

pub fn video(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn status(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn control(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}
