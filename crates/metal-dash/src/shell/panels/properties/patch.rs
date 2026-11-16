//! The patch view (M4.3 of the pixel map): every fixture's wire, universe,
//! address and channel span, with overlapping addresses flagged.
//!
//! The rows come straight from `GET /venue`'s own `patch` array (rule 5): the
//! route computes each span from the profile's channels, so the panel never
//! re-derives what a profile holds. The overlap flag is
//! [`crate::shell::panels::venue_editor::patch_rows`]'s — the same function
//! the editor's own patch folder paints with, one implementation.

use egui::{RichText, Ui};
use serde_json::Value;

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::i18n::tr;
use crate::shell::panels::venue_editor::{patch_rows, State as EditorState};

pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    venue_id: &str,
    editor: &mut EditorState,
) {
    let read = get(door, "/venue", &[("id", venue_id)]);
    let Some(read) = read else {
        crate::shell::components::note(ui, theme, tr("— could not read the venue —"));
        return;
    };
    let rows = patch_rows(&read);
    if rows.is_empty() {
        crate::shell::components::note(ui, theme, tr("— no fixtures —"));
        return;
    }
    let mut open = true;
    crate::shell::components::section(ui, theme, tr("patch"), &mut open);
    if !open {
        return;
    }
    let conflicts = rows.iter().filter(|r| r.conflict).count();
    if conflicts > 0 {
        ui.label(
            RichText::new(format!("{} {conflicts}", tr("address overlaps")))
                .font(theme.font_small.clone())
                .color(theme.phosphor_lit),
        );
    }
    egui::Grid::new("venue-patch/table")
        .striped(true)
        .show(ui, |ui| {
            for head in ["fixture", "wire", "universe", "address", "span"] {
                ui.label(
                    RichText::new(tr(head))
                        .font(theme.font_small.clone())
                        .color(theme.ink_faint),
                );
            }
            ui.end_row();
            for r in &rows {
                let selected = editor.selected.as_deref() == Some(r.fixture.as_str());
                let colour = if r.conflict {
                    theme.phosphor_lit
                } else if selected {
                    theme.ink
                } else {
                    theme.ink_weak
                };
                for cell in [
                    r.fixture.clone(),
                    r.wire.clone(),
                    r.universe.to_string(),
                    r.address.to_string(),
                    if r.conflict {
                        format!("{} ", r.span)
                    } else {
                        r.span.to_string()
                    },
                ] {
                    ui.label(
                        RichText::new(cell)
                            .font(theme.font_small.clone())
                            .color(colour),
                    );
                }
                ui.end_row();
            }
        });
}

fn get(door: &dyn Call, path: &str, query: &[(&str, &str)]) -> Option<Value> {
    let params = crate::call::Params {
        query: query
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        body: Vec::new(),
        tail: None,
    };
    match door.call("GET", path, params) {
        Some(reply) if reply.status == 200 => crate::json_memo::parse(&reply.body).ok(),
        _ => None,
    }
}
