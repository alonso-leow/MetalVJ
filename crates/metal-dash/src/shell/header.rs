//! The fixed header (`S.U8`, `plan-opus.md` § O.12.e): wordmark · collections
//! · the readouts · `settings` · Edit|Show · `IA`. The clock's freeze is the
//! nodes canvas bar's `pause all` and the spacebar; the header has no `more`
//! menu.
//!
//! Nothing here mutates [`super::Shell`]: [`paint`] takes the door by
//! reference and returns what fired, so the caller (`Shell::ui`) is the one
//! place that decides what a click *does* — matching how the region click in
//! `shell/regions.rs` is also reported outward rather than acted on here.

use egui::{Align, Id, Layout, RichText, Ui};
use serde_json::json;

use crate::call::Call;

use super::classes::{Arrangement, Placement, Theme};
use super::classes::glyph_font;
use super::components::{glyph, wordmark, Icon};
use super::i18n::{tr, trf};
use super::panels::Region;
use super::ShellView;

/// The header's own state between frames — the `collections` menu's inline
/// rename and "save current from" form (`paneles-inferiores.md` § P4,
/// moved here from the library panel's deleted `collections_section`). Held
/// by [`super::Shell`] the same way every other panel's state is.
#[derive(Default)]
pub(super) struct PanelState {
    /// The collection being renamed inline, and the field's buffer.
    rename_target: Option<String>,
    rename_buffer: String,
    /// Which device the save form is copying the current layers of.
    save_device: Option<String>,
    save_name: String,
    /// The last route's refusal, shown under the menu's items.
    note: Option<String>,
}

/// What happened in the header this frame — `S.U13`'s delegation controls
/// alongside the transport toggle `S.U8` already reported this way.
/// `Shell::ui` is the one place any of these are acted on; this file only
/// reports them, matching this module's own opening doc comment ("nothing
/// here mutates `Shell`").
#[derive(Default)]
pub(super) struct HeaderEvents {
    /// The library sidebar's collapse/expand toggle — only paints when
    /// [`Arrangement::library`] is [`Placement::Delegated`].
    pub sidebar_toggled: bool,
    /// The two-tab selector picked a new tab — only paints when
    /// [`Arrangement::tabs`] is [`Placement::Delegated`].
    pub tab_selected: Option<Region>,
    /// Whether the properties long-press button is down *this frame* —
    /// only paints when [`Arrangement::properties`] is
    /// [`Placement::Delegated`]. `Shell::ui` is what turns a run of
    /// consecutive `true`s past `theme.long_press` into
    /// `properties_overlay_open`; this file only reports the raw
    /// `Response::is_pointer_button_down_on` for that one widget, once.
    pub properties_button_down: bool,
    /// The `IA` button was clicked this frame (`S.U14`) — `Shell::ui` is
    /// what flips its own open/closed bool, matching every other toggle
    /// this struct already reports rather than acts on.
    pub harness_toggled: bool,
    /// The `settings ▸` button was clicked this frame (`S.AF`/`O.16`) —
    /// `Shell::ui` opens the settings overlay, exactly like every other
    /// toggle this struct reports rather than acts on.
    pub settings_toggled: bool,
    /// The sync indicator was clicked this frame (`panels::pairing`):
    /// `Shell::ui` opens or closes the pairing overlay.
    pub pairing_toggled: bool,
    /// A listen dot was clicked: `(graph, node)` of the `audio.live` to
    /// select, where its source is edited (plan "Audio input is a node",
    /// Design 7). `Shell::ui` opens the graph and selects the node.
    pub select_node: Option<(String, String)>,
    /// The Edit|Show toggle picked a view other than the current one. The
    /// caller persists it and, entering Show, asks for the monitors.
    pub view_selected: Option<ShellView>,
}

