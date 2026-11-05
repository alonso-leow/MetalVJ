//! The AI chat sidebar (`S.X`): the real conversation surface, and the panel
//! that replaces the old *AI harness* tool panel. It has two views: the
//! **conversation** (the transcript, with the composer docked below) and the
//! **history** (every chat, newest first). A slim header carries the open
//! chat's title and the `new chat` and `history` buttons.
//!
//! **Rule 6, again.** Every byte this panel paints arrives through the door
//! ([`Call::json`]): `GET /ai/chats` for the history, `GET /ai/chats/<id>` for
//! the open conversation, and `POST /ai/chats`, `/ai/chat/delete`, `/ai/ask`,
//! `/ai/chat/cancel`, `/ai/chat/approve` for the gestures. This file never
//! imports `metal_agent` and never names a `crate::call::<submodule>` — gate 1
//! (`gates.rs`) bans both in `shell/`, and neither needs relaxing: the plan's
//! data shape lives on disk (`data/chats/<id>.json`) and the door is its own
//! contract.
//!
//! **What it holds.** The selected chat and the view are the only things kept
//! between frames (`PanelState::chat`, `PanelState::view`): a turn's status
//! changes underneath the poll, and the open conversation must not swim back to
//! the newest one while the operator reads an older one.

use std::time::{SystemTime, UNIX_EPOCH};

use egui::text::LayoutJob;
use egui::{
    Align, Color32, Frame, Id, Label, Layout, Response, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, TextEdit, TextFormat, Ui,
};
use serde_json::{Value, json};

use crate::call::Call;
use crate::shell::classes;
use crate::shell::mention::{self, Candidate};
use crate::i18n::{tr, trf};

use crate::shell::classes::Theme;
use crate::shell::components::{Icon, lucide, panel_frame};

/// A stable id salt for the panel. It is not a [`super::Region`] (never
/// competes for `focus`/`cede` grid space; the chat sidebar sits outside the
/// ceding grid the way the old harness one did), so it gets its own constant
/// rather than a new `Region` variant.
pub const ID: &str = "shell/panel/chat";
/// The panel's accessible name (not a painted heading).
pub const TITLE: &str = "ai";

/// Which of the panel's two states is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// The open transcript, with the composer docked below it.
    #[default]
    Conversation,
    /// Every chat, newest first, with delete and open.
    History,
}

/// Everything this panel remembers between frames: the prompt still being
/// typed, the conversation currently open, the view, and the composer's height
/// last frame (so the transcript can fill the rest). Everything else — the chat
/// list, the transcript and its status — is re-read fresh every frame through
/// the door, the same no-cache trade every other panel in this directory makes
/// for anything that keeps moving whether or not the panel happens to be open.
pub struct PanelState {
    prompt: String,
    chat: Option<String>,
    view: View,
    composer_h: f32,
    /// The ref a chip click parked, for the focus task to consume.
    pub(crate) pending_focus: Option<Value>,
    /// The context chips above the composer: `{kind, id, graph, label}` each,
    /// sent with the next prompt and then cleared.
    pub(crate) context: Vec<Value>,
    /// The `@` candidates the shell offers this frame (the open graph's nodes,
    /// with the focused one marked). Set by the shell before [`paint`].
    pub(crate) sources: Vec<Candidate>,
    /// The caret's byte index as of the last painted frame: the `@` token is
    /// read from here, before the editor is added this frame.
    caret: usize,
    /// A caret index to place once the editor next paints (after an insert).
    pending_caret: Option<usize>,
    /// The highlighted row of the open `@` list.
    mention_sel: usize,
    /// How many times each token was inserted this session (recency).
    mention_uses: std::collections::HashMap<String, u32>,
    /// The node the AI is working on right now, as `(graph, node)`, read from
    /// the open chat's in-flight tool calls and held a moment after they end.
    pub(crate) ai_target: Option<(Option<String>, String)>,
    /// When `ai_target` was last seen in flight (egui time).
    ai_target_at: f64,
    /// `ai_target` moved to a new node since the shell last looked.
    pub(crate) ai_target_moved: bool,
    /// Files dropped from the library onto the composer, as library paths. Shown
    /// as chips above the text area; not yet sent with the prompt.
    pub attachments: Vec<String>,
    /// Set when the composer's `+` is clicked. The caller (which owns the file
    /// picker) must open it and clear this flag.
    pub attach_requested: bool,
    /// Set when the composer's gear is clicked. The caller (which owns the
    /// settings overlay) must open the AI settings section and clear this flag.
    pub open_ai_settings: bool,
}

/// How long the AI's mark stays on a node once its tool call is done, seconds.
const AI_FOCUS_HOLD: f64 = 4.0;

/// The `(graph, node)` the newest live tool call of `messages` names: calls
/// still in flight, or done within a turn that is still running.
fn live_target(messages: &[Value], busy: bool) -> Option<(Option<String>, String)> {
    let last = messages.iter().rev().find(|m| m["role"] == "assistant")?;
    last["tool_calls"].as_array()?.iter().rev().find_map(|c| {
        let status = c["status"].as_str().unwrap_or("");
        let live = matches!(status, "generating" | "generated" | "calling" | "awaiting_approval")
            || (busy && status == "done");
        let node = c["args"]["node"]
            .as_str()
            .or_else(|| c["args"]["id"].as_str())
            .or_else(|| c["args"]["updates"][0]["node"].as_str())
            .filter(|n| !n.is_empty())?;
        live.then(|| (c["args"]["graph"].as_str().map(str::to_string), node.to_string()))
    })
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState {
            prompt: String::new(),
            chat: None,
            view: View::Conversation,
            composer_h: 0.0,
            pending_focus: None,
            context: Vec::new(),
            sources: Vec::new(),
            caret: 0,
            pending_caret: None,
            mention_sel: 0,
            mention_uses: std::collections::HashMap::new(),
            ai_target: None,
            ai_target_at: 0.0,
            ai_target_moved: false,
            attachments: Vec::new(),
            attach_requested: false,
            open_ai_settings: false,
        }
    }
}

impl PanelState {
    /// Test-only writer — the seam a shell test preloads the field through.
    #[cfg(test)]
    pub(crate) fn set_prompt_for_test(&mut self, text: &str) {
        self.prompt = text.to_string();
        self.caret = self.prompt.len();
    }

    /// Test-only reader for the prompt text, as the composer holds it now.
    #[cfg(test)]
    pub(crate) fn prompt_for_test(&self) -> &str {
        &self.prompt
    }
}

