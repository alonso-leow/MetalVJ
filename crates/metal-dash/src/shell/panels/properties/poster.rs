//! The hover poster of a model in the surface source dropdown: the
//! `model-demo` JPEG (`GET /models/demo.jpg`) captioned with the model's name
//! and look line (`GET /models/demo`). When the demo was never rendered the
//! same slot shows the model's live solo tile instead.
//!
//! Only the hovered model is ever fetched or decoded; what was fetched stays
//! in a small per-thread cache so a second hover costs nothing.

use std::cell::RefCell;
use std::collections::HashMap;

use egui::{TextureHandle, TextureOptions, Ui};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{note, small_text};
use crate::shell::frames::{decode_jpeg_rgb, Frame};
use crate::shell::i18n::tr_dyn;

use super::shared::get;

/// What one hover has learned about a model.
struct Entry {
    name: String,
    look: String,
    /// The decoded `demos/<id>.jpg`, when the file exists.
    still: Option<TextureHandle>,
    /// The live fallback, ticked only while `still` is missing.
    live: Frame,
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Entry>> = RefCell::new(HashMap::new());
}

/// How many models were fetched this thread (a test reads it to prove a
/// second hover is a cache hit).
#[cfg(test)]
pub(crate) fn cached() -> usize {
    CACHE.with(|c| c.borrow().len())
}

#[cfg(test)]
pub(crate) fn forget() {
    CACHE.with(|c| c.borrow_mut().clear());
}

fn load(ui: &Ui, door: &dyn Call, id: &str) -> Entry {
    let q = [("id", id.to_string())];
    let info = get(door, "GET", "/models/demo", &q);
    let text = |k: &str| info.as_ref().and_then(|v| v[k].as_str()).unwrap_or_default().to_string();
    let still = door
        .bytes("GET", "/models/demo.jpg", &q)
        .ok()
        .and_then(|reply| decode_jpeg_rgb(&reply.body))
        .map(|(w, h, rgb)| {
            let image = egui::ColorImage::from_rgb([w as usize, h as usize], &rgb);
            ui.ctx().load_texture(format!("model-poster/{id}"), image, TextureOptions::LINEAR)
        });
    Entry { name: text("name"), look: text("look"), still, live: Frame::default() }
}

/// The poster card for `id`, drawn inside a hover popup.
pub(crate) fn show(ui: &mut Ui, theme: &Theme, door: &dyn Call, id: &str) {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let entry = cache.entry(id.to_string()).or_insert_with(|| load(ui, door, id));
        let width = theme.poster_width;
        let picture = match &entry.still {
            Some(tex) => Some((tex.id(), [tex.size()[0], tex.size()[1]])),
            None => {
                let ratio = theme.poster_ratio;
                let query = [
                    ("model", id.to_string()),
                    ("w", (width as u32).to_string()),
                    ("h", ((width / ratio) as u32).to_string()),
                ];
                entry.live.picture(
                    ui.ctx(),
                    door,
                    "/author/render/solo.jpg",
                    &query,
                    crate::shell::frames::live_gap(),
                )
            }
        };
        if let Some((tex, size)) = picture {
            let aspect = size[1] as f32 / size[0].max(1) as f32;
            ui.image((tex, egui::vec2(width, width * aspect)));
        } else {
            ui.allocate_space(egui::vec2(width, width / theme.poster_ratio));
        }
        let name = if entry.name.is_empty() { id } else { entry.name.as_str() };
        ui.label(small_text(theme, tr_dyn(name)));
        if !entry.look.is_empty() {
            note(ui, theme, &entry.look);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::FakeDoor;
    use egui_kittest::Harness;
    use serde_json::json;

    fn paths(door: &FakeDoor) -> Vec<String> {
        door.calls.borrow().iter().map(|(_, p, _)| p.clone()).collect()
    }

    fn draw(door: &FakeDoor, id: &'static str) {
        let theme = Theme::phosphor();
        let mut harness = Harness::new_ui(|ui| show(ui, &theme, door, id));
        harness.run();
    }

    /// With the JPEG on disk the card shows it, captioned, and never asks for
    /// the live tile; a second draw is a cache hit (no new door call).
    #[test]
    fn the_poster_shows_the_demo_jpeg_and_caches_it() {
        forget();
        let door = FakeDoor::new();
        door.respond("GET", "/models/demo", json!({ "id": "turing", "name": "Turing", "look": "generative" }));
        door.respond_bytes("GET", "/models/demo.jpg", "image/jpeg", crate::paint::encode_jpeg(&[200u8; 8 * 8 * 4], 8, 8));
        draw(&door, "turing");
        let first = paths(&door);
        assert!(first.contains(&"/models/demo.jpg".to_string()));
        assert!(!first.iter().any(|p| p.contains("solo")), "{first:?}");
        draw(&door, "turing");
        assert_eq!(paths(&door), first, "the second hover is served from the cache");
        assert_eq!(cached(), 1);
    }

    /// No JPEG: the same slot asks for the model's live solo tile.
    #[test]
    fn a_missing_jpeg_falls_back_to_the_live_tile() {
        forget();
        let door = FakeDoor::new();
        door.respond("GET", "/models/demo", json!({ "id": "rings", "name": "Rings", "look": "generative" }));
        door.respond_error("GET", "/models/demo.jpg", 404, "not found");
        door.respond_bytes("GET", "/author/render/solo.jpg", "image/jpeg", crate::paint::encode_jpeg(&[90u8; 8 * 8 * 4], 8, 8));
        draw(&door, "rings");
        let seen = paths(&door);
        assert!(seen.contains(&"/author/render/solo.jpg".to_string()), "{seen:?}");
        let calls = door.calls.borrow();
        let solo = calls.iter().find(|(_, p, _)| p == "/author/render/solo.jpg").unwrap();
        assert_eq!(solo.2.query.get("model").map(String::as_str), Some("rings"));
    }
}
