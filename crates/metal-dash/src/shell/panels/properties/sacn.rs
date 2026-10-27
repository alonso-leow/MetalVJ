//! `out.sacn`'s own panel. Universe and unicast are written through the same
//! `POST /graph/node {op:"config"}` merge [`super::text::post_text`] uses.
//! "last sent" is the one field this section does not write: it is `GET
//! /graph`'s own `wire.last_sent_ms_ago`, attached by `call::graph::graph_get`
//! straight off the running engine's socket cache — a live read every time this
//! section draws, never a value kept here between frames (rule 4).

use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{field_row, label_row, number_row};
use crate::shell::i18n::{tr, trf};

use super::text::post_text;

pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
) {
    let cfg = &node["config"];

    let mut universe = cfg["universe"].as_u64().unwrap_or(1).clamp(1, 63999) as f32;
    if number_row(
        ui,
        theme,
        tr("universe"),
        &mut universe,
        1.0..=63999.0,
        "",
        0,
        true,
    )
    .changed()
    {
        post_text(
            door,
            gid,
            node_id,
            json!({ "universe": universe.round() as u32 }),
        );
    }

    let mut unicast = cfg["unicast"].as_str().unwrap_or_default().to_string();
    if label_row(ui, theme, tr("unicast"), |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut unicast)
                .hint_text(tr("multicast when empty"))
                .desired_width(theme.control_width),
        )
    })
    .changed()
    {
        let value = if unicast.trim().is_empty() {
            Value::Null
        } else {
            json!(unicast)
        };
        post_text(door, gid, node_id, json!({ "unicast": value }));
    }

    let text = match node["wire"]["last_sent_ms_ago"].as_u64() {
        Some(ms) => trf("{ms} ms ago", &[("ms", &ms)]),
        None => tr("— never —").to_string(),
    };
    field_row(ui, theme, tr("last sent"), &text);
}
