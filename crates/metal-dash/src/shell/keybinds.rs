//! O.10b: the keyboard as a reader of the same `Trigger` (plan point 4, and
//! the `TriggerSource::Key` source of point 1).
//!
//! A grid keybind is **not** a shell shortcut: it is a `TriggerSource::Key`
//! on the same button OSC/MIDI/HTTP fire, so a key press ends in the one
//! `POST /trigger/<id>` route a human (or the AI) uses (`regla 6` — no second
//! path). This module owns the pure half: the keybind table, its contexts,
//! and [`drive`], the per-frame function that turns this frame's key events
//! into a door call. `Shell::ui` collects the events and calls [`drive`]; it
//! is a free function so a test can drive it against a recording door without
//! a window.
//!
//! **Contexts.** The plan splits the keyboard three ways — `global` (file,
//! `Tab` switches the focused panel), `panel` (arrows inside the panel) and
//! `grid` (the `Trigger`s). Which one is live comes from the focused region
//! ([`KbContext::from_focus`]): Lanes is the overlays grid, so it is `Grid`;
//! any other panel is `Panel`; nothing focused is `Global`.
//!
//! **Table.** [`default_binds`] is the static table, and `data/keybinds.json`
//! is merged over it ([`Keybinds::load`], the same "a missing file is the
//! defaults" discipline as `switch::Switches::load`). The four existing loose
//! shortcuts (`Space` transport, `Escape` close, `Ctrl+Z` undo, `Tab`
//! switch-panel) are named rows here; wiring them is a later lote — see the
//! `Action::Named` doc.

use super::panels::Region;

/// Which layer of the table a key belongs to. The context comes from the
/// focused region, never from a second piece of state (`regla 4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KbContext {
    /// No region focused: the table's always-on rows (`Space`, `Escape`,
    /// `Tab`).
    Global,
    /// A panel other than the overlays grid is focused.
    Panel,
    /// The overlays grid is focused (the Lanes region): the `TriggerSource::Key`
    /// rows fire.
    Grid,
}

impl KbContext {
    /// The context a focused region reads as. Lanes *is* the overlays grid
    /// (the panel and the canvas are one region), so it maps to `Grid`; any
    /// other panel maps to `Panel`; no focus at all maps to `Global`.
    pub fn from_focus(focus: Option<Region>) -> KbContext {
        match focus {
            None => KbContext::Global,
            Some(Region::Lanes) => KbContext::Grid,
            Some(_) => KbContext::Panel,
        }
    }

    #[cfg(test)]
    /// The JSON key of this context's bucket in `data/keybinds.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            KbContext::Global => "global",
            KbContext::Panel => "panel",
            KbContext::Grid => "grid",
        }
    }

    /// The inverse of [`Self::as_str`]; `None` for a bucket name the loader
    /// does not know (ignored, not an error).
    pub fn parse(name: &str) -> Option<KbContext> {
        match name {
            "global" => Some(KbContext::Global),
            "panel" => Some(KbContext::Panel),
            "grid" => Some(KbContext::Grid),
            _ => None,
        }
    }
}

/// What a bound key does. The grid rows carry the trigger id an operator
/// override points at; the global/panel rows name a built-in shortcut that a
/// later lote wires into the table (today they still live as the four loose
/// `key_pressed` sites, identical in behaviour).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Fire the grid trigger with this id (an operator override row with no
    /// `TriggerSource::Key` behind it still fires the same route).
    FireTrigger { trigger: Option<String> },
    /// One of the built-ins named in the plan: `"transport"`, `"close"`,
    /// `"undo"`, `"switch-panel"`.
    Named(&'static str),
}

/// One row of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keybind {
    pub context: KbContext,
    /// The egui key name (`"Space"`, `"F1"`, `"Z"`) — the same spelling
    /// `TriggerSource::Key.key` carries.
    pub key: String,
    /// Sorted modifier names (`"ctrl"`, `"shift"`, `"alt"`, `"command"`).
    pub mods: Vec<String>,
    pub action: Action,
}

/// The static table: the four built-in shortcuts the plan names. The grid
/// bucket is empty on purpose — a key only fires a grid button once the
/// operator bound one (`data/keybinds.json` or a learned `TriggerSource::Key`).
pub fn default_binds() -> Vec<Keybind> {
    let global = |key: &str, action: &'static str| Keybind {
        context: KbContext::Global,
        key: key.to_string(),
        mods: Vec::new(),
        action: Action::Named(action),
    };
    vec![
        global("Space", "transport"),
        global("Escape", "close"),
        global("Tab", "switch-panel"),
        Keybind {
            context: KbContext::Panel,
            key: "Z".to_string(),
            mods: vec!["ctrl".to_string()],
            action: Action::Named("undo"),
        },
    ]
}