/// Paints the sidebar. `selection` is the shell's current selection as a
/// context ref (`{kind, id, graph, label}`), or `None` when nothing is selected;
/// the composer's `@` button attaches it.
pub fn paint(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    selection: Option<&Value>,
) -> Response {
    let _span = crate::perf::Span::new("chat");
    ui.push_id(Id::new(ID), |ui| {
        // The composer sits on the panel's bottom edge: no gray strip below it.
        let mut frame = panel_frame(theme);
        frame.inner_margin.bottom = 0;
        frame.inner_margin.left = 0;
        frame.inner_margin.right = 0;
        let inner = frame.show(ui, |ui| {
            // Every label here wraps to the panel's width. egui's default in a
            // horizontal row is no-wrap, so a long tool summary or bullet would
            // widen the row past the sidebar's clamped width.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.expand_to_include_rect(ui.max_rect());
            // A side panel takes its stored width from its content's rect, so
            // one wide row (a code block, a long tool name) would stretch the
            // panel to its max and the drag handle could not shrink it back.
            // Pin the body to the panel's width and let anything wider scroll
            // sideways instead.
            let width = ui.available_width();
            let height = ui.available_height();
            ScrollArea::horizontal()
                .id_salt("shell/panel/chat/width")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.set_height(height);
                    let full_rect = ui.max_rect();
                    ui.vertical(|ui| {
                        // The click-catcher goes first, so the header buttons and the
                        // rows painted after it win a click on their own rect, and a
                        // stray click elsewhere in the panel still resolves to
                        // *something*.
                        let bg = ui.interact(full_rect, ui.id().with("click-bg"), Sense::click());
                        // The conversation view opens the newest chat when none is
                        // open (a fresh panel, or the open one just deleted).
                        if state.view == View::Conversation && state.chat.is_none() {
                            if let Some(first) = list_chats(door).first() {
                                state.chat = first["id"].as_str().map(str::to_string);
                            }
                        }
                        let chat = state.chat.as_deref().map(|id| read_chat(door, id));
                        let title = match chat.as_ref().and_then(|c| c["title"].as_str()) {
                            Some(t) if !t.is_empty() => t.to_string(),
                            _ if chat.is_some() => tr("untitled").to_string(),
                            _ => tr("new chat").to_string(),
                        };
                        let empty = chat
                            .as_ref()
                            .map_or(true, |c| c["messages"].as_array().map_or(true, |m| m.is_empty()));
                        egui::Frame::new().inner_margin(theme.side_margin()).show(ui, |ui| {
                            header(ui, theme, door, state, &title, empty);
                        });
                        match state.view {
                            View::Conversation => {
                                conversation_view(ui, theme, door, state, chat.as_ref(), selection)
                            }
                            View::History => {
                                egui::Frame::new().inner_margin(theme.side_margin()).show(ui, |ui| {
                                    history_view(ui, theme, door, state)
                                });
                            }
                        }
                        bg
                    })
                    .inner
                })
                .inner
        });
        // The panel's accessible name: no heading is painted any more, so
        // this is what a screen reader (and the kittest suite) finds it by.
        inner.response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Other, true, TITLE)
        });
        inner.inner
    })
    .inner
}

/// A header icon button in the grid cells' style: a square, unrounded frame
/// on the inset fill with the rule outline, and the glyph at 1.4 times the
/// small size so it reads as a target rather than a dot.
/// A frameless composer icon button (`+`, gear): the glyph alone, no outline.
fn composer_icon_button(theme: &Theme, icon: Icon) -> egui::Button<'static> {
    egui::Button::new(lucide(icon, theme.font_small.size * 1.4)).frame(false)
}

/// Paints the send arrow (a shaft with a head) inside `rect`.
fn paint_send_arrow(painter: &egui::Painter, rect: egui::Rect, stroke: Stroke) {
    let c = rect.center();
    let half = rect.height() * 0.3;
    let top = c.y - half;
    let bottom = c.y + half;
    painter.line_segment([egui::pos2(c.x, top), egui::pos2(c.x, bottom)], stroke);
    painter.line_segment([egui::pos2(c.x - half, c.y), egui::pos2(c.x, top)], stroke);
    painter.line_segment([egui::pos2(c.x + half, c.y), egui::pos2(c.x, top)], stroke);
}

fn header_icon_button(theme: &Theme, icon: Icon) -> egui::Button<'static> {
    egui::Button::new(lucide(icon, theme.font_small.size * 1.4))
        .fill(theme.inset)
        .stroke(theme.rule_stroke())
        .corner_radius(theme.sharp_corner())
}

/// Makes every widget drawn in `ui` square-cornered, the composer's field and
/// send button included, so the prompt area matches the grid cells.
fn sharp_widgets(ui: &mut Ui, theme: &Theme) {
    let w = &mut ui.visuals_mut().widgets;
    for v in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        v.corner_radius = theme.sharp_corner();
    }
}

/// The slim header row. In the conversation view: the open chat's title
/// (truncated, weak ink) and the `new chat` and `history` icon buttons. In the
/// history view: a back arrow and the view's name.
fn header(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    title: &str,
    empty: bool,
) {
    ui.horizontal(|ui| {
        let button_w = ui.spacing().interact_size.x * 2.0 + ui.spacing().item_spacing.x;
        match state.view {
            View::Conversation => {
                let btn_w = ui.spacing().interact_size.x + ui.spacing().item_spacing.x;
                let button_w = if empty { btn_w } else { button_w };
                let w = (ui.available_width() - button_w).max(0.0);
                ui.add_sized(
                    [w, ui.spacing().interact_size.y],
                    Label::new(
                        RichText::new(if empty { "" } else { title })
                            .font(theme.font_body.clone())
                            .color(theme.ink)
                            .strong(),
                    )
                    .truncate(),
                );
                if !empty
                    && ui
                        .add(header_icon_button(theme, Icon::New))
                        .on_hover_text(tr("new chat"))
                        .clicked()
                {
                    new_chat(door, state);
                }
                if ui
                    .add(header_icon_button(theme, Icon::History))
                    .on_hover_text(tr("history"))
                    .clicked()
                {
                    state.view = View::History;
                }
            }
            View::History => {
                if ui
                    .button(lucide(Icon::ArrowLeft, theme.font_small.size))
                    .on_hover_text(tr("back"))
                    .clicked()
                {
                    state.view = View::Conversation;
                }
                ui.label(
                    RichText::new(tr("history"))
                        .font(theme.font_body.clone())
                        .color(theme.ink),
                );
            }
        }
    });
}

/// The conversation view: the transcript fills every row the composer does not
/// take, and the composer is docked at the panel's bottom edge. The composer's
/// height is the one measured last frame, so the split settles after a frame —
/// and it is a real footer even when no chat is open, so the field and its
/// buttons are always at the bottom rather than floating under the header.
fn conversation_view(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    chat: Option<&Value>,
    selection: Option<&Value>,
) {
    let area = ui.available_rect_before_wrap();
    let reserve = state
        .composer_h
        .max(ui.spacing().interact_size.y)
        .min(area.height());
    let split = area.max.y - reserve;
    let side = theme.pad as f32;
    let content = egui::Rect::from_min_max(
        egui::pos2(area.min.x + side, area.min.y),
        egui::pos2(area.max.x - side, split),
    );
    // The footer runs edge to edge.
    let footer = egui::Rect::from_min_max(egui::pos2(area.min.x, split), area.max);

    ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| match chat {
        Some(chat) => chat_container(ui, theme, door, state, chat, ui.available_height()),
        None => {}
    });
    let mut used = 0.0;
    ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| {
        let top = ui.cursor().top();
        prompt_box(ui, theme, door, state, chat, selection);
        used = ui.cursor().top() - top;
    });
    state.composer_h = used;
}

