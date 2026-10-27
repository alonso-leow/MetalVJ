//! Input / Output / Source / Parameters — the four I/O folders of the node
//! browser, all read off `GET /graph/io` (one node's whole I/O) and
//! `GET /graph/live` (the running engine's last frame).
//!
//! **Live values** are polled at ~4–5 Hz and cached between frames — not a GET
//! per frame. `/graph/io` may be read per frame or cached briefly and
//! invalidated after this panel's own writes.

use crate::shell::components::small_text;
use egui::{Id, RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{
    float_row, kv_row, kv_wrap, label_row, number_row, section_heading, toggle_row, Float,
};
use crate::shell::i18n::{tr, trf};

use super::browser::{fmt, Column, Row, RowKey, Table};
use super::shared::{get, post};
use super::sources;
use super::text::post_text;
use super::PanelState;

/// How often `/graph/live` is refetched, in seconds — the ~4–5 Hz the brief
/// asks for. `PanelState::live_poll` waits this long between GETs; every frame
/// in between reads the cached reply.
pub(crate) const LIVE_POLL_SECONDS: f64 = 0.22;

/// The key `io_cache` is filed under.
fn io_key(gid: &str, node: &str) -> String {
    format!("{gid}\u{1}{node}")
}

/// One node's I/O, cached briefly. The cache is invalidated after any write
/// this panel makes (see [`invalidate`]) and otherwise lives at most one second
/// — the same trade `get` already makes for a route the panel drives.
pub(crate) fn fetch_io(state: &mut PanelState, door: &dyn Call, now: f64, gid: &str, node: &str) -> Option<Value> {
    let key = io_key(gid, node);
    if let Some((v, at)) = state.io_cache.get(&key) {
        if now - at < 1.0 {
            return Some(v.clone());
        }
    }
    let v = get(door, "GET", "/graph/io", &[("id", gid.to_string()), ("node", node.to_string())])?;
    state.io_cache.insert(key, (v.clone(), now));
    Some(v)
}

/// Drops every cached `/graph/io` reply — called after a bind/param write so the
/// next frame re-reads the truth the door now holds (rule 4).
pub(crate) fn invalidate(state: &mut PanelState) {
    state.io_cache.clear();
}

/// The last `/graph/live` frame for `gid`, refetched at [`LIVE_POLL_SECONDS`].
pub(crate) fn fetch_live(state: &mut PanelState, door: &dyn Call, now: f64, gid: &str) -> Value {
    let fresh =
        state.live_cache.as_ref().is_none_or(|(_, at, id)| id != gid || now - at >= LIVE_POLL_SECONDS);
    if fresh {
        let v = get(door, "GET", "/graph/live", &[("id", gid.to_string())])
            .unwrap_or_else(|| json!({ "nodes": {} }));
        state.live_cache = Some((v.clone(), now, gid.to_string()));
    }
    state.live_cache.as_ref().map(|(v, _, _)| v.clone()).unwrap_or_else(|| json!({ "nodes": {} }))
}

/// The live `in`/`out` maps for `node` off a `/graph/live` reply.
fn live_for<'a>(live: &'a Value, node: &str) -> (&'a Value, &'a Value) {
    let n = &live["nodes"][node];
    (&n["in"], &n["out"])
}

/// The label of an `IoEnd` (`from`/`to`), falling back to the node id.
fn end_label(v: &Value) -> String {
    v["label"].as_str().filter(|s| !s.is_empty()).unwrap_or_else(|| v["node"].as_str().unwrap_or("")).to_string()
}

/// Input table: From (label) | Kind | Port (with its var) | → Port | Type.
pub(crate) fn input_table(ui: &mut Ui, theme: &Theme, state: &mut PanelState, io: &Value) -> Option<RowKey> {
    let inputs = io["inputs"].as_array().cloned().unwrap_or_default();
    let rows = inputs
        .iter()
        .map(|i| {
            let var = i["from"]["var"].as_str().unwrap_or("");
            let port = i["from"]["port"].as_str().unwrap_or("");
            let port_cell = if var.is_empty() { port.to_string() } else { format!("{port}.{var}") };
            let ty = if var.is_empty() { tr("cable") } else { tr("number") };
            Row {
                cells: vec![
                    end_label(&i["from"]),
                    i["from"]["kind"].as_str().unwrap_or("").to_string(),
                    port_cell,
                    i["to_port"].as_str().unwrap_or("").to_string(),
                    ty.to_string(),
                ],
                used: true,
                key: RowKey::Text(i["from"]["node"].as_str().unwrap_or("").to_string()),
            }
        })
        .collect();
    let mut table = Table::new(
        vec![
            Column::new(crate::shell::i18n::tr_key("From"), 120.0),
            Column::new(crate::shell::i18n::tr_key("Kind"), 96.0),
            Column::new(crate::shell::i18n::tr_key("Port"), 96.0),
            Column::new(crate::shell::i18n::tr_key("→ Port"), 90.0),
            Column::new(crate::shell::i18n::tr_key("Type"), 60.0),
        ],
        rows,
    );
    let search = std::mem::take(&mut state.io_search);
    let clicked = ui.push_id(super::browser::table_id("input"), |ui| table.ui(ui, theme, &search)).inner;
    state.io_search = search;
    clicked
}

