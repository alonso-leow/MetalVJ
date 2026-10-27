//! The machine pickers (2026-10-09): **Compone en** on a screen
//! (`Output.composed_on`) and **Renderiza en** on a source (`Layer.render_on`),
//! both over one list — this machine plus the `metal-sync` peers `GET /peers`
//! relays. An offline peer is listed but greyed out: visible, not pickable.
//!
//! The source's panel also shows the **device pool grouped by machine**: every
//! screen, under the machine that shows it (`Output.node`, this machine when
//! that is null), with the ones this source feeds marked. Read-only — routing
//! a source to a screen stays the layer table's job.
//!
//! Nothing here renders or reaches a peer. The frame route is another
//! agent's; these panels only write the two fields.


use crate::shell::components::small_text;
use egui::{Id, RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::label_row;
use crate::shell::i18n::{tr, trf};

use super::shared::{get, post};

/// How long one `GET /peers` answer is reused, in seconds. `metal-sync` answers
/// on loopback, but a panel repaints every frame; the list does not change
/// that fast.
const PEERS_TTL: f64 = 2.0;

/// One machine a picker can name.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Machine {
    pub(crate) name: String,
    /// This machine is always reachable from itself.
    pub(crate) online: bool,
    pub(crate) local: bool,
}

/// `GET /peers`, cached in egui memory for [`PEERS_TTL`].
fn peers_listing(ui: &Ui, door: &dyn Call) -> Value {
    let id = Id::new("shell/properties/peers-listing");
    let now = ui.input(|i| i.time);
    let cached: Option<(Value, f64)> = ui.ctx().data(|d| d.get_temp(id));
    if let Some((v, _)) = cached.filter(|(_, at)| now - at < PEERS_TTL) {
        return v;
    }
    let v = get(door, "GET", "/peers", &[]).unwrap_or_else(|| json!({}));
    ui.ctx().data_mut(|d| d.insert_temp(id, (v.clone(), now)));
    v
}

/// The machines this instance can name: this one first (always online), then
/// each peer `metal-sync` pairs with, in its own order. A peer whose name
/// repeats this machine's, or another peer's, is dropped.
pub(crate) fn machines(ui: &Ui, door: &dyn Call) -> Vec<Machine> {
    machines_from(&peers_listing(ui, door))
}

/// Pure half of [`machines`]: the `GET /peers` body into the picker's list.
pub(crate) fn machines_from(listing: &Value) -> Vec<Machine> {
    let local = listing["local"].as_str().unwrap_or_default().to_string();
    let mut out = Vec::new();
    if !local.is_empty() {
        out.push(Machine { name: local.clone(), online: true, local: true });
    }
    for p in listing["peers"].as_array().into_iter().flatten() {
        let Some(name) = p["name"].as_str().filter(|n| !n.is_empty()) else {
            continue;
        };
        if out.iter().any(|m| m.name == name) {
            continue;
        }
        out.push(Machine {
            name: name.to_string(),
            online: p["online"].as_bool().unwrap_or(false),
            local: false,
        });
    }
    out
}

/// A machine dropdown. `current` is the machine name, `None` the default:
/// with `default_label` it is a choice of its own (`Some(None)` on pick);
/// without, `None` shows nothing selected. Returns the pick when the operator
/// made one this frame. An offline machine stays in the list, greyed, and
/// cannot be picked. The response is the combo's, for a [`label_row`].
pub(crate) fn machine_picker(
    ui: &mut Ui,
    theme: &Theme,
    id: &str,
    current: Option<&str>,
    machines: &[Machine],
    default_label: Option<&str>,
) -> (egui::Response, Option<Option<String>>) {
    let shown = match (current, default_label) {
        (Some(name), _) => name,
        (None, Some(d)) => d,
        (None, None) => "",
    };
    // A menu rather than a `ComboBox`: its button carries its text as the
    // accessible label, and a menu item can be disabled outright.
    let mut picked: Option<Option<String>> = None;
    let resp = ui
        .push_id(Id::new(id), |ui| {
            ui.menu_button(small_text(theme, shown), |ui| {
                if let Some(d) = default_label {
                    if ui.button(d).clicked() {
                        picked = Some(None);
                    }
                }
                for m in machines {
                    let text = if m.online {
                        m.name.clone()
                    } else {
                        trf("{name} · offline", &[("name", &m.name)])
                    };
                    if ui.add_enabled(m.online, egui::Button::new(text)).clicked() {
                        picked = Some(Some(m.name.clone()));
                    }
                }
            })
            .response
        })
        .inner;
    (resp, picked)
}

#[cfg(test)]
/// The `composed_on` value a pick writes: `None` clears it (the default).
pub(crate) fn composed_on_body(pick: &Option<String>) -> Value {
    match pick {
        Some(name) => json!(name),
        None => Value::Null,
    }
}

/// The `render_on` value a pick writes: `"each"`, or `{ "node": name }`.
pub(crate) fn render_on_body(pick: &Option<String>) -> Value {
    match pick {
        Some(name) => json!({ "node": name }),
        None => json!("each"),
    }
}