/// The recognised built-in names, so an operator file can only point at one
/// of the four (an unknown name is ignored rather than silently dropped into
/// a row nothing can ever run).
fn named_action(name: &str) -> Option<&'static str> {
    match name {
        "transport" => Some("transport"),
        "close" => Some("close"),
        "undo" => Some("undo"),
        "switch-panel" => Some("switch-panel"),
        _ => None,
    }
}

/// The whole table: the defaults, with `data/keybinds.json` merged over them
/// ([`Keybinds::from_overrides`]).
#[derive(Clone, Debug, Default)]
pub struct Keybinds {
    binds: Vec<Keybind>,
}

impl Keybinds {
    #[cfg(test)]
    /// Every row, defaults first. Test-only reader plus the door for a later
    /// settings UI that lets the operator see the table.
    pub fn binds(&self) -> &[Keybind] {
        &self.binds
    }

    #[cfg(test)]
    /// The grid rows only — what a settings surface would edit.
    pub fn grid_binds(&self) -> impl Iterator<Item = &Keybind> {
        self.binds
            .iter()
            .filter(|b| b.context == KbContext::Grid)
    }

    /// The trigger id bound to `key`+`mods` in the `Grid` context, if any —
    /// the operator-override half of [`drive`]'s grid fire. `mods` is
    /// compared sorted, the same shape [`mod_names`] produces.
    pub fn grid_trigger(&self, key: &str, mods: &[String]) -> Option<&str> {
        self.binds.iter().find_map(|b| {
            if b.context == KbContext::Grid && b.key == key && b.mods.as_slice() == mods {
                if let Action::FireTrigger { trigger } = &b.action {
                    return trigger.as_deref();
                }
            }
            None
        })
    }

    /// Merge the defaults under a `data/keybinds.json` value. A row whose
    /// `(context, key, mods)` already exists replaces that row's action; any
    /// other row is appended. Unknown buckets and rows with no usable action
    /// are ignored — an unreadable file must never cost the built-ins.
    pub fn from_overrides(overrides: &serde_json::Value) -> Keybinds {
        let mut binds = default_binds();
        let Some(buckets) = overrides.as_object() else {
            return Keybinds { binds };
        };
        for (bucket, rows) in buckets {
            let Some(context) = KbContext::parse(bucket) else {
                continue;
            };
            let Some(rows) = rows.as_array() else {
                continue;
            };
            for row in rows {
                let Some(key) = row.get("key").and_then(|v| v.as_str()) else {
                    continue;
                };
                let mut mods: Vec<String> = row
                    .get("mods")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                mods.sort();
                let action = if let Some(trigger) = row.get("trigger").and_then(|v| v.as_str()) {
                    Action::FireTrigger {
                        trigger: Some(trigger.to_string()),
                    }
                } else if let Some(name) = row
                    .get("action")
                    .and_then(|v| v.as_str())
                    .and_then(named_action)
                {
                    Action::Named(name)
                } else {
                    continue;
                };
                let key = key.to_string();
                if let Some(existing) = binds.iter_mut().find(|b| {
                    b.context == context && b.key == key && b.mods.as_slice() == mods.as_slice()
                }) {
                    existing.action = action;
                } else {
                    binds.push(Keybind {
                        context,
                        key,
                        mods,
                        action,
                    });
                }
            }
        }
        Keybinds { binds }
    }

    /// The shipped table: `data/keybinds.json` over the defaults. A missing
    /// or unreadable file (or a malformed one) is the defaults, not an error —
    /// the same discipline `switch::Switches::load` keeps (`regla 4`: config
    /// is a read, and a bad read falls back, it does not fail the frame).
    pub fn load() -> Keybinds {
        let path = crate::output::data_root().join("keybinds.json");
        let Ok(text) = metal_vfs::read_to_string(&path) else {
            return Keybinds::from_overrides(&serde_json::Value::Null);
        };
        let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        Keybinds::from_overrides(&value)
    }
}

/// A trigger known to be live, as `GET /state` reports it. The shell caches
/// these each frame so [`drive`] can match a key against the same triggers
/// HTTP fires, without walking app state directly (`regla 9`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownTrigger {
    pub id: String,
    pub source: crate::trigger::TriggerSource,
}

/// Read the screens' triggers out of a `GET /state` reply. Each `outputs`
/// row carries a `triggers` array; a row whose trigger has no parseable
/// `source` is skipped (the real route reports `id`/`target`/`params` today,
/// a key-trigger reports its `source` too).
pub fn triggers_from_state(state: &serde_json::Value) -> Vec<KnownTrigger> {
    let mut out = Vec::new();
    let Some(rows) = state["outputs"].as_array() else {
        return out;
    };
    for row in rows {
        let Some(triggers) = row["triggers"].as_array() else {
            continue;
        };
        for t in triggers {
            let Some(id) = t.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(source) = t
                .get("source")
                .and_then(|v| serde_json::from_value::<crate::trigger::TriggerSource>(v.clone()).ok())
            else {
                continue;
            };
            out.push(KnownTrigger {
                id: id.to_string(),
                source,
            });
        }
    }
    out
}