/// Output table: To | Kind | Port | From port | Type.
pub(crate) fn output_table(ui: &mut Ui, theme: &Theme, state: &mut PanelState, io: &Value) -> Option<RowKey> {
    let outputs = io["outputs"].as_array().cloned().unwrap_or_default();
    let rows = outputs
        .iter()
        .map(|o| {
            let var = o["var"].as_str().unwrap_or("");
            let from = o["from_port"].as_str().unwrap_or("");
            let from_cell = if var.is_empty() { from.to_string() } else { format!("{from}.{var}") };
            let ty = if var.is_empty() { tr("cable") } else { tr("number") };
            Row {
                cells: vec![
                    end_label(&o["to"]),
                    o["to"]["kind"].as_str().unwrap_or("").to_string(),
                    o["to"]["port"].as_str().unwrap_or("").to_string(),
                    from_cell,
                    ty.to_string(),
                ],
                used: true,
                key: RowKey::Text(o["to"]["node"].as_str().unwrap_or("").to_string()),
            }
        })
        .collect();
    let mut table = Table::new(
        vec![
            Column::new(crate::shell::i18n::tr_key("To"), 120.0),
            Column::new(crate::shell::i18n::tr_key("Kind"), 96.0),
            Column::new(crate::shell::i18n::tr_key("Port"), 96.0),
            Column::new(crate::shell::i18n::tr_key("From port"), 90.0),
            Column::new(crate::shell::i18n::tr_key("Type"), 60.0),
        ],
        rows,
    );
    let search = std::mem::take(&mut state.io_search);
    let clicked = ui.push_id(super::browser::table_id("output"), |ui| table.ui(ui, theme, &search)).inner;
    state.io_search = search;
    clicked
}

/// `POST /graph/node {op:"bind"}` — the one writer of a binding
/// (`metal_graph::bind::bind`'s route body). `from` is the producer triple, or
/// `None` to unbind.
pub(crate) fn post_bind(door: &dyn Call, gid: &str, node: &str, param: &str, from: Option<(&str, &str, &str)>) {
    let from = match from {
        Some((n, p, v)) => json!({ "node": n, "port": p, "var": v }),
        None => Value::Null,
    };
    post(
        door,
        "/graph/node",
        json!({ "op": "bind", "graph": gid, "node": node, "param": param, "from": from }),
    );
}

/// `POST /graph/node {op:"param"}` — one parameter's held value, `None` to
/// release it.
pub(crate) fn post_param(door: &dyn Call, gid: &str, node: &str, name: &str, value: f32) {
    post(
        door,
        "/graph/node",
        json!({ "op": "param", "graph": gid, "node": node, "name": name, "value": value }),
    );
}

/// The shape the source table takes once its rows are built — split out so the
/// kittest tests can hold the used/unused decision without a live door.
pub(crate) struct SourceRow {
    pub variable: String,
    pub from: String,
    pub port: String,
    pub ty: String,
    pub value: Option<f64>,
    pub hops: usize,
    pub used_by: Vec<String>,
    pub used: bool,
    /// The `from` triple a binding of this row would carry.
    pub bind: (String, String, String),
}

