//! The "loop" page of a `gen.*`/`fx.*` node (the loops design notes T9, §5): which template
//! the node plays (picker), what that means right now (badge: looped /
//! reactive / fixed / off), and the loop itself as a **16-slot keyframe grid**
//! — one cell per half-beat slot, click to toggle on/off (off holds the
//! previous slot). The selected slot's keyframe opens below the strip, one
//! numeric row per float the loop targets.
//!
//! Everything shown is read off the door (`GET /loops/node`, `/loops`,
//! `/loops/get`, `/loops/palettes`, rule 5). The one thing kept between
//! frames is the draft an operator is editing: a user loop saves when a
//! stroke ends; a factory template never changes — editing one asks to
//! "save as copy", which writes a user loop under a new id and selects it.

use crate::shell::components::small_text;
use egui::{CornerRadius, Pos2, Rect, RichText, Sense, Ui, Vec2};
use metal_perform::pattern::{Curve, Loop, StepValue, TrackTarget};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_row, kv_wrap, note, num_value};
use crate::shell::i18n::{tr, trf};

use super::shared::get;

/// The draft under edit, kept across frames.
#[derive(Default)]
pub(crate) struct Edit {
    /// The node the draft belongs to (`gid\u{1}node`): another node drops it.
    owner: String,
    draft: Option<Loop>,
    factory: bool,
    dirty: bool,
    note: Option<String>,
    /// The slot the editor is open on.
    selected: usize,
    /// A slot copied for pasting.
    copied: Option<Vec<f32>>,
}

const STEPS: usize = metal_perform::clock::STEPS as usize;

/// The value a step shows, and whether it is the step's own (else held).
fn held(steps: &[Option<StepValue>], step: usize) -> (Option<StepValue>, bool) {
    let n = steps.len();
    if n == 0 {
        return (None, false);
    }
    if let Some(v) = steps[step % n] {
        return (Some(v), true);
    }
    ((1..n).find_map(|back| steps[(step + n - back) % n]), false)
}

/// The palette set's own sRGB colour for a slot's first palette track, or
/// `None` when the loop holds no palette pick there (the strip then paints
/// the float bars instead).
fn palette_swatch(l: &Loop, palettes: &Value, step: usize) -> Option<egui::Color32> {
    for t in &l.tracks {
        if let TrackTarget::Palette { palette, group, .. } = &t.target {
            if let Some(StepValue::Pick(p)) = held(&t.steps, step).0 {
                return rgb(palettes, palette, p.group.unwrap_or(*group), p.colour);
            }
        }
    }
    None
}

fn rgb(palettes: &Value, palette: &str, group: u16, colour: u8) -> Option<egui::Color32> {
    let p = palettes["palettes"].as_array()?.iter().find(|p| p["id"].as_str() == Some(palette))?;
    let groups = p["groups"].as_array()?;
    if groups.is_empty() {
        return None;
    }
    let c = groups[group as usize % groups.len()].get(colour as usize)?.as_array()?;
    let f = |i: usize| Some(c.get(i)?.as_f64()? as f32);
    Some(egui::Rgba::from_rgb(f(0)?, f(1)?, f(2)?).into())
}

fn track_label(t: &TrackTarget) -> String {
    match t {
        TrackTarget::Float { param } => param.clone(),
        TrackTarget::Amount => tr("amount").to_string(),
        TrackTarget::Palette { slot, palette, .. } => format!("{slot} · {palette}"),
    }
}

fn state_label(s: &str) -> &'static str {
    match s {
        "looped" => tr("Looped"),
        "reactive" => tr("Reactive"),
        "fixed" => tr("Fixed"),
        _ => tr("Off"),
    }
}

/// Toggles slot `step` on/off: off holds the previous slot, on starts from the
/// value in force there. Whether it changed.
fn toggle_slot(l: &mut Loop, step: usize) -> bool {
    if l.slot_on(step) {
        l.set_slot(step, None)
    } else {
        let cols = l.slot_tracks().count();
        let row = l.slot_held(step).cloned().unwrap_or_else(|| vec![0.0; cols]);
        l.set_slot(step, Some(row))
    }
}

