//! Properties for the colour kinds (`color`, `palette`): the HSB picker,
//! palette slots and their relational editing.
//!
//! The properties panel is a browser: a tree of folders for the selected
//! node on the left, the chosen folder's content on the right. This module
//! adds its own folders to that tree: [`folders`] names them for a node kind
//! (English source text, painted through `i18n::tr`; empty for kinds this
//! module does not own), and [`section`] paints one of them when the operator
//! opens it.
//!
//! # Rule 6 — every control is a route call
//!
//! A picker does not write the node's config itself: releasing it posts
//! `POST /graph/node {op:"config"}` through the door, and the palette folder's
//! relational move posts `POST /author/palettes {op:"set", id, slot, value,
//! relative}`. The AI's `set_palette` tool posts the same route, so the
//! operator and the model write a palette one way.

use egui::Ui;
use metal_notation::color::Hsb;
use serde_json::Value;

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components;
use crate::shell::i18n::{tr, tr_key};

/// What this module keeps between frames (drafts, fetched lists, notes).
#[derive(Default)]
pub struct State {
    /// The palette ids fetched from `GET /author/palettes`, and their resolved
    /// four-slot colours — the list the stored-palette combo offers. Fetched
    /// once and kept; a save elsewhere is reread the next time the folder is
    /// reopened.
    palettes: Vec<(String, Vec<[f32; 3]>)>,
    /// Whether a palette slot edit moves the **whole** palette (a relational
    /// shift) rather than setting one slot in isolation — the "move the
    /// palette" toggle.
    relative: bool,
    /// Set once the list has been asked for, so a door that answers nothing
    /// is not re-asked every frame.
    fetched: bool,
}

/// The folder names this module owns, as `tr_key` literals so the i18n scanner
/// sees them and the panel paints them through the table — see [`folders`].
const COLOUR_FOLDERS: [&str; 1] = [tr_key("Colour")];
const PALETTE_FOLDERS: [&str; 1] = [tr_key("Palette")];

/// The folders this module adds for a node of `kind`, in tree order. Each
/// name is an English literal marked with `i18n::tr_key` (its Spanish in
/// this module's locale file).
pub fn folders(kind: &str) -> &'static [&'static str] {
    match kind {
        "color" => &COLOUR_FOLDERS,
        "palette" => &PALETTE_FOLDERS,
        _ => &[],
    }
}

/// Paint `folder` (one of [`folders`]) for `node` — the `GET /graph` node
/// object, `graph` the whole reply — and answer whether it painted anything.
#[allow(clippy::too_many_arguments)]
pub fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    _graph: &Value,
    folder: &str,
    state: &mut State,
) -> bool {
    super::properties::shared::repaint_while_pending(ui.ctx());
    match folder {
        "Colour" => {
            colour_folder(ui, theme, door, gid, node_id, node);
            true
        }
        "Palette" => {
            palette_folder(ui, theme, door, gid, node_id, node, state);
            true
        }
        _ => false,
    }
}

/// The node's config as an object, or an empty one.
fn config_of(node: &Value) -> serde_json::Map<String, Value> {
    node.get("config").and_then(Value::as_object).cloned().unwrap_or_default()
}

/// The `[r, g, b]` a config key holds, defaulted when missing.
fn rgb_of(config: &serde_json::Map<String, Value>) -> [f32; 3] {
    let a = config.get("rgb").and_then(Value::as_array);
    let get = |i: usize, d: f32| a.and_then(|a| a.get(i)).and_then(Value::as_f64).map(|v| v as f32).unwrap_or(d);
    [get(0, 1.0), get(1, 1.0), get(2, 1.0)]
}

/// One colour the config's `colors` slot holds, or white.
fn slot_of(config: &serde_json::Map<String, Value>, slot: usize) -> [f32; 3] {
    config
        .get("colors")
        .and_then(Value::as_array)
        .and_then(|a| a.get(slot))
        .and_then(Value::as_array)
        .map(|c| {
            let get = |i: usize| c.get(i).and_then(Value::as_f64).map(|v| v as f32).unwrap_or(1.0);
            [get(0), get(1), get(2)]
        })
        .unwrap_or([1.0, 1.0, 1.0])
}