/// Paint the header bar's contents (the caller wraps this in the
/// `Panel::top` that gives it its fixed height and frame) and report every
/// control that fired this frame.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub(super) fn paint(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    arrangement: Arrangement,
    active_tab: Region,
    sidebar_open: bool,
    harness_open: bool,
    view: ShellView,
    pairing: &super::panels::pairing::PanelState,
) -> HeaderEvents {
    let _span = crate::perf::Span::new("header");
    let mut events = HeaderEvents::default();
    ui.horizontal_centered(|ui| {
        if arrangement.is_full_grid() {
            wordmark(ui, theme);
            collections_menu(ui, theme, door, state);
            loop_readout(ui, theme, door);
            listen_dots(ui, theme, door, &mut events);
            pairing_chip(ui, theme, pairing, &mut events);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Right-to-left: the toggle is added after `IA`, so it sits
                // just left of it (and in Show, where `IA` is gone, at the
                // edge).
                if view == ShellView::Edit {
                    ia_button(ui, theme, harness_open, &mut events);
                }
                view_toggle(ui, theme, view, &mut events);
                settings_button(ui, theme, &mut events);
                // The frame-rate readout lives here, left of the panel toggles,
                // so it is on screen without covering the AI chat sidebar.
                super::perf_hud::badge(ui, theme);
            });
        } else {
            // Delegated (`S.U13b`, phone/phone-small): the whole row becomes
            // one hidden-scrollbar horizontal strip, so the controls that
            // used to run past the right edge at 390px (`IA`, `hold:
            // properties`) stay reachable by scrolling instead of being cut
            // off — before this batch the row was a single left-to-right flow
            // and the right cluster ended past the viewport. `kittest` never
            // saw it because `click_accesskit` acts on the node and ignores
            // its coordinates.
            egui::ScrollArea::horizontal()
                .id_salt("shell/header/row")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        wordmark(ui, theme);
                        // The tab picker sits first, next to the wordmark, so
                        // it is on screen without scrolling the strip.
                        if arrangement.tabs == Placement::Delegated {
                            tab_selector(ui, theme, active_tab, &mut events);
                        }
                        collections_menu(ui, theme, door, state);
                        loop_readout(ui, theme, door);
                        listen_dots(ui, theme, door, &mut events);
                        // Right after the readouts, where the full grid puts it, so
                        // it is on screen at phone width.
                        pairing_chip(ui, theme, pairing, &mut events);
                        // Show paints none of the buttons that open a panel.
                        if view == ShellView::Edit {
                            if arrangement.library == Placement::Delegated {
                                sidebar_toggle(ui, theme, sidebar_open, &mut events);
                            }
                            if arrangement.properties == Placement::Delegated {
                                properties_long_press_button(ui, theme, &mut events);
                            }
                        }
                        // Before `IA`, so the toggle sits just left of it.
                        view_toggle(ui, theme, view, &mut events);
                        if view == ShellView::Edit {
                            ia_button(ui, theme, harness_open, &mut events);
                        }
                        settings_button(ui, theme, &mut events);
                        super::perf_hud::badge(ui, theme);
                    });
                });
        }
    });

    // The rename field, in its own popup: a menu closes on any click, so an
    // inline field inside it would never survive to be typed into
    // (`paneles-inferiores.md` § P4 allows a popup).
    if let Some(id) = state.rename_target.clone() {
        let mut close = false;
        egui::Area::new(Id::new("shell/header/collections/rename"))
            .order(egui::Order::Foreground)
            .anchor(
                egui::Align2::LEFT_TOP,
                egui::vec2(theme.region_min.x, theme.header_height),
            )
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.rename_buffer)
                                .desired_width(theme.text_input_width),
                        );
                        if ui
                            .button(RichText::new("save name").font(theme.font_small.clone()))
                            .clicked()
                        {
                            let next = state.rename_buffer.clone();
                            match door.json(
                                "POST",
                                "/collections",
                                Some(json!({ "op": "rename", "id": id, "name": next })),
                            ) {
                                Ok(_) => state.note = None,
                                Err((_, msg)) => state.note = Some(msg),
                            }
                            close = true;
                        }
                        if ui
                            .button(RichText::new("cancel").font(theme.font_small.clone()))
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
            });
        if close {
            state.rename_target = None;
        }
    }
    events
}

