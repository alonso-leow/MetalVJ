//! 2026-10-15: `GET /author/image.jpg?path=<file>` — a still served as JPEG.
//!
//! The floating preview and the video-scenes gallery point at a file on disk
//! (an absolute path, as an authored `still`/`reel.batch` item carries it) and
//! need one picture back. JPEG is decoded by `metal_reel::index::decode`; a PNG
//! is decoded by the same small helper `metal_perform`'s `reel.batch` uses.
//! Either way the pixels go out through the shared [`crate::paint::encode_jpeg`],
//! so a caller sees one content type whatever it pointed at.

use std::path::Path;

use crate::call::{Bytes, CallError, Params};

use super::Ctx;

/// One image file, as JPEG. The `path` is absolute — the same path the
/// authored item named, never resolved against the data root here.
pub fn image(_ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let path = params
        .param("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| CallError::Bad("path?".into()))?;
    if !metal_vfs::is_file(Path::new(path)) {
        return Err(CallError::Bad(format!("no such image: {path}")));
    }
    let (rgba, w, h) = decode_rgba(Path::new(path)).ok_or_else(|| CallError::Bad(format!("cannot decode {path}")))?;
    Ok(Bytes { kind: "image/jpeg", body: crate::paint::encode_jpeg(&rgba, w, h) })
}

/// A file's pixels as RGBA8 (what [`crate::paint::encode_jpeg`] takes): a JPEG
/// through `metal_reel::index::decode`, a PNG through [`decode_png`]. `None`
/// when neither decodes.
fn decode_rgba(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    if let Ok(rgb) = metal_reel::index::decode(path) {
        let px = rgb.px.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect();
        return Some((px, rgb.width, rgb.height));
    }
    decode_png(path)
}

/// A PNG as RGBA8 (8-bit gray/gray-alpha/RGB/RGBA; palettes expanded), the
/// same helper `metal_perform`'s `reel.batch` uses.
fn decode_png(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let bytes = metal_vfs::read(path).ok()?;
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let n = (w * h) as usize;
    let px: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..n * 4].to_vec(),
        png::ColorType::Rgb => buf[..n * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..n * 2].chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf[..n].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((px, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `author::State::new` reads `data/schema.json` off a bare relative path,
    /// so the cwd is the repo root for its construction, as in `mod`'s own
    /// tests; the handler itself ignores the state.
    fn ctx_state() -> crate::author::State {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&repo_root).expect("chdir to repo root");
        let state = crate::author::State::new(None, std::path::PathBuf::from("data/tracks/untitled.track.json"));
        std::env::set_current_dir(&prior).expect("restore cwd");
        state
    }

    /// A JPEG written to a tempdir is served back as `image/jpeg` bytes the
    /// gallery's own decoder can read; a missing path is refused.
    #[test]
    fn the_route_serves_a_jpeg_the_gallery_can_decode() {
        let dir = std::env::temp_dir().join(format!("fw-author-image-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let path = dir.join("still.jpg");
        let rgba: Vec<u8> = (0..4 * 3).flat_map(|_| [200u8, 40, 10, 255]).collect();
        metal_vfs::write(&path, crate::paint::encode_jpeg(&rgba, 4, 3)).unwrap();

        let mut state = ctx_state();
        let mut ctx = Ctx { state: &mut state };
        let params = Params {
            query: [("path".to_string(), path.to_string_lossy().into_owned())].into_iter().collect(),
            body: Vec::new(),
            tail: None,
        };
        let bytes = image(&mut ctx, &params).expect("the route serves");
        assert_eq!(bytes.kind, "image/jpeg");
        let (w, h, rgb) = crate::shell::frames::decode_jpeg_rgb(&bytes.body).expect("decodes");
        assert_eq!((w, h), (4, 3));
        assert_eq!(rgb.len(), 4 * 3 * 3);

        assert!(image(&mut ctx, &Params::default()).is_err(), "no path is refused");
        let missing = Params {
            query: [("path".to_string(), dir.join("nope.jpg").to_string_lossy().into_owned())].into_iter().collect(),
            body: Vec::new(),
            tail: None,
        };
        assert!(image(&mut ctx, &missing).is_err(), "a non-file is refused");
        let _ = metal_vfs::remove_dir_all(&dir);
    }

    /// The same route decodes a PNG (via the `reel.batch` helper) and answers
    /// JPEG, so a gallery mixing both formats reads one content type.
    #[test]
    fn a_png_is_served_as_jpeg_too() {
        let dir = std::env::temp_dir().join(format!("fw-author-image-png-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        let path = dir.join("still.png");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 3, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let px: Vec<u8> = (0..3 * 2).flat_map(|_| [10u8, 220, 30, 255]).collect();
            enc.write_header().unwrap().write_image_data(&px).unwrap();
        }

        let mut state = ctx_state();
        let mut ctx = Ctx { state: &mut state };
        let params = Params {
            query: [("path".to_string(), path.to_string_lossy().into_owned())].into_iter().collect(),
            body: Vec::new(),
            tail: None,
        };
        let bytes = image(&mut ctx, &params).expect("the route serves");
        let (w, h, rgb) = crate::shell::frames::decode_jpeg_rgb(&bytes.body).expect("decodes");
        assert_eq!((w, h), (3, 2));
        assert!(rgb.chunks_exact(3).all(|c| c[1] > 150 && c[0] < 80), "green came through");
        let _ = metal_vfs::remove_dir_all(&dir);
    }
}
