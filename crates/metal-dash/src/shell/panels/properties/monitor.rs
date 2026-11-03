//! The Monitor group: whether each output is really sending its image, and the
//! hand-over that stops it. A venue node lists every output; an `output.device`
//! node shows its own. Hand over is a `cede` on `POST /outputs`: the output is
//! blocked here until it is taken back — no separate switch.

use egui::{RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::i18n::{tr, trf};

use super::shared::get;

/// The label of one output's chip: where it shows, or who holds it.
pub(crate) fn chip_label(id: &str, node: Option<&str>, ceded_to: Option<&str>) -> String {
    match (ceded_to, node) {
        (Some(to), _) => trf("{id} · handed to {to}", &[("id", &id), ("to", &to)]),
        (None, Some(n)) => trf("{id} · on {n}", &[("id", &id), ("n", &n)]),
        (None, None) => trf("{id} · local", &[("id", &id)]),
    }
}

/// What the output is really doing, read off `GET /state`: blocked by a
/// hand-over, calibrating, or sending its image.
pub(crate) fn status_label(output: &Value) -> String {
    if let Some(to) = output["ceded_to"].as_str() {
        return trf("blocked · handed to {to}, not sending", &[("to", &to)]);
    }
    if output["calibrating"].as_bool() == Some(true) {
        return tr("calibrating").to_string();
    }
    tr("sending image").to_string()
}

/// The Monitor rows: one per output (or only `device`, when given). Each shows
/// its chip and status, and a button to hand it over or take it back.
pub(crate) fn rows(ui: &mut Ui, theme: &Theme, door: &dyn Call, device: Option<&str>) {
    let outputs: Vec<Value> = get(door, "GET", "/state", &[])
        .and_then(|s| s["outputs"].as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|o| device.is_none_or(|d| o["id"].as_str() == Some(d)))
        .collect();
    if outputs.is_empty() {
        ui.label(
            RichText::new(tr("no output on this machine"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    }
    for o in &outputs {
        let Some(id) = o["id"].as_str() else { continue };
        let ceded = o["ceded_to"].as_str();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(chip_label(id, o["node"].as_str(), ceded))
                    .font(theme.font_small.clone())
                    .color(theme.ink),
            );
            ui.label(
                RichText::new(status_label(o))
                    .font(theme.font_small.clone())
                    .color(if ceded.is_some() { theme.warn } else { theme.ink_weak }),
            );
            let verb = if ceded.is_some() { tr("take back") } else { tr("hand over") };
            if ui.button(RichText::new(verb).font(theme.font_small.clone())).clicked() {
                let to = if ceded.is_some() { Value::Null } else { json!("otro programa") };
                let _ = door.json("POST", "/outputs", Some(json!({ "op": "cede", "id": id, "to": to })));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_label_shows_local_when_neither_node_nor_ceded_to_is_set() {
        assert_eq!(chip_label("screen-1", None, None), "screen-1 · local");
    }

    #[test]
    fn chip_label_shows_the_node_when_nobody_holds_it_ceded() {
        assert_eq!(chip_label("screen-1", Some("otro-pc"), None), "screen-1 · on otro-pc");
    }

    /// Ceded wins over where it is shown — the order the operator cares about.
    #[test]
    fn chip_label_prefers_ceded_to_over_node_when_both_are_set() {
        assert_eq!(
            chip_label("screen-1", Some("otro-pc"), Some("resolume")),
            "screen-1 · handed to resolume"
        );
    }

    #[test]
    fn a_handed_over_output_reads_as_blocked_not_sending() {
        let o = json!({ "id": "screen-1", "ceded_to": "resolume" });
        assert!(status_label(&o).contains("not sending"));
    }

    #[test]
    fn an_output_nobody_holds_reads_as_sending() {
        let o = json!({ "id": "screen-1", "ceded_to": null, "calibrating": false });
        assert_eq!(status_label(&o), "sending image");
    }
}
