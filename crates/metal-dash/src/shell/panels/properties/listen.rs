//! Plan "Audio input is a node": the two property pages it adds.
//!
//! - `audio.live` → **input device** ([`audio_live_section`]): the source
//!   combo (`GET /listen/sources`), with the deliberate `stand-in` entry; the
//!   `probar` button (`POST /listen/probe`, two seconds of flux); and the live
//!   readout `{ real, describe, bpm, dropped }` the `/graph` reply attaches to
//!   the node (`listen`). The pick is written to the node's config
//!   (`source`, `null` = the seed) — the only place a source is chosen.
//! - `gen.*` / `text.source` → **reaction** ([`reaction_section`]): the
//!   source's own `drive` dials (`depth`, `smooth`, `colour`, `palette`),
//!   which used to be global (`POST /drive`); the `colour source` selector
//!   (`auto`/`tone`/`chroma`, the tone design notes D8 step 11), which picks the analysis
//!   that drives the colour slots; and "sin audio" when nothing audio is wired
//!   to its `control` — a static source.
//!
//! Both write through `POST /graph/node {op:"config"}`, the route the AI's
//! `set_params` also reaches (rule 6).

use crate::shell::components::small_text;
use egui::{Id, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_row, kv_wrap, note, num_value};
use crate::shell::i18n::{tr, tr_dyn, trf};

use super::shared::get;
use super::text::post_text;

/// How long one `GET /listen/sources` answer is reused, in seconds — the
/// route spawns `pactl`, which is not a per-frame cost.
const SOURCES_TTL: f64 = 2.0;