/// `POST /graph/node {op:"config"}` — the one way a panel writes a node's
/// config (the same body `nodes.rs` writes a still's file with).
fn post_config(door: &dyn Call, gid: &str, node_id: &str, config: Value) {
    super::properties::shared::post_node_config(door, gid, node_id, config);
}

/// The `color` folder: the picker, its numbers and its hex.
fn colour_folder(ui: &mut Ui, theme: &Theme, door: &dyn Call, gid: &str, node_id: &str, node: &Value) {
    let config = config_of(node);
    let mut hsb = Hsb::from_rgb(rgb_of(&config)[0], rgb_of(&config)[1], rgb_of(&config)[2]);

    let response = components::colour_editor(ui, theme, &mut hsb);
    let [r, g, b] = hsb.to_rgb();
    if response.changed() {
        post_config(
            door,
            gid,
            node_id,
            serde_json::json!({ "rgb": [r, g, b], "h": hsb.h, "s": hsb.s, "b": hsb.b }),
        );
    }
}

/// The `palette` folder: a picker per slot, the stored-palette combo, and the
/// relational-move toggle.
fn palette_folder(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    state: &mut State,
) {
    if !state.fetched {
        if let Ok(reply) = door.json("GET", "/author/palettes", None) {
            state.palettes = reply
                .get("palettes")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|p| {
                            let name = p.get("name")?.as_str()?.to_string();
                            let colors = p
                                .get("colors")?
                                .as_array()?
                                .chunks_exact(3)
                                .map(|c| {
                                    [
                                        c[0].as_f64().unwrap_or(1.0) as f32,
                                        c[1].as_f64().unwrap_or(1.0) as f32,
                                        c[2].as_f64().unwrap_or(1.0) as f32,
                                    ]
                                })
                                .collect();
                            Some((name, colors))
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        state.fetched = true;
    }

    let config = config_of(node);
    let id = config.get("palette").and_then(Value::as_str).unwrap_or("").to_string();
    components::kv_row(ui, theme, tr("Palettes"), |ui| {
        components::combo_box(theme, "palette-node-stored")
            .width(ui.available_width())
            .selected_text(if id.is_empty() { tr("(none)").to_string() } else { id.clone() })
            .show_ui(ui, |ui| {
                for (name, colors) in &state.palettes {
                    // The combination itself: its slots as chips ahead of the name.
                    let clicked = ui
                        .horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for rgb in colors {
                                let (rect, _) = ui.allocate_exact_size(theme.swatch_size, egui::Sense::hover());
                                ui.painter().rect_filled(rect, theme.radius, theme.color_of(*rgb));
                            }
                            ui.add_space(theme.item_gap.x);
                            ui.selectable_label(*name == id, name.clone()).clicked()
                        })
                        .inner;
                    if clicked {
                        let colors: Vec<Value> =
                            colors.iter().map(|c| serde_json::json!([c[0], c[1], c[2]])).collect();
                        post_config(
                            door,
                            gid,
                            node_id,
                            serde_json::json!({ "palette": name, "colors": colors }),
                        );
                    }
                }
            })
            .response
    });

    // The relational toggle: check = move the whole palette, unset = set the
    // one slot.
    components::toggle_row(ui, theme, tr("Move the palette"), &mut state.relative);

    ui.add_space(theme.section_gap);
    for slot in 0..4 {
        let mut hsb = Hsb::from_rgb(
            slot_of(&config, slot)[0],
            slot_of(&config, slot)[1],
            slot_of(&config, slot)[2],
        );
        let slot_label = format!("{} {}", tr("Slot"), slot);
        let response = components::colour_row(ui, theme, &slot_label, &mut hsb);
        {
            if response.changed() {
                let [r, g, b] = hsb.to_rgb();
                if state.relative && !id.is_empty() {
                    // The relational move is the route's own op, so the whole
                    // palette shifts together (the operator's gesture and the
                    // AI's `set_palette` reach the same function).
                    let _ = door.json(
                        "POST",
                        "/author/palettes",
                        Some(serde_json::json!({
                            "op": "set", "id": id, "slot": slot,
                            "value": [r, g, b], "relative": true,
                        })),
                    );
                } else {
                    // Set this slot in the node's own colours.
                    let mut colors: Vec<[f32; 3]> = (0..4).map(|i| slot_of(&config, i)).collect();
                    colors[slot as usize] = [r, g, b];
                    let colors: Vec<Value> =
                        colors.iter().map(|c| serde_json::json!([c[0], c[1], c[2]])).collect();
                    post_config(door, gid, node_id, serde_json::json!({ "colors": colors }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::rc::Rc;

    use egui_kittest::{kittest::Queryable, Harness};

    use crate::shell::classes::Theme;
    use crate::shell::{FakeDoor, DESKTOP_VIEWPORT};

    /// The harness viewport — the app's own desktop size, so no test carries a
    /// visual literal (gate 2) and the folder lays out in real conditions.
    fn folder_viewport() -> egui::Vec2 {
        egui::vec2(DESKTOP_VIEWPORT.0 as f32, DESKTOP_VIEWPORT.1 as f32)
    }

    /// The harness every folder test runs: `props_colour::section` painted with
    /// a door the test keeps a handle on, so a picker's post is inspected as
    /// the exact body the panel sent.
    fn folder_harness(
        kind: &str,
        folder: &str,
        node: Value,
    ) -> (Harness<'static, TestState>, Rc<FakeDoor>) {
        let door = Rc::new(FakeDoor::new());
        let theme = Theme::phosphor();
        assert!(
            folders(kind).contains(&folder),
            "{kind} must own the {folder:?} folder"
        );
        let folder = folder.to_string();
        let door_for_ui = Rc::clone(&door);
        let state = TestState {
            theme: theme.clone(),
            node,
            folder,
            door: door_for_ui,
            panel: State::default(),
        };
        let harness = Harness::builder()
            .with_size(folder_viewport())
            .build_ui_state(
                |ui, s: &mut TestState| {
                    let door: &dyn Call = s.door.as_ref();
                    section(ui, &s.theme, door, "g1", "node-0", &s.node, &Value::Null, &s.folder, &mut s.panel);
                },
                state,
            );
        theme.apply(&harness.ctx);
        (harness, door)
    }

    struct TestState {
        theme: Theme,
        node: Value,
        folder: String,
        door: Rc<FakeDoor>,
        panel: State,
    }

    fn colour_node(rgb: [f32; 3]) -> Value {
        serde_json::json!({ "id": "node-0", "kind": "color", "config": { "rgb": rgb } })
    }

    fn palette_node() -> Value {
        serde_json::json!({
            "id": "node-0", "kind": "palette",
            "config": { "palette": "hotel", "colors": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]] },
        })
    }

    fn posts(door: &FakeDoor) -> Vec<Value> {
        door.calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .filter_map(|(_, _, params)| serde_json::from_slice(&params.body).ok())
            .collect()
    }

    fn palette_posts(door: &FakeDoor) -> Vec<Value> {
        door.calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/author/palettes")
            .filter_map(|(_, _, params)| serde_json::from_slice(&params.body).ok())
            .collect()
    }

    /// A drag in the S×B square writes the node **once**, on release — not once
    /// per pixel of the drag. The picker is painted at a rect the test chooses,
    /// so the square's coordinates are known exactly.
    #[test]
    fn a_drag_in_the_sb_square_posts_once_on_release() {
        let door = Rc::new(FakeDoor::new());
        let theme = Theme::phosphor();
        let square = theme.picker_square;
        let origin = egui::pos2(20.0, 20.0);
        let rect = egui::Rect::from_min_size(origin, square);
        let door_for_ui = Rc::clone(&door);
        let theme_for_ui = theme.clone();
        let mut harness = Harness::builder()
            .with_size(folder_viewport())
            .build_ui_state(
                move |ui, value: &mut Hsb| {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        let response = components::hsb_picker(ui, &theme_for_ui, value);
                        if response.changed() {
                            let [r, g, b] = value.to_rgb();
                            let door: &dyn Call = door_for_ui.as_ref();
                            let _ = door.json(
                                "POST",
                                "/graph/node",
                                Some(serde_json::json!({ "graph": "g1", "node": "node-0", "op": "config", "config": { "rgb": [r, g, b] } })),
                            );
                        }
                    });
                },
                Hsb::new(0.0, 1.0, 1.0),
            );
        theme.apply(&harness.ctx);
        harness.run();

        // Press inside the square, drag across it, release. Every frame of the
        // drag updates the value; only the release posts.
        let start = origin + egui::vec2(square.x * 0.25, square.y * 0.75);
        let end = origin + egui::vec2(square.x * 0.8, square.y * 0.2);
        harness.drag_at(start);
        harness.run();
        harness.hover_at(end);
        harness.run();
        harness.drop_at(end);
        harness.run();
        harness.run();

        let written = posts(&door);
        assert_eq!(written.len(), 1, "a drag posts exactly once, on release: {written:?}");
        assert_eq!(written[0]["op"], "config");
    }

    /// The `Colour` folder paints the picker for a colour node and posts
    /// nothing until it is touched.
    #[test]
    fn the_colour_folder_paints_without_posting() {
        let (mut harness, door) = folder_harness("color", "Colour", colour_node([1.0, 0.0, 0.0]));
        harness.run();
        assert!(posts(&door).is_empty(), "opening the folder must not write anything");
        // It painted the numeric readouts the picker sits beside.
        harness.get_by_label("H");
        harness.get_by_label("S");
        harness.get_by_label("B");
    }

    /// The `Palette` folder's stored combo, its relational toggle and one picker
    /// per slot — and dragging a slot's picker with the toggle on posts
    /// `POST /author/palettes {op:"set", relative:true}` for the whole palette.
    /// Open slot 0's colour popup and commit a typed hex.
    fn edit_slot0(harness: &mut Harness<'static, TestState>) {
        let slot0 = harness.get_by_label("Slot 0").rect();
        let at = egui::pos2(slot0.right() + 24.0, slot0.center().y);
        harness.drag_at(at);
        harness.run();
        harness.drop_at(at);
        harness.run();
        let hex = harness.get_by_role(egui::accesskit::Role::TextInput);
        hex.click();
        harness.run();
        harness.get_by_role(egui::accesskit::Role::TextInput).type_text("00ff00");
        harness.run();
        harness.key_press(egui::Key::Enter);
        // A write stays pending (and repainting) until the read-back agrees.
        harness.run_steps(4);
    }

    #[test]
    fn a_relative_palette_edit_posts_the_set_op() {
        let (mut harness, door) = folder_harness("palette", "Palette", palette_node());
        harness.run();
        // The stored list is fetched once.
        assert!(door.calls.borrow().iter().any(|(m, p, _)| m == "GET" && p == "/author/palettes"));

        // Turn on the relational move, then drag the first slot's picker.
        harness.get_by_label("Move the palette").click();
        harness.run();

        edit_slot0(&mut harness);

        let sets = palette_posts(&door);
        assert_eq!(sets.len(), 1, "one drag, one relational set: {sets:?}");
        assert_eq!(sets[0]["op"], "set");
        assert_eq!(sets[0]["id"], "hotel");
        assert_eq!(sets[0]["slot"], 0);
        assert_eq!(sets[0]["relative"], true);
    }

    /// With the toggle off, the same drag writes the node's own `colors` through
    /// `POST /graph/node` and leaves the stored palette alone.
    #[test]
    fn an_isolated_palette_edit_posts_the_nodes_config() {
        let (mut harness, door) = folder_harness("palette", "Palette", palette_node());
        harness.run();

        edit_slot0(&mut harness);

        assert!(palette_posts(&door).is_empty(), "no stored write without the toggle");
        let config = posts(&door);
        assert_eq!(config.len(), 1, "one isolated write: {config:?}");
        assert!(config[0]["config"]["colors"].is_array());
    }
}