/// Builds the source rows: every upstream variable (transitive, nearest first),
/// with its used state.
pub(crate) fn source_rows(io: &Value, live: &Value) -> Vec<SourceRow> {
    let mut out = Vec::new();
    for src in io["sources"].as_array().cloned().unwrap_or_default() {
        let from_node = src["node"].as_str().unwrap_or("").to_string();
        let from_label = end_label(&src);
        let hops = src["hops"].as_u64().unwrap_or(0) as usize;
        for v in src["vars"].as_array().cloned().unwrap_or_default() {
            let name = v["name"].as_str().unwrap_or("").to_string();
            let port = v["port"].as_str().unwrap_or("").to_string();
            let kind = v["kind"].as_str().unwrap_or("").to_string();
            let port_ref = v["port_ref"].as_str().unwrap_or("").to_string();
            let used = v["used"].as_bool().unwrap_or(false);
            let used_by: Vec<String> = v["used_by"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let via: Vec<String> = v["via"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let value = live
                .get("nodes")
                .and_then(|n| n.get(&from_node))
                .and_then(|n| n.get("out"))
                .and_then(|o| o.get(&name))
                .and_then(Value::as_f64)
                .or_else(|| v["value"].as_f64());
            let (port_part, var_name) = port_ref.split_once('.').unwrap_or((port.as_str(), name.as_str()));
            let port_part = port_part.to_string();
            let var_name = var_name.to_string();
            let used_by_cell = if !used_by.is_empty() {
                used_by.join(", ")
            } else if !via.is_empty() {
                via.join(", ")
            } else {
                "—".to_string()
            };
            out.push(SourceRow {
                variable: name,
                from: from_label.clone(),
                port,
                ty: kind,
                value,
                hops,
                used_by: if used_by_cell == "—" { vec![] } else { vec![used_by_cell] },
                used,
                bind: (from_node.clone(), port_part, var_name),
            });
        }
    }
    out
}

/// Source table: Variable | From | Port | Type | Value (live) | Hops | Used by | ●,
/// plus a "→ param" combo per row that binds the variable to a compatible param.
#[allow(clippy::too_many_arguments)]
pub(crate) fn source_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    gid: &str,
    node: &str,
    io: &Value,
) -> Option<RowKey> {
    let live = fetch_live(state, door, ui.input(|i| i.time), gid);
    let _ = live_for(&live, node);
    let rows = source_rows(io, &live);
    let params: Vec<String> = io["params"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|p| p["kind"] == json!("float"))
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect();

    // `restore sources`: re-apply the node's model defaults. The table comes
    // off the model entry `/author/models/state` already answers for the
    // browser; `/author/pagedata` carries the same `bindings` key for the
    // models that route has not grown it for yet. Read once per click; each
    // entry whose var an upstream source offers posts the same bind op the
    // per-row combos do.
    if ui.button(small_text(theme, tr("restore sources"))).clicked() {
        let model_id = io["kind"].as_str().unwrap_or_default().strip_prefix("gen.").unwrap_or_default();
        let bindings_of = |data: Value| -> Option<Vec<Value>> {
            data["models"]
                .as_array()?
                .iter()
                .find(|m| m["id"].as_str() == Some(model_id))?["bindings"]
                .as_array()
                .cloned()
        };
        let defaults = bindings_of(get(door, "GET", "/author/models/state", &[]).unwrap_or_default())
            .filter(|b| !b.is_empty())
            .or_else(|| bindings_of(get(door, "GET", "/author/pagedata", &[]).unwrap_or_default()))
            .unwrap_or_default();
        let mut landed = 0usize;
        for b in &defaults {
            let (param, var) = (b["param"].as_str().unwrap_or_default(), b["var"].as_str().unwrap_or_default());
            if let Some(r) = rows.iter().find(|r| r.variable == var) {
                let (n, p, v) = (&r.bind.0, &r.bind.1, &r.bind.2);
                post_bind(door, gid, node, param, Some((n, p, v)));
                landed += 1;
            }
        }
        invalidate(state);
        state.browser_note = Some(trf("sources restored: {n}", &[("n", &landed)]));
    }

    let mut table = Table::new(
        vec![
            Column::new(crate::shell::i18n::tr_key("Variable"), 100.0),
            Column::new(crate::shell::i18n::tr_key("From"), 100.0),
            Column::new(crate::shell::i18n::tr_key("Port"), 70.0),
            Column::new(crate::shell::i18n::tr_key("Type"), 52.0),
            Column::new(crate::shell::i18n::tr_key("Value"), 60.0),
            Column::new(crate::shell::i18n::tr_key("Hops"), 40.0),
            Column::new(crate::shell::i18n::tr_key("Used by"), 100.0),
        ],
        rows.iter()
            .map(|r| Row {
                cells: vec![
                    r.variable.clone(),
                    r.from.clone(),
                    r.port.clone(),
                    r.ty.clone(),
                    fmt(r.value),
                    r.hops.to_string(),
                    r.used_by.first().cloned().unwrap_or_else(|| "—".to_string()),
                ],
                used: r.used,
                key: RowKey::Text(r.variable.clone()),
            })
            .collect(),
    );
    let clicked = ui
        .push_id(Id::new("shell/region/properties/source"), |ui| table.ui(ui, theme, &state.io_search))
        .inner;
    // The bind combos, one row per variable, under the table — a combo of the
    // node's float params, posting through the bind op on pick.
    for r in &rows {
        {
            let colour = if r.used { theme.accent_vivid } else { theme.ink_faint };
            ui.add(
                egui::Label::new(
                    RichText::new(format!("{} · {}.{}", r.variable, r.bind.1, r.bind.2))
                        .font(theme.font_small.clone())
                        .color(colour),
                )
                .wrap(),
            );
            let mut picked: Option<String> = None;
            kv_row(ui, theme, tr("bind to"), |ui| {
                crate::shell::components::combo_box(theme, Id::new(("bind", &r.bind.2, &r.from)))
                    .width(ui.available_width())
                    .selected_text((tr("→ param")).to_string())
                    .show_ui(ui, |ui| {
                        for p in &params {
                            if ui.selectable_label(false, p).clicked() {
                                picked = Some(p.clone());
                            }
                        }
                    })
                    .response
            });
            if let Some(param) = picked {
                let (n, p, v) = (&r.bind.0, &r.bind.1, &r.bind.2);
                post_bind(door, gid, node, &param, Some((n, p, v)));
                invalidate(state);
                state.browser_note = Some(trf("bound {param} ← {var}", &[("param", &param), ("var", &r.variable)]));
            }
        }
    }
    clicked
}

/// Parameters (2026-10-08): **one** list of the node's float params. When the
/// node is the timeline's authored model (`authored`), each row edits
/// `ingest.row[offset]` live and shows whether the playhead tick is keyed;
/// otherwise the field posts `post_param` once per gesture. Bound rows carry a
/// `⟵ var` badge and an unbind action; the `customise sources` toggle
/// reveals the binding editor inline, off by default. For a `gen.*` in `ai_fx`
/// mode an **Effects** group follows, and a `gen.*`/`text source`'s reaction
/// controls fold in after the floats.
#[allow(clippy::too_many_arguments)]
pub(crate) fn parameters_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    io: &Value,
    ingest: &mut crate::shell::lanes::IngestLane,
    authored: bool,
) -> Option<RowKey> {
    let params = io["params"].as_array().cloned().unwrap_or_default();
    let live = fetch_live(state, door, ui.input(|i| i.time), gid);
    let (ins, _) = live_for(&live, node_id);
    let keyed = ingest.keyframes.contains_key(&ingest.playhead);
    let mut clicked = None;

    for (i, p) in params.iter().filter(|p| p["kind"] == json!("float")).enumerate() {
        let name = p["name"].as_str().unwrap_or("").to_string();
        let min = p["min"].as_f64().unwrap_or(0.0) as f32;
        let max = p["max"].as_f64().unwrap_or(1.0) as f32;
        let offset = p["index"].as_u64().map(|n| n as usize).unwrap_or(i);
        let bound = !p["bound"].is_null();
        let mut value = if authored {
            if offset >= ingest.row.len() {
                ingest.row.resize(offset + 1, min);
            }
            ingest.row[offset]
        } else {
            ins.get(&name)
                .and_then(Value::as_f64)
                .or_else(|| p["live"].as_f64())
                .or_else(|| p["value"].as_f64())
                .unwrap_or(f64::from(min)) as f32
        };
        let response = label_row(ui, theme, &name, |ui| {
            // Badges sit at the far right; the track fills what is left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if authored {
                    let dot = if keyed { "" } else { "" };
                    let colour = if keyed { theme.accent_vivid } else { theme.ink_faint };
                    ui.label(small_text(theme, dot).color(colour));
                }
                if bound {
                    let var = p["bound"]["var"].as_str().unwrap_or("");
                    ui.label(small_text(theme, var).color(theme.ink_weak));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    float_row(ui, &mut value, min..=max, Float::Drag { speed: 0.01, decimals: None })
                })
                .inner
            })
            .inner
        });
        if response.drag_stopped() || (response.changed() && !response.dragged()) {
            if authored {
                if offset < ingest.row.len() {
                    ingest.row[offset] = value;
                }
            } else {
                post_param(door, gid, node_id, &name, value);
                invalidate(state);
                state.browser_note = Some(trf("set {param} = {v:.3}", &[("param", &name), ("v", &value)]));
            }
        }
    }

    // Unbind actions for bound rows.
    for p in params.iter().filter(|p| !p["bound"].is_null()) {
        let name = p["name"].as_str().unwrap_or("").to_string();
        if ui.button(small_text(theme, trf("unbind {param}", &[("param", &name)]))).clicked() {
            post_bind(door, gid, node_id, &name, None);
            invalidate(state);
            state.browser_note = Some(trf("unbound {param}", &[("param", &name)]));
        }
    }

    if authored {
        kv_wrap(ui, theme, "", |ui| {
            if ui.button(small_text(theme, tr("set keypoint"))).clicked() {
                ingest.write_keyframe();
            }
            if keyed && ui.button(small_text(theme, tr("remove keypoint"))).clicked() {
                ingest.clear_keyframe();
            }
        });
    }

    let kind = node["kind"].as_str().unwrap_or_default();
    let mode = node["config"]["mode"].as_str().unwrap_or("");
    if kind.starts_with("gen.") && mode == "ai_fx" {
        let effects = embedded_effects(node, io);
        if !effects.is_empty() {
            ui.add_space(theme.section_gap);
            section_heading(ui, theme, tr("Effects"));
            for (name, mut value, min, max) in effects {
                if number_row(ui, theme, &name, &mut value, min..=max, "", 3, true).changed() {
                    let mut body = serde_json::Map::new();
                    body.insert(name, json!(value));
                    post_text(door, gid, node_id, json!({ "effects": Value::Object(body) }));
                }
            }
        }
    }

    if kind.starts_with("gen.") || kind == metal_graph::TEXT_SOURCE_KIND {
        sources::reaction_section(ui, theme, door, gid, node_id, node, graph);
    }

    toggle_row(ui, theme, tr("customise sources"), &mut state.source_advanced);
    if state.source_advanced {
        clicked = source_section(ui, theme, door, state, gid, node_id, io).or(clicked);
    }
    clicked
}

