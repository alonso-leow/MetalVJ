//! The settings overlay (`O.16`'s first slice, batch `S.AF`): a foreground
//! window with a sidebar of sections, opened by the header's settings
//! button — one place a shell-wide preference lives. **UI** holds the
//! `ShellMode` dropdown that manually overrides `classes::arrangement`'s
//! viewport decision; **language** switches the whole shell between the
//! languages `shell::i18n` knows; **theme** is still an honest placeholder
//! naming the batch that fills it, the same way `more ▾`'s disabled provider
//! item pointed at `S.U14` before `S.U14` landed.
//!
//! **Not a region.** Like the harness (`S.U14`'s same call), settings is
//! never one of `Region::ALL` and never competes for `focus`/`cede` space —
//! it is an `Area` at foreground order, painted by `Shell::ui` the way the
//! properties long-press overlay and the harness overlay are, and closed
//! the same two ways (Esc, click outside).
//!
//! **Rule 6, the whole mechanism.** The dropdowns' only read is `GET
//! /settings`, their only write is `POST /settings` — this file never
//! touches the settings file itself, and the override the arrangement reads
//! (and the language `i18n` paints in) comes off the reply the door just
//! answered, not off a shell-local value a resize could forget.

use egui::{Id, Response, RichText, TextEdit, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::{ShellMode, Theme};
use crate::shell::components::{nav_list, panel_frame, section_heading, Icon};
use crate::shell::i18n::{self, tr, tr_dyn, tr_key, trf, Lang};
use crate::shell::panels::pairing;

/// Stable id salt for the overlay's widgets.
pub const ID: &str = "shell/panel/ajustes";

/// `POST /agent/tool` — the route the in-process tools (the camera's
/// `discover_fixture`) run through. Named here as the string a panel names
/// any other route; the call module's own const stays out of the shell
/// (gate 1: a panel imports only the call door surface).
const AGENT_TOOL_PATH: &str = "/agent/tool";

/// The sections, in sidebar order (English source text; painted through
/// [`tr`]). `theme` is still the placeholder `O.16`'s tokens/theme settings
/// fill. A fourth section means one entry here and one match in `paint`'s
/// section dispatch, nothing else (a list this short spelled once it is,
/// not counted twice).
const SECTIONS: [(&str, Section); 9] = [
    (tr_key("ui"), Section::Ui),
    (tr_key("theme"), Section::Theme),
    (tr_key("language"), Section::Language),
    (tr_key("ai"), Section::Ai),
    (tr_key("fixtures"), Section::Fixtures),
    (tr_key("discover"), Section::Discover),
    (tr_key("P2P"), Section::Devices),
    (tr_key("export"), Section::Export),
    (tr_key("performance"), Section::Performance),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Ui,
    Theme,
    Language,
    Ai,
    /// D1: the active venue's fixtures — the single place to list and map
    /// devices, read/written through `POST /venue { op: "fixture" }`.
    Fixtures,
    /// F13: discovery — Art-Net ArtPoll, sACN Universe Discovery and the
    /// camera flash. Every control posts through the same routes the rest of
    /// the shell uses; the section only *proposes* fixture rows and the
    /// operator accepts them.
    Discover,
    /// The paired machines and this one's sync state (`panels::pairing`).
    Devices,
    /// P-G: where "Export frame" writes (`export_dir`).
    Export,
    /// The global frame-rate cap (`fps_cap`).
    Performance,
}

/// The provider presets the `IA` section offers, exactly the wire strings
/// `metal_agent::config::Preset` parses (`claude`/`openai`/`deepinfra`/`manual`)
/// — read from the door's reply, never imported (gate 1).
const PROVIDER_PRESETS: [&str; 4] = ["claude", "openai", "deepinfra", "manual"];

/// The three capability permissions, `(key, label)`: the key is what
/// `POST /ai-config {"permissions": {key: ...}}` names (`metal_agent::harness::
/// Capability::key`), the label is painted through [`tr`] — `tr_key` marks the
/// literal so the locale test sees it, the pair so a translation never
/// reaches the wire.
const CAPABILITY_PERMISSIONS: [(&str, &str); 3] = [
    ("internet", tr_key("internet")),
    ("dispositivos", tr_key("devices")),
    ("archivos", tr_key("files")),
];

/// The three answers a capability takes, `(wire, label)`: `allow` runs
/// without asking, `ask` pauses for the operator, `deny` never offers the tool.
const PERMISSION_CHOICES: [(&str, &str); 3] = [
    ("allow", tr_key("always")),
    ("ask", tr_key("ask")),
    ("deny", tr_key("deny")),
];

/// The body that answers one capability. Pure, so the wire shape the sidebar
/// posts is pinned by a test instead of only by a click.
fn permission_body(key: &str, wire: &str) -> serde_json::Value {
    json!({ "permissions": { key: wire } })
}

/// The body that saves the operator's free-text instructions.
fn instructions_body(text: &str) -> serde_json::Value {
    json!({ "instructions": text })
}

/// Everything settings remembers: which sidebar section is showing, the
/// cached `shell_mode` — the value the arrangement (every frame, open or
/// closed) needs from the settings file — and the language last answered.
pub struct PanelState {
    section: Section,
    shell_mode: ShellMode,
    language: Lang,
    /// The one machine switch: the tick level the listener, the lanes and
    /// every consumer speak. Base until the door has answered.
    tick_level: metal_schema::TickRate,
    /// The header's Edit|Show view, as `GET /settings` last answered it (or
    /// as this run last posted it) — `Shell::ui` reads it once, on the first
    /// frame, to decide whether the app opens in Show.
    shell_view: crate::shell::ShellView,
    /// One `GET /settings` per run of the app (`Shell::ui`'s first frame);
    /// every later change comes back from this section's own `POST`, so
    /// `fetched` means "the cache is the disk's truth", not "I read it
    /// recently" — there is no refresh loop, and nothing else clears it.
    fetched: bool,
    /// The `IA` section's editable provider scratch: the preset picked in the
    /// combo plus the `url`/`model`/`key` still being typed. Seeded once from
    /// the first `GET /ai-config` reply (`fetched_provider`), then written by
    /// `save` and overwritten from the reply it gets back.
    provider_preset: String,
    provider_url: String,
    provider_model: String,
    provider_key: String,
    /// The `IA` section's "AI instructions" box: seeded once from the
    /// first `GET /ai-config`, then saved on focus loss or `save`. Kept out
    /// of `adopt_provider` so a provider save never overwrites a half-typed
    /// instruction.
    provider_instructions: String,
    /// One `GET /ai-config` seed per run, the same trade `fetched` makes for
    /// `/settings`: the fields above start from disk once and never swim back
    /// under the operator while they type.
    fetched_provider: bool,
    /// P-G: the `export_dir` setting as typed — empty is the default folder.
    export_dir: String,
    /// The global fps cap as last answered; 60 until the door has.
    fps_cap: u32,
    /// The global UI scale percentage as last answered; 100 until the door has.
    ui_scale: u32,
    /// The `tone_mode` setting as last answered (`off`/`sample`/`live`).
    tone_mode: String,
    /// The `ingest_proxy` setting as last answered — whether a dropped clip
    /// builds its HAP proxy automatically; on until the door has.
    ingest_proxy: bool,
    /// The `theme` setting: the stored palette recolouring the client ("" is
    /// the house look), its colours, and whether the shell still has to
    /// rebuild its theme from them (`take_theme_change`).
    theme_name: String,
    theme_colors: Vec<[f32; 3]>,
    theme_dirty: bool,
    /// The stored palettes the Theme dropdown lists, read once per open.
    theme_palettes: Vec<(String, Vec<[f32; 3]>)>,
    /// The house look the settings hold (`call::settings::THEME_STYLES`), and
    /// the one a change set that the shell has not read yet
    /// ([`PanelState::take_style_change`]).
    theme_style: String,
    style_change: Option<String>,
    /// D1: the Fixtures section's edit scratch. `fixtures_edit` is the id
    /// being edited (`None` means "add"); the rest are the form's typed
    /// values, seeded from a row's `edit` and cleared on `cancel` or a
    /// save. Numbers stay text until the post, where an unparsable one is
    /// sent as `null` (cleared) rather than guessed.
    fixtures_edit: Option<String>,
    fixtures_kind: String,
    fixtures_name: String,
    fixtures_profile: String,
    fixtures_group: String,
    fixtures_wire: String,
    fixtures_universe: String,
    fixtures_address: String,
    /// The last Fixtures note — a save confirmation or the route's refusal.
    fixtures_note: Option<String>,
    /// F8: a node the Fixtures section asked the canvas to bring into view —
    /// the shell drains it once per frame, the same cross-panel reveal the
    /// properties panel already makes.
    pending_reveal: Option<String>,
    /// F13: the last Art-Net ArtPoll reply (`GET /artnet/nodes`), the last
    /// sACN Universe Discovery listen (`GET /sacn/nodes`) and the section's
    /// own note — a save confirmation, the camera stub's pending note, or a
    /// route's refusal. Discovery results are cached here, not re-listened
    /// every frame.
    discover_artnet: Option<Value>,
    discover_sacn: Option<Value>,
    discover_note: Option<String>,
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState {
            section: Section::Ui,
            shell_mode: ShellMode::Auto,
            language: Lang::En,
            tick_level: metal_schema::TickRate::Base,
            shell_view: crate::shell::ShellView::Edit,
            fetched: false,
            provider_preset: "manual".to_string(),
            provider_url: String::new(),
            provider_model: String::new(),
            provider_key: String::new(),
            provider_instructions: String::new(),
            fetched_provider: false,
            export_dir: String::new(),
            fps_cap: crate::frame_cap::DEFAULT_FPS_CAP,
            ui_scale: crate::ui_scale::DEFAULT,
            tone_mode: "sample".to_string(),
            ingest_proxy: true,
            theme_name: String::new(),
            theme_colors: Vec::new(),
            theme_dirty: false,
            theme_palettes: Vec::new(),
            theme_style: String::new(),
            style_change: None,
            fixtures_edit: None,
            fixtures_kind: "wash".to_string(),
            fixtures_name: String::new(),
            fixtures_profile: String::new(),
            fixtures_group: String::new(),
            fixtures_wire: "dmx".to_string(),
            fixtures_universe: String::new(),
            fixtures_address: String::new(),
            fixtures_note: None,
            pending_reveal: None,
            discover_artnet: None,
            discover_sacn: None,
            discover_note: None,
        }
    }
}