/// The history view: every chat from `GET /ai/chats`, newest first, one card
/// each. With no chats it says so and offers `new chat`.
fn history_view(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    let chats = list_chats(door);
    if chats.is_empty() {
        ui.label(
            RichText::new(tr("no chats yet"))
                .font(theme.font_body.clone())
                .color(theme.ink_weak),
        );
        if ui
            .button(RichText::new(tr("new chat")).font(theme.font_body.clone()))
            .clicked()
        {
            new_chat(door, state);
        }
        return;
    }
    let motion_id = Id::new("shell/motion/chat-history");
    crate::shell::motion::begin(ui.ctx(), motion_id, crate::shell::motion::key_of(chats.len()), false);
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("shell/panel/chat/history")
        .show(ui, |ui| {
            for chat in &chats {
                crate::shell::motion::scoped(ui, |ui| history_row(ui, theme, door, state, chat));
            }
        });
    crate::shell::motion::end(ui.ctx(), motion_id);
}

/// One history card: the title, the preview (one line, weak), a meta line with
/// the relative date and the message count, and the trash button. Clicking the
/// text opens the chat and switches to the conversation view; the trash posts
/// `POST /ai/chat/delete` and the row is simply gone from the next reply.
fn history_row(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState, chat: &Value) {
    let id = chat["id"].as_str().unwrap_or("?").to_string();
    let title = chat["title"]
        .as_str()
        .filter(|t| !t.is_empty())
        .unwrap_or(tr("untitled"))
        .to_string();
    let preview = chat["preview"].as_str().unwrap_or("").to_string();
    let mut meta: Vec<String> = Vec::new();
    if let Some(then) = chat["updated"].as_u64() {
        meta.push(ago_text(&ago(now_secs(), then)));
    }
    if let Some(count) = chat["count"].as_u64() {
        meta.push(trf("{n} messages", &[("n", &count)]));
    }

    ui.push_id(Id::new(("shell/panel/chat/history", id.as_str())), |ui| {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(width, theme.library_card_h),
            Sense::hover(),
        );
        let open = ui.interact(rect, ui.id().with("open"), Sense::click());
        let fill = if open.hovered() {
            theme.library_card_hover
        } else {
            theme.inset
        };
        ui.painter().rect_filled(rect, theme.sharp_corner(), fill);
        let trash = egui::Rect::from_min_size(
            egui::pos2(rect.right() - theme.row_menu_width, rect.top()),
            egui::vec2(theme.row_menu_width, rect.height()),
        );
        let text = egui::Rect::from_min_max(
            egui::pos2(rect.left() + theme.library_card_pad.x, rect.top()),
            egui::pos2(trash.left(), rect.bottom()),
        );
        let sub = if preview.is_empty() {
            meta.join(" · ")
        } else {
            format!("{} · {}", meta.join(" · "), preview)
        };
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(text)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        child.spacing_mut().item_spacing.y = theme.library_detail_gap;
        let block = ui.ctx().fonts_mut(|f| {
            f.row_height(&theme.font_body) + theme.library_detail_gap + f.row_height(&theme.font_small)
        });
        child.add_space(((rect.height() - block) / 2.0).max(0.0));
        child.add(
            Label::new(RichText::new(&title).font(theme.font_body.clone()).color(theme.ink))
                .truncate()
                .selectable(false),
        );
        child.add(
            Label::new(RichText::new(sub).font(theme.font_small.clone()).color(theme.ink_weak))
                .truncate()
                .selectable(false),
        );
        if open.clicked() {
            state.chat = Some(id.clone());
            state.view = View::Conversation;
        }
        let mut cell = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(trash)
                .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        if cell
            .button(lucide(Icon::Trash, theme.font_small.size))
            .on_hover_text(tr("delete chat"))
            .clicked()
        {
            let _ = door.json("POST", "/ai/chat/delete", Some(json!({ "id": id })));
            if state.chat.as_deref() == Some(id.as_str()) {
                state.chat = None;
            }
        }
    });
    ui.add_space(theme.item_gap.y);
}

/// Opens a fresh conversation (`POST /ai/chats`) and shows it.
fn new_chat(door: &dyn Call, state: &mut PanelState) {
    if let Some(id) = door
        .json("POST", "/ai/chats", None)
        .ok()
        .and_then(|c| c["id"].as_str().map(str::to_string))
    {
        state.chat = Some(id);
        state.view = View::Conversation;
    }
}

/// `GET /ai/chats`, the `chats` array (empty when the door cannot answer).
fn list_chats(door: &dyn Call) -> Vec<Value> {
    door.json("GET", "/ai/chats", None)
        .unwrap_or_else(|_| json!({}))["chats"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `GET /ai/chats/<id>`, or an idle stand-in when the door cannot answer (the
/// chat was just deleted under us).
fn read_chat(door: &dyn Call, id: &str) -> Value {
    door.json("GET", &format!("/ai/chats/{id}"), None)
        .unwrap_or_else(|_| json!({ "id": id, "status": "idle", "messages": [] }))
}

/// Every message of the open conversation, one row each, in a scroll area of
/// `height` that sticks to the newest message. An empty chat says so. A busy
/// turn shows a spinner and `working…` at the foot. There is no stop button
/// here: cancelling belongs to the composer (see [`cancel_turn`]).
fn chat_container(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    chat: &Value,
    height: f32,
) {
    let busy = chat["status"].as_str() == Some("busy");
    let messages = chat["messages"].as_array().cloned().unwrap_or_default();

    let waiting: Vec<super::proposals::Pending> = messages
        .iter()
        .filter_map(|m| m["tool_calls"].as_array())
        .flatten()
        .filter(|c| c["status"].as_str() == Some("awaiting_approval"))
        .map(|c| super::proposals::Pending {
            id: c["id"].as_str().unwrap_or_default().to_string(),
            name: c["name"].as_str().unwrap_or_default().to_string(),
            args: c["args"].clone(),
        })
        .collect();
    super::proposals::publish(ui.ctx(), waiting);

    let now = ui.input(|i| i.time);
    match live_target(&messages, busy) {
        Some(t) => {
            if state.ai_target.as_ref() != Some(&t) {
                state.ai_target_moved = true;
                state.ai_target = Some(t);
            }
            state.ai_target_at = now;
        }
        None if state.ai_target.is_some() => {
            if now - state.ai_target_at > AI_FOCUS_HOLD {
                state.ai_target = None;
            } else {
                crate::shell::frames::repaint_after(ui.ctx(), std::time::Duration::from_millis(500));
            }
        }
        None => {}
    }

    ScrollArea::vertical()
        .max_height(height)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .id_salt("shell/panel/chat/transcript")
        .show(ui, |ui| {
            if messages.is_empty() {
                ui.label(
                    RichText::new(tr("no messages yet"))
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                );
            }
            for message in &messages {
                message_row(ui, theme, door, state, message);
            }
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new(tr("working…"))
                            .font(theme.font_small.clone())
                            .color(theme.ink_weak),
                    );
                });
            }
        });
}

