//! P-D: `out.artnet`'s own panel — Net / Sub-Net / Universe (the 15-bit
//! Port-Address), unicast or broadcast, ArtSync, the live "last sent" off
//! `GET /graph` (as `out.sacn`'s), and "buscar nodos": `GET /artnet/nodes`
//! (ArtPoll), the same route the AI's `artnet_nodes` tool reads (rule 6).
//! Clicking a found node's port writes its IP and Port-Address back through
//! `POST /graph/node {op:"config"}`.

use egui::Ui;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{field_row, label_row, number_row, toggle_row};
use crate::shell::i18n::{tr, trf};

use super::text::post_text;

/// How long a discovery listens, in ms: short, since it blocks the frame.
const DISCOVER_MS: u32 = 700;

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

    for (key, label, hint) in [
        ("unicast", tr("unicast"), tr("broadcast when empty")),
        ("broadcast", tr("broadcast"), "255.255.255.255"),
    ] {
        let mut text = cfg[key].as_str().unwrap_or_default().to_string();
        if label_row(ui, theme, label, |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).hint_text(hint).desired_width(theme.control_width))
        })
        .changed()
        {
            let value = if text.trim().is_empty() { Value::Null } else { json!(text) };
            post_text(door, gid, node_id, json!({ key: value }));
        }
    }

    let mut sync = cfg["sync"].as_bool().unwrap_or(false);
    if toggle_row(ui, theme, tr("ArtSync after each frame"), &mut sync).changed() {
        post_text(door, gid, node_id, json!({ "sync": sync }));
    }

    let text = match node["wire"]["last_sent_ms_ago"].as_u64() {
        Some(ms) => trf("{ms} ms ago", &[("ms", &ms)]),
        None => tr("— never —").to_string(),
    };
    field_row(ui, theme, tr("last sent"), &text);

    // Discovery: kept in egui's temp memory per node, so it survives frames
    // without a field on the panel's state.
    let found_id = egui::Id::new(("artnet-found", node_id));
    if ui.button(tr("find nodes")).clicked() {
        let reply = door.json("GET", &format!("/artnet/nodes?ms={DISCOVER_MS}"), None);
        let value = match reply {
            Ok(v) => v,
            Err((_, msg)) => json!({ "error": msg }),
        };
        ui.data_mut(|d| d.insert_temp(found_id, value));
    }
    let found: Option<Value> = ui.data(|d| d.get_temp(found_id));
    let Some(found) = found else { return };
    if let Some(msg) = found["error"].as_str() {
        field_row(ui, theme, tr("discovery"), msg);
        return;
    }
    let nodes = found["nodes"].as_array().cloned().unwrap_or_default();
    if nodes.is_empty() {
        field_row(ui, theme, tr("discovery"), tr("— no node answered —"));
    }
    for n in nodes {
        let ip = n["ip"].as_str().unwrap_or_default().to_string();
        let name = n["short_name"].as_str().unwrap_or_default();
        field_row(ui, theme, &ip, name);
        let outputs: Vec<u64> = n["outputs"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        for out in outputs {
            let (net, sub, uni) = metal_perform::artnet::split_port_address(out as u16);
            if ui.small_button(trf("use port {pa}", &[("pa", &out)])).clicked() {
                post_text(
                    door,
                    gid,
                    node_id,
                    json!({ "unicast": ip, "net": net, "subnet": sub, "universe": uni }),
                );
            }
        }
    }
}
