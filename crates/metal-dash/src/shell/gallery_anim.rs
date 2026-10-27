//! The gallery's animated pictures: one GIF per `gen.*` and `fx.*` kind, the
//! four float variants a card cycles through, written once by an ignored
//! test ([`generate`]) into `~/.cache/metalvj/gallery/<kind>.gif`.
//!
//! The shell reads a kind's GIF lazily, the first time one of its cards is on
//! screen: it is decoded once and its frames are uploaded as textures once,
//! then kept in the egui context. A kind with no file has no entry and its
//! card keeps the live picture from the render route. Nothing is decoded per
//! frame; a card only asks for a repaint when its next frame is due.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use egui::{ColorImage, Context, Id, TextureHandle, TextureOptions};

/// How long one frame stays up: the gallery's own cadence, one float variant
/// per second.
pub const FRAME_SECS: f64 = 1.0;

/// The directory the GIFs live in (`~/.cache/metalvj/gallery`), or `None`
/// without a home directory.
pub fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/metalvj/gallery"))
}

/// The file a kind's GIF is stored under: the kind's own name, with anything
/// that is not a plain filename character replaced.
pub fn file_name(kind: &str) -> String {
    let safe: String = kind.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
    format!("{safe}.gif")
}

/// The frame a card shows at `time` (seconds): frames advance once per
/// [`FRAME_SECS`] and loop.
pub fn frame_index(time: f64, frames: usize) -> usize {
    if frames == 0 {
        return 0;
    }
    ((time.max(0.0) / FRAME_SECS) as usize) % frames
}

/// Seconds until the next frame of an animation is due at `time`.
pub fn secs_to_next_frame(time: f64) -> f64 {
    FRAME_SECS - time.max(0.0) % FRAME_SECS
}

/// Decodes a GIF into its frames as RGBA pixels, with the size of the canvas.
pub fn decode(bytes: &[u8]) -> Option<([usize; 2], Vec<Vec<u8>>)> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut reader = options.read_info(bytes).ok()?;
    let size = [usize::from(reader.width()), usize::from(reader.height())];
    let mut frames = Vec::new();
    while let Some(frame) = reader.read_next_frame().ok()? {
        frames.push(frame.buffer.to_vec());
    }
    (!frames.is_empty()).then_some((size, frames))
}

/// Encodes RGB frames of one size as a looping GIF, one frame per
/// [`FRAME_SECS`]. The generator writes these; the shell only reads them.
#[cfg(test)]
pub fn encode(size: [usize; 2], frames: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let (w, h) = (size[0] as u16, size[1] as u16);
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, w, h, &[]).map_err(|e| e.to_string())?;
        encoder.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
        for rgb in frames {
            let mut frame = gif::Frame::from_rgb_speed(w, h, rgb, 10);
            frame.delay = (FRAME_SECS * 100.0).round() as u16;
            encoder.write_frame(&frame).map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

/// The textures of every frame of one kind's GIF, shared by its cards.
pub type Frames = Arc<Vec<TextureHandle>>;

/// What the context remembers per kind: the frames, or `None` when the kind
/// has no GIF (so the file is looked for once, not every frame).
#[derive(Clone, Default)]
struct Cache(HashMap<String, Option<Frames>>);

fn cache_id() -> Id {
    Id::new("shell/gallery-anim/cache")
}

/// The animated frames of `kind`'s card, loaded on first use. `None` when the
/// kind has no GIF, or the GIF does not decode.
pub fn frames(ctx: &Context, kind: &str) -> Option<Frames> {
    let hit = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(cache_id()).0.get(kind).cloned());
    if let Some(entry) = hit {
        return entry;
    }
    // Loaded outside the data lock: uploading a texture takes the context.
    let loaded = load(ctx, kind).map(Arc::new);
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(cache_id()).0.insert(kind.to_string(), loaded.clone()));
    loaded
}

fn load(ctx: &Context, kind: &str) -> Option<Vec<TextureHandle>> {
    let path = cache_dir()?.join(file_name(kind));
    let bytes = std::fs::read(path).ok()?;
    let (size, frames) = decode(&bytes)?;
    Some(
        frames
            .iter()
            .enumerate()
            .map(|(i, rgba)| {
                let image = ColorImage::from_rgba_unmultiplied(size, rgba);
                ctx.load_texture(format!("gallery-anim/{kind}/{i}"), image, TextureOptions::LINEAR)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_advance_once_per_second_and_loop() {
        assert_eq!(frame_index(0.0, 4), 0);
        assert_eq!(frame_index(0.99, 4), 0);
        assert_eq!(frame_index(1.0, 4), 1);
        assert_eq!(frame_index(4.0, 4), 0, "loops");
        assert_eq!(frame_index(-3.0, 4), 0, "no negative clock");
        assert_eq!(frame_index(5.0, 0), 0, "no frames, no index");
    }

    #[test]
    fn the_next_frame_is_due_at_the_next_boundary() {
        assert!((secs_to_next_frame(0.25) - 0.75).abs() < 1e-9);
        assert!((secs_to_next_frame(2.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_kind_names_its_file_with_plain_characters_only() {
        assert_eq!(file_name("gen.bloom"), "gen.bloom.gif");
        assert_eq!(file_name("fx/odd name"), "fx_odd_name.gif");
    }

    /// A GIF written by [`encode`] decodes back to the same frames' size and
    /// count (the colours are quantised, so only the shape is compared).
    #[test]
    fn a_gif_round_trips_its_frame_count_and_size() {
        let size = [4, 2];
        let frames: Vec<Vec<u8>> = (0..3).map(|i| vec![(i * 80) as u8; 4 * 2 * 3]).collect();
        let bytes = encode(size, &frames).expect("encodes");
        let (back, decoded) = decode(&bytes).expect("decodes");
        assert_eq!(back, size);
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].len(), 4 * 2 * 4, "RGBA out");
    }
}