/// The source's own page: **Renderiza en** and the device pool grouped by
/// machine. The source is the layer a `gen.*` node names in its config
/// (`source_id`, the `layer_node_config` of `metal-perform`), and its pool entry
/// is read off `GET /state`'s `devices[].layers[]`.
pub(crate) fn source_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    node: &Value,
) {
    let Some(source_id) = node["config"]["source_id"].as_str().filter(|s| !s.is_empty()) else {
        ui.label(
            RichText::new(tr("— no source behind this node —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    };
    let state = get(door, "GET", "/state", &[]).unwrap_or_else(|| json!({}));
    let devices = state["devices"].as_array().cloned().unwrap_or_default();
    // Where the layer lives: the first device whose routed list names it.
    let host = devices.iter().find_map(|d| {
        let layer = d["layers"]
            .as_array()?
            .iter()
            .find(|l| l["id"].as_str() == Some(source_id))?;
        Some((d["id"].as_str()?.to_string(), layer.clone()))
    });
    let Some((device_id, layer)) = host else {
        ui.label(
            RichText::new(tr("— this source is not routed to any screen —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    };

    let machines = machines(ui, door);
    let local = machines
        .iter()
        .find(|m| m.local)
        .map(|m| m.name.clone())
        .unwrap_or_default();
    let current: Option<String> = layer["render_on"]["node"].as_str().map(str::to_string);
    let mut picked: Option<Option<String>> = None;
    label_row(ui, theme, tr("render on"), |ui| {
        let (resp, pick) = machine_picker(
            ui,
            theme,
            &format!("shell/region/properties/render-on-{source_id}"),
            current.as_deref(),
            &machines,
            Some(tr("each")),
        );
        picked = pick;
        resp
    });
    if let Some(pick) = picked {
        post(
            door,
            "/layers",
            json!({
                "op": "set",
                "layer": source_id,
                "device": device_id,
                "render_on": render_on_body(&pick),
            }),
        );
    }

    ui.label(
        RichText::new(tr("devices by machine"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );
    let outputs = state["outputs"].as_array().cloned().unwrap_or_default();
    let fed: Vec<String> = layer["outputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    // The peers' own screens, as `GET /peers` last cached them. This
    // machine's screens come from `/state` alone.
    let known = peers_listing(ui, door)["screens"].clone();
    for m in &machines {
        let mut screens: Vec<Value> = outputs
            .iter()
            .filter(|o| o["node"].as_str().unwrap_or(&local) == m.name)
            .cloned()
            .collect();
        // A peer's screens are the ones its own dash shows; any id already on
        // this machine's `/state` is the same screen and listed once.
        let peer = if m.local { None } else { known[&m.name]["screens"].as_array() };
        if let Some(peer_screens) = peer {
            for s in peer_screens {
                if !screens.iter().any(|o| o["id"] == s["id"]) {
                    screens.push(s.clone());
                }
            }
        }
        // An offline peer is greyed: its screens are last-known, not live.
        let live = m.online;
        let text_colour = if live { theme.ink } else { theme.ink_faint };
        let heading = if live {
            trf("{name}: {n} screens", &[("name", &m.name), ("n", &screens.len())])
        } else {
            trf("{name} · offline: {n} screens", &[("name", &m.name), ("n", &screens.len())])
        };
        ui.label(
            RichText::new(heading)
                .font(theme.font_small.clone())
                .color(text_colour),
        );
        if screens.is_empty() {
            ui.label(
                RichText::new(tr("— no screens on this machine —"))
                    .font(theme.font_small.clone())
                    .color(theme.ink_faint),
            );
        }
        for o in screens {
            let id = o["id"].as_str().unwrap_or_default();
            let is_fed = live && fed.iter().any(|f| f == id);
            let colour = if is_fed { theme.accent_vivid } else { theme.ink_faint };
            ui.label(
                RichText::new(format!("{id} · {}", o["label"].as_str().unwrap_or(id)))
                    .font(theme.font_small.clone())
                    .color(colour),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machine_leads_and_peers_follow_in_fw_sync_order() {
        let listing = json!({
            "local": "casa",
            "sync": true,
            "peers": [
                { "name": "pi", "online": true },
                { "name": "laptop", "online": false },
                { "name": "casa", "online": true },
                { "name": "pi", "online": false }
            ]
        });
        let m = machines_from(&listing);
        let names: Vec<(&str, bool, bool)> =
            m.iter().map(|x| (x.name.as_str(), x.online, x.local)).collect();
        assert_eq!(
            names,
            vec![("casa", true, true), ("pi", true, false), ("laptop", false, false)],
            "this machine first, repeats dropped, offline peers kept"
        );
    }

    #[test]
    fn no_sync_leaves_only_this_machine() {
        let m = machines_from(&json!({ "local": "casa", "sync": false, "peers": [] }));
        assert_eq!(m.len(), 1);
        assert!(m[0].local && m[0].online);
    }

    #[test]
    fn a_pick_writes_the_shapes_the_model_reads() {
        assert_eq!(composed_on_body(&None), Value::Null, "default clears composed_on");
        assert_eq!(composed_on_body(&Some("pi".into())), json!("pi"));
        assert_eq!(render_on_body(&None), json!("each"));
        assert_eq!(render_on_body(&Some("pi".into())), json!({ "node": "pi" }));
    }
}