impl PanelState {
    /// The arrangement override the shipped app runs under — `Auto` until
    /// the door has answered, then whatever `data/settings.json` said.
    pub(crate) fn mode_override(&self) -> ShellMode {
        self.shell_mode
    }

    /// The view the cache holds — `Edit` until the door has answered.
    pub(crate) fn shell_view(&self) -> crate::shell::ShellView {
        self.shell_view
    }

    /// Persists the header's Edit|Show choice (`POST /settings`) and keeps
    /// the cache in step with what the route stored, so the next frame's
    /// read agrees with the disk without a re-fetch. A refused write leaves
    /// the cache where it was.
    pub(crate) fn set_shell_view(&mut self, door: &dyn Call, view: crate::shell::ShellView) {
        if door
            .json(
                "POST",
                "/settings",
                Some(json!({ "shell_view": view.code() })),
            )
            .is_ok()
        {
            self.shell_view = view;
        }
    }

    /// One `GET /settings`, on the first frame that has a door to ask.
    /// A reply whose `shell_mode` is absent or unknown leaves the default
    /// — `call::settings` already refuses unknown values at the write, so
    /// an unknown value here means the file was hand-edited past the
    /// layer's own contract and the honest response is the default.
    pub(crate) fn refresh(&mut self, door: &dyn Call) {
        if self.fetched {
            return;
        }
        self.fetched = true;
        if let Ok(reply) = door.json("GET", "/settings", None) {
            if let Some(mode) = reply["shell_mode"].as_str().and_then(ShellMode::parse) {
                self.shell_mode = mode;
            }
            if let Some(tick) = reply["tick_level"]
                .as_str()
                .and_then(metal_schema::TickRate::parse)
            {
                self.tick_level = tick;
            }
            if let Some(view) = reply["shell_view"]
                .as_str()
                .and_then(crate::shell::ShellView::parse)
            {
                self.shell_view = view;
            }
            if let Some(n) = reply["ui_scale"].as_u64() {
                self.ui_scale = n as u32;
                crate::ui_scale::set(n as u32);
            }
            if let Some(n) = reply["fps_cap"].as_u64() {
                self.fps_cap = n as u32;
                crate::frame_cap::set(n as u32);
            }
            if let Some(m) = reply["tone_mode"].as_str() {
                self.tone_mode = m.to_string();
            }
            if let Some(dir) = reply["export_dir"].as_str() {
                self.export_dir = dir.to_string();
            }
            if let Some(b) = reply["ingest_proxy"].as_bool() {
                self.ingest_proxy = b;
            }
            if let Some(name) = reply["theme"].as_str().filter(|n| !n.is_empty()) {
                self.theme_palettes = read_palettes(door);
                self.pick_theme(name, None);
            }
            if let Some(style) = reply["theme_style"].as_str() {
                self.set_style(style);
            }
            self.apply_language(&reply);
        }
    }

    /// Select the theme palette `name` ("" is the house look): its colours are
    /// the ones given or the stored palette of that name; the shell rebuilds
    /// its theme on the next frame.
    fn pick_theme(&mut self, name: &str, colors: Option<Vec<[f32; 3]>>) {
        self.theme_colors = colors
            .or_else(|| self.theme_palettes.iter().find(|(n, _)| n == name).map(|(_, c)| c.clone()))
            .unwrap_or_default();
        self.theme_name = name.to_string();
        self.theme_dirty = true;
    }

    /// `Some(colours)` once after the theme setting changed; empty colours
    /// mean the house look.
    pub(crate) fn take_theme_change(&mut self) -> Option<Vec<[f32; 3]>> {
        std::mem::take(&mut self.theme_dirty).then(|| self.theme_colors.clone())
    }

    /// The palette colours the settings hold: the shell lays them over a new
    /// base look when only the style changed.
    pub(crate) fn theme_colors(&self) -> &[[f32; 3]] {
        &self.theme_colors
    }

    /// `Some(style)` once after `theme_style` changed (`phosphor`/`daylight`),
    /// read or written; the shell swaps its base look to it.
    pub(crate) fn take_style_change(&mut self) -> Option<String> {
        self.style_change.take()
    }

    /// Adopts `style` as the house look when it differs from the one held.
    fn set_style(&mut self, style: &str) {
        if style != self.theme_style {
            self.theme_style = style.to_string();
            self.style_change = Some(style.to_string());
        }
    }

    /// Adopt the reply's `language`, if it names one the shell knows: the
    /// whole shell paints in it from the next frame on.
    /// Moves the UI scale by `steps` notches (`0` resets) and persists it.
    pub(crate) fn step_ui_scale(&mut self, door: &dyn Call, steps: i32) {
        self.set_ui_scale(door, crate::ui_scale::stepped(self.ui_scale, steps));
    }

    fn set_ui_scale(&mut self, door: &dyn Call, pct: u32) {
        if pct == self.ui_scale {
            return;
        }
        if let Ok(reply) = door.json("POST", "/settings", Some(json!({ "ui_scale": pct }))) {
            if let Some(n) = reply["ui_scale"].as_u64() {
                self.ui_scale = n as u32;
                crate::ui_scale::set(n as u32);
            }
        }
    }

    fn apply_language(&mut self, reply: &serde_json::Value) {
        if let Some(lang) = reply["language"].as_str().and_then(Lang::parse) {
            self.language = lang;
            i18n::set_lang(lang);
        }
    }

    /// One `GET /ai-config`, on the first frame — the same shape [`refresh`]
    /// has for `/settings`. Only the editable provider scratch is seeded; the
    /// section rereads the reply per frame for `configured`/`key_masked`/
    /// `tools`, so a provider save that lands is never a frame behind.
    pub(crate) fn refresh_provider(&mut self, door: &dyn Call) {
        if self.fetched_provider {
            return;
        }
        self.fetched_provider = true;
        if let Ok(reply) = door.json("GET", "/ai-config", None) {
            adopt_provider(self, &reply);
            if let Some(text) = reply["instructions"].as_str() {
                self.provider_instructions = text.to_string();
            }
        }
    }

    /// F8: the node the Fixtures section last asked the canvas to show, one
    /// read per frame — the shell takes it and calls `nodes.reveal`.
    pub(crate) fn take_pending_reveal(&mut self) -> Option<String> {
        self.pending_reveal.take()
    }

    /// F8: asks the canvas to bring `node` into view on its next paint.
    fn request_reveal(&mut self, node: impl Into<String>) {
        self.pending_reveal = Some(node.into());
    }

    /// Test-only writer — lets a kittest test preload the override without
    /// simulating a combo pick, the same trade
    /// `harness::PanelState::set_prompt_for_test` makes.
    #[cfg(test)]
    pub(crate) fn set_mode_for_test(&mut self, mode: ShellMode) {
        self.shell_mode = mode;
        self.fetched = true;
    }
}

/// Adopt a `GET`/`POST /ai-config` reply's provider fields into the scratch
/// and clear the key field: the reply never echoes the raw key, only
/// `key_masked`, so the empty field means "keep whatever the door holds" on
/// the next `save`.
fn adopt_provider(state: &mut PanelState, reply: &serde_json::Value) {
    if let Some(preset) = reply["preset"].as_str() {
        state.provider_preset = preset.to_string();
    }
    if let Some(url) = reply["url"].as_str() {
        state.provider_url = url.to_string();
    }
    if let Some(model) = reply["model"].as_str() {
        state.provider_model = model.to_string();
    }
    state.provider_key.clear();
}