/// Cancels the open chat's running turn (`POST /ai/chat/cancel`). The composer's
/// send/stop toggle is its caller.
pub(crate) fn cancel_turn(door: &dyn Call, chat_id: &str) {
    let _ = door.json("POST", "/ai/chat/cancel", Some(json!({ "id": chat_id })));
}

/// One transcript row, by role. The operator's text is a right-leaning filled
/// bubble; the assistant's is plain flowing text (with markdown blocks), its
/// tool calls as compact cards and its `refs` as chips. Notices are small and
/// centred. `tool` rows are skipped — their result rides in the card.
fn message_row(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    message: &Value,
) {
    let content = message["content"].as_str().unwrap_or("");
    match message["role"].as_str().unwrap_or("?") {
        "user" => {
            ui.with_layout(Layout::top_down(Align::Max), |ui| {
                let cap = ui.available_width() * 0.85;
                Frame::new()
                    .fill(theme.tube_raised)
                    .stroke(theme.rule_stroke())
                    .corner_radius(theme.corner_radius())
                    .inner_margin(theme.panel_margin())
                    .show(ui, |ui| {
                        ui.set_max_width(cap);
                        ui.label(inline_job(theme, content, theme.ink));
                    });
            });
        }
        "assistant" => {
            paint_markdown(ui, theme, content, theme.ink);
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    tool_call_card(ui, theme, door, call);
                }
            }
            if let Some(refs) = message["refs"].as_array().filter(|r| !r.is_empty()) {
                ref_chips(ui, theme, state, refs);
            }
        }
        "notice" => {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(content)
                        .font(theme.font_small.clone())
                        .color(theme.ink_faint),
                );
            });
        }
        // `tool` (also `system` and anything unknown): nothing standalone.
        _ => return,
    }
    ui.add_space(ui.spacing().item_spacing.y * 2.0);
}

/// The clickable chips under an assistant row: one small framed button per
/// ref, labelled by its `label` (else its `id`). A click parks the ref in
/// `pending_focus` for the focus task to consume.
fn ref_chips(ui: &mut Ui, theme: &Theme, state: &mut PanelState, refs: &[Value]) {
    ui.horizontal_wrapped(|ui| {
        for r in refs {
            let label = r["label"]
                .as_str()
                .or_else(|| r["id"].as_str())
                .map(str::to_string)
                .unwrap_or_else(|| r.to_string());
            if crate::shell::components::ai_button(ui, theme, &label).clicked() {
                state.pending_focus = Some(r.clone());
            }
        }
    });
}

/// One tool call: a single compact row — a status dot in the status colour, the
/// tool name and a short argument summary — collapsed by default. Opening it
/// shows the args (pretty JSON), the output and any error. While the call waits
/// on the operator (`awaiting_approval`) the allow/deny pair stays visible
/// outside the fold.
fn tool_call_card(ui: &mut Ui, theme: &Theme, door: &dyn Call, call: &Value) {
    let name = call["name"].as_str().unwrap_or("?").to_string();
    let id = call["id"].as_str().unwrap_or_default().to_string();
    let status = call["status"].as_str().unwrap_or("");
    let args = call.get("args").filter(|a| !a.is_null());

    let mut head = LayoutJob::default();
    head.append(
        "● ",
        0.0,
        TextFormat::simple(theme.font_small.clone(), status_color(theme, status)),
    );
    head.append(
        &name,
        0.0,
        TextFormat::simple(theme.font_small.clone(), theme.ai),
    );
    let summary = arg_summary(args);
    if !summary.is_empty() {
        head.append(
            &format!("  {summary}"),
            0.0,
            TextFormat::simple(theme.font_small.clone(), theme.ink),
        );
    }

    ui.push_id(Id::new(("shell/panel/chat/call", id.as_str())), |ui| {
        egui::CollapsingHeader::new(head)
            .id_salt(("shell/panel/chat/fold", id.as_str()))
            .default_open(false)
            .show(ui, |ui| {
                if let Some(args) = args {
                    ui.label(tr("args"));
                    let pretty = serde_json::to_string_pretty(args).unwrap_or_default();
                    code_block(ui, theme, "json", &pretty);
                }
                if let Some(output) = call["output"].as_str().filter(|o| !o.is_empty()) {
                    ui.label(tr("output"));
                    code_block(ui, theme, "", output);
                }
                if let Some(error) = call["error"].as_str().filter(|e| !e.is_empty()) {
                    ui.label(
                        RichText::new(format!("{}: {error}", tr("error")))
                            .font(theme.font_small.clone())
                            .color(theme.warn),
                    );
                }
            });

        if status == "awaiting_approval" {
            // The change the call would make, before the decision.
            let pending = super::proposals::Pending { id: id.clone(), name: name.clone(), args: call["args"].clone() };
            if let Some(node) = super::proposals::node_of(&pending) {
                let rows = super::proposals::changes(&pending, &node, &Value::Null);
                super::proposals::change_rows(ui, theme, &rows);
                if let Some(src) = super::proposals::script_source(&pending, &node) {
                    let old = fetch_script(door, &pending);
                    super::proposals::diff_view(ui, theme, &super::proposals::diff_lines(&old, &src));
                }
            }
            ui.horizontal(|ui| {
                if crate::shell::components::ai_button(ui, theme, &tr("allow")).clicked() {
                    let _ = door.json(
                        "POST",
                        "/ai/chat/approve",
                        Some(json!({ "call": id, "allow": true })),
                    );
                }
                if crate::shell::components::ai_button(ui, theme, &tr("deny")).clicked() {
                    let _ = door.json(
                        "POST",
                        "/ai/chat/approve",
                        Some(json!({ "call": id, "allow": false })),
                    );
                }
            });
        }
    });
}