fn sources(ui: &Ui, door: &dyn Call) -> Vec<(String, String)> {
    let id = Id::new("shell/properties/listen-sources");
    let now = ui.input(|i| i.time);
    let cached: Option<(Value, f64)> = ui.ctx().data(|d| d.get_temp(id));
    let v = match cached.filter(|(_, at)| now - at < SOURCES_TTL) {
        Some((v, _)) => v,
        None => {
            let v = get(door, "GET", "/listen/sources", &[]).unwrap_or_else(|| json!({}));
            ui.ctx().data_mut(|d| d.insert_temp(id, (v.clone(), now)));
            v
        }
    };
    v["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| Some((s["id"].as_str()?.to_string(), s["label"].as_str()?.to_string())))
        .collect()
}

fn small(ui: &mut Ui, theme: &Theme, text: &str) {
    note(ui, theme, text);
}

/// A wrapped small line in the warning ink.
fn warn(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.add(egui::Label::new(small_text(theme, text).color(theme.warn)).wrap());
}

/// What a node's config asks for: `Some(pick)`, `None` = the seed. `device`
/// is the key the 2026-10-01 picker wrote (`metal_perform::listeners::wanted`).
fn wanted(node: &Value) -> Option<String> {
    let c = &node["config"];
    c.get("source").or_else(|| c.get("device")).and_then(Value::as_str).map(str::to_string)
}

/// `audio.live`'s page — see the module note.
pub(crate) fn audio_live_section(ui: &mut Ui, theme: &Theme, door: &dyn Call, gid: &str, node_id: &str, node: &Value) {
    let want = wanted(node);
    // `None` (the seed) is its own entry, first: "what --listen says".
    let seed = tr("default (seed)").to_string();
    let mut choices: Vec<(Option<String>, String)> = vec![(None, seed)];
    choices.extend(sources(ui, door).into_iter().map(|(id, label)| (Some(id), label)));
    let shown = choices
        .iter()
        .find(|(id, _)| *id == want)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| want.clone().unwrap_or_default());
    let mut picked: Option<Option<String>> = None;
    kv_row(ui, theme, tr("source"), |ui| {
        crate::shell::components::combo_box(theme, Id::new(("shell/region/properties", "listen-source")))
            .width(theme.control_width)
            .selected_text((shown).to_string())
            .show_ui(ui, |ui| {
                for (id, label) in &choices {
                    if ui.selectable_label(*id == want, label).clicked() && *id != want {
                        picked = Some(id.clone());
                    }
                }
            })
            .response
    });
    if let Some(pick) = picked {
        post_text(door, gid, node_id, json!({ "source": pick }));
    }

    let probe_id = Id::new(("shell/properties/listen-probe", node_id));
    let mut probe = false;
    kv_wrap(ui, theme, "", |ui| {
        probe = ui.button(small_text(theme, tr("probar"))).clicked();
    });
    if probe {
        let source = want.clone().unwrap_or_default();
        let note = match door.json("POST", "/listen/probe", Some(json!({ "source": source }))) {
            Ok(r) if r["ok"] == json!(true) => {
                trf("flux {flux}", &[("flux", &format!("{:.2}", r["flux"].as_f64().unwrap_or(0.0)))])
            }
            Ok(r) => r["error"].as_str().unwrap_or("?").to_string(),
            Err((_, msg)) => msg,
        };
        ui.ctx().data_mut(|d| d.insert_temp(probe_id, note));
    }
    if let Some(note) = ui.ctx().data(|d| d.get_temp::<String>(probe_id)) {
        small(ui, theme, &note);
    }

    let live = &node["listen"];
    if live.is_null() {
        small(ui, theme, tr("not listening yet — no device draws this graph"));
        return;
    }
    if let Some(err) = live["error"].as_str() {
        warn(ui, theme, err);
    }
    let real = live["real"].as_bool().unwrap_or(false);
    let describe = live["describe"].as_str().unwrap_or_default();
    if real {
        small(ui, theme, describe);
    } else {
        warn(ui, theme, &trf("stand-in · {what}", &[("what", &describe)]));
    }
    small(
        ui,
        theme,
        &trf("{bpm} BPM · {dropped} dropped", &[
            ("bpm", &format!("{:.0}", live["bpm"].as_f64().unwrap_or(0.0))),
            ("dropped", &live["dropped"].as_u64().unwrap_or(0)),
        ]),
    );

    // The live voice read, off the graph frame's own `control` variables — the
    // same row the face and every binding see, never a second probe of the
    // input. Presence and the winning style are what say whether a mouth would
    // move at all, and how (the voice design notes V3's `audio.live` readout). Kept on
    // this page from V3 when the 2026-10-01 picker moved here.
    if let Some(gl) = get(door, "GET", "/graph/live", &[("id", gid.to_string())]) {
        let out = &gl["nodes"][node_id]["out"];
        if let Some(presence) = out["voice"].as_f64() {
            let mut best = "speak";
            let mut best_v = -1.0f64;
            for style in ["sing", "rap", "speak", "whisper"] {
                let v = out[style].as_f64().unwrap_or(0.0);
                if v > best_v {
                    best_v = v;
                    best = style;
                }
            }
            let name = if presence > 0.05 { tr_dyn(best) } else { tr("— none —").to_string() };
            let pct = format!("{presence:.2}");
            small(
                ui,
                theme,
                &trf("{live}: voice {presence} · {style} {name}", &[("live", &tr("live")), ("presence", &pct), ("style", &tr("style")), ("name", &name)]),
            );
        }
    }
}

/// Whether anything audio reaches `node_id`'s `control`, walking control
/// cables upstream (through an `ai.*` or a `script.*`) — the panel's reading
/// of `metal_perform::listeners::Digests::wired`.
pub(crate) fn has_audio(graph: &Value, node_id: &str) -> bool {
    let nodes = graph["nodes"].as_array().cloned().unwrap_or_default();
    let edges = graph["edges"].as_array().cloned().unwrap_or_default();
    let kind_of = |id: &str| {
        nodes.iter().find(|n| n["id"].as_str() == Some(id)).and_then(|n| n["kind"].as_str()).unwrap_or("").to_string()
    };
    let mut todo = vec![node_id.to_string()];
    let mut seen: Vec<String> = Vec::new();
    while let Some(at) = todo.pop() {
        if seen.contains(&at) {
            continue;
        }
        seen.push(at.clone());
        for e in edges.iter().filter(|e| e["to"]["node"].as_str() == Some(at.as_str()) && e["to"]["port"] == json!("control")) {
            let Some(from) = e["from"]["node"].as_str() else { continue };
            let k = kind_of(from);
            if k.starts_with("audio.") || k.starts_with("sensor.") {
                return true;
            }
            todo.push(from.to_string());
        }
    }
    false
}

