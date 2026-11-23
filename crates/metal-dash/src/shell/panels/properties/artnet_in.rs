//! P-D: `in.artnet`'s own panel — Net / Sub-Net / Universe (the 15-bit
//! Port-Address) the row listens to, and the channel window it holds
//! (`first`, `count`). Written through `POST /graph/node {op:"config"}`, as
//! every node's panel is; the row itself is read by bindings (`control.outN`).

use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{field_row, number_row};
use crate::shell::i18n::{tr, trf};

use super::text::post_text;

pub(crate) fn section(ui: &mut Ui, theme: &Theme, door: &dyn Call, gid: &str, node_id: &str, node: &Value) {
    let cfg = &node["config"];
    let parts = [("net", tr("net"), 127.0), ("subnet", tr("subnet"), 15.0), ("universe", tr("universe"), 15.0)];
    for (key, label, max) in parts {
        let mut v = cfg[key].as_u64().unwrap_or(0).min(max as u64) as f32;
        if number_row(ui, theme, label, &mut v, 0.0..=max, "", 0, true).changed() {
            post_text(door, gid, node_id, json!({ key: v.round() as u32 }));
        }
    }
    let pa = metal_perform::artnet::port_address(
        cfg["net"].as_u64().unwrap_or(0) as u8,
        cfg["subnet"].as_u64().unwrap_or(0) as u8,
        cfg["universe"].as_u64().unwrap_or(0) as u8,
    );
    field_row(ui, theme, tr("port-address"), &pa.to_string());

    let mut first = cfg["first"].as_u64().unwrap_or(1).clamp(1, 512) as f32;
    if number_row(ui, theme, tr("first channel"), &mut first, 1.0..=512.0, "", 0, true).changed() {
        post_text(door, gid, node_id, json!({ "first": first.round() as u32 }));
    }
    let mut count = cfg["count"].as_u64().unwrap_or(16).clamp(1, 512) as f32;
    if number_row(ui, theme, tr("channels"), &mut count, 1.0..=512.0, "", 0, true).changed() {
        post_text(door, gid, node_id, json!({ "count": count.round() as u32 }));
    }
    let last = (count as u32).saturating_sub(1);
    field_row(ui, theme, tr("row"), &trf("control.out0 … control.out{last}", &[("last", &last)]));

    // The live readout: the one listener's state (`GET /artnet/in`, the route
    // the AI's `artnet_in_status` reads too — rule 6) and the values this
    // window reads now.
    let path = format!(
        "/artnet/in?net={}&subnet={}&universe={}&first={}&count={}",
        cfg["net"].as_u64().unwrap_or(0),
        cfg["subnet"].as_u64().unwrap_or(0),
        cfg["universe"].as_u64().unwrap_or(0),
        first as u32,
        count as u32,
    );
    let Some(r) = door.json("GET", &path, None).ok() else {
        field_row(ui, theme, tr("listener"), tr("— not read yet —"));
        return;
    };
    let state = if r["listening"].as_bool() == Some(true) { tr("listening") } else { tr("not listening") };
    field_row(ui, theme, tr("listener"), &state);
    if let Some(e) = r["error"].as_str() {
        field_row(ui, theme, tr("error"), e);
    }
    let values: Vec<String> = r["row"]
        .as_array()
        .map(|a| a.iter().take(16).map(|v| format!("{:.2}", v.as_f64().unwrap_or(0.0))).collect())
        .unwrap_or_default();
    let mut text = values.join(" ");
    if r["row"].as_array().is_some_and(|a| a.len() > 16) {
        text.push_str(" …");
    }
    field_row(ui, theme, tr("values"), &text);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
}