/// The script a waiting `save_script` would replace (empty when unreadable).
fn fetch_script(door: &dyn Call, p: &super::proposals::Pending) -> String {
    let (Some(node), graph) = (p.args["node"].as_str(), p.args["graph"].as_str().unwrap_or_default()) else {
        return String::new();
    };
    door.bytes("GET", &format!("/script/{node}"), &[("graph", graph.to_string())])
        .ok()
        .and_then(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .and_then(|v| v["source"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A short `key=value` summary of a call's args, cut to [`SUMMARY_CHARS`].
fn arg_summary(args: Option<&Value>) -> String {
    let Some(Value::Object(map)) = args else {
        return String::new();
    };
    let joined = map
        .iter()
        .map(|(k, v)| match v {
            Value::String(t) => format!("{k}={t}"),
            other => format!("{k}={other}"),
        })
        .collect::<Vec<_>>()
        .join(" ");
    if joined.chars().count() > SUMMARY_CHARS {
        let cut: String = joined.chars().take(SUMMARY_CHARS).collect();
        format!("{cut}…")
    } else {
        joined
    }
}

/// How many characters of argument summary a tool row shows.
const SUMMARY_CHARS: usize = 48;

/// The chip colour for a tool call's `status`, read off the theme — never a
/// literal: in flight in the weak ink, waiting on the operator in the amber,
/// running in the phosphor, done in the ok green, errored in the amber,
/// canceled in the faint ink.
fn status_color(theme: &Theme, status: &str) -> egui::Color32 {
    match status {
        "generating" | "generated" => theme.ink_weak,
        "awaiting_approval" => theme.warn,
        "calling" => theme.phosphor,
        "done" => theme.ok,
        "errored" => theme.warn,
        "canceled" => theme.ink_faint,
        _ => theme.ink_weak,
    }
}

/// Paints markdown-lite `text`: paragraphs, fenced code boxes and bullet lists.
fn paint_markdown(ui: &mut Ui, theme: &Theme, text: &str, colour: Color32) {
    for block in parse_blocks(text) {
        match block {
            Block::Para(p) => {
                ui.label(inline_job(theme, &p, colour));
            }
            Block::Code { lang, body } => code_block(ui, theme, &lang, &body),
            Block::List(items) => {
                ui.vertical(|ui| {
                    for item in &items {
                        ui.horizontal_top(|ui| {
                            ui.label(
                                RichText::new("•")
                                    .font(theme.font_body.clone())
                                    .color(theme.ink_weak),
                            );
                            ui.label(inline_job(theme, item, colour));
                        });
                    }
                });
            }
        }
    }
}

/// A fenced code box: a framed, recessed monospace body under a header row
/// with the language (if any) and a copy button.
fn code_block(ui: &mut Ui, theme: &Theme, lang: &str, body: &str) {
    Frame::new()
        .fill(theme.inset)
        .stroke(theme.rule_stroke())
        .corner_radius(theme.corner_radius())
        .inner_margin(theme.panel_margin())
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if !lang.is_empty() {
                    ui.label(
                        RichText::new(lang)
                            .font(theme.font_small.clone())
                            .color(theme.ink_faint),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .small_button(RichText::new(tr("copy")).font(theme.font_small.clone()))
                        .clicked()
                    {
                        ui.ctx().copy_text(body.to_string());
                    }
                });
            });
            ui.label(
                RichText::new(body)
                    .font(theme.font_mono(theme.font_small.size))
                    .color(theme.ink_weak),
            );
        });
}

/// A single inline-markdown run as one [`LayoutJob`]: `code` in the monospace
/// on the inset, **bold** in the AI blue, the rest in `base`.
fn inline_job(theme: &Theme, text: &str, base: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let body = theme.font_body.clone();
    for part in parse_inline(text) {
        match part {
            Inline::Text(t) => job.append(&t, 0.0, TextFormat::simple(body.clone(), base)),
            Inline::Bold(t) => job.append(
                &t,
                0.0,
                TextFormat::simple(body.clone(), theme.ai),
            ),
            Inline::Code(t) => job.append(
                &t,
                0.0,
                TextFormat {
                    font_id: theme.font_mono(body.size),
                    color: theme.ink,
                    background: theme.inset,
                    ..Default::default()
                },
            ),
        }
    }
    job
}

/// One block of markdown-lite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Consecutive prose lines (joined with `\n`).
    Para(String),
    /// A fenced block: its info string (`lang`) and its body, lines joined.
    Code { lang: String, body: String },
    /// A run of `-` (or `*`) bullet items, without their markers.
    List(Vec<String>),
}

/// One run of inline markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Code(String),
    Bold(String),
}

