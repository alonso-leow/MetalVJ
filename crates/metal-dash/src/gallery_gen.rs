//! Test-only: renders the gallery GIFs `shell::gallery_anim` reads. Lives outside
//! `shell/` because it talks to the author directly (the shell may not).

// Writes every `gen.*` and `fx.*`/`mat.*` kind's GIF into the cache. Run by
// hand, once (and again when the look of a kind changes):
//
// `cargo test -p metal-dash --lib gallery_gen::generate -- --ignored --nocapture`
//
// `FW_GALLERY_ONLY=gen.bloom,fx.blur` limits it to some kinds.

    use crate::shell::gallery_anim::{cache_dir, encode, file_name};
    use crate::shell::frames::decode_jpeg_rgb;
    use crate::shell::panels::nodes::{tile_uv, GALLERY_VARIANTS, KIND_H, KIND_W, TILE_COLS, TILE_H, TILE_W};

    fn picture(author: &crate::Author, path: &str, query: &[(&str, String)]) -> Option<(u32, u32, Vec<u8>)> {
        let req = metal_web::Request {
            method: "GET".into(),
            path: path.into(),
            query: query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            headers: Default::default(),
            body: Vec::new(),
        };
        let reply = author.dispatch(&req);
        if reply.status != 200 {
            println!("GALLERY {path} {} {}", reply.status, String::from_utf8_lossy(&reply.body[..reply.body.len().min(200)]));
        }
        (reply.status == 200).then(|| decode_jpeg_rgb(&reply.body)).flatten()
    }

    /// The variants of a `gen.*` model: its cell of the grid at each variant.
    fn model_frames(author: &crate::Author, index: usize, n: usize) -> Option<([usize; 2], Vec<Vec<u8>>)> {
        let uv = tile_uv(index, n);
        let mut out = Vec::new();
        let mut size = [0, 0];
        for k in 0..GALLERY_VARIANTS {
            let query = [
                ("w", TILE_W.to_string()),
                ("h", TILE_H.to_string()),
                ("cols", TILE_COLS.to_string()),
                ("cells", n.to_string()),
                ("variant", k.to_string()),
            ];
            let (w, h, rgb) = picture(author, "/render/grid.jpg", &query)?;
            let (w, h) = (w as usize, h as usize);
            let x0 = (uv.min.x * w as f32).round() as usize;
            let y0 = (uv.min.y * h as f32).round() as usize;
            let cw = (uv.width() * w as f32).round() as usize;
            let ch = (uv.height() * h as f32).round() as usize;
            let mut cell = Vec::with_capacity(cw * ch * 3);
            for y in y0..y0 + ch {
                let row = (y * w + x0) * 3;
                cell.extend_from_slice(&rgb[row..row + cw * 3]);
            }
            size = [cw, ch];
            out.push(cell);
        }
        Some((size, out))
    }

    /// The variants of an `fx.*` or `mat.*` kind, its own picture at each: the
    /// engine's `render_kind`, the call `GET /author/render/kind.jpg` answers with.
    fn kind_frames(paint: &mut dyn metal_perform::paint::Paint, sim: &mut metal_perform::listeners::Listeners, state: &mut metal_perform::engine::State, kind: &str) -> Option<([usize; 2], Vec<Vec<u8>>)> {
        let mut out = Vec::new();
        let mut size = [0, 0];
        for k in 0..GALLERY_VARIANTS {
            let (rgba, w, h) = metal_perform::engine::render_kind(paint, sim, state, kind, k, KIND_W, KIND_H)?;
            size = [w as usize, h as usize];
            out.push(rgba.chunks_exact(4).flat_map(|px| [px[0], px[1], px[2]]).collect());
        }
        Some((size, out))
    }

    #[test]
    #[ignore = "renders the gallery GIFs into ~/.cache/metalvj/gallery"]
    fn generate() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root");
        let mut paint = metal_perform::paint::painter();
        let mut sim = metal_perform::listeners::Listeners::stand_in();
        let mut engine = metal_perform::engine::State::new(paint.as_ref());
        let author = crate::Author::with_gpu(None);
        std::env::set_current_dir(&prior).expect("restore cwd");

        let state = author.dispatch(&metal_web::Request {
            method: "GET".into(),
            path: "/render/state".into(),
            query: Default::default(),
            headers: Default::default(),
            body: Vec::new(),
        });
        let state: serde_json::Value = serde_json::from_slice(&state.body).expect("state json");
        let names: Vec<String> = state["names"].as_array().map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let manifest = crate::node_manifest().expect("manifest");
        let fx: Vec<String> = metal_graph::NodeKind::catalog(&manifest)
            .into_iter()
            .map(|k| k.kind)
            .filter(|k| k.starts_with("fx.") || k.starts_with("mat."))
            .collect();
        let only: Option<Vec<String>> = std::env::var("FW_GALLERY_ONLY").ok().map(|s| s.split(',').map(str::to_string).collect());
        let wanted = |kind: &str| only.as_ref().map_or(true, |o| o.iter().any(|k| k == kind));

        println!("GALLERY {} models, {} fx kinds", names.len(), fx.len());
        let dir = cache_dir().expect("HOME");
        std::fs::create_dir_all(&dir).expect("cache dir");
        let mut written = 0;
        for (i, model) in names.iter().enumerate() {
            let kind = format!("gen.{model}");
            if !wanted(&kind) {
                continue;
            }
            if let Some((size, frames)) = model_frames(&author, i, names.len()) {
                std::fs::write(dir.join(file_name(&kind)), encode(size, &frames).expect("gif")).expect("write");
                written += 1;
            }
        }
        for kind in fx.iter().filter(|k| wanted(k)) {
            if let Some((size, frames)) = kind_frames(paint.as_mut(), &mut sim, &mut engine, kind) {
                std::fs::write(dir.join(file_name(kind)), encode(size, &frames).expect("gif")).expect("write");
                written += 1;
            }
        }
        println!("GALLERY wrote {written} GIFs to {}", dir.display());
    }