/// A titled run of rows inside the Loop section's own card (the section is
/// the card; its parts are only separated by a small title and a gap).
fn block<R>(ui: &mut Ui, theme: &Theme, title: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> R {
    if let Some(title) = title {
        ui.label(small_text(theme, title).color(theme.ink_weak).strong());
    }
    let out = add(ui);
    ui.add_space(theme.section_gap);
    out
}

/// The page. `gid`/`node_id` name the selected node.
pub(crate) fn section(ui: &mut Ui, theme: &Theme, door: &dyn Call, gid: &str, node_id: &str, edit: &mut Edit) {
    let owner = format!("{gid}\u{1}{node_id}");
    if edit.owner != owner {
        *edit = Edit { owner, ..Edit::default() };
    }
    let Some(info) = get(door, "GET", "/loops/node", &[("graph", gid.to_string()), ("node", node_id.to_string())])
    else {
        block(ui, theme, None, |ui| note(ui, theme, tr("— no loops for this node —")));
        return;
    };
    let target = info["target"].as_str().unwrap_or_default().to_string();
    let choice = info["choice"].as_str().unwrap_or("default").to_string();
    let playing = info["playing"].as_str().map(str::to_string);
    let state = info["state"].as_str().unwrap_or("off").to_string();
    let step = info["step"].as_u64().map(|s| s as usize);

    let list = get(door, "GET", "/loops", &[("target", target.clone())]).unwrap_or_default();
    let default_name = list["default"]["name"].as_str().unwrap_or("—").to_string();

    // The picker: named templates, one dropdown.
    let mut pick: Option<String> = None;
    block(ui, theme, Some(tr("Loop")), |ui| {
        kv_row(ui, theme, tr("state"), |ui| {
            ui.label(small_text(theme, state_label(&state)).color(theme.ink).strong())
        });
        kv_wrap(ui, theme, tr("clock"), |ui| clock_header(ui, theme, door, &info));
        let mut options: Vec<(String, String)> = vec![
            ("default".to_string(), trf("Default ({name})", &[("name", &default_name)])),
            ("none".to_string(), tr("None — audio/fixed").to_string()),
        ];
        for l in list["loops"].as_array().into_iter().flatten() {
            let (Some(id), Some(name)) = (l["id"].as_str(), l["name"].as_str()) else { continue };
            let text = if l["origin"] == json!("user") {
                trf("{name} (mine)", &[("name", &name)])
            } else {
                name.to_string()
            };
            options.push((id.to_string(), text));
        }
        let shown = options.iter().find(|(v, _)| *v == choice).map(|(_, t)| t.clone()).unwrap_or_default();
        kv_row(ui, theme, tr("template"), |ui| {
            crate::shell::components::combo_box(theme, ("loops/template", node_id))
                .width(ui.available_width())
                .selected_text((shown).to_string())
                .show_ui(ui, |ui| {
                    for (value, text) in &options {
                        if ui.selectable_label(*value == choice, small_text(theme, text)).clicked()
                            && *value != choice
                        {
                            pick = Some(value.clone());
                        }
                    }
                })
                .response
        });
    });
    if let Some(p) = pick {
        choose(door, gid, node_id, &p);
        edit.draft = None;
        edit.dirty = false;
    }

    // The loop shown: the draft under edit, else the one that plays.
    let Some(id) = playing else {
        block(ui, theme, None, |ui| note(ui, theme, tr("— no loop plays: audio or the fixed value drives it —")));
        return;
    };
    if edit.draft.as_ref().is_none_or(|d| d.id != id) || !edit.dirty {
        let Some(reply) = get(door, "GET", "/loops/get", &[("id", id.clone())]) else { return };
        let Ok(mut l) = serde_json::from_value::<Loop>(reply["loop"].clone()) else { return };
        l.migrate();
        edit.factory = reply["origin"] != json!("user");
        edit.draft = Some(l);
        edit.dirty = false;
        edit.selected = 0;
    }

    let palettes = if edit
        .draft
        .as_ref()
        .is_some_and(|d| d.tracks.iter().any(|t| matches!(t.target, TrackTarget::Palette { .. })))
    {
        get(door, "GET", "/loops/palettes", &[]).unwrap_or_default()
    } else {
        Value::Null
    };

    let mut changed = false;
    let mut commit = false;
    if let Some(draft) = edit.draft.as_mut() {
        let cols = draft.slot_tracks().count().max(1);
        let playhead = if state == "looped" { step } else { None };
        block(ui, theme, Some(tr("Pattern")), |ui| {
            if slot_strip(ui, theme, draft, cols, playhead, &palettes, &mut edit.selected) {
                changed = true;
                commit = true;
            }
        });
        let title = format!("{} {}", tr("Slot"), edit.selected.min(STEPS - 1) + 1);
        block(ui, theme, Some(&title), |ui| {
            if slot_editor(ui, theme, draft, &mut edit.selected, &mut edit.copied) {
                changed = true;
                commit = true;
            }
        });
    }
    edit.dirty |= changed;
    if step.is_some() && state == "looped" {
        crate::shell::frames::repaint_after(ui.ctx(), std::time::Duration::from_millis(60));
    }

    if edit.dirty && edit.factory {
        block(ui, theme, None, |ui| {
            note(ui, theme, tr("A factory template stays as shipped."));
            ui.horizontal_wrapped(|ui| {
            if ui.button(small_text(theme, tr("Save as copy"))).clicked() {
                if let Some(d) = edit.draft.as_ref() {
                    match save_as_copy(door, gid, node_id, d) {
                        Ok(new_id) => {
                            edit.note = Some(trf("saved as {id}", &[("id", &new_id)]));
                            edit.draft = None;
                            edit.dirty = false;
                        }
                        Err(e) => edit.note = Some(e),
                    }
                }
            }
            if ui.button(small_text(theme, tr("Discard"))).clicked() {
                edit.draft = None;
                edit.dirty = false;
            }
            });
        });
    } else if commit && edit.dirty {
        if let Some(d) = edit.draft.as_ref() {
            match door.json("POST", "/loops/save", Some(json!({ "loop": d }))) {
                Ok(_) => edit.dirty = false,
                Err((_, e)) => edit.note = Some(e),
            }
        }
    }
    if let Some(n) = &edit.note {
        note(ui, theme, n);
    }
}

/// The 16 cell rects of the strip: one row of 16, or two rows of 8 when the
/// panel is too narrow to read them.
fn slot_cells(rect: Rect, gap: f32, two_rows: bool) -> Vec<Rect> {
    let (per, rows) = if two_rows { (8, 2) } else { (16, 1) };
    let w = (rect.width() - gap * (per as f32 - 1.0)) / per as f32;
    let h = (rect.height() - gap * (rows as f32 - 1.0)) / rows as f32;
    (0..STEPS)
        .map(|i| {
            let (r, c) = (i / per, i % per);
            Rect::from_min_size(
                Pos2 { x: rect.left() + c as f32 * (w + gap), y: rect.top() + r as f32 * (h + gap) },
                Vec2 { x: w, y: h },
            )
        })
        .collect()
}

/// The 16-cell strip: click a cell to toggle its slot on/off, right-click to
/// clear it. The selected cell is outlined, the clock's slot lit, and an
/// on-cell paints a mini bar per float it holds. Whether the draft changed.
fn slot_strip(
    ui: &mut Ui,
    theme: &Theme,
    draft: &mut Loop,
    cols: usize,
    playhead: Option<usize>,
    palettes: &Value,
    selected: &mut usize,
) -> bool {
    let two_rows = ui.available_width() < theme.control_width * 8.0;
    let rows = if two_rows { 2.0 } else { 1.0 };
    let size = Vec2 {
        x: ui.available_width(),
        y: theme.interact_size.y * 2.0 * rows,
    };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let cells = slot_cells(rect, theme.item_gap.x.max(1.0), two_rows);
    let radius: CornerRadius = theme.radius.into();
    let painter = ui.painter_at(rect);
    for (i, cell) in cells.iter().enumerate() {
        painter.rect_filled(*cell, radius, theme.ink_faint.gamma_multiply(0.25));
        if let Some(swatch) = palette_swatch(draft, palettes, i) {
            painter.rect_filled(*cell, radius, swatch);
        } else if let Some(row) = draft.slots.get(i).and_then(|s| s.as_ref()) {
            let bw = cell.width() / cols as f32;
            for (j, v) in row.iter().take(cols).enumerate() {
                let h = cell.height() * v.clamp(0.0, 1.0);
                let bar = Rect::from_min_max(
                    Pos2 { x: cell.left() + j as f32 * bw, y: cell.bottom() - h },
                    Pos2 { x: cell.left() + (j as f32 + 1.0) * bw, y: cell.bottom() },
                );
                painter.rect_filled(bar, radius, theme.ink);
            }
        }
        if playhead == Some(i) {
            painter.rect_stroke(*cell, radius, theme.accent(), egui::StrokeKind::Inside);
        }
        if *selected == i {
            painter.rect_stroke(*cell, radius, theme.line_stroke(theme.ink), egui::StrokeKind::Inside);
        }
    }
    let at = |p: Pos2| cells.iter().position(|c| c.contains(p));
    if resp.secondary_clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(at) {
            *selected = i;
            return draft.set_slot(i, None);
        }
    } else if resp.clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(at) {
            *selected = i;
            return toggle_slot(draft, i);
        }
    }
    false
}