/// Splits markdown-lite into blocks. A ``` fence opens a code block that runs
/// to the next fence (or to the end, if never closed); `-`/`*` lines form a
/// list; blank lines end a paragraph or list; anything else is prose.
pub fn parse_blocks(src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut list: Vec<String> = Vec::new();
    let mut code: Option<(String, Vec<String>)> = None;
    for line in src.lines() {
        let trimmed = line.trim_start();
        if code.is_some() {
            if trimmed.starts_with("```") {
                if let Some((lang, body)) = code.take() {
                    out.push(Block::Code {
                        lang,
                        body: body.join("\n"),
                    });
                }
            } else if let Some((_, body)) = code.as_mut() {
                body.push(line.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("```") {
            flush(&mut para, &mut list, &mut out);
            code = Some((rest.trim().to_string(), Vec::new()));
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            flush_para(&mut para, &mut out);
            list.push(item.trim().to_string());
        } else if trimmed.is_empty() {
            flush(&mut para, &mut list, &mut out);
        } else {
            flush_list(&mut list, &mut out);
            para.push(line.to_string());
        }
    }
    if let Some((lang, body)) = code.take() {
        out.push(Block::Code {
            lang,
            body: body.join("\n"),
        });
    }
    flush(&mut para, &mut list, &mut out);
    out
}

fn flush(para: &mut Vec<String>, list: &mut Vec<String>, out: &mut Vec<Block>) {
    flush_para(para, out);
    flush_list(list, out);
}

fn flush_para(para: &mut Vec<String>, out: &mut Vec<Block>) {
    if !para.is_empty() {
        out.push(Block::Para(para.join("\n")));
        para.clear();
    }
}

fn flush_list(list: &mut Vec<String>, out: &mut Vec<Block>) {
    if !list.is_empty() {
        out.push(Block::List(std::mem::take(list)));
    }
}

/// Splits one line of prose into inline runs: `code` and **bold** spans; an
/// unmatched marker stays as plain text.
pub fn parse_inline(s: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        let span = if let Some(r) = rest.strip_prefix('`') {
            r.find('`')
                .map(|end| (Inline::Code(r[..end].to_string()), &r[end + 1..]))
        } else if let Some(r) = rest.strip_prefix("**") {
            r.find("**")
                .map(|end| (Inline::Bold(r[..end].to_string()), &r[end + 2..]))
        } else {
            None
        };
        match span {
            Some((inline, tail)) => {
                if !text.is_empty() {
                    out.push(Inline::Text(std::mem::take(&mut text)));
                }
                out.push(inline);
                rest = tail;
            }
            None => {
                let ch = rest.chars().next().unwrap_or_default();
                text.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    if !text.is_empty() {
        out.push(Inline::Text(text));
    }
    out
}

/// The id of the prompt's `TextEdit`, so [`prompt_box`] can ask whether the
/// editor holds focus before the editor is added this frame.
const PROMPT_ID: &str = "shell/panel/chat/prompt";

/// The most rows the composer grows to before it scrolls.
const COMPOSER_MAX_ROWS: usize = 12;

/// The fewest rows the composer shows: twice the old single line.
const COMPOSER_MIN_ROWS: usize = 2;

/// The composer: one framed box docked under the transcript. Above the text
/// area, the context and attachment chips. The text area fills the box's width
/// with no frame of its own; below it, a bottom bar in the same box holds `+`
/// (attach files, sets `attach_requested`), the gear (AI settings, sets
/// `open_ai_settings`) and the send button, which turns into `stop` while the
/// chat is busy or awaiting. A library file dragged onto the box is added to
/// `attachments`. The field grows with its lines (1 to [`COMPOSER_MAX_ROWS`])
/// and scrolls past that. Enter sends (the same path as the button,
/// [`send_prompt`]); Shift+Enter falls through to the editor as a newline. The
/// Enter key is consumed *before* the editor is added, so the newline it would
/// otherwise insert never lands.
fn prompt_box(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    chat: Option<&Value>,
    _selection: Option<&Value>,
) {
    let prompt_id = Id::new(PROMPT_ID);
    let focused = ui.memory(|m| m.has_focus(prompt_id));
    // While an `@` token is open, Tab and Enter insert the highlighted
    // candidate and the arrows move through the list; the keys are consumed
    // here so the editor and the send path never see them.
    let mentions = if focused { open_mentions(state) } else { Vec::new() };
    let mut mention_key = false;
    if !mentions.is_empty() {
        let (down, up, tab, enter_key) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        let n = mentions.len();
        if down {
            state.mention_sel = (state.mention_sel + 1) % n;
        }
        if up {
            state.mention_sel = (state.mention_sel + n - 1) % n;
        }
        if tab || enter_key {
            let token = mentions[state.mention_sel.min(n - 1)].token.clone();
            insert_mention(state, &token);
            mention_key = true;
        }
    }
    let enter = !mention_key
        && focused
        && ui.input_mut(|i| {
            !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
        });
    if enter {
        send_prompt(door, state);
    }

    let busy = chat.is_some_and(|c| matches!(c["status"].as_str(), Some("busy" | "awaiting")));
    let chat_id = chat.and_then(|c| c["id"].as_str()).map(str::to_string);
    let rows = state.prompt.split('\n').count().clamp(COMPOSER_MIN_ROWS, COMPOSER_MAX_ROWS);
    let max_h = ui.text_style_height(&egui::TextStyle::Body) * COMPOSER_MAX_ROWS as f32
        + theme.field_pad.y * 2.0;

    let stroke_frame = Frame::new()
        .fill(theme.inset)
        .stroke(theme.rule_stroke())
        .corner_radius(theme.sharp_corner())
        .inner_margin(theme.panel_margin());
    let boxed = stroke_frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        context_chips(ui, theme, state);
        attachment_chips(ui, theme, state);
        if focused && !mentions.is_empty() {
            mention_list(ui, theme, state, &mentions);
        }
        let spacing = ui.spacing().item_spacing.x;

        // The text area: full width, no frame or fill of its own, so the box is
        // the one surface and the text is its content.
        if let Some(at) = state.pending_caret.take() {
            place_caret(ui.ctx(), prompt_id, &state.prompt[..at]);
        }
        let btn_w = ui.spacing().interact_size.x;
        let bar_h = ui.spacing().interact_size.y;
        // The row follows the panel's width; the field takes what the button column leaves.
        let row_w = ui.available_width();
        let field_w = (row_w - btn_w - spacing).max(0.0);
        let margin = theme.composer_margin();
        let text_w = (field_w - (margin.left + margin.right) as f32).max(0.0);
        // The row is one allocation of the panel's width; both columns are
        // children placed inside it, so neither can widen the box.
        let (row, _) = ui.allocate_exact_size(egui::vec2(row_w, max_h), Sense::hover());
        {
            let origin = row.min;
            // Each column gets its own disjoint rect, so the field never
            // covers the `+`/gear column and their clicks always land.
            let field_rect = egui::Rect::from_min_size(origin, egui::vec2(field_w, max_h));
            let mut field_ui = ui.new_child(egui::UiBuilder::new().max_rect(field_rect));
            {
                let ui = &mut field_ui;
                ScrollArea::vertical()
                    .max_height(max_h)
                    .auto_shrink([false, true])
                    .id_salt("shell/panel/chat/composer")
                    .show(ui, |ui| {
                        sharp_widgets(ui, theme);
                        let out = TextEdit::multiline(&mut state.prompt)
                            .id(prompt_id)
                            .frame(Frame::NONE)
                            .desired_width(text_w)
                            .margin(margin)
                            .desired_rows(rows)
                            .hint_text(tr("ask about the graph…"))
                            .show(ui);
                        // The caret, as a byte index, for the next frame's `@`.
                        if let Some(range) = out.cursor_range {
                            let chars = range.primary.index.0;
                            state.caret = state
                                .prompt
                                .char_indices()
                                .nth(chars)
                                .map_or(state.prompt.len(), |(b, _)| b);
                        }
                    });
            }
            // The right column: `+` over the gear, frameless.
            let col_rect = egui::Rect::from_min_size(
                egui::pos2(origin.x + field_w + spacing, origin.y),
                egui::vec2(btn_w, bar_h * 2.0),
            );
            let mut col_ui = ui.new_child(egui::UiBuilder::new().max_rect(col_rect));
            {
                let ui = &mut col_ui;
                let plus_label = tr("attach files");
                let plus = ui
                    .add_sized([btn_w, bar_h], composer_icon_button(theme, Icon::Plus))
                    .on_hover_text(plus_label);
                plus.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, plus_label)
                });
                if plus.clicked() {
                    // The real file picker lives with the caller, which must read and clear this.
                    state.attach_requested = true;
                }

                let gear_label = tr("AI settings");
                let gear = ui
                    .add_sized([btn_w, bar_h], composer_icon_button(theme, Icon::Settings))
                    .on_hover_text(gear_label);
                gear.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, gear_label)
                });
                if gear.clicked() {
                    // The settings overlay lives with the caller, which must read and clear this.
                    state.open_ai_settings = true;
                }
            }
        }

        // The bottom bar, inside the same box: the send/stop control at the right edge.
        let (bar, _) = ui.allocate_exact_size(egui::vec2(row_w, bar_h), Sense::hover());
        let send_rect = egui::Rect::from_min_size(
            egui::pos2(bar.max.x - btn_w, bar.min.y),
            egui::vec2(btn_w, bar_h),
        );

        // The send/stop control sits at the right edge of the bar.
        if busy {
            let stop = ui
                .put(
                    send_rect,
                    egui::Button::new(
                        RichText::new("■")
                            .font(theme.font_body.clone())
                            .color(theme.warn),
                    ),
                )
                .on_hover_text(tr("stop"));
            if stop.clicked() {
                if let Some(id) = chat_id.as_deref() {
                    cancel_turn(door, id);
                }
            }
        } else {
            // A white CTA with a dark arrow and no outline; it turns blue with a
            // white arrow on hover.
            let hot = ui.rect_contains_pointer(send_rect);
            let ink = if hot { Color32::WHITE } else { theme.hard_ink() };
            let send = ui
                .scope(|ui| {
                    sharp_widgets(ui, theme);
                    let w = &mut ui.visuals_mut().widgets.hovered;
                    w.weak_bg_fill = theme.ai;
                    w.bg_fill = theme.ai;
                    w.bg_stroke = Stroke::NONE;
                    let resp = ui.put(
                        send_rect,
                        egui::Button::new("")
                            .fill(if hot { theme.ai } else { Color32::WHITE })
                            .stroke(Stroke::NONE),
                    );
                    paint_send_arrow(ui.painter(), resp.rect, theme.send_glyph_stroke(ink));
                    resp
                })
                .inner
                .on_hover_text(tr("send"));
            // The glyph is the paint; `send` is the button's accessible
            // name, so the harness (and a screen reader) finds it.
            send.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr("send"))
            });
            if send.clicked() {
                send_prompt(door, state);
            }
        }
    });

    // Library drags: the box lights its outline while a library file hovers it,
    // and a release adds the file to the attachments. The drop catcher is
    // registered after the box's content, so it sits on top for hover only.
    let rect = boxed.response.rect;
    let drop = ui.interact(rect, Id::new(PROMPT_ID).with("composer-drop"), Sense::hover());
    if drop.dnd_hover_payload::<crate::shell::panels::FileDrag>().is_some() {
        ui.painter()
            .rect_stroke(rect, theme.sharp_corner(), theme.ai_stroke(), StrokeKind::Inside);
    }
    if let Some(file) = crate::shell::panels::release_typed::<crate::shell::panels::FileDrag>(&drop) {
        accept_library_drop(state, &file.path);
    }
}