/// Which view a `Ctrl+Z` reverts, read off the focused region: the nodes
/// canvas or the sequence editor (Lanes). Any other focus, or none, has no
/// undo of its own, so the press does nothing there.
pub fn undo_target(focus: Option<Region>) -> Option<Region> {
    match focus {
        Some(region @ (Region::Nodes | Region::Lanes)) => Some(region),
        _ => None,
    }
}

/// Whether `Ctrl+Z` (`Cmd+Z` on macOS) was pressed this frame, with no
/// `Shift` or `Alt`, and no text field holding keyboard focus. A text field
/// keeps its own undo, so this backs off for it. `Ctrl+Shift+Z` is left to a
/// future redo. A focused button that kept the focus after a click does not
/// count as a text field (the same check the nodes Delete key uses).
pub fn undo_pressed(ctx: &egui::Context) -> bool {
    let text_field_focused = ctx
        .memory(|m| m.focused())
        .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
    if text_field_focused {
        return false;
    }
    ctx.input(|i| {
        i.key_pressed(egui::Key::Z)
            && (i.modifiers.ctrl || i.modifiers.command)
            && !i.modifiers.shift
            && !i.modifiers.alt
    })
}

/// The UI-scale chord pressed this frame: `Ctrl+=`/`Ctrl++` is `Some(1)`,
/// `Ctrl+-` is `Some(-1)`, `Ctrl+0` is `Some(0)` (reset). Backs off while a
/// text field holds the focus, like [`undo_pressed`].
pub fn scale_steps(ctx: &egui::Context) -> Option<i32> {
    let text_field_focused = ctx
        .memory(|m| m.focused())
        .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
    if text_field_focused {
        return None;
    }
    ctx.input(|i| {
        if !(i.modifiers.ctrl || i.modifiers.command) || i.modifiers.alt {
            return None;
        }
        if i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus) {
            Some(1)
        } else if i.key_pressed(egui::Key::Minus) {
            Some(-1)
        } else if i.key_pressed(egui::Key::Num0) {
            Some(0)
        } else {
            None
        }
    })
}

/// The egui key name a `TriggerSource::Key.key` carries. `egui::Key`'s `Debug`
/// is exactly that spelling (`F1`, `Space`, `Z`), so the table and the learn
/// write agree with no mapping table to rot.
pub fn key_name(key: egui::Key) -> String {
    format!("{key:?}")
}

/// The sorted modifier names a `TriggerSource::Key.mods` carries, in the
/// plan's spelling (`ctrl`, `shift`, `alt`, `command`). Sorted so the same
/// chord is the same list whichever order the OS reports it in.
pub fn mod_names(modifiers: &egui::Modifiers) -> Vec<String> {
    let mut mods = Vec::new();
    if modifiers.ctrl {
        mods.push("ctrl".to_string());
    }
    if modifiers.shift {
        mods.push("shift".to_string());
    }
    if modifiers.alt {
        mods.push("alt".to_string());
    }
    if modifiers.command {
        mods.push("command".to_string());
    }
    mods.sort();
    mods
}

