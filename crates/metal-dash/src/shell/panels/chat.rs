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
    Align, Color32, Frame, Id, Label, Layout, Response, RichText, ScrollArea, Sense, TextEdit,
    TextFormat, Ui,
};
use serde_json::{Value, json};

use crate::call::Call;
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
        }
    }
}

impl PanelState {
    /// Test-only writer — the seam a shell test preloads the field through.
    #[cfg(test)]
    pub(crate) fn set_prompt_for_test(&mut self, text: &str) {
        self.prompt = text.to_string();
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
                        header(ui, theme, door, state, &title, empty);
                        match state.view {
                            View::Conversation => {
                                conversation_view(ui, theme, door, state, chat.as_ref(), selection)
                            }
                            View::History => history_view(ui, theme, door, state),
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
                            .font(theme.font_small.clone())
                            .color(theme.ink_weak),
                    )
                    .truncate(),
                );
                if !empty
                    && ui
                        .button(lucide(Icon::New, theme.font_small.size))
                        .on_hover_text(tr("new chat"))
                        .clicked()
                {
                    new_chat(door, state);
                }
                if ui
                    .button(lucide(Icon::History, theme.font_small.size))
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
    let content = egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, split));
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
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("shell/panel/chat/history")
        .show(ui, |ui| {
            for chat in &chats {
                history_row(ui, theme, door, state, chat);
            }
        });
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
        Frame::new()
            .fill(theme.inset)
            .stroke(theme.rule_stroke())
            .corner_radius(theme.corner_radius())
            .inner_margin(theme.panel_margin())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let text = ui
                        .vertical(|ui| {
                            ui.add(
                                Label::new(
                                    RichText::new(&title)
                                        .font(theme.font_body.clone())
                                        .color(theme.ink),
                                )
                                .truncate(),
                            );
                            if !preview.is_empty() {
                                ui.add(
                                    Label::new(
                                        RichText::new(&preview)
                                            .font(theme.font_small.clone())
                                            .color(theme.ink_weak),
                                    )
                                    .truncate(),
                                );
                            }
                            if !meta.is_empty() {
                                ui.label(
                                    RichText::new(meta.join(" · "))
                                        .font(theme.font_small.clone())
                                        .color(theme.ink_faint),
                                );
                            }
                        })
                        .response;
                    if ui
                        .interact(text.rect, ui.id().with("open"), Sense::click())
                        .clicked()
                    {
                        state.chat = Some(id.clone());
                        state.view = View::Conversation;
                    }
                    if ui
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
            });
    });
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
            paint_markdown(ui, theme, content, theme.phosphor_lit);
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
            let chip = egui::Button::new(
                RichText::new(label)
                    .font(theme.font_small.clone())
                    .color(theme.phosphor),
            );
            if ui.add(chip).clicked() {
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
        TextFormat::simple(theme.font_small.clone(), theme.ink),
    );
    let summary = arg_summary(args);
    if !summary.is_empty() {
        head.append(
            &format!("  {summary}"),
            0.0,
            TextFormat::simple(theme.font_small.clone(), theme.ink_weak),
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
            ui.horizontal(|ui| {
                if ui
                    .button(
                        RichText::new(tr("allow"))
                            .font(theme.font_body.clone())
                            .color(theme.ok),
                    )
                    .clicked()
                {
                    let _ = door.json(
                        "POST",
                        "/ai/chat/approve",
                        Some(json!({ "call": id, "allow": true })),
                    );
                }
                if ui
                    .button(
                        RichText::new(tr("deny"))
                            .font(theme.font_body.clone())
                            .color(theme.warn),
                    )
                    .clicked()
                {
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
/// on the inset, **bold** in the lit phosphor, the rest in `base`.
fn inline_job(theme: &Theme, text: &str, base: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let body = theme.font_body.clone();
    for part in parse_inline(text) {
        match part {
            Inline::Text(t) => job.append(&t, 0.0, TextFormat::simple(body.clone(), base)),
            Inline::Bold(t) => job.append(
                &t,
                0.0,
                TextFormat::simple(body.clone(), theme.phosphor_lit),
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
const COMPOSER_MAX_ROWS: usize = 6;

/// The composer: a rounded, framed box docked under the transcript. Above the
/// text field, the context chips; beside it, `@` (attach the shell's current
/// selection) and the send button, which turns into `stop` while the chat is
/// busy or awaiting. The field grows with its lines (1 to [`COMPOSER_MAX_ROWS`])
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
    selection: Option<&Value>,
) {
    let prompt_id = Id::new(PROMPT_ID);
    let focused = ui.memory(|m| m.has_focus(prompt_id));
    let enter = focused
        && ui.input_mut(|i| {
            !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
        });
    if enter {
        send_prompt(door, state);
    }

    let busy = chat.is_some_and(|c| matches!(c["status"].as_str(), Some("busy" | "awaiting")));
    let chat_id = chat.and_then(|c| c["id"].as_str()).map(str::to_string);
    let rows = state.prompt.split('\n').count().clamp(1, COMPOSER_MAX_ROWS);
    let max_h = ui.text_style_height(&egui::TextStyle::Body) * COMPOSER_MAX_ROWS as f32
        + theme.field_pad.y * 2.0;

    Frame::new()
        .fill(theme.inset)
        .stroke(theme.rule_stroke())
        .corner_radius(theme.corner_radius())
        .inner_margin(theme.panel_margin())
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            context_chips(ui, theme, state);
            let row_h = (ui.text_style_height(&egui::TextStyle::Body) * rows as f32
                + theme.field_pad.y * 2.0)
                .min(max_h)
                .max(ui.spacing().interact_size.y);
            let spacing = ui.spacing().item_spacing.x;
            // Both buttons are exactly one field row tall and `btn_w` wide. The
            // row is a fixed rect and each child is placed at an absolute spot
            // in it, so the field's own height can never shift the buttons off
            // the row's centre line.
            let btn_w = ui.spacing().interact_size.x;
            let btn_size = egui::vec2(btn_w, row_h);
            let field_w = (ui.available_width() - 2.0 * btn_w - 2.0 * spacing).max(0.0);
            let (row, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), row_h),
                Sense::hover(),
            );
            let attach_rect = egui::Rect::from_min_size(row.min, btn_size);
            let field_rect = egui::Rect::from_min_size(
                egui::pos2(row.min.x + btn_w + spacing, row.min.y),
                egui::vec2(field_w, row_h),
            );
            let send_rect = egui::Rect::from_min_size(
                egui::pos2(row.max.x - btn_w, row.min.y),
                btn_size,
            );

            // `@` attaches the shell's selection, at the left edge.
            let attach = ui
                .add_enabled_ui(selection.is_some(), |ui| {
                    ui.put(
                        attach_rect,
                        egui::Button::new(RichText::new("@").font(theme.font_body.clone())),
                    )
                })
                .inner
                .on_hover_text(tr("attach the selection"));
            if attach.clicked() {
                if let Some(sel) = selection {
                    if !state.context.contains(sel) {
                        state.context.push(sel.clone());
                    }
                }
            }
            // The field gets an exact rect, so it can never squeeze the
            // send/stop control past the panel's right edge.
            ui.scope_builder(egui::UiBuilder::new().max_rect(field_rect), |ui| {
                ScrollArea::vertical()
                    .max_height(max_h)
                    .auto_shrink([false, true])
                    .id_salt("shell/panel/chat/composer")
                    .show(ui, |ui| {
                        ui.add(
                            TextEdit::multiline(&mut state.prompt)
                                .id(prompt_id)
                                .desired_width(field_w)
                                .margin(theme.field_pad)
                                .desired_rows(rows)
                                .hint_text(tr("ask about the graph…")),
                        );
                    });
            });
            // The send/stop control sits at the right edge, after the field.
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
                let send = ui
                    .put(
                        send_rect,
                        egui::Button::new(
                            RichText::new("↑")
                                .font(theme.font_body.clone())
                                .color(theme.phosphor),
                        ),
                    )
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
}

/// The context chips above the field: one framed chip per ref, labelled, with
/// an ✕ that drops it. Empty when nothing is attached.
fn context_chips(ui: &mut Ui, theme: &Theme, state: &mut PanelState) {
    if state.context.is_empty() {
        return;
    }
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (i, chip) in state.context.iter().enumerate() {
            let label = chip["label"]
                .as_str()
                .or_else(|| chip["id"].as_str())
                .unwrap_or("?");
            Frame::new()
                .fill(theme.tube_raised)
                .stroke(theme.rule_stroke())
                .corner_radius(theme.corner_radius())
                .inner_margin(theme.panel_margin())
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(label)
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
    if let Some(i) = remove {
        state.context.remove(i);
    }
}

/// Posts the prompt as one turn — `POST /ai/ask {chat, prompt, context}` — and
/// clears the field and the context chips. A blank or whitespace-only prompt
/// sends nothing and keeps the field as it is. Returns whether a turn was sent.
fn send_prompt(door: &dyn Call, state: &mut PanelState) -> bool {
    if !prompt_is_sendable(&state.prompt) {
        return false;
    }
    let _ = door.json(
        "POST",
        "/ai/ask",
        Some(json!({
            "chat": state.chat.clone(),
            "prompt": state.prompt.clone(),
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
