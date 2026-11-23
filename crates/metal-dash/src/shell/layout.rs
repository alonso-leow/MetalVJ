//! The shell's layout memory (`metal_config::Layout`): every resizable
//! panel's size and the graph canvas's view, saved to the main config file
//! and restored on the next start.
//!
//! egui already remembers each panel's size in its own memory, but that
//! memory dies with the process. This module is the bridge: on the first
//! frame it seeds egui's memory from the config (egui reads a panel's stored
//! size before it falls back to `default_size`), and on later frames it reads
//! the sizes back out and writes the config when they changed. The panel
//! names here are the config's keys; the `egui` ids are the ones the panels
//! already paint under (`regions.rs`, `panels/library.rs`, `mod.rs`).

use egui::containers::panel::PanelState;
use egui::{Context, Id, Pos2, Rect, Vec2};
use metal_config::{CanvasView, Layout};

/// Which axis a panel is resized along — the one its saved size is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    /// A left panel: the saved size is its width.
    Width,
    /// The bottom row: the saved size is its height.
    Height,
}

/// Every panel the layout memory covers: config key, egui id, axis.
pub(crate) const PANELS: &[(&str, &str, Axis)] = &[
    ("bottom-row", "shell/regions/bottom-row", Axis::Height),
    ("properties-col", "shell/region/properties-col", Axis::Width),
    ("library-col", "shell/region/library-col", Axis::Width),
    ("library-dirs", "shell/region/library/dirs", Axis::Width),
    ("library-sidebar", "shell/regions/library-sidebar", Axis::Width),
    ("harness-sidebar", "shell/harness-sidebar", Axis::Width),
];

/// The saved sizes as egui memory entries. A panel with no saved size is left
/// alone, so it keeps its theme default.
pub(crate) fn seed(ctx: &Context, layout: &Layout) {
    for (key, id, _) in PANELS {
        if let Some(&size) = layout.panels.get(*key) {
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(size));
            ctx.data_mut(|d| d.insert_persisted(Id::new(*id), PanelState { outer_rect: rect }));
        }
    }
}

/// The layout as egui currently holds it: every panel that has been shown at
/// least once, plus the canvas view the caller passes in.
pub(crate) fn capture(ctx: &Context, canvas: Option<CanvasView>) -> Layout {
    let mut layout = Layout { canvas, ..Layout::default() };
    for (key, id, axis) in PANELS {
        if let Some(state) = PanelState::load(ctx, Id::new(*id)) {
            let size = match axis {
                Axis::Width => state.outer_rect.width(),
                Axis::Height => state.outer_rect.height(),
            };
            layout.panels.insert((*key).to_string(), size);
        }
    }
    layout
}

/// The config path the layout is written to, when the app is run for real.
/// Tests never set one, so a kittest harness never touches the operator's
/// config file.
pub(crate) fn config_file() -> std::path::PathBuf {
    metal_config::default_path(&metal_config::config_dir())
}

/// Writes `layout` into the config file at `path`, keeping every other section
/// as it is. A file that does not parse is left alone rather than rewritten
/// from defaults.
pub(crate) fn write(path: &std::path::Path, layout: Layout) -> Result<(), String> {
    let mut cfg = metal_config::Config::load(path)?;
    cfg.layout = layout;
    cfg.save(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_sizes_come_back_out_of_capture() {
        let ctx = Context::default();
        let mut layout = Layout::default();
        layout.panels.insert("bottom-row".to_string(), 412.5);
        layout.panels.insert("library-dirs".to_string(), 180.0);
        layout.panels.insert("harness-sidebar".to_string(), 300.0);
        seed(&ctx, &layout);
        let back = capture(&ctx, None);
        assert_eq!(back.panels, layout.panels);
        // A panel nobody seeded or showed is absent, not zero.
        assert!(!back.panels.contains_key("properties-col"));
    }

    #[test]
    fn write_keeps_the_other_sections_of_the_file() {
        let dir = std::env::temp_dir().join(format!("mvj-layout-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        metal_vfs::write(&path, "version = 2\nschema = \"fw/config/2\"\n[render]\ntick_level = \"fine\"\n").unwrap();
        let mut layout = Layout::default();
        layout.canvas = Some(CanvasView { pan: [1.0, 2.0], zoom: 0.5 });
        write(&path, layout.clone()).unwrap();
        let loaded = metal_config::Config::load(&path).unwrap();
        assert_eq!(loaded.layout, layout);
        assert_eq!(loaded.render.tick_level, metal_schema::TickRate::Fine);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