/// Drive the table once for one frame's key events. The shell collects the
/// pressed keys from egui and calls this after `focus` is known; it is a free
/// function so a test can hand it a recording door.
///
/// 1. **Learn first.** With a key learn armed, the first event writes that key
///    as the armed trigger's source over `POST /trigger/<id>{"op":"learn",…}`
///    and nothing else fires this frame — the route clears the arm and
///    persists it. The key that armed the learn never also fires a grid
///    button on the same press.
/// 2. **Grid fire.** Otherwise, in the `Grid` context, a key that matches a
///    live trigger's `TriggerSource::Key` posts `POST /trigger/<id>` with an
///    empty body; a key with no live source falls back to the operator's
///    `data/keybinds.json` override row, posting the same route.
///
/// The caller must not call this while a text field has keyboard focus (the
/// same guard the `Space` shortcut keeps): typing a key into a text box must
/// never fire a trigger. The shell checks `ui.memory(|m| m.focused())` before
/// calling.
pub fn drive(
    binds: &Keybinds,
    context: KbContext,
    events: &[(egui::Key, egui::Modifiers)],
    door: &dyn crate::call::Call,
    learning: Option<&crate::input::LearnArm>,
    triggers: &[KnownTrigger],
) {
    // 1. Learn first: the first key heard is the source, and it does not
    // also fire.
    if let Some(arm) = learning {
        if arm.kind == "key" {
            if let Some((key, modifiers)) = events.first() {
                let body = serde_json::json!({
                    "op": "learn",
                    "source": {
                        "source": "key",
                        "key": key_name(*key),
                        "mods": mod_names(modifiers),
                    }
                });
                let _ = door.json("POST", &format!("/trigger/{}", arm.trigger), Some(body));
                return;
            }
        }
    }

    // 2. Grid fire.
    if context != KbContext::Grid {
        return;
    }
    for (key, modifiers) in events {
        let key = key_name(*key);
        let mods = mod_names(modifiers);
        if let Some(trigger) = triggers
            .iter()
            .find(|t| t.source.matches_key(&key, &mods))
        {
            let _ = door.json("POST", &format!("/trigger/{}", trigger.id), None);
            continue;
        }
        if let Some(id) = binds.grid_trigger(&key, &mods) {
            let _ = door.json("POST", &format!("/trigger/{id}"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::TriggerSource;

    /// The `data/keybinds.json` shape, merged over the defaults: the grid row
    /// is added and readable, and a global row replaces the built-in's action
    /// rather than duplicating it.
    #[test]
    fn an_override_is_merged_over_the_defaults() {
        let overrides = serde_json::json!({
            "grid": [
                { "key": "F1", "mods": [], "trigger": "pedal" }
            ],
            "global": [
                { "key": "F2", "mods": [], "action": "close" }
            ]
        });
        let binds = Keybinds::from_overrides(&overrides);

        assert_eq!(
            binds.grid_trigger("F1", &[]),
            Some("pedal"),
            "the grid override must be merged in"
        );
        assert!(
            binds.binds().iter().any(|b| b.key == "F2"
                && b.context == KbContext::Global
                && b.action == Action::Named("close")),
            "a new global row is appended"
        );
        assert!(
            binds.binds().iter().any(|b| b.key == "Space"
                && b.action == Action::Named("transport")),
            "the defaults survive the merge"
        );
    }

    /// A key that is in `data/keybinds.json` but must still match a live
    /// `TriggerSource::Key` is exactly the point-4 contract: the table and the
    /// source use the same spelling, and `mods` is compared sorted.
    #[test]
    fn a_key_source_matches_the_same_spelling_the_table_carries() {
        let source = TriggerSource::Key {
            key: "F1".to_string(),
            mods: Vec::new(),
        };
        assert!(source.matches_key("F1", &[]));
        assert!(!source.matches_key("F2", &[]));
    }

    /// `get /state` rows are where the shell discovers live key triggers;
    /// a trigger with no `source` (the old shape) is skipped, not fatal.
    #[test]
    fn triggers_are_read_off_the_state_rows() {
        let state = serde_json::json!({
            "outputs": [
                { "id": "probe", "triggers": [
                    { "id": "pedal", "target": "sequence-x",
                      "source": { "source": "key", "key": "F1", "mods": [] } },
                    { "id": "old", "target": "sequence-y" }
                ] }
            ]
        });
        let triggers = triggers_from_state(&state);
        assert_eq!(triggers.len(), 1, "the source-less row is skipped");
        assert_eq!(triggers[0].id, "pedal");
        assert!(triggers[0].source.matches_key("F1", &[]));
    }

    /// `Ctrl+Z` reverts the focused view and nothing else: the nodes canvas
    /// and the sequence editor have an undo; other panels and no focus do not.
    #[test]
    fn ctrl_z_reverts_the_focused_view_only() {
        assert_eq!(undo_target(Some(Region::Nodes)), Some(Region::Nodes));
        assert_eq!(undo_target(Some(Region::Lanes)), Some(Region::Lanes));
        assert_eq!(undo_target(Some(Region::Library)), None);
        assert_eq!(undo_target(None), None);
    }

    #[test]
    fn ctrl_equals_minus_zero_step_the_ui_scale() {
        let press = |key, ctrl| {
            let ctx = egui::Context::default();
            let mut raw = egui::RawInput::default();
            raw.events.push(egui::Event::ModifiersChanged(if ctrl {
                egui::Modifiers::CTRL
            } else {
                egui::Modifiers::NONE
            }));
            raw.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: if ctrl { egui::Modifiers::CTRL } else { egui::Modifiers::NONE },
            });
            let mut out = None;
            let mut full = ctx.run_ui(raw, |ui| out = scale_steps(ui.ctx()));
            full.textures_delta.clear();
            out
        };
        assert_eq!(press(egui::Key::Equals, true), Some(1));
        assert_eq!(press(egui::Key::Minus, true), Some(-1));
        assert_eq!(press(egui::Key::Num0, true), Some(0));
        assert_eq!(press(egui::Key::Minus, false), None);
    }
}
