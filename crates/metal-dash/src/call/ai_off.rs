//! The AI routes on a build without the desktop feature. `metal-agent` (and its
//! TLS stack) is desktop-only, so every AI route answers with the same refusal:
//! the route exists, the build cannot serve it. See `call::ai` for the real
//! handlers, which this file must mirror by name.

use super::{CallError, Ctx, Params};
use serde_json::Value;

fn refuse() -> Result<Value, CallError> {
    Err(CallError::Internal("this build has no AI harness (desktop feature off)".into()))
}

pub fn get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn post(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn ask(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn log(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn chats(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn chats_new(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn chat_get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn chat_cancel(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn chat_approve(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn chat_delete(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}

pub fn train(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    refuse()
}