/// The overlay's whole content: heading, then sidebar of sections plus the
/// active section's own panel. The surrounding `Area`/frame is the
/// caller's (`Shell::ui` wraps this the way it wraps the properties and
/// harness overlays).
pub fn paint(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    pairing: &pairing::PanelState,
    open_graph: Option<&str>,
) -> Response {
    state.refresh_provider(door);
    ui.push_id(Id::new(ID), |ui| {
        panel_frame(theme)
            .show(ui, |ui| {
                // The heading is the overlay window's title bar (`mod.rs`).
                // The frame takes the window's full width, so the painted
                // panel matches the title bar instead of shrinking to content.
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    let sidebar = ui.vertical(|ui| {
                        ui.set_width(theme.region_min.x);
                        let rows: Vec<(Icon, &str, Option<String>)> = SECTIONS
                            .iter()
                            .map(|(label, section)| (section_icon(*section), tr(label), None))
                            .collect();
                        let selected = SECTIONS.iter().position(|(_, s)| *s == state.section);
                        if let Some(i) = nav_list(ui, theme, &rows, selected) {
                            state.section = SECTIONS[i].1;
                        }
                    });
                    // A vertical rule sized to the sidebar, not `ui.separator()`:
                    // a separator in a horizontal row takes *all* the height
                    // offered, and this Area is anchored at the centre, so its
                    // offered height depends on its own last size. That loop
                    // grew the window every frame.
                    let rule_height = sidebar.response.rect.height();
                    let (rule, _) = ui.allocate_exact_size(
                        egui::vec2(ui.spacing().item_spacing.x, rule_height),
                        egui::Sense::hover(),
                    );
                    ui.painter().vline(
                        rule.center().x,
                        rule.y_range(),
                        ui.visuals().widgets.noninteractive.bg_stroke,
                    );
                    ui.vertical(|ui| {
                        ui.set_min_width(theme.region_min.x);
                        match state.section {
                            Section::Ui => section_ui(ui, theme, door, state),
                            Section::Theme => section_theme(ui, theme, door, state),
                            Section::Language => section_language(ui, theme, door, state),
                            Section::Ai => section_ai(ui, theme, door, state),
                            Section::Fixtures => section_fixtures(ui, theme, door, state, open_graph),
                            Section::Discover => section_discover(ui, theme, door, state),
                            Section::Devices => pairing::paint_devices(ui, theme, pairing),
                            Section::Export => section_export(ui, theme, door, state),
                            Section::Performance => section_performance(ui, theme, door, state),
                        }
                    });
                });
                ui.add_space(theme.item_gap.y);
            })
            .response
    })
    .inner
}

/// A section's glyph in the left column.
fn section_icon(section: Section) -> Icon {
    match section {
        Section::Ui => Icon::Interface,
        Section::Theme => Icon::Palette,
        Section::Language => Icon::Language,
        Section::Ai => Icon::Spark,
        Section::Fixtures => Icon::Lamp,
        Section::Discover => Icon::Radar,
        Section::Devices => Icon::Devices,
        Section::Export => Icon::Export,
        Section::Performance => Icon::Gauge,
    }
}

/// Placeholder footer: a disabled row per future preference, named and
/// dated. **`add_enabled(false)`, not a plain label** — these must look like
/// the controls they will become and be visibly unclickable, the exact
/// affordance `more ▾`'s disabled provider item (`S.U8`) used and the reason
/// clicking one does nothing.
const UI_PLACEHOLDERS: [(&str, &str); 1] = [
    (tr_key("borders per surface type"), "O.16"),
];

fn section_ui(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    section_heading(ui, theme, tr("interface scale"));
    ui.horizontal_wrapped(|ui| {
        let pct = state.ui_scale;
        if ui.small_button(RichText::new("-").font(theme.font_small.clone())).clicked() {
            state.step_ui_scale(door, -1);
        }
        ui.label(RichText::new(format!("{pct}%")).font(theme.font_small.clone()));
        if ui.small_button(RichText::new("+").font(theme.font_small.clone())).clicked() {
            state.step_ui_scale(door, 1);
        }
        if ui
            .small_button(RichText::new(tr("reset to default")).font(theme.font_small.clone()))
            .clicked()
        {
            state.step_ui_scale(door, 0);
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("shell mode"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        let selected = state.shell_mode;
        let selected_text = ShellMode::ALL
            .iter()
            .find(|(_, m)| *m == selected)
            .map(|(label, _)| *label)
            .unwrap_or("auto");
        crate::shell::components::combo_box(theme, Id::new("shell/panel/ajustes/shell-mode"))
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for (label, mode) in ShellMode::ALL {
                    if ui
                        .selectable_label(
                            selected == mode,
                            RichText::new(label).font(theme.font_small.clone()),
                        )
                        .clicked()
                        && selected != mode
                    {
                        if let Ok(reply) = door.json(
                            "POST",
                            "/settings",
                            Some(json!({ "shell_mode": mode.wire() })),
                        ) {
                            // Read the reply, not the body we sent: the route
                            // is the one place that knows what landed.
                            if let Some(mode) =
                                reply["shell_mode"].as_str().and_then(ShellMode::parse)
                            {
                                state.shell_mode = mode;
                            }
                        }
                    }
                }
            });
    });

    // The tick level: base (8 Hz) or fine (32 Hz). The same `POST /settings`
    // the shell mode uses, and the same vocabulary `metal_schema::TickRate` holds,
    // so the control, the route and the digester speak one ladder.
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("tick level"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        let selected = state.tick_level;
        crate::shell::components::combo_box(theme, Id::new("shell/panel/ajustes/tick-level"))
            .selected_text(tr_dyn(selected.wire()).to_string())
            .show_ui(ui, |ui| {
                for &rate in metal_schema::TickRate::ALL {
                    if ui
                        .selectable_label(
                            selected == rate,
                            RichText::new(tr_dyn(rate.wire())).font(theme.font_small.clone()),
                        )
                        .clicked()
                        && selected != rate
                    {
                        if let Ok(reply) = door.json(
                            "POST",
                            "/settings",
                            Some(json!({ "tick_level": rate.wire() })),
                        ) {
                            if let Some(t) = reply["tick_level"]
                                .as_str()
                                .and_then(metal_schema::TickRate::parse)
                            {
                                state.tick_level = t;
                            }
                        }
                    }
                }
            });
    });

    // Whether a dropped clip builds its HAP proxy automatically. The same
    // `POST /settings` the two above use, and the same read-the-reply trade.
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("build video proxies on drop"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        let selected = state.ingest_proxy;
        let selected_text = if selected { tr("on") } else { tr("off") };
        crate::shell::components::combo_box(theme, Id::new("shell/panel/ajustes/ingest-proxy"))
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for (label, value) in [(tr("on"), true), (tr("off"), false)] {
                    if ui
                        .selectable_label(
                            selected == value,
                            RichText::new(label).font(theme.font_small.clone()),
                        )
                        .clicked()
                        && selected != value
                    {
                        if let Ok(reply) = door.json(
                            "POST",
                            "/settings",
                            Some(json!({ "ingest_proxy": value })),
                        ) {
                            if let Some(b) = reply["ingest_proxy"].as_bool() {
                                state.ingest_proxy = b;
                            }
                        }
                    }
                }
            });
    });

    ui.label(
        RichText::new(tr("— designed below, no batch open yet —"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );

    for (label, batch) in UI_PLACEHOLDERS {
        ui.add_enabled(
            false,
            egui::Button::new(
                RichText::new(format!("{} — {batch}", tr(label)))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            ),
        );
    }
}

/// The global frame-rate cap: one dropdown, 30 to 300 fps, applied to the
/// client, the previews and the output through `POST /settings`.
fn section_performance(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    section_heading(ui, theme, tr("performance"));
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("frame rate cap"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        let selected = state.fps_cap;
        crate::shell::components::combo_box(theme, Id::new("shell/panel/ajustes/fps-cap"))
            .selected_text(format!("{selected} fps"))
            .show_ui(ui, |ui| {
                for &cap in &crate::frame_cap::FPS_CAPS {
                    if ui
                        .selectable_label(
                            selected == cap,
                            RichText::new(format!("{cap} fps")).font(theme.font_small.clone()),
                        )
                        .clicked()
                        && selected != cap
                    {
                        if let Ok(reply) =
                            door.json("POST", "/settings", Some(json!({ "fps_cap": cap })))
                        {
                            if let Some(n) = reply["fps_cap"].as_u64() {
                                state.fps_cap = n as u32;
                                crate::frame_cap::set(n as u32);
                            }
                        }
                    }
                }
            });
    });
    // How the harmony (tone) model listens; applied live by the route.
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("tone analysis"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        let selected = state.tone_mode.clone();
        crate::shell::components::combo_box(theme, Id::new("shell/panel/ajustes/tone-mode"))
            .selected_text(tr_tone_mode(&selected))
            .show_ui(ui, |ui| {
                for mode in ["off", "sample", "live"] {
                    if ui
                        .selectable_label(
                            selected == mode,
                            RichText::new(tr_tone_mode(mode)).font(theme.font_small.clone()),
                        )
                        .clicked()
                        && selected != mode
                    {
                        if let Ok(reply) =
                            door.json("POST", "/settings", Some(json!({ "tone_mode": mode })))
                        {
                            if let Some(m) = reply["tone_mode"].as_str() {
                                state.tone_mode = m.to_string();
                            }
                        }
                    }
                }
            });
    });
}

/// The dropdown label of a tone mode. Literal `tr` calls so the locale test
/// sees every key.
fn tr_tone_mode(mode: &str) -> String {
    match mode {
        "off" => tr("off").to_string(),
        "live" => tr("live (continuous)").to_string(),
        _ => tr("sample (every few seconds)").to_string(),
    }
}

