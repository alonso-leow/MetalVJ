//! The per-device output page (`/out/<device>`) that gets dragged onto a
//! venue screen. It answers HTML, so it is `Handler::Bytes`.
//!
//! There is no `GET /` any more (S.U18): the client is the native egui
//! window, and the only HTTP surface is the API plus this page. The HTML
//! dashboard it once lived beside was deleted with its build step and its
//! harness — see `plan-sonnet.md` S.U18.

use crate::call::{Bytes, CallError, Ctx, Params};

/// `GET /out/<device>` — the prefix route's tail names the device; a device
/// that does not exist is a 404, same as before.
pub fn out_device(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let id = params.tail.as_deref().unwrap_or_default();
    if ctx.state.outputs.find(id).is_none() {
        return Err(CallError::NotFound);
    }
    Ok(Bytes {
        kind: "text/html; charset=utf-8",
        body: crate::OUT_PAGE.as_bytes().to_vec(),
    })
}
