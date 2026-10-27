//! `POST /frame/export` — "Exportar cuadro" (parity lote P-G): the floating
//! preview's camera button and the `export_frame` AI tool both post here
//! (regla 6). It draws the previewed node's current frame at the device's
//! full resolution and writes it as a PNG to
//! `<dir>/<node>-<YYYYMMDD-HHMMSS>.png`.
//!
//! Body: `{ device, node?, model?, mask?, name?, dir? }`. No `node` is the
//! device's composed frame; `model` (+`mask`) is an authored sequence's row
//! (the ingest-lane preview) at the device's resolution; `name` names the file;
//! `dir` overrides the folder for one call (tests, the agent); otherwise the
//! `export_dir` setting, otherwise `~/Pictures/MetalVJ`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

/// The default folder, under `$HOME`.
pub fn default_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Pictures").join("MetalVJ")
}

pub fn export(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json { device, node? }".into()));
    };
    // A sequence's authored row (`model`, cut by `mask`), at the device's
    // resolution (1920×1080 with none) — the picture `/author/render/solo.jpg`
    // previews.
    let model = body.get("model").and_then(Value::as_str).filter(|s| !s.is_empty());
    let device = body.get("device").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let node = body.get("node").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let (device, w, h) = match (device, model) {
        (Some(d), _) => {
            let out = ctx.state.outputs.find(&d).cloned().ok_or(CallError::NotFound)?;
            (d, out.width.max(1), out.height.max(1))
        }
        (None, Some(_)) => (String::new(), 1920, 1080),
        (None, None) => return Err(CallError::Bad("which device".into())),
    };
    // G3: the device's own frame stays a texture; the PNG is written once the
    // GPU has finished the copy, on a thread, so the export holds nothing.
    if node.is_none() && model.is_none() {
        let out = ctx.state.outputs.find(&device).cloned().ok_or(CallError::NotFound)?;
        if let metal_perform::engine::DeviceFrame::Gpu(frame) =
            crate::call::render::device_texture(ctx.paint, ctx.sim, ctx.state, &out, w, h)
        {
            if let Some(wait) = ctx.paint.readback_deferred(&frame) {
                let dir = body
                    .get("dir")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(crate::call::settings::export_dir);
                let name = body
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&device)
                    .to_string();
                let (pw, ph) = (frame.width(), frame.height());
                let path = target_path(&dir, &name);
                let (to, rgba_dir) = (path.clone(), dir.clone());
                std::thread::spawn(move || {
                    let px = wait();
                    if let Err(e) = std::fs::create_dir_all(&rgba_dir).map_err(|e| e.to_string()).and_then(|_| write_png_to(&to, &px, pw, ph)) {
                        eprintln!("  export_frame: {e}");
                    }
                });
                return Ok(json!({ "path": path.to_string_lossy(), "width": pw, "height": ph }));
            }
        }
    }
    let (pixels, pw, ph) = match (&node, model) {
        (_, Some(model)) => {
            let mask = body.get("mask").and_then(Value::as_str);
            ctx.author.solo_pixels(model, Some(device.as_str()).filter(|d| !d.is_empty()), mask, w, h)?
        }
        (Some(node), None) => crate::render_node(ctx.paint, ctx.sim, ctx.state, &device, node, w, h)
            .ok_or(CallError::NotFound)?,
        (None, None) => crate::render_device(ctx.paint, ctx.sim, ctx.state, &device, w, h),
    };
    let dir = body
        .get("dir")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(crate::call::settings::export_dir);
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or(node.as_deref())
        .unwrap_or(&device);
    let path = write_png(&dir, name, &pixels, pw, ph).map_err(CallError::Bad)?;
    Ok(json!({ "path": path.to_string_lossy(), "width": pw, "height": ph }))
}

/// Writes `rgba` as `<dir>/<name>-<stamp>.png`, creating `dir`.
pub fn write_png(dir: &Path, name: &str, rgba: &[u8], w: u32, h: u32) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = target_path(dir, name);
    write_png_to(&path, rgba, w, h)?;
    Ok(path)
}

/// `<dir>/<name>-<stamp>.png`, the name made filesystem-safe.
fn target_path(dir: &Path, name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    dir.join(format!("{safe}-{}.png", stamp()))
}