/// P-G: the folder "Export frame" writes into — a text field saved on
/// focus loss or `save`, and a reset back to the default (an empty
/// `export_dir`, which the route reads as `~/Pictures/MetalVJ`). Both go
/// through `POST /settings` and adopt what the reply says landed.
fn section_export(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    section_heading(ui, theme, tr("export"));
    ui.label(
        RichText::new(tr("export folder"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );
    let response = ui.add(
        TextEdit::singleline(&mut state.export_dir)
            .hint_text(tr("~/Pictures/MetalVJ (default)"))
            .desired_width(ui.available_width())
            .font(theme.font_small.clone()),
    );
    let mut post = None;
    ui.horizontal(|ui| {
        if ui
            .button(RichText::new(tr("save")).font(theme.font_body.clone()))
            .clicked()
            || response.lost_focus()
        {
            post = Some(state.export_dir.trim().to_string());
        }
        if ui
            .button(RichText::new(tr("reset to default")).font(theme.font_body.clone()))
            .clicked()
        {
            post = Some(String::new());
        }
    });
    if let Some(dir) = post {
        if let Ok(reply) = door.json("POST", "/settings", Some(json!({ "export_dir": dir }))) {
            state.export_dir = reply["export_dir"].as_str().unwrap_or(&dir).to_string();
        }
    }
}

/// A section whose batch has not landed: one line, the truth, no fake
/// controls — the exact trade `more ▾`'s disabled provider item made
/// (`S.U8`), which `S.U14` closed by landing.
fn placeholder_section(ui: &mut Ui, theme: &Theme, title: &str, note: &str) {
    section_heading(ui, theme, title);
    ui.label(
        RichText::new(trf("coming soon — {note}", &[("note", &note)]))
            .font(theme.font_body.clone())
            .color(theme.ink),
    );
}

/// The language selector: every [`Lang`] by its own name (never translated,
/// so an operator who cannot read the current language still finds theirs).
/// A pick posts `language` and adopts what the reply says landed.
fn section_language(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    section_heading(ui, theme, tr("language"));
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("interface language"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        let selected = state.language;
        crate::shell::components::combo_box(theme, Id::new("shell/panel/settings/language"))
            .selected_text(selected.native_name())
            .show_ui(ui, |ui| {
                for lang in Lang::ALL {
                    if ui
                        .selectable_label(
                            selected == lang,
                            RichText::new(lang.native_name()).font(theme.font_small.clone()),
                        )
                        .clicked()
                        && selected != lang
                    {
                        if let Ok(reply) = door.json(
                            "POST",
                            "/settings",
                            Some(json!({ "language": lang.code() })),
                        ) {
                            state.apply_language(&reply);
                        }
                    }
                }
            });
    });
    ui.label(
        RichText::new(tr("text without a translation stays in English"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );
}

/// Every stored palette (`GET /author/palettes`): name and its colours.
fn read_palettes(door: &dyn Call) -> Vec<(String, Vec<[f32; 3]>)> {
    let Ok(reply) = door.json("GET", "/author/palettes", None) else { return Vec::new() };
    reply["palettes"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|p| {
                    let name = p["name"].as_str()?.to_string();
                    let colors = p["colors"]
                        .as_array()?
                        .chunks_exact(3)
                        .map(|c| [0, 1, 2].map(|i| c[i].as_f64().unwrap_or(1.0) as f32))
                        .collect();
                    Some((name, colors))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Four swatches of a palette, the combination as it will look.
fn palette_swatches(ui: &mut Ui, theme: &Theme, colors: &[[f32; 3]]) {
    let size = egui::vec2(theme.font_small.size, theme.font_small.size);
    for c in colors.iter().take(4) {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let col = egui::Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
        ui.painter().rect_filled(rect, 1.0, col);
    }
}

/// The `theme` section: one Palettes dropdown (the stored combinations, with
/// their colours) that recolours the whole client; "default" is the house look.
fn section_theme(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    section_heading(ui, theme, tr("theme"));
    // The house look: phosphor (dark) or daylight (light). A pick is persisted
    // through the settings route, which answers the style that landed.
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tr("theme style")).font(theme.font_small.clone()).color(theme.ink_weak));
        let mut style = state.theme_style.clone();
        let options = [("phosphor".to_string(), "phosphor"), ("daylight".to_string(), "daylight")];
        if crate::shell::components::dropdown(ui, theme, "shell/region/settings/theme-style", &mut style, &options)
            .changed()
        {
            if let Ok(reply) = door.json("POST", "/settings", Some(json!({ "theme_style": style }))) {
                if let Some(landed) = reply["theme_style"].as_str() {
                    state.set_style(landed);
                }
            }
        }
    });
    if state.theme_palettes.is_empty() {
        state.theme_palettes = read_palettes(door);
    }
    let mut picked: Option<(String, Vec<[f32; 3]>)> = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tr("Palettes")).font(theme.font_small.clone()).color(theme.ink_weak));
        let shown = if state.theme_name.is_empty() { tr("default").to_string() } else { state.theme_name.clone() };
        crate::shell::components::combo_box(theme, Id::new("shell/panel/settings/theme"))
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui.selectable_label(state.theme_name.is_empty(), tr("default")).clicked() {
                    picked = Some((String::new(), Vec::new()));
                }
                for (name, colors) in &state.theme_palettes {
                    ui.horizontal(|ui| {
                        palette_swatches(ui, theme, colors);
                        if ui.selectable_label(*name == state.theme_name, name.clone()).clicked() {
                            picked = Some((name.clone(), colors.clone()));
                        }
                    });
                }
            });
    });
    if let Some((name, colors)) = picked {
        if door.json("POST", "/settings", Some(json!({ "theme": name }))).is_ok() {
            state.pick_theme(&name, Some(colors));
        }
    }
}

/// The `IA` section: the provider configuration, moved out of the harness
/// sidebar, plus the per-tool policy table. Both halves are one door route
/// (`GET`/`POST /ai-config`, `crate::call::ai`) — the provider fields carry a
/// `preset` and never a `tools` key, a tool pick carries `tools` and never a
/// `preset`, so the two paths never blank each other (see `call::ai`'s own
/// module doc). No run buttons here: the harness is still where a tool runs.
fn section_ai(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    let cfg = door
        .json("GET", "/ai-config", None)
        .unwrap_or_else(|_| json!({}));

    section_heading(ui, theme, tr("provider"));
    let configured = cfg["configured"].as_bool().unwrap_or(false);
    let status = if configured {
        tr("configured")
    } else {
        tr("not configured")
    };
    ui.label(
        RichText::new(status)
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("preset"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        crate::shell::components::combo_box(theme, Id::new("shell/panel/ajustes/provider-preset"))
            .selected_text(&state.provider_preset)
            .show_ui(ui, |ui| {
                for preset in PROVIDER_PRESETS {
                    if ui
                        .selectable_label(
                            state.provider_preset == preset,
                            RichText::new(preset).font(theme.font_small.clone()),
                        )
                        .clicked()
                    {
                        state.provider_preset = preset.to_string();
                    }
                }
            });
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("url"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        ui.add(TextEdit::singleline(&mut state.provider_url).desired_width(theme.text_input_width));
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("model"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        ui.add(
            TextEdit::singleline(&mut state.provider_model).desired_width(theme.text_input_width),
        );
    });

    let key_hint = cfg["key_masked"].as_str().unwrap_or("").to_string();
    let key_empty = state.provider_key.is_empty();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("api key"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        let mut edit = TextEdit::singleline(&mut state.provider_key)
            .password(true)
            .desired_width(theme.text_input_width);
        if key_empty && !key_hint.is_empty() {
            edit = edit.hint_text(key_hint.clone());
        }
        ui.add(edit);
    });

    if ui
        .button(RichText::new(tr("save")).font(theme.font_body.clone()))
        .clicked()
    {
        let mut body = json!({
            "preset": state.provider_preset.clone(),
            "url": state.provider_url.clone(),
            "model": state.provider_model.clone(),
        });
        // An empty key field means "keep the stored one" — never send `key`.
        if !state.provider_key.is_empty() {
            body["key"] = json!(state.provider_key.clone());
        }
        if let Ok(reply) = door.json("POST", "/ai-config", Some(body)) {
            adopt_provider(state, &reply);
        }
    }

    ui.separator();
    section_ai_instructions(ui, theme, door, state);
    ui.separator();
    section_ai_permissions(ui, theme, door, &cfg);
}

/// The free-text box the system prompt carries under "Operator instructions"
/// (`harness::with_operator_instructions`). Saved on focus loss — not per
/// keystroke — and by `save`, which is the same post for a sighted
/// operator who would rather click.
fn section_ai_instructions(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    section_heading(ui, theme, tr("AI instructions"));
    let edit = TextEdit::multiline(&mut state.provider_instructions)
        .desired_width(ui.available_width())
        .desired_rows(theme.ai_instructions_rows)
        .font(theme.font_small.clone());
    let response = ui.add(edit);
    let save_clicked = ui
        .button(RichText::new(tr("save")).font(theme.font_body.clone()))
        .clicked();
    if response.lost_focus() || save_clicked {
        let _ = door.json(
            "POST",
            "/ai-config",
            Some(instructions_body(&state.provider_instructions)),
        );
    }
}

/// Three rows, one per capability (`internet`, `dispositivos`, `archivos`),
/// each with `always`/`ask`/`deny`. The current answer is read from
/// the door's reply on every frame (`permissions`, default `ask`), so a pick
/// lands on the next paint without this panel keeping its own copy.
fn section_ai_permissions(ui: &mut Ui, theme: &Theme, door: &dyn Call, cfg: &serde_json::Value) {
    section_heading(ui, theme, tr("permissions"));
    for (key, label) in CAPABILITY_PERMISSIONS {
        let current = cfg["permissions"][key]
            .as_str()
            .unwrap_or("ask")
            .to_string();
        ui.push_id(Id::new(("shell/panel/ajustes/permission", key)), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(tr(label))
                        .font(theme.font_body.clone())
                        .color(theme.ink),
                );
                for (wire, choice) in PERMISSION_CHOICES {
                    let selected = current == wire;
                    if ui
                        .selectable_label(
                            selected,
                            RichText::new(tr(choice)).font(theme.font_small.clone()),
                        )
                        .clicked()
                        && !selected
                    {
                        let _ = door.json("POST", "/ai-config", Some(permission_body(key, wire)));
                    }
                }
            });
        });
    }
}

