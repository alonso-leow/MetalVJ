//! The WGSL editor's route surface (`§9.4 V2`): `GET /shaders` lists the
//! shader ids with each one's kind, `GET /shaders/<id>` reads one's source,
//! and `POST /shaders {id, wgsl}` is the **one writer** of
//! `data/shaders/<id>.wgsl`.
//!
//! The operator's editor (`shell/panels/props_shader.rs`) and the AI's
//! `save_shader` tool both go through this route (rule 6), so a shader the
//! model saves and one the operator saves pass the same gate. That gate is
//! `metal_forge::check` (naga parse + validate + the float block), applied
//! before anything is written: a shader that does not check comes back
//! `{ok: false, errors: [...]}` and **no file is touched**. Under a build
//! without `gpu` `metal-forge` is not linked, so the write is refused with
//! its reason, exactly as the shell's older `op:"shader"` gate does.

use std::path::Path;

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

/// `GET /shaders` — every shader id on disk with its kind, so the editor's
/// picker can tell an `fx.forge` effect from a `gen.forge` generator without
/// reading each source itself. Reuses `metal_perform::forge`'s own listing.
pub fn list(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(json!({ "shaders": list_at(&metal_perform::output::data_root()) }))
}

fn list_at(root: &Path) -> Vec<Value> {
    let generators = metal_perform::forge::generators(root);
    metal_perform::forge::list(root)
        .into_iter()
        .map(|id| {
            let kind = if generators.contains(&id) {
                "generator"
            } else {
                "effect"
            };
            json!({ "id": id, "kind": kind })
        })
        .collect()
}

/// `GET /shaders/<id>` — one shader's source, saved file first then the
/// few-shot example, the same fallback the renderer uses.
pub fn get(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(id) = params.tail.as_deref().filter(|s| !s.is_empty()) else {
        return Err(CallError::Bad("which shader".into()));
    };
    get_at(&metal_perform::output::data_root(), id)
}

fn get_at(root: &Path, id: &str) -> Result<Value, CallError> {
    let wgsl = metal_perform::forge::source(root, id).map_err(CallError::Bad)?;
    Ok(json!({ "id": id, "wgsl": wgsl }))
}

/// `POST /shaders {id, wgsl}` — validate, then write. The one writer: a
/// failure to validate leaves the file alone.
pub fn save(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(id) = body["id"].as_str().filter(|s| !s.is_empty()) else {
        return Err(CallError::Bad("which shader id".into()));
    };
    let Some(wgsl) = body["wgsl"].as_str() else {
        return Err(CallError::Bad("which wgsl".into()));
    };
    save_at(&metal_perform::output::data_root(), id, wgsl)
}

/// The write itself, over an explicit root so the route and the tests share
/// one body (rule 8: a test builds its fixture in a tempdir).
#[cfg(feature = "gpu")]
fn save_at(root: &Path, id: &str, wgsl: &str) -> Result<Value, CallError> {
    if !metal_perform::forge::valid_id(id) {
        return Err(CallError::Bad(format!(
            "{id:?} is not a shader id (letters, digits, `-`, `_`)"
        )));
    }
    let checked = match metal_forge::check(wgsl) {
        Ok(c) => c,
        Err(e) => return Ok(json!({ "ok": false, "errors": [e.to_string()] })),
    };
    let dir = metal_perform::forge::dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| CallError::Internal(e.to_string()))?;
    let path = dir.join(format!("{id}.wgsl"));
    std::fs::write(&path, wgsl)
        .map_err(|e| CallError::Internal(format!("{}: {e}", path.display())))?;
    let kind = if checked.effect {
        "effect"
    } else {
        "generator"
    };
    Ok(json!({ "ok": true, "kind": kind }))
}

#[cfg(not(feature = "gpu"))]
fn save_at(_root: &Path, _id: &str, _wgsl: &str) -> Result<Value, CallError> {
    Err(CallError::Bad(
        "saving a shader needs the gpu feature (the naga check)".into(),
    ))
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;

    /// A fixture root nothing else uses; the test removes it when done.
    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("metal-dash-shaders-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// (a) Invalid WGSL comes back with naga's errors and **writes nothing**;
    /// (b) valid WGSL is written and reads back through the same route's read.
    #[test]
    fn post_shaders_validates_before_it_writes() {
        let root = temp_root("validate");
        let effect = include_str!("../../../../data/agent/shaders/cup-caustics.wgsl");

        // (a) the broken shader: errors, and no file on disk.
        let bad =
            save_at(&root, "broken", "fn fs_main( {").expect("a normal result to repair from");
        assert_eq!(bad["ok"], json!(false), "{bad}");
        assert!(
            bad["errors"].as_array().is_some_and(|e| !e.is_empty()),
            "naga's errors are relayed: {bad}"
        );
        assert!(
            !root.join("shaders").join("broken.wgsl").exists(),
            "a shader that does not check is never written"
        );

        // (b) the shader that checks: written, then read back by id.
        let good = save_at(&root, "cup", effect).expect("a checked shader writes");
        assert_eq!(
            (good["ok"].as_bool(), good["kind"].as_str()),
            (Some(true), Some("effect")),
            "{good}"
        );
        let read = get_at(&root, "cup").expect("reads back");
        assert_eq!(read["id"], json!("cup"));
        assert_eq!(read["wgsl"], json!(effect));
        assert_eq!(list_at(&root).len(), 1, "the saved shader is listed once");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// An id is a file name, never a path — `../escape` is refused before
    /// anything is written.
    #[test]
    fn post_shaders_refuses_an_id_that_is_a_path() {
        let root = temp_root("id");
        let effect = include_str!("../../../../data/agent/shaders/cup-caustics.wgsl");
        assert!(matches!(
            save_at(&root, "../escape", effect),
            Err(CallError::Bad(_))
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
}