/// The loop readout (Phase 4B): `L=<length> ●/○ <mode>` — the engaged loop's
/// length and a filled marker while it is on air, read fresh off the `GET
/// /state` the header already polls through the door (its `repeat` block,
/// `call::transmit::state`; regla 5 — the readout recomputes nothing). The
/// Looper has no home control surface yet (no beat-loop grid exists in this
/// worktree — see the Phase 4B report), so this is the one always-on sign of
/// what the machine is looping. Painted `font_small` in `ink_weak`, with the
/// lit marker in `phosphor_lit`, so a glance across a booth reads on/off
/// before the length.
fn loop_readout(ui: &mut Ui, theme: &Theme, door: &dyn Call) {
    let reply = door.json("GET", "/state", None);
    let repeat = reply.as_ref().ok().map(|s| &s["repeat"]);
    let engaged = repeat
        .and_then(|r| r["engaged"].as_bool())
        .unwrap_or(false);
    let length = repeat
        .and_then(|r| r["length_beats"].as_u64())
        .unwrap_or(0) as u32;
    let mode = repeat
        .and_then(|r| r["mode"].as_str())
        .unwrap_or("off");
    let text = loop_readout_text(engaged, length, mode);
    let color = if engaged {
        theme.phosphor_lit
    } else {
        theme.ink_weak
    };
    ui.label(RichText::new(text).font(theme.font_small.clone()).color(color));
}

/// The listen indicator (plan "Audio input is a node", Design 7): one dot
/// per live `audio.live` with its source's short name, read fresh off `GET
/// /listen/status` (the pool's own status, regla 4/5). A stand-in
/// (`real: false`) or a source that will not open is red and says
/// `stand-in`, so an operator cannot take a simulated input to a venue
/// without seeing it; clicking a dot selects that node.
/// The sync indicator (`panels::pairing`): `sync off` / `unpaired` /
/// `paired · N`, clicked to open the pairing overlay. Painted on both
/// viewports, in the same place in each row.
fn pairing_chip(
    ui: &mut Ui,
    theme: &Theme,
    pairing: &super::panels::pairing::PanelState,
    events: &mut HeaderEvents,
) {
    if super::panels::pairing::chip(ui, theme, pairing) {
        events.pairing_toggled = true;
    }
}

fn listen_dots(ui: &mut Ui, theme: &Theme, door: &dyn Call, events: &mut HeaderEvents) {
    let Ok(status) = door.json("GET", "/listen/status", None) else { return };
    for n in status["nodes"].as_array().into_iter().flatten() {
        let (Some(graph), Some(node)) = (n["graph"].as_str(), n["node"].as_str()) else { continue };
        let real = n["real"].as_bool().unwrap_or(false) && n["error"].is_null();
        let text = listen_dot_text(n["short"].as_str().unwrap_or("?"), real);
        let color = if real { theme.phosphor_lit } else { theme.warn };
        let clicked = ui
            .add(egui::Label::new(RichText::new(text).font(theme.font_small.clone()).color(color)).sense(egui::Sense::click()))
            .on_hover_text(n["describe"].as_str().unwrap_or_default())
            .clicked();
        if clicked {
            events.select_node = Some((graph.to_string(), node.to_string()));
        }
    }
}

/// One dot's text, pure for the same reason [`loop_readout_text`] is:
/// `● <short>` for a real source, `● <short> stand-in` otherwise.
fn listen_dot_text(short: &str, real: bool) -> String {
    if real {
        trf("● {short}", &[("short", &short)])
    } else {
        trf("● {short} stand-in", &[("short", &short)])
    }
}

/// The readout's text, pure so it is testable without a door or a frame:
/// `L=4 ● auto` engaged, `L=0 ○ arm` idle. The `{n}`/`{glyph}`/`{mode}`
/// placeholders go through [`trf`] so the Spanish entry may move them, and the
/// `●`/`○` marker is a symbol rather than language (the engaged fact, not a
/// translated word).
fn loop_readout_text(engaged: bool, length_beats: u32, mode: &str) -> String {
    let glyph = if engaged { "●" } else { "○" };
    trf("L={n} {glyph} {mode}", &[
        ("n", &length_beats),
        ("glyph", &glyph),
        ("mode", &mode),
    ])
}