/// A source's `reaction` page — see the module note. `graph` is the open
/// `/graph` reply (its `graph` is read).
pub(crate) fn reaction_section(ui: &mut Ui, theme: &Theme, door: &dyn Call, gid: &str, node_id: &str, node: &Value, graph: &Value) {
    if !has_audio(&graph["graph"], node_id) {
        warn(ui, theme, tr("sin audio"));
        small(ui, theme, tr("nothing audio is wired to control: this source is static (its base row)."));
        return;
    }
    let d = &node["config"][metal_perform::drive::CONFIG_KEY];
    let base = metal_perform::drive::Drive::default();
    let mut depth = d["depth"].as_f64().map_or(base.depth, |v| v as f32);
    let mut smooth = d["smooth"].as_f64().map_or(base.smooth, |v| v as f32);
    let mut colour = d["colour"].as_bool().unwrap_or(base.colour);
    let mut palette = d["palette"].as_str().unwrap_or(base.palette.as_str()).to_string();
    // Which analysis drives the colour slots — the tone design notes D8. Absent (or an
    // unknown spelling) reads as the default `Auto` crossfade, exactly what
    // `Drive::from_config` does, so the selector cannot disagree with the
    // engine about a node saved before the key existed.
    let mut colour_source = d["colour_source"]
        .as_str()
        .map(metal_perform::drive::ColourSource::parse)
        .unwrap_or(base.colour_source);
    let mut changed = false;
    kv_row(ui, theme, tr("depth"), |ui| {
        let w = ui.available_width();
        let r = num_value(ui, theme, w, &mut depth, 0.0..=1.0, "", 2, base.depth, true);
        changed |= r.changed();
        r
    });
    kv_row(ui, theme, tr("smooth"), |ui| {
        let w = ui.available_width();
        let r = num_value(ui, theme, w, &mut smooth, 0.0..=1.0, "", 2, base.smooth, true);
        changed |= r.changed();
        r
    });
    changed |= crate::shell::components::toggle_row(ui, theme, tr("colour follows harmony"), &mut colour).changed();
    kv_wrap(ui, theme, tr("palette"), |ui| {
        for p in ["metalvj", "spectrum", "hold"] {
            if ui.selectable_label(palette == p, p).clicked() && palette != p {
                palette = p.to_string();
                changed = true;
            }
        }
    });
    // The same segmented control the palette uses, one option per
    // `ColourSource` the engine knows — the value stored is the enum's own
    // wire string (`as_str`), so the selector and `Drive::from_config`/`to_json`
    // can never drift apart.
    kv_wrap(ui, theme, tr("colour source"), |ui| {
        for (src, label) in [
            (metal_perform::drive::ColourSource::Auto, tr("auto")),
            (metal_perform::drive::ColourSource::Tone, tr("tone")),
            (metal_perform::drive::ColourSource::Chroma, tr("chroma")),
        ] {
            if ui.selectable_label(colour_source == src, label).clicked() && colour_source != src {
                colour_source = src;
                changed = true;
            }
        }
    });
    if changed {
        let dials = json!({
            "depth": depth,
            "smooth": smooth,
            "colour": colour,
            "palette": palette,
            "colour_source": colour_source.as_str(),
        });
        let mut patch = serde_json::Map::new();
        patch.insert(metal_perform::drive::CONFIG_KEY.to_string(), dials);
        post_text(door, gid, node_id, Value::Object(patch));
    }
}