/// The selected slot's keyframe: one numeric row per float the loop targets,
/// its curve toggle, and copy/paste/previous/clear. Whether the draft changed.
fn slot_editor(
    ui: &mut Ui,
    theme: &Theme,
    draft: &mut Loop,
    selection: &mut usize,
    copied: &mut Option<Vec<f32>>,
) -> bool {
    let selected = (*selection).min(STEPS - 1);
    let labels: Vec<(usize, String)> = draft.slot_tracks().map(|(i, t)| (i, track_label(&t.target))).collect();
    if !draft.slot_on(selected) {
        note(ui, theme, tr("holds the previous slot"));
    }
    let mut changed = false;
    let mut row = draft.slot_held(selected).cloned().unwrap_or_else(|| vec![0.0; labels.len()]);
    for (j, (track, label)) in labels.iter().enumerate() {
        kv_row(ui, theme, label, |ui| {
            let curve = draft.tracks[*track].curve;
            let text = match curve {
                Curve::Step => tr("Step"),
                Curve::Ramp => tr("Ramp"),
            };
            let button_w = theme.interact_size.x * 1.5;
            let track_w = (ui.available_width() - button_w - theme.item_gap.x).max(theme.interact_size.x);
            let mut resp = None;
            if let Some(value) = row.get_mut(j) {
                let r = num_value(ui, theme, track_w, value, 0.0..=1.0, "", 2, 0.0, true);
                changed |= r.changed();
                resp = Some(r);
            }
            let b = ui.add_sized(
                [button_w, ui.spacing().interact_size.y],
                egui::Button::new(small_text(theme, text)),
            );
            if b.clicked() {
                draft.tracks[*track].curve = match curve {
                    Curve::Step => Curve::Ramp,
                    Curve::Ramp => Curve::Step,
                };
                changed = true;
            }
            resp.unwrap_or(b)
        });
    }
    if changed {
        draft.set_slot(selected, Some(row));
    }
    kv_wrap(ui, theme, tr("actions"), |ui| {
        if ui.button(small_text(theme, tr("Copy slot"))).clicked() {
            *copied = draft.slot_held(selected).cloned();
        }
        if ui.button(small_text(theme, tr("Paste slot"))).clicked() {
            if let Some(row) = copied.as_ref().cloned() {
                changed |= draft.set_slot(selected, Some(row));
            }
        }
        if ui.button(small_text(theme, tr("Copy previous"))).clicked() {
            let prev = (selected + STEPS - 1) % STEPS;
            if let Some(row) = draft.slot_held(prev).cloned() {
                changed |= draft.set_slot(selected, Some(row));
            }
        }
        if ui.button(small_text(theme, tr("Clear slot"))).clicked() {
            changed |= draft.set_slot(selected, None);
        }
    });
    changed
}