/// Takes a file dropped from the library onto the composer. A path already
/// attached is not added twice.
fn accept_library_drop(state: &mut PanelState, path: &str) {
    if !path.is_empty() && !state.attachments.iter().any(|p| p == path) {
        state.attachments.push(path.to_string());
    }
}

/// The attachment chips above the field: one chip per dropped file, labelled
/// with its file name, with an ✕ that drops it. Empty when none is attached.
fn attachment_chips(ui: &mut Ui, theme: &Theme, state: &mut PanelState) {
    if state.attachments.is_empty() {
        return;
    }
    let labels: Vec<String> = state
        .attachments
        .iter()
        .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string())
        .collect();
    if let Some(i) = chip_row(ui, theme, &labels) {
        state.attachments.remove(i);
    }
}

/// A wrapped row of framed chips, one per label, each with an ✕. Returns the
/// index whose ✕ was clicked this frame, if any.
fn chip_row(ui: &mut Ui, theme: &Theme, labels: &[String]) -> Option<usize> {
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (i, label) in labels.iter().enumerate() {
            Frame::new()
                .fill(theme.tube_raised)
                .stroke(theme.rule_stroke())
                .corner_radius(theme.corner_radius())
                .inner_margin(theme.panel_margin())
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(label.as_str())
                                .font(theme.font_small.clone())
                                .color(theme.ink),
                        );
                        if ui
                            .small_button(lucide(Icon::Close, theme.font_small.size))
                            .on_hover_text(tr("remove"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                });
        }
    });
    remove
}

/// The context chips above the field: one framed chip per ref, labelled, with
/// an ✕ that drops it. Empty when nothing is attached.
fn context_chips(ui: &mut Ui, theme: &Theme, state: &mut PanelState) {
    if state.context.is_empty() {
        return;
    }
    let labels: Vec<String> = state
        .context
        .iter()
        .map(|chip| {
            chip["label"]
                .as_str()
                .or_else(|| chip["id"].as_str())
                .unwrap_or("?")
                .to_string()
        })
        .collect();
    if let Some(i) = chip_row(ui, theme, &labels) {
        state.context.remove(i);
    }
}

/// Posts the prompt as one turn — `POST /ai/ask {chat, prompt, context}` — and
/// clears the field and the context chips. A blank or whitespace-only prompt
/// sends nothing and keeps the field as it is. Returns whether a turn was sent.
/// The candidates the open `@` token matches, best first, with the session's
/// recency filled in. Empty when no `@` token is open under the caret.
fn open_mentions(state: &PanelState) -> Vec<Candidate> {
    let caret = state.caret.min(state.prompt.len());
    let Some(m) = mention::active(&state.prompt, caret) else {
        return Vec::new();
    };
    let mut sources = state.sources.clone();
    for c in &mut sources {
        c.recent = state.mention_uses.get(&c.token).copied().unwrap_or(0);
    }
    mention::rank(&m.query, &sources).into_iter().cloned().collect()
}

/// Replaces the open `@` token with `@token ` and remembers the insert for the
/// recency ranking; the caret moves to just after it on the next frame.
fn insert_mention(state: &mut PanelState, token: &str) {
    let caret = state.caret.min(state.prompt.len());
    let Some(m) = mention::active(&state.prompt, caret) else {
        return;
    };
    let (text, after) = mention::splice(&state.prompt, &m, caret, token);
    state.prompt = text;
    state.caret = after;
    state.pending_caret = Some(after);
    state.mention_sel = 0;
    *state.mention_uses.entry(token.to_string()).or_default() += 1;
}

/// The completion list: one row per candidate (up to [`classes::MENTION_ROWS`]),
/// the highlighted row first. A click inserts the row.
fn mention_list(ui: &mut Ui, theme: &Theme, state: &mut PanelState, mentions: &[Candidate]) {
    let sel = state.mention_sel.min(mentions.len().saturating_sub(1));
    let mut chosen = None;
    let list = ui
        .vertical(|ui| {
            for (i, c) in mentions.iter().take(classes::MENTION_ROWS).enumerate() {
                let text = format!("{}  {}", c.label, kind_word(c.kind));
                let row = ui.selectable_label(
                    i == sel,
                    RichText::new(text).font(theme.font_body.clone()),
                );
                if row.clicked() {
                    chosen = Some(c.token.clone());
                }
            }
        })
        .response;
    list.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, tr("mention"))
    });
    if let Some(token) = chosen {
        insert_mention(state, &token);
    }
}

fn kind_word(kind: mention::Kind) -> String {
    match kind {
        mention::Kind::Node => tr("mention node").to_string(),
        mention::Kind::Property => tr("mention property").to_string(),
        mention::Kind::File => tr("mention file").to_string(),
        mention::Kind::Overlay => tr("mention overlay").to_string(),
    }
}