/// The `ai_fx` node's embedded effect floats: `node["config"]["effects"]` (a
/// name → value object) when the node carries them, else `io["effects"]` (an
/// array of `{name, min?, max?, value?}`). Best-effort — the engine-side
/// wiring is separate.
fn embedded_effects(node: &Value, io: &Value) -> Vec<(String, f32, f32, f32)> {
    let mut out = Vec::new();
    if let Some(map) = node["config"]["effects"].as_object() {
        for (name, v) in map {
            if let Some(x) = v.as_f64() {
                out.push((name.clone(), x as f32, 0.0, 1.0));
            }
        }
    }
    if out.is_empty() {
        if let Some(arr) = io["effects"].as_array() {
            for e in arr {
                let Some(name) = e["name"].as_str() else { continue };
                let value = e["value"].as_f64().or_else(|| e["live"].as_f64()).unwrap_or(0.0) as f32;
                let min = e["min"].as_f64().unwrap_or(0.0) as f32;
                let max = e["max"].as_f64().unwrap_or(1.0) as f32;
                out.push((name.to_string(), value, min, max));
            }
        }
    }
    out
}

/// The variable count a folder label shows — inputs / outputs / sources /
/// params, off the `NodeIo` reply.
pub(crate) fn counts(io: &Value) -> (usize, usize, usize, usize) {
    let inputs = io["inputs"].as_array().map(Vec::len).unwrap_or(0);
    let outputs = io["outputs"].as_array().map(Vec::len).unwrap_or(0);
    let sources = io["sources"]
        .as_array()
        .map(|s| s.iter().map(|x| x["vars"].as_array().map(Vec::len).unwrap_or(0)).sum())
        .unwrap_or(0);
    let params = io["params"].as_array().map(Vec::len).unwrap_or(0);
    (inputs, outputs, sources, params)
}

/// The node card's one-line I/O summary: the `inputs:` and `outputs:` port
/// lists (`from.port`/`to.port`, comma-separated, `—` when empty), off
/// `GET /graph/io`.
pub(crate) fn io_summary(io: &Value) -> (String, String) {
    let names = |arr: &Value, side: &str| -> String {
        let list: Vec<String> = arr
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|e| e[side]["port"].as_str().map(str::to_string))
            .collect();
        if list.is_empty() {
            "—".to_string()
        } else {
            list.join(", ")
        }
    };
    (names(&io["inputs"], "from"), names(&io["outputs"], "to"))
}