// ---------------------------------------------------------------------------
// D1 — the Fixtures section
// ---------------------------------------------------------------------------

/// The fixture kinds the form cycles, the exact wire words
/// `metal_perform::venue::FixtureKind` spells — a data vocabulary (a wash is a
/// wash in every language), so the combo paints the word itself.
const FIXTURE_KINDS: &[&str] = &[
    "wash",
    "spot",
    "led_strip",
    "moving",
    "backlight",
    "panel",
    "continuous",
    "led_bar",
    "camera",
    "matrix",
];

/// The wires a fixture rides: the same `dmx`/`sacn`/`usb`/`bluetooth`
/// (`wire = …`) `metal_perform::fixture` reads, plus `artnet` — what the
/// Discover section's accepted Art-Net rows carry, so the form can keep or
/// change one.
const FIXTURE_WIRES: &[&str] = &["dmx", "sacn", "usb", "bluetooth", "artnet"];

/// One row of the Fixtures table: the venue's own fixture (name/profile/group)
/// joined with its patch row — the wire, universe, address and channel span
/// `GET /venue` computes from the profile.
struct FixtureRow {
    id: String,
    name: String,
    kind: String,
    profile: String,
    group: String,
    wire: String,
    universe: u16,
    address: u16,
    span: u16,
}

/// The Fixtures table of a `GET /venue` reply: one row per `patch` entry, with
/// the venue fixture's own name/profile/group looked up by id.
fn fixture_rows(read: &Value) -> Vec<FixtureRow> {
    let fixtures = read["venue"]["fixture"].as_array();
    let field = |id: &str, key: &str| {
        fixtures
            .and_then(|a| a.iter().find(|f| f["id"].as_str() == Some(id)))
            .and_then(|f| f[key].as_str())
            .unwrap_or_default()
            .to_string()
    };
    read["patch"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| {
                    let id = r["fixture"].as_str().unwrap_or_default().to_string();
                    FixtureRow {
                        name: field(&id, "name"),
                        profile: field(&id, "profile"),
                        group: field(&id, "group"),
                        kind: r["kind"].as_str().unwrap_or_default().to_string(),
                        wire: r["wire"].as_str().unwrap_or_default().to_string(),
                        universe: r["universe"].as_u64().unwrap_or(0) as u16,
                        address: r["address"].as_u64().unwrap_or(0) as u16,
                        span: r["span"].as_u64().unwrap_or(0) as u16,
                        id,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// F8: the `venue.fixture` nodes of the open graph as `(node id, venue,
/// fixture)` — what the Fixtures table's "In graph" column matches a row
/// against, and what the orphan list shows. Empty without an open graph.
fn fixture_graph_nodes(door: &dyn Call, open_graph: Option<&str>) -> Vec<(String, String, String)> {
    let Some(gid) = open_graph else {
        return Vec::new();
    };
    let mut query = std::collections::HashMap::new();
    query.insert("id".to_string(), gid.to_string());
    let params = crate::call::Params { query, body: Vec::new(), tail: None };
    let reply = match door.call("GET", "/graph", params) {
        Some(reply) if reply.status == 200 => crate::json_memo::parse(&reply.body).unwrap_or(Value::Null),
        _ => Value::Null,
    };
    reply["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            if n["kind"].as_str() != Some("venue.fixture") {
                return None;
            }
            let id = n["id"].as_str()?.to_string();
            let venue = n["config"]["venue"].as_str()?.to_string();
            let fixture = n["config"]["fixture"].as_str()?.to_string();
            Some((id, venue, fixture))
        })
        .collect()
}

/// `GET /venue?id=<id>` through the door, parsed — the same read the venue
/// editor makes.
fn read_venue(door: &dyn Call, id: &str) -> Option<Value> {
    let mut query = std::collections::HashMap::new();
    query.insert("id".to_string(), id.to_string());
    let params = crate::call::Params {
        query,
        body: Vec::new(),
        tail: None,
    };
    match door.call("GET", "/venue", params) {
        Some(reply) if reply.status == 200 => crate::json_memo::parse(&reply.body).ok(),
        _ => None,
    }
}

/// The `POST /venue { op: "fixture" }` body the form's scratch turns into:
/// the venue is named with `venue`, the edited fixture with `id` (absent
/// adds), and an empty optional field is sent as `null` so "clear it" is
/// distinguishable from "leave it alone". Numbers are sent as numbers; an
/// unparsable one is sent as `null` rather than guessed.
fn fixture_body(state: &PanelState, venue: &str) -> Value {
    let opt = |s: &str| if s.is_empty() { Value::Null } else { json!(s) };
    let num = |s: &str| match s.trim().parse::<u64>() {
        Ok(n) => json!(n),
        Err(_) => Value::Null,
    };
    let mut body = json!({
        "op": "fixture",
        "venue": venue,
        "kind": state.fixtures_kind,
        "profile": opt(&state.fixtures_profile),
        "group": opt(&state.fixtures_group),
        "wire": state.fixtures_wire,
        "universe": num(&state.fixtures_universe),
        "address": num(&state.fixtures_address),
    });
    if let Some(id) = &state.fixtures_edit {
        body["id"] = json!(id);
    }
    if !state.fixtures_name.is_empty() {
        body["name"] = json!(state.fixtures_name);
    }
    body
}

/// Seed the edit scratch from a table row's `edit`.
fn adopt_fixture_row(state: &mut PanelState, row: &FixtureRow) {
    state.fixtures_edit = Some(row.id.clone());
    state.fixtures_kind = row.kind.clone();
    state.fixtures_name = row.name.clone();
    state.fixtures_profile = row.profile.clone();
    state.fixtures_group = row.group.clone();
    state.fixtures_wire = row.wire.clone();
    state.fixtures_universe = row.universe.to_string();
    state.fixtures_address = row.address.to_string();
    state.fixtures_note = None;
}

/// Reset the scratch back to "adding a wash".
fn clear_fixture_form(state: &mut PanelState) {
    state.fixtures_edit = None;
    state.fixtures_kind = "wash".to_string();
    state.fixtures_name.clear();
    state.fixtures_profile.clear();
    state.fixtures_group.clear();
    state.fixtures_wire = "dmx".to_string();
    state.fixtures_universe.clear();
    state.fixtures_address.clear();
    state.fixtures_note = None;
}

/// A labelled single-line text field, the shape every free-text fixture field
/// uses.
fn fixture_text_field(ui: &mut Ui, theme: &Theme, label: &str, value: &mut String) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(label)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        ui.add(
            TextEdit::singleline(value)
                .desired_width(theme.text_input_width)
                .font(theme.font_small.clone()),
        );
    });
}

/// A labelled combo over a fixed vocabulary, painting the word itself.
fn fixture_combo(
    ui: &mut Ui,
    theme: &Theme,
    salt: &str,
    label: &str,
    current: &mut String,
    options: &[&str],
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(label)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        crate::shell::components::combo_box(theme, Id::new(("shell/panel/ajustes/fixture", salt)))
            .selected_text(current.clone())
            .show_ui(ui, |ui| {
                for opt in options {
                    if ui
                        .selectable_label(
                            *current == *opt,
                            RichText::new(*opt).font(theme.font_small.clone()),
                        )
                        .clicked()
                    {
                        *current = (*opt).to_string();
                    }
                }
            });
    });
}