/// `collections`: what the deleted library `collections_section` did, moved
/// here (`paneles-inferiores.md` § P4). Per collection: `apply to ▸ <device>`
/// (`POST /layers {op:"apply"}`, the switch `apply_layers_op`'s own arm
/// performs), `rename…` (an inline field) and `remove`
/// (`POST /collections {op:"remove"}`). At the foot, `save current from ▸
/// <device>` saves the device's current pool as a new collection
/// (`POST /collections {op:"save"}`). The list is the same `GET /state` read
/// the old panel made, so a collection's name is still never derived twice.
fn collections_menu(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    ui.push_id(Id::new("shell/header/collections"), |ui| {
        ui.menu_button("collections", |ui| {
            let (collections, devices) = match door.json("GET", "/state", None) {
                Ok(state_val) => (
                    state_val["collections"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                    state_val["devices"].as_array().cloned().unwrap_or_default(),
                ),
                Err(_) => (Vec::new(), Vec::new()),
            };

            if collections.is_empty() {
                ui.label(
                    RichText::new("— no collections —")
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                );
            }
            for c in &collections {
                let id = c["id"].as_str().unwrap_or_default().to_string();
                let name = c["name"].as_str().unwrap_or_default().to_string();

                // The submenu's own `⏵` is added by `ui.menu_button`; the
                // label is just the name, so it reads and matches as itself.
                ui.menu_button(
                    RichText::new(name.clone()).font(theme.font_body.clone()),
                    |ui| {
                        ui.menu_button(
                            RichText::new("apply to").font(theme.font_small.clone()),
                            |ui| {
                                for d in &devices {
                                    let Some(did) = d["id"].as_str() else {
                                        continue;
                                    };
                                    let label = d["label"].as_str().unwrap_or(did);
                                    if ui
                                        .button(RichText::new(label).font(theme.font_small.clone()))
                                        .clicked()
                                    {
                                        match door.json(
                                            "POST",
                                            "/layers",
                                            Some(json!({
                                                "op": "apply",
                                                "device": did,
                                                "collection": id,
                                            })),
                                        ) {
                                            Ok(_) => state.note = None,
                                            Err((_, msg)) => state.note = Some(msg),
                                        }
                                        ui.close();
                                    }
                                }
                            },
                        );
                        if ui
                            .button(RichText::new("rename…").font(theme.font_small.clone()))
                            .clicked()
                        {
                            // Deliberately no `ui.close()`: the inline field
                            // replaces this row next frame, and closing the
                            // menu would hide it behind a reopen.
                            state.rename_target = Some(id.clone());
                            state.rename_buffer = name.clone();
                        }
                        if ui
                            .button(RichText::new("remove").font(theme.font_small.clone()))
                            .clicked()
                        {
                            match door.json(
                                "POST",
                                "/collections",
                                Some(json!({ "op": "remove", "id": id })),
                            ) {
                                Ok(_) => state.note = None,
                                Err((_, msg)) => state.note = Some(msg),
                            }
                            ui.close();
                        }
                    },
                );
            }

            ui.separator();
            ui.menu_button(
                RichText::new("save current from").font(theme.font_small.clone()),
                |ui| {
                    for d in &devices {
                        let Some(did) = d["id"].as_str() else {
                            continue;
                        };
                        let label = d["label"].as_str().unwrap_or(did).to_string();
                        if ui
                            .selectable_label(state.save_device.as_deref() == Some(did), &label)
                            .clicked()
                        {
                            state.save_device = Some(did.to_string());
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.save_name)
                                .hint_text("name")
                                .desired_width(theme.text_input_width),
                        );
                        if ui
                            .button(RichText::new("save").font(theme.font_small.clone()))
                            .clicked()
                        {
                            let device = state.save_device.clone().unwrap_or_default();
                            let name = if state.save_name.trim().is_empty() {
                                "collection".to_string()
                            } else {
                                state.save_name.clone()
                            };
                            match door.json(
                                "POST",
                                "/collections",
                                Some(json!({ "op": "save", "device": device, "name": name })),
                            ) {
                                Ok(_) => {
                                    state.note = None;
                                    state.save_name.clear();
                                }
                                Err((_, msg)) => state.note = Some(msg),
                            }
                        }
                    });
                },
            );

            if let Some(note) = state.note.clone() {
                ui.label(
                    RichText::new(note)
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                );
            }
        });
    });
}