/// `POST /loops/choose`.
/// T16: "{bpm} BPM", four beat lights (the "1" first, the playing one lit)
/// and the "Tap 1" button (`POST /loops/tap`: the beat nearest now becomes
/// the "1").
fn clock_header(ui: &mut Ui, theme: &Theme, door: &dyn Call, info: &Value) {
    if let Some(bpm) = info["bpm"].as_f64().filter(|b| *b > 0.0) {
        ui.label(
            RichText::new(trf("{bpm} BPM", &[("bpm", &format!("{bpm:.0}"))]))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }
    let beat = info["beat"].as_u64().unwrap_or(0) as usize;
    let lights: String = (0..4).map(|i| if i == beat { if i == 0 { '●' } else { '◆' } } else { '·' }).collect();
    ui.label(small_text(theme, lights).color(theme.ink))
        .on_hover_text(tr("The bar's four beats; the first is the \"1\"."));
    let tap = ui
        .button(small_text(theme, tr("Tap 1")))
        .on_hover_text(tr("Press on the \"1\": the nearest beat becomes the start of the bar."));
    if tap.clicked() {
        tap_one(door);
    }
}

/// `POST /loops/tap`.
pub(crate) fn tap_one(door: &dyn Call) {
    let _ = door.json("POST", "/loops/tap", Some(json!({})));
}

pub(crate) fn choose(door: &dyn Call, gid: &str, node_id: &str, choice: &str) {
    let _ = door.json("POST", "/loops/choose", Some(json!({ "graph": gid, "node": node_id, "loop": choice })));
}

/// Saves `draft` as a new user loop and selects it on the node; the new id.
pub(crate) fn save_as_copy(door: &dyn Call, gid: &str, node_id: &str, draft: &Loop) -> Result<String, String> {
    let mut copy = draft.clone();
    copy.name = format!("{} copy", draft.name);
    let reply = door
        .json("POST", "/loops/save", Some(json!({ "loop": copy, "as_copy": true })))
        .map_err(|(_, e)| e)?;
    let id = reply["id"].as_str().ok_or_else(|| "no id".to_string())?.to_string();
    choose(door, gid, node_id, &id);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use crate::shell::FakeDoor;
    use egui_kittest::kittest::Queryable;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("metal-dash-loops-{tag}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&d);
        metal_vfs::create_dir_all(&d).unwrap();
        d
    }

    fn factory() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/loops/templates")
    }

    /// The real door, with the library re-pointed at the repo's factory
    /// templates and a tempdir for user loops (rule 8).
    fn real_door(user: &Path) -> crate::call::Door {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let author = {
            let _chdir = crate::chdir_lock();
            let prior = std::env::current_dir().expect("cwd");
            std::env::set_current_dir(&repo_root).expect("chdir");
            let a = crate::Author::new();
            std::env::set_current_dir(&prior).expect("restore cwd");
            std::sync::Arc::new(a)
        };
        let paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        state.loops = metal_perform::pattern::Library::load(&factory(), user);
        let sim = crate::listeners::Listeners::stand_in();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(crate::Shared { state, paint, sim }));
        crate::call::Door::new(shared, author)
    }

    /// `audio → gen.turing → fx.vignette`, saved to the scratch graphs dir.
    fn saved_graph(tag: &str) -> (String, String, String) {
        let gid = format!("loops-{tag}-{}", std::process::id());
        let mut g = metal_graph::Graph::new(&gid, "loops", metal_graph::Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [200.0, 0.0]);
        let fx = g.add_node("fx", "fx.vignette", [400.0, 0.0]);
        g.add_edge(metal_graph::Endpoint::new(&audio, "control"), metal_graph::Endpoint::new(&gen, "control"));
        g.add_edge(metal_graph::Endpoint::new(&gen, "image"), metal_graph::Endpoint::new(&fx, "image"));
        crate::save_graph(&g).expect("scratch graphs dir");
        (gid, gen, fx)
    }

    fn loop_config(gid: &str, node: &str) -> Option<Value> {
        crate::load_graph(gid).unwrap().node(node).unwrap().config.get("loop").cloned()
    }

    fn harness(door: Rc<FakeDoor>, gid: &str, node: &str) -> egui_kittest::Harness<'static, Edit> {
        let theme = Theme::phosphor();
        let (gid, node) = (gid.to_string(), node.to_string());
        let t = theme.clone();
        let h = egui_kittest::Harness::builder().with_size(Vec2 { x: 700.0, y: 500.0 }).build_ui_state(
            move |ui, edit: &mut Edit| section(ui, &t, door.as_ref(), &gid, &node, edit),
            Edit::default(),
        );
        theme.apply(&h.ctx);
        h
    }

    /// The picker lists the node's named templates beside Default and None,
    /// the badge says looped, and the selected slot's keyframe shows its float.
    #[test]
    fn the_picker_lists_templates_and_the_slot_editor_shows() {
        let door = Rc::new(FakeDoor::new());
        door.respond(
            "GET",
            "/loops/node",
            json!({ "target": "model:turing", "choice": "default", "playing": "turing-heartbeat",
                    "state": "looped", "step": 3, "bar": 0, "beat": 0, "bpm": 128.0 }),
        );
        door.respond(
            "GET",
            "/loops",
            json!({ "default": { "id": "turing-heartbeat", "name": "Heartbeat" }, "loops": [
                { "id": "turing-climb", "name": "Climb", "origin": "factory" },
                { "id": "turing-heartbeat", "name": "Heartbeat", "origin": "factory" },
                { "id": "turing-mine", "name": "Mine", "origin": "user" } ] }),
        );
        door.respond(
            "GET",
            "/loops/get",
            json!({ "origin": "factory", "loop": { "id": "turing-heartbeat", "name": "Heartbeat",
                "target": { "model": "turing" }, "tracks": [
                    { "target": { "kind": "float", "param": "scale" }, "curve": "step",
                      "steps": [0.5, null, null, null, 0.2, null, null, null, 0.9, null, null, null, 0.1, null, null, null] } ] } }),
        );
        let mut h = harness(door.clone(), "g1", "gen-0");
        h.run_steps(3);
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run_steps(3);
        h.get_by_label("None — audio/fixed");
        h.get_by_label("Climb");
        h.get_by_label("Mine (mine)");
        h.get_by_label("Looped");
        h.get_by_label("scale");
        h.get_by_label("Step");
        h.get_by_label("128 BPM");
        h.get_by_label("Copy slot");
        h.get_by_label("Copy previous");
        h.get_by_label("Clear slot");
        h.get_by_label("Tap 1").click();
        h.run_steps(3);
        assert!(door.calls.borrow().iter().any(|(m, p, _)| m == "POST" && p == "/loops/tap"), "tap posted");
        h.get_by_role(egui::accesskit::Role::ComboBox).click();
        h.run_steps(3);
        h.get_by_label("Climb").click();
        h.run_steps(3);
        let calls = door.calls.borrow();
        let posted = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/loops/choose")
            .map(|(_, _, p)| serde_json::from_slice::<Value>(&p.body).unwrap())
            .expect("a choose post");
        assert_eq!(posted, json!({ "graph": "g1", "node": "gen-0", "loop": "turing-climb" }));
    }

    /// A slot toggles on/off in the draft: off holds the previous value.
    #[test]
    fn a_slot_toggles_on_and_off() {
        let mut l: Loop = serde_json::from_value(json!({ "id": "t", "name": "T",
            "target": { "model": "turing" }, "tracks": [
                { "target": { "kind": "float", "param": "scale" }, "curve": "step",
                  "steps": [0.5, null, null, null, 0.2, null, null, null, 0.9, null, null, null, 0.1, null, null, null] } ] }))
        .unwrap();
        l.migrate();
        assert!(l.slot_on(0));
        assert!(!l.slot_on(1));
        assert!(toggle_slot(&mut l, 1), "turning an off slot on changes it");
        assert!(l.slot_on(1));
        assert_eq!(l.slots[1], Some(vec![0.5]), "it starts from the held value");
        assert!(toggle_slot(&mut l, 1));
        assert!(!l.slot_on(1), "and turns off again");
    }

    /// Choosing writes the node's `loop` key: an id, `"none"`, and `default`
    /// removes it again; `/loops/node` reads each back.
    #[test]
    fn choosing_writes_the_config_key() {
        let user = scratch("choose");
        let door = real_door(&user);
        let (gid, gen, _) = saved_graph("choose");
        let node_info = || get(&door, "GET", "/loops/node", &[("graph", gid.clone()), ("node", gen.clone())]).unwrap();
        choose(&door, &gid, &gen, "turing-climb");
        assert_eq!(loop_config(&gid, &gen), Some(json!("turing-climb")));
        assert_eq!(node_info()["playing"], json!("turing-climb"));
        assert_eq!(node_info()["state"], json!("looped"));
        choose(&door, &gid, &gen, "none");
        assert_eq!(loop_config(&gid, &gen), Some(json!("none")));
        assert_eq!(node_info()["state"], json!("reactive"));
        choose(&door, &gid, &gen, "default");
        assert_eq!(loop_config(&gid, &gen), None);
        assert!(node_info()["playing"].is_string(), "the default factory template plays");
        let list = get(&door, "GET", "/loops", &[("target", "model:turing".into())]).unwrap();
        assert!(list["loops"].as_array().unwrap().len() >= 3);
        assert!(list["default"]["id"].is_string());
        let _ = metal_vfs::remove_dir_all(&user);
    }

    /// `set_effect` (the effect page's fixed slider) and the layer op keep the
    /// node's `loop` key.
    #[test]
    fn set_effect_and_layer_preserve_the_loop_key() {
        let user = scratch("preserve");
        let door = real_door(&user);
        let (gid, gen, fx) = saved_graph("preserve");
        choose(&door, &gid, &fx, "fx-vignette-iris");
        choose(&door, &gid, &gen, "turing-skip");
        super::super::effects::post_effect(&door, &gid, &gen, "vignette", "fixed", 0.4);
        door.json("POST", "/graph/node", Some(json!({ "graph": gid, "op": "layer", "node": gen, "opacity": 0.5 })))
            .unwrap();
        assert_eq!(loop_config(&gid, &fx), Some(json!("fx-vignette-iris")));
        let g = crate::load_graph(&gid).unwrap();
        assert_eq!(g.node(&fx).unwrap().config.get("fixed").and_then(Value::as_f64).map(|v| v as f32), Some(0.4));
        assert_eq!(loop_config(&gid, &gen), Some(json!("turing-skip")));
        let info = get(&door, "GET", "/loops/node", &[("graph", gid.clone()), ("node", fx.clone())]).unwrap();
        assert_eq!(info["state"], json!("looped"));
        let _ = metal_vfs::remove_dir_all(&user);
    }

    /// A slot edit on a factory template, saved as a copy, lands as a user
    /// loop in the (temp) user dir, is selected on the node, and the factory
    /// template is unchanged; saving over a factory id is refused.
    #[test]
    fn slot_edit_and_save_as_copy_create_a_user_loop() {
        let user = scratch("copy");
        let door = real_door(&user);
        let (gid, gen, _) = saved_graph("copy");
        let reply = get(&door, "GET", "/loops/get", &[("id", "turing-heartbeat".into())]).unwrap();
        assert_eq!(reply["origin"], json!("factory"));
        let mut draft: Loop = serde_json::from_value(reply["loop"].clone()).unwrap();
        draft.migrate();
        let before = draft.clone();
        let cols = draft.slot_tracks().count();
        let mut row = draft.slot_held(5).cloned().unwrap_or_else(|| vec![0.0; cols]);
        row[0] = 0.75;
        assert!(draft.set_slot(5, Some(row)));
        assert!(door.json("POST", "/loops/save", Some(json!({ "loop": draft }))).is_err(), "factory id refused");
        let id = save_as_copy(&door, &gid, &gen, &draft).unwrap();
        assert_ne!(id, "turing-heartbeat");
        assert!(metal_vfs::is_file(&user.join(format!("{id}.json"))));
        let saved = get(&door, "GET", "/loops/get", &[("id", id.clone())]).unwrap();
        assert_eq!(saved["origin"], json!("user"));
        assert_eq!(saved["loop"]["slots"][5][0], json!(0.75));
        assert_eq!(loop_config(&gid, &gen), Some(json!(id.clone())));
        let orig = get(&door, "GET", "/loops/get", &[("id", "turing-heartbeat".into())]).unwrap();
        assert_eq!(serde_json::from_value::<Loop>(orig["loop"].clone()).unwrap(), before);
        // The user loop itself saves in place.
        let mut mine: Loop = serde_json::from_value(saved["loop"].clone()).unwrap();
        mine.set_slot(0, None);
        door.json("POST", "/loops/save", Some(json!({ "loop": mine }))).unwrap();
        let list = get(&door, "GET", "/loops", &[("target", "model:turing".into())]).unwrap();
        assert!(list["loops"].as_array().unwrap().iter().any(|l| l["id"] == json!(id) && l["origin"] == json!("user")));
        let _ = metal_vfs::remove_dir_all(&user);
    }
}