/// Moves the editor's caret to char index of `text` (the end of `text`).
fn place_caret(ctx: &egui::Context, id: Id, text: &str) {
    use egui::text::{CCursor, CCursorRange};
    let mut edit = TextEdit::load_state(ctx, id).unwrap_or_default();
    let at = text.chars().count();
    edit.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(at))));
    edit.store(ctx, id);
}

fn send_prompt(door: &dyn Call, state: &mut PanelState) -> bool {
    if !prompt_is_sendable(&state.prompt) {
        return false;
    }
    let mut prompt = state.prompt.clone();
    for path in state.attachments.drain(..) {
        prompt.push_str("\nAdjunto: ");
        prompt.push_str(&path);
    }
    let _ = door.json(
        "POST",
        "/ai/ask",
        Some(json!({
            "chat": state.chat.clone(),
            "prompt": prompt,
            "context": state.context.clone(),
        })),
    );
    state.prompt.clear();
    state.context.clear();
    true
}

/// The send decision on its own: a prompt is worth a turn only when it has
/// something other than whitespace in it.
fn prompt_is_sendable(prompt: &str) -> bool {
    !prompt.trim().is_empty()
}

/// The current time in whole Unix seconds (`0` if the clock is before 1970).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How long ago a chat was last touched, in the coarsest unit that reads true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ago {
    Now,
    Minutes(u64),
    Hours(u64),
    Yesterday,
    Days(u64),
    Weeks(u64),
}

/// The relative date of `then` as seen at `now`, both in Unix seconds. A time
/// in the future (clock skew) reads as `Now`.
pub fn ago(now: u64, then: u64) -> Ago {
    let secs = now.saturating_sub(then);
    match secs {
        0..=59 => Ago::Now,
        60..=3_599 => Ago::Minutes(secs / 60),
        3_600..=86_399 => Ago::Hours(secs / 3_600),
        86_400..=172_799 => Ago::Yesterday,
        172_800..=604_799 => Ago::Days(secs / 86_400),
        _ => Ago::Weeks(secs / 604_800),
    }
}

/// The Spanish label for an [`Ago`]: `ahora`, `hace 5 min`, `ayer`, …
fn ago_text(ago: &Ago) -> String {
    match *ago {
        Ago::Now => tr("now").to_string(),
        Ago::Minutes(n) => trf("{n} min ago", &[("n", &n)]),
        Ago::Hours(n) => trf("{n} h ago", &[("n", &n)]),
        Ago::Yesterday => tr("yesterday").to_string(),
        Ago::Days(n) => trf("{n} days ago", &[("n", &n)]),
        Ago::Weeks(n) => trf("{n} weeks ago", &[("n", &n)]),
    }
}

#[cfg(test)]
mod tests {
    use super::{Ago, ago, prompt_is_sendable};

    #[test]
    fn blank_prompts_are_not_sent() {
        assert!(!prompt_is_sendable(""));
        assert!(!prompt_is_sendable("   \n\t "));
    }

    #[test]
    fn a_prompt_with_text_is_sent() {
        assert!(prompt_is_sendable("hola"));
        assert!(prompt_is_sendable("  hola\n"));
    }

    #[test]
    fn a_minute_or_less_is_now() {
        assert_eq!(ago(1_000, 1_000), Ago::Now);
        assert_eq!(ago(1_000, 941), Ago::Now);
    }

    #[test]
    fn a_future_stamp_reads_as_now() {
        assert_eq!(ago(1_000, 5_000), Ago::Now);
    }

    #[test]
    fn minutes_then_hours() {
        assert_eq!(ago(100_000, 100_000 - 60), Ago::Minutes(1));
        assert_eq!(ago(100_000, 100_000 - 5 * 60), Ago::Minutes(5));
        assert_eq!(ago(100_000, 100_000 - 3_599), Ago::Minutes(59));
        assert_eq!(ago(100_000, 100_000 - 3_600), Ago::Hours(1));
        assert_eq!(ago(100_000, 100_000 - 3 * 3_600), Ago::Hours(3));
    }

    #[test]
    fn yesterday_spans_the_second_day() {
        assert_eq!(ago(200_000, 200_000 - 86_400), Ago::Yesterday);
        assert_eq!(ago(200_000, 200_000 - 172_799), Ago::Yesterday);
        assert_eq!(ago(200_000, 200_000 - 172_800), Ago::Days(2));
    }

    #[test]
    fn days_then_weeks() {
        assert_eq!(ago(10_000_000, 10_000_000 - 5 * 86_400), Ago::Days(5));
        assert_eq!(ago(10_000_000, 10_000_000 - 604_799), Ago::Days(6));
        assert_eq!(ago(10_000_000, 10_000_000 - 21 * 86_400), Ago::Weeks(3));
    }
}

#[cfg(test)]
mod markdown_tests {
    use super::{Block, Inline, parse_blocks, parse_inline};

    #[test]
    fn prose_is_one_paragraph_until_a_blank_line() {
        assert_eq!(
            parse_blocks("hola\nmundo\n\notra"),
            vec![
                Block::Para("hola\nmundo".into()),
                Block::Para("otra".into())
            ]
        );
    }

    #[test]
    fn fenced_code_keeps_its_lang_and_body_verbatim() {
        let src = "antes\n```rust\nlet x = 1;\n\n- no es lista\n```\ndespués";
        assert_eq!(
            parse_blocks(src),
            vec![
                Block::Para("antes".into()),
                Block::Code {
                    lang: "rust".into(),
                    body: "let x = 1;\n\n- no es lista".into()
                },
                Block::Para("después".into()),
            ]
        );
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        assert_eq!(
            parse_blocks("```\na\nb"),
            vec![Block::Code {
                lang: String::new(),
                body: "a\nb".into()
            }]
        );
    }

    #[test]
    fn dash_lines_form_a_list() {
        assert_eq!(
            parse_blocks("intro\n- uno\n- dos\nfin"),
            vec![
                Block::Para("intro".into()),
                Block::List(vec!["uno".into(), "dos".into()]),
                Block::Para("fin".into()),
            ]
        );
    }

    #[test]
    fn empty_input_has_no_blocks() {
        assert!(parse_blocks("").is_empty());
        assert!(parse_blocks("\n\n").is_empty());
    }

    #[test]
    fn inline_code_and_bold_split_the_run() {
        assert_eq!(
            parse_inline("usa `nodo` y **ya**!"),
            vec![
                Inline::Text("usa ".into()),
                Inline::Code("nodo".into()),
                Inline::Text(" y ".into()),
                Inline::Bold("ya".into()),
                Inline::Text("!".into()),
            ]
        );
    }

    #[test]
    fn unmatched_markers_stay_as_text() {
        assert_eq!(parse_inline("a `b"), vec![Inline::Text("a `b".into())]);
        assert_eq!(parse_inline("**x"), vec![Inline::Text("**x".into())]);
    }
}