/// `settings`: the shell-wide settings overlay's opener (`S.AF`, `O.16`'s
/// first slice), on every viewport. The overlay itself reports open state,
/// not this button, so `harness.get_by_label` never meets two widgets named
/// "settings".
fn settings_button(ui: &mut Ui, theme: &Theme, events: &mut HeaderEvents) {
    ui.push_id(Id::new("shell/header/settings"), |ui| {
        if ui
            .button(RichText::new("settings").font(theme.font_body.clone()))
            .clicked()
        {
            events.settings_toggled = true;
        }
    });
}

/// A header button's label: a Lucide icon, then `text`. The icon is set in
/// the Proportional family, where the Lucide fallback is installed; the body
/// face (`font_body`) is Monospace in some themes, which has no Lucide
/// fallback and painted every icon as an empty square.
fn icon_label(icon: Icon, text: &str, theme: &Theme) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(&glyph(icon).to_string(), 0.0, egui::TextFormat::simple(glyph_font(theme.font_body.size), theme.ink));
    job.append(&format!(" {text}"), 0.0, egui::TextFormat::simple(theme.font_body.clone(), theme.ink));
    job
}

/// The same as [`icon_label`], with the icon after the text (`view: nodes ▾`).
fn icon_label_after(text: &str, icon: Icon, theme: &Theme) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat::simple(theme.font_body.clone(), theme.ink));
    job.append(&format!(" {}", glyph(icon)), 0.0, egui::TextFormat::simple(glyph_font(theme.font_body.size), theme.ink));
    job
}

/// The `IA` button's accessible label: the sparkles icon, then `IA`. Tests
/// find the button by it.
#[cfg(test)]
pub(crate) fn ia_label() -> String {
    format!("{} IA", glyph(Icon::Spark))
}

/// The delegated tab picker's accessible label for the active `title`: the
/// name, then the chevron the button ends with. Tests find the button by it.
#[cfg(test)]
pub(crate) fn view_label(title: &str) -> String {
    format!("view: {} {}", title, glyph(Icon::ChevronDown))
}

/// The library toggle's accessible label: the chevron that says which way
/// it moves (`open` collapses it, so it points left).
#[cfg(test)]
pub(crate) fn library_label(open: bool) -> String {
    let icon = if open { Icon::ChevronLeft } else { Icon::ChevronRight };
    format!("{} library", glyph(icon))
}

/// `IA`: the harness sidebar's own toggle (`S.U14`, `plan-opus.md` §
/// O.12.f). A sparkles icon then `IA`, the same icon-then-name shape
/// [`sidebar_toggle`] uses for the library, rather than the bare
/// `harness de ia` the panel's own heading paints (`panels::chat::TITLE`),
/// so `harness.get_by_label` stays unambiguous the moment the sidebar is
/// open, exactly the collision [`tab_selector`]'s own doc comment already
/// names for the same reason.
fn ia_button(ui: &mut Ui, theme: &Theme, _harness_open: bool, events: &mut HeaderEvents) {
    ui.push_id(Id::new("shell/header/ia"), |ui| {
        if ui.button(icon_label(Icon::Spark, "IA", theme)).clicked() {
            events.harness_toggled = true;
        }
    });
}

/// Library's collapse/expand toggle (`S.U13`, `O.12.f`): only shown once
/// [`Arrangement::library`] has delegated it out of the grid into a
/// [`super::components::side_panel`] — `regions.rs` is what actually reads
/// `sidebar_open` to decide whether that sidebar paints its content or just
/// its collapsed rail.
fn sidebar_toggle(ui: &mut Ui, theme: &Theme, sidebar_open: bool, events: &mut HeaderEvents) {
    ui.push_id(Id::new("shell/header/sidebar-toggle"), |ui| {
        let icon = if sidebar_open { Icon::ChevronLeft } else { Icon::ChevronRight };
        if ui.button(icon_label(icon, "library", theme)).clicked() {
            events.sidebar_toggled = true;
        }
    });
}