fn write_png_to(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
    let file = std::fs::File::create(&path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// `YYYYMMDD-HHMMSS` in local time for now.
fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::call::dispatch;
    use crate::chdir_lock;

    /// The route draws the device at its full resolution and writes a PNG
    /// into the `dir` it was handed (a tempdir, never `~/Pictures`).
    #[test]
    fn export_frame_writes_a_full_resolution_png_into_the_dir() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let mut paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        state.outputs.list.push(crate::output::Output::new("probe", "Probe", 96, 54));
        let mut sim = crate::listeners::Listeners::stand_in();
        let dir = std::env::temp_dir().join(format!("mvj-export-frame-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let body = json!({ "device": "probe", "dir": dir.to_string_lossy() }).to_string();
        let params = Params { query: HashMap::new(), body: body.into_bytes(), tail: None };
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let reply = dispatch("POST", "/frame/export", &params, &mut ctx).expect("dispatches");
        assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
        let v: Value = serde_json::from_slice(&reply.body).expect("json");
        let path = PathBuf::from(v["path"].as_str().expect("path"));
        assert!(path.starts_with(&dir), "{path:?} not under {dir:?}");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("probe-") && name.ends_with(".png"), "{name}");
        // On the GPU backend the PNG is written once the copy lands, off the
        // request thread: wait for a complete file.
        let mut size = None;
        for _ in 0..500 {
            if let Ok(file) = std::fs::File::open(&path) {
                if let Ok(reader) = png::Decoder::new(std::io::BufReader::new(file)).read_info() {
                    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
                    let mut reader = reader;
                    if reader.next_frame(&mut buf).is_ok() {
                        size = Some((reader.info().width, reader.info().height));
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(size, Some((96, 54)));
        let _ = std::fs::remove_dir_all(&dir);

        // No body / unknown device: refused, nothing written.
        let empty = Params::default();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        assert_eq!(dispatch("POST", "/frame/export", &empty, &mut ctx).unwrap().status, 400);
    }

    /// E2: with `node`, the route writes the PNG of that graph node's solo
    /// output at the device's native resolution — `render_node`, the same path
    /// `/node.jpg` and a sequence row's solo read.
    #[test]
    fn capture_export_frame_writes_a_nodes_png() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let mut paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        let device = format!("export-node-{}", std::process::id());
        state.outputs.list.push(crate::output::Output::new(&device, "Node", 96, 54));
        state.sources.entry(device.clone()).or_insert_with(crate::layers::Device::default);
        let mut sim = crate::listeners::Listeners::stand_in();

        // A generator node in the device's own graph, so `render_node` draws.
        let root = crate::output::data_root();
        let model = paint.ids()[0].to_string();
        let mut graph =
            metal_graph::Graph::new(&crate::device_graph_id(&device), &device, metal_graph::Domain::Live);
        let gen = graph.add_node("gen", &format!("gen.{model}"), [0.0, 0.0]);
        crate::save_graph_at(&root, &graph).expect("save the device graph");

        let dir = std::env::temp_dir().join(format!("mvj-export-node-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let body = json!({ "device": device, "node": gen, "dir": dir.to_string_lossy() }).to_string();
        let params = Params { query: HashMap::new(), body: body.into_bytes(), tail: None };
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let reply = dispatch("POST", "/frame/export", &params, &mut ctx).expect("dispatches");
        assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.body));
        let v: Value = serde_json::from_slice(&reply.body).expect("json");
        let path = PathBuf::from(v["path"].as_str().expect("path"));
        assert!(path.starts_with(&dir), "{path:?} not under {dir:?}");
        // A node export is CPU bytes written on the request thread, so the PNG
        // is complete when the reply lands.
        let file = std::fs::File::open(&path).expect("the node PNG is written");
        let reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().expect("png");
        assert_eq!((reader.info().width, reader.info().height), (96, 54), "the device's native size");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = metal_vfs::remove_file(crate::graph_path_at(&root, &crate::device_graph_id(&device)));
    }

    #[test]
    fn stamp_is_fifteen_chars() {
        let s = stamp();
        assert_eq!(s.len(), 15, "{s}");
        assert_eq!(&s[8..9], "-");
    }
}