/// D1: the `Fixtures` section — the active venue's fixtures in one table with
/// add/edit/remove rows. Every read is `GET /venue` (the active venue is the
/// one `GET /venue` answers as `last`) and every write is
/// `POST /venue { op: "fixture" }` — the same route the `set_fixture` tool
/// posts (rule 6). The panel never touches the venue file: it paints the
/// door's own reply.
fn section_fixtures(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    open_graph: Option<&str>,
) {
    section_heading(ui, theme, tr("fixtures"));
    let list = door
        .json("GET", "/venue", None)
        .unwrap_or_else(|_| json!({}));
    let Some(active) = list["last"].as_str().map(str::to_string) else {
        ui.label(
            RichText::new(tr("no active venue — project one first"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    };
    let read = read_venue(door, &active).unwrap_or_else(|| json!({}));
    let rows = fixture_rows(&read);
    // F8: the open graph's per-fixture nodes — one per row when it exists, the
    // rest orphans the operator can remove.
    let graph_nodes = fixture_graph_nodes(door, open_graph);

    if rows.is_empty() {
        ui.label(
            RichText::new(tr("— no fixtures —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    } else {
        egui::Grid::new("shell/panel/ajustes/fixtures").striped(true).show(ui, |ui| {
            for head in [tr("name"), tr("kind"), tr("profile"), tr("group"), tr("wire"), tr("universe"), tr("address"), tr("span"), tr("in graph")] {
                ui.label(RichText::new(head).font(theme.font_small.clone()).color(theme.ink_faint));
            }
            ui.end_row();
            for row in &rows {
                ui.label(RichText::new(&row.name).font(theme.font_small.clone()).color(theme.ink));
                for value in [&row.kind, &row.profile, &row.group, &row.wire] {
                    ui.label(RichText::new(value).font(theme.font_small.clone()).color(theme.ink_weak));
                }
                for value in [row.universe, row.address, row.span] {
                    ui.label(RichText::new(value.to_string()).font(theme.font_small.clone()).color(theme.ink_weak));
                }
                // F8: the row's own `venue.fixture` node — "go to node" when
                // the open graph has one, "add node" when it does not. Both
                // go through the same routes the canvas and the AI use.
                let node = graph_nodes
                    .iter()
                    .find(|(_, v, f)| v == &active && f == &row.id)
                    .map(|(id, _, _)| id.clone());
                match node {
                    Some(id) => {
                        if ui
                            .small_button(RichText::new(tr("go to node")).font(theme.font_small.clone()))
                            .clicked()
                        {
                            state.request_reveal(id);
                        }
                    }
                    None => {
                        if ui
                            .small_button(RichText::new(tr("add node")).font(theme.font_small.clone()))
                            .clicked()
                        {
                            let mut body = json!({
                                "op": "add", "kind": "venue.fixture",
                                "config": { "venue": active, "fixture": row.id },
                            });
                            if let Some(g) = open_graph {
                                body["graph"] = json!(g);
                            }
                            state.fixtures_note = Some(match door.json("POST", "/graph/node", Some(body)) {
                                Ok(_) => tr("node added").to_string(),
                                Err((_, msg)) => msg,
                            });
                        }
                    }
                }
                if ui.small_button(RichText::new(tr("edit")).font(theme.font_small.clone())).clicked() {
                    adopt_fixture_row(state, row);
                }
                if ui.small_button(RichText::new(tr("remove")).font(theme.font_small.clone())).clicked() {
                    let body = json!({ "op": "fixture", "venue": active, "id": row.id, "delete": true });
                    state.fixtures_note = Some(match door.json("POST", "/venue", Some(body)) {
                        Ok(_) => tr("fixture removed").to_string(),
                        Err((_, msg)) => msg,
                    });
                }
                ui.end_row();
            }
        });

        // F8: a node whose row is gone — an orphan the operator can drop.
        let orphans: Vec<&(String, String, String)> = graph_nodes
            .iter()
            .filter(|(_, v, f)| v == &active && !rows.iter().any(|r| &r.id == f))
            .collect();
        if !orphans.is_empty() {
            ui.separator();
            ui.label(RichText::new(tr("orphan nodes")).font(theme.font_small.clone()).color(theme.ink_faint));
            for (id, _, fixture) in orphans {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(trf("node for {fixture}", &[("fixture", fixture)]))
                            .font(theme.font_small.clone())
                            .color(theme.ink_weak),
                    );
                    if ui.small_button(RichText::new(tr("remove")).font(theme.font_small.clone())).clicked() {
                        if let Some(g) = open_graph {
                            let body = json!({ "op": "remove", "graph": g, "node": id });
                            state.fixtures_note = Some(match door.json("POST", "/graph/node", Some(body)) {
                                Ok(_) => tr("node removed").to_string(),
                                Err((_, msg)) => msg,
                            });
                        }
                    }
                });
            }
        }
    }

    ui.separator();
    let editing = state.fixtures_edit.is_some();
    section_heading(
        ui,
        theme,
        if editing {
            tr("edit")
        } else {
            tr("add fixture")
        },
    );
    fixture_text_field(ui, theme, tr("name"), &mut state.fixtures_name);
    fixture_combo(
        ui,
        theme,
        "kind",
        tr("kind"),
        &mut state.fixtures_kind,
        FIXTURE_KINDS,
    );
    // The profile combo: every model `GET /fixtures` answers, plus `none`.
    let profiles: Vec<String> = door
        .json("GET", "/fixtures", None)
        .ok()
        .and_then(|r| {
            r["fixtures"].as_array().map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(tr("profile"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        let selected = if state.fixtures_profile.is_empty() {
            tr("none").to_string()
        } else {
            state.fixtures_profile.clone()
        };
        crate::shell::components::combo_box(theme, Id::new("shell/panel/ajustes/fixture-profile"))
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        state.fixtures_profile.is_empty(),
                        RichText::new(tr("none")).font(theme.font_small.clone()),
                    )
                    .clicked()
                {
                    state.fixtures_profile.clear();
                }
                for model in &profiles {
                    if ui
                        .selectable_label(
                            &state.fixtures_profile == model,
                            RichText::new(model).font(theme.font_small.clone()),
                        )
                        .clicked()
                    {
                        state.fixtures_profile = model.clone();
                    }
                }
            });
    });
    fixture_text_field(ui, theme, tr("group"), &mut state.fixtures_group);
    fixture_combo(
        ui,
        theme,
        "wire",
        tr("wire"),
        &mut state.fixtures_wire,
        FIXTURE_WIRES,
    );
    fixture_text_field(ui, theme, tr("universe"), &mut state.fixtures_universe);
    fixture_text_field(ui, theme, tr("address"), &mut state.fixtures_address);

    ui.horizontal(|ui| {
        if ui
            .button(RichText::new(tr("save")).font(theme.font_body.clone()))
            .clicked()
        {
            let body = fixture_body(state, &active);
            match door.json("POST", "/venue", Some(body)) {
                Ok(_) => {
                    clear_fixture_form(state);
                    state.fixtures_note = Some(tr("fixture saved").to_string());
                }
                Err((_, msg)) => state.fixtures_note = Some(msg),
            }
        }
        if editing
            && ui
                .button(RichText::new(tr("cancel")).font(theme.font_body.clone()))
                .clicked()
        {
            clear_fixture_form(state);
        }
    });
    if let Some(note) = &state.fixtures_note {
        ui.label(
            RichText::new(note.clone())
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }
}

/// F13: one fixture discovery proposes. Discovery never writes the venue
/// itself; the operator accepts a row and it becomes a venue fixture through
/// `POST /venue { op: "fixture" }` — the same route the Fixtures page and the
/// `set_fixture` tool post (rule 6).
struct DiscoverRow {
    /// The source's own label (the Art-Net short name or the sACN source
    /// name), falling back to its IP.
    name: String,
    /// `"artnet"` or `"sacn"`.
    wire: &'static str,
    /// The Art-Net Port-Address the source advertised (`artnet`, the whole
    /// 15-bit Net·Sub-Net·Universe) or the sACN universe (`sacn`).
    universe: u16,
}

/// The rows a `GET /artnet/nodes` reply proposes: one per output
/// Port-Address of every node that answered ArtPoll. A node with no output
/// port proposes nothing.
fn artnet_rows(reply: &Value) -> Vec<DiscoverRow> {
    let mut rows = Vec::new();
    for n in reply["nodes"].as_array().into_iter().flatten() {
        let ip = n["ip"].as_str().unwrap_or_default();
        let name = n["short_name"].as_str().filter(|s| !s.is_empty()).unwrap_or(ip);
        for pa in n["outputs"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
            rows.push(DiscoverRow {
                name: name.to_string(),
                wire: "artnet",
                universe: pa.min(u16::MAX as u64) as u16,
            });
        }
    }
    rows
}

/// The rows a `GET /sacn/nodes` reply proposes: one per universe every source
/// announced on the discovery multicast.
fn sacn_rows(reply: &Value) -> Vec<DiscoverRow> {
    let mut rows = Vec::new();
    for s in reply["sources"].as_array().into_iter().flatten() {
        let ip = s["ip"].as_str().unwrap_or_default();
        let name = s["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(ip);
        for u in s["universes"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
            rows.push(DiscoverRow {
                name: name.to_string(),
                wire: "sacn",
                universe: u.min(u16::MAX as u64) as u16,
            });
        }
    }
    rows
}

/// The body an accepted proposal posts. A discovered port carries no profile
/// yet, so the row starts as a profile-less `wash` the operator completes in
/// Fixtures (rule 8: this only ever writes through the door, never a file).
fn discover_fixture_body(venue: &str, row: &DiscoverRow) -> Value {
    json!({
        "op": "fixture",
        "venue": venue,
        "kind": "wash",
        "name": row.name,
        "wire": row.wire,
        "universe": row.universe,
    })
}

/// The note the camera stub leaves for the operator: until M5
/// `discover_fixture` posts a venue draft carrying a "pending camera solve"
/// line rather than failing, and that line (or the route's own refusal) is
/// what the button shows.
fn camera_note(reply: &Value) -> String {
    if let Some(err) = reply["error"].as_str() {
        return err.to_string();
    }
    let notes = reply["draft"]["venue"]["venue"]["based_on"].as_array();
    if let Some(last) = notes.and_then(|a| a.iter().filter_map(Value::as_str).last()) {
        return last.to_string();
    }
    tr("draft proposed").to_string()
}

/// F13: the `Discover` section — three sub-sections (Art-Net, sACN, Camera)
/// that only *propose* fixture rows; the operator accepts each one. Every
/// read and write goes through the door (rule 9): `GET /artnet/nodes`,
/// `GET /sacn/nodes`, `POST /venue { op: "fixture" }` and the camera tool's
/// `POST /agent/tool`.
fn section_discover(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    section_heading(ui, theme, tr("discover"));
    ui.label(
        RichText::new(tr("discovery only proposes — accept each row to write it"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );

    // An accepted row is a venue row, so the venue is read first — the same
    // `GET /venue` the Fixtures section reads.
    let list = door.json("GET", "/venue", None).unwrap_or_else(|_| json!({}));
    let Some(active) = list["last"].as_str().map(str::to_string) else {
        ui.label(
            RichText::new(tr("no active venue — project one first"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    };

    section_heading(ui, theme, tr("art-net"));
    if ui.button(tr("find art-net nodes")).clicked() {
        state.discover_artnet = Some(match door.json("GET", "/artnet/nodes", None) {
            Ok(v) => v,
            Err((_, msg)) => json!({ "error": msg }),
        });
    }
    if let Some(reply) = state.discover_artnet.clone() {
        discover_reply(ui, theme, door, state, &active, "artnet", &reply);
    }

    section_heading(ui, theme, tr("sacn"));
    if ui.button(tr("listen for sources")).clicked() {
        state.discover_sacn = Some(match door.json("GET", "/sacn/nodes", None) {
            Ok(v) => v,
            Err((_, msg)) => json!({ "error": msg }),
        });
    }
    if let Some(reply) = state.discover_sacn.clone() {
        discover_reply(ui, theme, door, state, &active, "sacn", &reply);
    }

    section_heading(ui, theme, tr("camera (flash)"));
    if ui.button(tr("discover by flash")).clicked() {
        let body = json!({ "name": "discover_fixture", "args": {}, "allow_ask": true });
        state.discover_note = Some(match door.json("POST", AGENT_TOOL_PATH, Some(body)) {
            Ok(v) => camera_note(&v),
            Err((_, msg)) => msg,
        });
    }

    if let Some(note) = &state.discover_note {
        ui.label(
            RichText::new(note.clone())
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }
}

/// One discovery reply (Art-Net or sACN) painted as proposed rows: an error
/// line, "— nothing answered —", or a table whose every row can be accepted.
fn discover_reply(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    venue: &str,
    salt: &str,
    reply: &Value,
) {
    if let Some(msg) = reply["error"].as_str() {
        ui.label(
            RichText::new(format!("{}: {msg}", tr("discovery")))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    }
    let rows = if salt == "artnet" { artnet_rows(reply) } else { sacn_rows(reply) };
    if rows.is_empty() {
        ui.label(
            RichText::new(tr("— no node answered —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    }
    egui::Grid::new(("shell/panel/ajustes/discover", salt))
        .striped(true)
        .show(ui, |ui| {
            for head in [tr("name"), tr("wire"), tr("port-address")] {
                ui.label(RichText::new(head).font(theme.font_small.clone()).color(theme.ink_faint));
            }
            ui.end_row();
            for row in &rows {
                ui.label(RichText::new(&row.name).font(theme.font_small.clone()).color(theme.ink));
                ui.label(RichText::new(row.wire).font(theme.font_small.clone()).color(theme.ink_weak));
                ui.label(RichText::new(row.universe.to_string()).font(theme.font_small.clone()).color(theme.ink_weak));
                if ui
                    .small_button(RichText::new(tr("add as fixture")).font(theme.font_small.clone()))
                    .clicked()
                {
                    let body = discover_fixture_body(venue, row);
                    state.discover_note = Some(match door.json("POST", "/venue", Some(body)) {
                        Ok(_) => tr("fixture saved").to_string(),
                        Err((_, msg)) => msg,
                    });
                }
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capability pick posts exactly `{"permissions": {key: wire}}` — the
    /// tools-only route never sees it, so the two halves cannot collide.
    #[test]
    fn a_capability_pick_posts_only_its_own_key() {
        assert_eq!(
            permission_body("internet", "allow"),
            json!({ "permissions": { "internet": "allow" } })
        );
    }

    /// The instructions box saves the whole text under one `instructions` key.
    #[test]
    fn the_instructions_box_posts_the_whole_text_under_one_key() {
        assert_eq!(
            instructions_body("avoid devices"),
            json!({ "instructions": "avoid devices" })
        );
    }

    /// The keys and answers are the wire words the door parses, in the order
    /// the rows and buttons paint them.
    #[test]
    fn the_capability_keys_and_answers_are_the_wire_words() {
        let keys: Vec<&str> = CAPABILITY_PERMISSIONS.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, ["internet", "dispositivos", "archivos"]);
        let wires: Vec<&str> = PERMISSION_CHOICES.iter().map(|(w, _)| *w).collect();
        assert_eq!(wires, ["allow", "ask", "deny"]);
    }

    // -- D1: the Fixtures section (rule 6: it writes through the door, never
    //    a handler, and paints the reply).

    /// A `GET /venue` reply that answers both shapes the section asks for —
    /// `last` (the active venue) and, for `?id=`, the venue and its patch —
    /// so the same sticky reply serves the list read and the venue read.
    fn venue_fixture_read() -> serde_json::Value {
        json!({
            "venues": [],
            "last": "bar",
            "venue": {
                "venue": { "id": "bar", "name": "Bar", "setting": "indoor" },
                "fixture": [
                    { "id": "wash-1", "name": "wash 1", "kind": "wash", "profile": "example-wash",
                      "group": "front", "wire": "dmx", "universe": 1, "address": 20 }
                ]
            },
            "patch": [
                { "fixture": "wash-1", "kind": "wash", "wire": "dmx", "universe": 1, "address": 20, "span": 4 }
            ]
        })
    }

    fn fixture_door() -> crate::shell::FakeDoor {
        let door = crate::shell::FakeDoor::new();
        door.respond("GET", "/venue", venue_fixture_read());
        door.respond("GET", "/fixtures", json!({ "fixtures": ["example-wash"] }));
        door
    }

    fn fixtures_harness(
        door: std::rc::Rc<crate::shell::FakeDoor>,
    ) -> egui_kittest::Harness<'static, PanelState> {
        let theme = Theme::phosphor();
        let t = theme.clone();
        let h = egui_kittest::Harness::builder()
            .with_size(egui::Vec2 { x: 720.0, y: 560.0 })
            .build_ui_state(
                move |ui, state: &mut PanelState| section_fixtures(ui, &t, door.as_ref(), state, None),
                PanelState::default(),
            );
        theme.apply(&h.ctx);
        h
    }

    /// The `POST /venue {op:"fixture"}` the form posted, parsed.
    fn posted_fixture(door: &crate::shell::FakeDoor) -> Value {
        door.calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/venue")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .find(|b| b["op"] == "fixture")
            .expect("the form posts POST /venue {op: fixture}")
    }

    /// The table joins the patch row with the venue fixture's own
    /// name/profile/group.
    #[test]
    fn the_fixture_table_joins_the_patch_with_the_venue_fixture() {
        let rows = fixture_rows(&venue_fixture_read());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "wash-1");
        assert_eq!(rows[0].name, "wash 1");
        assert_eq!(rows[0].profile, "example-wash");
        assert_eq!(rows[0].group, "front");
        assert_eq!(rows[0].span, 4);
    }

    /// D4: a matrix fixture (kind `matrix`, no profile) lists in the same
    /// table, its channel span the one `GET /venue` computed from its map —
    /// the page never needs a profile to show it.
    #[test]
    fn the_fixture_table_lists_a_matrix_fixture() {
        let read = json!({
            "venues": [],
            "last": "bar",
            "venue": {
                "venue": { "id": "bar", "name": "Bar", "setting": "indoor" },
                "fixture": [
                    { "id": "media-1", "name": "media", "kind": "matrix",
                      "group": "wall", "wire": "sacn", "universe": 2, "address": 300 }
                ]
            },
            "patch": [
                { "fixture": "media-1", "kind": "matrix", "wire": "sacn",
                  "universe": 2, "address": 300, "span": 15 }
            ]
        });
        let rows = fixture_rows(&read);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "matrix");
        assert_eq!(rows[0].wire, "sacn");
        assert_eq!(rows[0].universe, 2);
        assert_eq!(rows[0].address, 300);
        assert_eq!(rows[0].span, 15);
        assert!(
            FIXTURE_KINDS.contains(&"matrix"),
            "the form can pick a matrix kind"
        );
    }

    /// Numbers go as numbers and an unparsable one as `null`, so "clear it"
    /// is never guessed as zero.
    #[test]
    fn the_fixture_body_sends_numbers_and_clears_with_null() {
        let mut state = PanelState::default();
        state.fixtures_universe = "3".to_string();
        state.fixtures_address = "9999".to_string();
        state.fixtures_group = "front".to_string();
        let body = fixture_body(&state, "bar");
        assert_eq!(body["universe"], json!(3));
        assert_eq!(body["address"], json!(9999));
        assert_eq!(body["group"], json!("front"));
        assert_eq!(body["profile"], Value::Null);
        assert!(body.get("id").is_none(), "adding posts no id");
        state.fixtures_universe = "abc".to_string();
        assert_eq!(fixture_body(&state, "bar")["universe"], Value::Null);
        state.fixtures_edit = Some("wash-1".to_string());
        assert_eq!(fixture_body(&state, "bar")["id"], json!("wash-1"));
    }

    /// Adding from the section posts the op with no `id` — the same
    /// `POST /venue {op:"fixture"}` the `set_fixture` tool posts.
    #[test]
    fn adding_a_fixture_from_settings_posts_the_fixture_op() {
        use egui_kittest::kittest::Queryable;
        let door = std::rc::Rc::new(fixture_door());
        let mut h = fixtures_harness(std::rc::Rc::clone(&door));
        h.run();
        h.get_by_label("save").click();
        h.run();
        let posted = posted_fixture(&door);
        assert_eq!(posted["op"], json!("fixture"));
        assert_eq!(posted["venue"], json!("bar"));
        assert_eq!(posted["kind"], json!("wash"));
        assert_eq!(posted["wire"], json!("dmx"));
        assert!(
            posted.get("id").is_none(),
            "adding never posts an id: {posted}"
        );
    }

    /// `edit` seeds the form from the row and the save posts its id, so an
    /// edit rewrites that fixture rather than adding a second.
    #[test]
    fn editing_a_fixture_row_posts_its_id() {
        use egui_kittest::kittest::Queryable;
        let door = std::rc::Rc::new(fixture_door());
        let mut h = fixtures_harness(std::rc::Rc::clone(&door));
        h.run();
        h.get_by_label("edit").click();
        h.run();
        h.get_by_label("save").click();
        h.run();
        let posted = posted_fixture(&door);
        assert_eq!(posted["id"], json!("wash-1"));
        assert_eq!(posted["group"], json!("front"));
        assert_eq!(posted["universe"], json!(1));
    }

    /// F8: the "In graph" column reads the open graph's `venue.fixture`
    /// nodes and matches a row to its own node.
    #[test]
    fn fixture_graph_nodes_reads_the_open_graphs_per_fixture_nodes() {
        let door = fixture_door();
        door.respond(
            "GET",
            "/graph",
            json!({ "graph": { "id": "device-bar", "nodes": [
                { "id": "f-0", "kind": "venue.fixture", "config": { "venue": "bar", "fixture": "wash-1" } },
                { "id": "f-1", "kind": "venue.fixture", "config": { "venue": "bar", "fixture": "gone-9" } },
                { "id": "g", "kind": "venue.fixtures", "config": { "venue": "bar" } }
            ] } }),
        );
        let nodes = fixture_graph_nodes(&door, Some("device-bar"));
        assert_eq!(nodes, vec![
            ("f-0".to_string(), "bar".to_string(), "wash-1".to_string()),
            ("f-1".to_string(), "bar".to_string(), "gone-9".to_string()),
        ]);
        assert!(fixture_graph_nodes(&door, None).is_empty(), "no open graph, no nodes");
    }

    /// F8: with no node for a row, the section offers "add node" and posts the
    /// same `POST /graph/node {op:"add", kind:"venue.fixture"}` the canvas and
    /// the `add_fixture_node` tool post (rule 6).
    #[test]
    fn the_fixtures_section_adds_a_node_from_a_row() {
        use egui_kittest::kittest::Queryable;
        let door = std::rc::Rc::new(fixture_door());
        door.respond("GET", "/graph", json!({ "graph": { "id": "device-bar", "nodes": [] } }));
        let theme = Theme::phosphor();
        let t = theme.clone();
        let d = std::rc::Rc::clone(&door);
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::Vec2 { x: 720.0, y: 560.0 })
            .build_ui_state(
                move |ui, state: &mut PanelState| section_fixtures(ui, &t, d.as_ref(), state, Some("device-bar")),
                PanelState::default(),
            );
        theme.apply(&h.ctx);
        h.run();
        h.get_by_label("add node").click();
        h.run();
        let posted: Value = door
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/graph/node")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .last()
            .expect("the column posts POST /graph/node");
        assert_eq!(posted["op"], json!("add"));
        assert_eq!(posted["kind"], json!("venue.fixture"));
        assert_eq!(posted["graph"], json!("device-bar"));
        assert_eq!(posted["config"]["venue"], json!("bar"));
        assert_eq!(posted["config"]["fixture"], json!("wash-1"));
    }

    // -- F13: the Discover section. Discovery only proposes rows; the
    //    operator accepts each one, and the accept is the one `POST /venue
    //    {op:"fixture"}` the Fixtures page posts (rule 6). Tests never touch
    //    real `data/`: the door is a fake, the venue disk test (in
    //    `call::venue`) builds its own tempdir (rule 8).

    /// A fake ArtPollReply (the shape `GET /artnet/nodes` answers) proposes
    /// exactly one row per output Port-Address, on wire `artnet`.
    #[test]
    fn an_artpoll_reply_proposes_a_row_per_port_address() {
        let reply = json!({ "nodes": [
            { "ip": "2.0.0.9", "short_name": "node", "outputs": [0x0120, 0x0121] },
            { "ip": "2.0.0.10", "short_name": "empty", "outputs": [] },
        ]});
        let rows = artnet_rows(&reply);
        assert_eq!(rows.len(), 2, "one row per port; a portless node proposes none");
        assert_eq!(rows[0].name, "node");
        assert_eq!(rows[0].wire, "artnet");
        assert_eq!(rows[0].universe, 0x0120);
        assert_eq!(rows[1].universe, 0x0121);
    }

    /// A fake sACN discovery reply proposes one row per universe, on wire
    /// `sacn`; a source with no name falls back to its IP.
    #[test]
    fn a_sacn_reply_proposes_a_row_per_universe() {
        let reply = json!({ "sources": [
            { "ip": "2.0.0.5", "name": "room", "universes": [1, 20, 300] },
            { "ip": "2.0.0.6", "name": "", "universes": [2] },
        ]});
        let rows = sacn_rows(&reply);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].wire, "sacn");
        assert_eq!(rows[0].universe, 1);
        assert_eq!(rows[3].name, "2.0.0.6");
        assert_eq!(rows[3].universe, 2);
    }

    /// The body an accept posts names the venue, the discovered wire and
    /// universe, and adds (no `id`) a profile-less `wash`.
    #[test]
    fn a_proposed_row_posts_a_fixture_body() {
        let row = DiscoverRow { name: "node".into(), wire: "artnet", universe: 0x0120 };
        assert_eq!(
            discover_fixture_body("bar", &row),
            json!({ "op": "fixture", "venue": "bar", "kind": "wash",
                    "name": "node", "wire": "artnet", "universe": 0x0120 })
        );
    }

    /// The camera stub's reply is the pending-solve note, not a failure; a
    /// refusal or an empty reply reads honestly too.
    #[test]
    fn the_camera_stub_shows_the_pending_solve_note() {
        let reply = json!({ "ok": true, "draft": { "venue": { "venue": {
            "based_on": ["discover_fixture: pending camera solve (TODO M5)"] } } } });
        assert_eq!(camera_note(&reply), "discover_fixture: pending camera solve (TODO M5)");
        assert_eq!(camera_note(&json!({ "error": "no camera" })), "no camera");
        assert_eq!(camera_note(&json!({})), tr("draft proposed"));
    }

    fn discover_door() -> crate::shell::FakeDoor {
        let door = crate::shell::FakeDoor::new();
        door.respond("GET", "/venue", json!({ "last": "bar" }));
        door.respond(
            "GET",
            "/artnet/nodes",
            json!({ "nodes": [{ "ip": "2.0.0.9", "short_name": "node", "outputs": [0x0120] }] }),
        );
        door
    }

    fn discover_harness(
        door: std::rc::Rc<crate::shell::FakeDoor>,
    ) -> egui_kittest::Harness<'static, PanelState> {
        let theme = Theme::phosphor();
        let t = theme.clone();
        let h = egui_kittest::Harness::builder()
            .with_size(egui::Vec2 { x: 720.0, y: 560.0 })
            .build_ui_state(
                move |ui, state: &mut PanelState| section_discover(ui, &t, door.as_ref(), state),
                PanelState::default(),
            );
        theme.apply(&h.ctx);
        h
    }

    /// The whole Art-Net path through the door: poll (`GET /artnet/nodes`),
    /// then accept the proposed row — which writes the venue row and nothing
    /// else (exactly one `POST`, to `/venue`).
    #[test]
    fn accepting_a_proposal_writes_the_venue_row_and_nothing_else() {
        use egui_kittest::kittest::Queryable;
        let door = std::rc::Rc::new(discover_door());
        let mut h = discover_harness(std::rc::Rc::clone(&door));
        h.run();
        h.get_by_label("find art-net nodes").click();
        h.run();
        h.get_by_label("add as fixture").click();
        h.run();
        let posts: Vec<(String, Value)> = door
            .calls
            .borrow()
            .iter()
            .filter(|(m, _, _)| m == "POST")
            .map(|(_, path, params)| {
                (path.clone(), serde_json::from_slice::<Value>(&params.body).unwrap())
            })
            .collect();
        assert_eq!(posts.len(), 1, "accepting writes exactly once: {posts:?}");
        assert_eq!(posts[0].0, "/venue");
        let body = &posts[0].1;
        assert_eq!(body["op"], json!("fixture"));
        assert_eq!(body["venue"], json!("bar"));
        assert_eq!(body["wire"], json!("artnet"));
        assert_eq!(body["universe"], json!(0x0120));
        assert_eq!(body["kind"], json!("wash"));
        assert_eq!(body["name"], json!("node"));
        assert!(body.get("id").is_none(), "adding never posts an id");
    }

    /// The camera button calls the `discover_fixture` tool through the same
    /// `POST /agent/tool` route the chat uses (rules 6 and 9), allowing the
    /// `ask` policy because the operator pressed it.
    #[test]
    fn the_camera_button_calls_discover_fixture_through_the_door() {
        use egui_kittest::kittest::Queryable;
        let door = std::rc::Rc::new(discover_door());
        let mut h = discover_harness(std::rc::Rc::clone(&door));
        h.run();
        h.get_by_label("discover by flash").click();
        h.run();
        let call: Option<Value> = door
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == AGENT_TOOL_PATH)
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .next();
        let body = call.expect("the camera button posts POST /agent/tool");
        assert_eq!(body["name"], json!("discover_fixture"));
        assert_eq!(body["allow_ask"], json!(true));
    }

    /// The house look is taken once per change: a style the settings already
    /// hold does not fire again, and a new one fires exactly once.
    #[test]
    fn a_style_change_is_taken_once_and_only_when_it_differs() {
        let mut state = PanelState::default();
        state.set_style("phosphor");
        assert_eq!(state.take_style_change().as_deref(), Some("phosphor"), "the first read is a change");
        state.set_style("phosphor");
        assert_eq!(state.take_style_change(), None, "the same style is no change");
        state.set_style("daylight");
        assert_eq!(state.take_style_change().as_deref(), Some("daylight"));
        assert_eq!(state.take_style_change(), None, "taken once");
    }
}