/// The Edit/Show mode toggle: one button, aware of the current view, whose
/// label names the view it switches to (`Show` while editing, `Edit` while
/// showing).
fn view_toggle(ui: &mut Ui, theme: &Theme, view: ShellView, events: &mut HeaderEvents) {
    ui.push_id(Id::new("shell/header/view"), |ui| {
        let (target, label) = match view {
            ShellView::Edit => (ShellView::Show, tr("Show")),
            ShellView::Show => (ShellView::Edit, tr("Edit")),
        };
        if ui.button(RichText::new(label).font(theme.font_body.clone())).clicked() {
            events.view_selected = Some(target);
        }
    });
}

/// The grafo/lanes tab picker on a delegated (phone) header, as one compact
/// menu button: its label names the active tab (`view: nodes ▾`) and the
/// popup lists both tabs. Two plain buttons ran past the right edge of a
/// 390px phone and, inside the header's scroll strip, were unreachable by
/// touch; one button always fits, and the popup is anchored by egui inside
/// the viewport. Picking a tab reports it through [`HeaderEvents`].
fn tab_selector(ui: &mut Ui, theme: &Theme, active_tab: Region, events: &mut HeaderEvents) {
    ui.push_id(Id::new("shell/header/tabs"), |ui| {
        let label = icon_label_after(&format!("view: {}", active_tab.title()), Icon::ChevronDown, theme);
        ui.menu_button(label, |ui| {
            for tab in [Region::Nodes, Region::Lanes] {
                let item = format!("view: {}", tab.title());
                if ui
                    .selectable_label(
                        active_tab == tab,
                        RichText::new(item).font(theme.font_body.clone()),
                    )
                    .clicked()
                {
                    if active_tab != tab {
                        events.tab_selected = Some(tab);
                    }
                    ui.close();
                }
            }
        });
    });
}

/// Properties' long-press button (`S.U13`, `O.12.f`): only shown once
/// [`Arrangement::properties`] has delegated it into the long-press
/// overlay. This function does not time the press — it only reports
/// whether the button is down *this* frame
/// ([`egui::Response::is_pointer_button_down_on`], the same primitive a
/// real long-press-to-open gesture needs anywhere else it shows up later);
/// `Shell::ui` is what turns a run of `true`s past `theme.long_press`
/// (`ui.input(|i| i.time)`) into `properties_overlay_open`.
fn properties_long_press_button(ui: &mut Ui, theme: &Theme, events: &mut HeaderEvents) {
    ui.push_id(Id::new("shell/header/properties-long-press"), |ui| {
        // Labelled "hold: properties", not the bare "properties" —
        // when the overlay is open its own content paints a heading with
        // that exact word (`properties::PanelState::ui`'s `section_heading`),
        // and an identically-labelled header button would make
        // `harness.get_by_label("propiedades")` ambiguous the moment it is.
        let response =
            ui.button(RichText::new("hold: properties").font(theme.font_body.clone()));
        events.properties_button_down = response.is_pointer_button_down_on();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plan Design 9's indicator kittest: a `FakeDoor` whose `/listen/status`
    /// answers `real: false` paints the dot red with `stand-in`, and clicking
    /// it opens the node's graph and selects the node.
    #[test]
    fn a_stand_in_source_is_a_dot_that_selects_its_node() {
        use std::rc::Rc;
        use egui_kittest::{kittest::Queryable, Harness};
        let _chdir = crate::chdir_lock();
        let door = Rc::new(crate::shell::FakeDoor::new());
        door.respond(
            "GET",
            "/listen/status",
            serde_json::json!({ "nodes": [{
                "graph": "device-A", "node": "audio-3", "short": "mic", "real": false,
                "describe": "simulated · 124 BPM", "error": null,
            }] }),
        );
        let theme = Theme::phosphor();
        let size = egui::Vec2::new(
            crate::shell::DESKTOP_VIEWPORT.0 as f32,
            crate::shell::DESKTOP_VIEWPORT.1 as f32,
        );
        let mut h = Harness::builder().with_size(size).build_ui_state(
            |ui, shell: &mut crate::shell::Shell| shell.ui(ui),
            crate::shell::Shell::new(theme.clone(), Box::new(Rc::clone(&door))),
        );
        theme.apply(&h.ctx);
        h.run();
        h.get_by_label("● mic stand-in").click();
        h.run();
        assert_eq!(
            h.state().selection(),
            &crate::shell::panels::Selection::Node("audio-3".to_string())
        );
        let opened = door
            .calls
            .borrow()
            .iter()
            .any(|(m, p, q)| m == "GET" && p == "/graph" && q.query.get("id").map(String::as_str) == Some("device-A"));
        assert!(opened, "clicking the dot opens the node's graph");
    }

    /// The sync chip reads the club off `GET /sync/*` and a click opens the
    /// pairing overlay, the same way the listen dot selects its node.
    #[test]
    fn the_sync_chip_reads_the_club_and_opens_the_pairing_overlay() {
        use std::rc::Rc;
        use egui_kittest::{kittest::Queryable, Harness};
        let _chdir = crate::chdir_lock();
        let door = Rc::new(crate::shell::FakeDoor::new());
        door.respond("GET", "/sync/status", serde_json::json!({ "running": true, "name": "sala" }));
        door.respond(
            "GET",
            "/sync/peers",
            serde_json::json!({ "running": true, "peers": [
                { "name": "cocina", "node_id": "abcdef0123", "online": true },
            ] }),
        );
        let theme = Theme::phosphor();
        let size = egui::Vec2::new(
            crate::shell::DESKTOP_VIEWPORT.0 as f32,
            crate::shell::DESKTOP_VIEWPORT.1 as f32,
        );
        let mut h = Harness::builder().with_size(size).build_ui_state(
            |ui, shell: &mut crate::shell::Shell| shell.ui(ui),
            crate::shell::Shell::new(theme.clone(), Box::new(Rc::clone(&door))),
        );
        theme.apply(&h.ctx);
        h.run();
        h.get_by_label("paired · 1").click();
        h.run();
        assert!(h.state().pairing_open(), "clicking the sync chip opens the overlay");
    }

    #[test]
    fn a_stand_in_dot_says_so_and_a_real_one_does_not() {
        assert!(listen_dot_text("stand-in", false).ends_with("stand-in"));
        assert!(listen_dot_text("mic", false).contains("stand-in"));
        assert!(!listen_dot_text("mic", true).contains("stand-in"));
    }

    /// The readout reads `L=<n> <marker> <mode>`, and the marker is the
    /// engaged *fact*: filled while the loop is on, hollow when idle. The
    /// test pins the exact string the Phase 4B brief asks for (`L=4 ●`) plus
    /// the mode, since the marker is the one sign a booth can read at a
    /// glance.
    #[test]
    fn the_loop_readout_reads_engaged_as_filled() {
        assert_eq!(loop_readout_text(true, 4, "auto"), "L=4 ● auto");
        assert_eq!(loop_readout_text(false, 0, "arm"), "L=0 ○ arm");
    }

    /// The placeholders survive `trf`: the length and mode are filled from
    /// the `/state` reply, not baked into the template.
    #[test]
    fn the_loop_readout_fills_its_placeholders() {
        assert!(loop_readout_text(true, 8, "off").contains("L=8"));
        assert!(loop_readout_text(true, 8, "off").contains("off"));
    }

    /// The header buttons' accessible labels name their Lucide icon first (the
    /// icon the button paints), so a test or an assistive reader finds the
    /// same button the painter drew.
    #[test]
    fn header_button_labels_lead_with_their_icon() {
        assert_eq!(ia_label(), format!("{} IA", glyph(Icon::Spark)));
        assert_eq!(library_label(true), format!("{} library", glyph(Icon::ChevronLeft)));
        assert_eq!(library_label(false), format!("{} library", glyph(Icon::ChevronRight)));
        assert_eq!(view_label("nodes"), format!("view: nodes {}", glyph(Icon::ChevronDown)));
    }
}
