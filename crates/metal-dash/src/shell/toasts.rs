//! The Sonner-style toast logger (W3, `shell-ux-plan.md`).
//!
//! Three parts:
//! - [`Toasts`]: the store. Live toasts expire after their level's lifetime
//!   (and never while the pointer is over the stack); every toast is also
//!   written to a ring buffer log ([`LOG_CAP`] entries) that outlives it.
//! - [`notify`]: the one way a call site reports an outcome. It needs no
//!   handle to the shell, so a panel or a door reply can push from anywhere;
//!   the shell drains the queue into its [`Toasts`] once per frame
//!   ([`Toasts::drain`]).
//! - [`paint`] / [`log_window`]: the bottom-right stack (newest in front, two
//!   older cards peeking behind; hover fans them out) and the log window.

use std::collections::VecDeque;
use egui::{Align2, Area, Color32, Id, Order, Rect, RichText, Sense};

use super::classes::Theme;
use super::gallery;
use super::i18n::tr;

/// How many log entries the ring buffer keeps.
pub const LOG_CAP: usize = 500;
/// Live toasts kept at once; the oldest is dropped past this.
const LIVE_CAP: usize = 20;
/// Cards painted behind the newest one while the stack is collapsed.
const PEEK: usize = 2;
/// Lifetime of an info, success or warning toast, in seconds.
const DEFAULT_SECS: f32 = 5.0;
/// Lifetime of an error toast, in seconds: errors stay longer.
const ERROR_SECS: f32 = 8.0;

/// A node an outcome is about: its graph and its id, so the log can open the
/// graph again before it selects the node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRef {
    pub graph: String,
    pub node: String,
}

/// How serious an outcome is. Only the colour and the lifetime depend on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warn,
    Error,
}

impl Level {
    /// Seconds a toast of this level stays on screen (pointer not over it).
    pub fn lifetime(self) -> f32 {
        match self {
            Level::Error => ERROR_SECS,
            _ => DEFAULT_SECS,
        }
    }

    fn colour(self, theme: &Theme) -> Color32 {
        match self {
            Level::Info => theme.ink_weak,
            Level::Success => theme.ok,
            Level::Warn => theme.warn,
            Level::Error => theme.phosphor,
        }
    }
}

/// A toast on screen. `left` is the seconds it has before it expires.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub level: Level,
    pub title: String,
    pub detail: Option<String>,
    /// The node the outcome is about, if any: clicking it in the log selects it.
    pub node: Option<NodeRef>,
    pub left: f32,
}

/// One line of the log. Kept after its toast expires.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    /// Seconds on the shell's clock (`ui.input(|i| i.time)`) when it was pushed.
    pub at: f64,
    pub level: Level,
    pub title: String,
    pub detail: Option<String>,
    pub node: Option<NodeRef>,
}

/// The store the shell holds: live toasts plus the ring buffer log.
#[derive(Debug, Default)]
pub struct Toasts {
    /// Oldest first; the newest is last.
    live: Vec<Toast>,
    log: VecDeque<LogEntry>,
    next: u64,
    /// Whether the pointer was over the stack last frame. While it is, no
    /// toast counts down.
    hovered: bool,
    /// Whether the log window is open (the header's `log` button flips it).
    pub log_open: bool,
}

impl Toasts {
    /// Adds a toast and a log line for it. Returns the toast's id.
    pub fn push(
        &mut self,
        at: f64,
        level: Level,
        title: impl Into<String>,
        detail: Option<String>,
        node: Option<NodeRef>,
    ) -> u64 {
        let title = title.into();
        let id = self.next;
        self.next += 1;
        self.log.push_back(LogEntry {
            at,
            level,
            title: title.clone(),
            detail: detail.clone(),
            node: node.clone(),
        });
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
        self.live.push(Toast { id, level, title, detail, node, left: level.lifetime() });
        if self.live.len() > LIVE_CAP {
            self.live.remove(0);
        }
        id
    }

    /// Moves every queued [`notify`] into the store, stamped `at`.
    pub fn drain(&mut self, at: f64) {
        for p in take_queue() {
            self.push(at, p.level, p.title, p.detail, p.node);
        }
    }

    /// Counts the live toasts down by `dt` seconds, unless the pointer is
    /// over the stack, and drops the ones that ran out.
    pub fn tick(&mut self, dt: f32) {
        if self.hovered {
            return;
        }
        for t in &mut self.live {
            t.left -= dt;
        }
        self.live.retain(|t| t.left > 0.0);
    }

    /// Pauses (`true`) or resumes (`false`) every countdown.
    pub fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    pub fn hovered(&self) -> bool {
        self.hovered
    }

    /// Closes one toast by hand. Its log line stays.
    pub fn dismiss(&mut self, id: u64) {
        self.live.retain(|t| t.id != id);
    }

    /// The live toasts, oldest first.
    pub fn live(&self) -> &[Toast] {
        &self.live
    }

    /// The log, oldest first.
    pub fn log(&self) -> impl DoubleEndedIterator<Item = &LogEntry> {
        self.log.iter()
    }

    pub fn log_len(&self) -> usize {
        self.log.len()
    }
}

/// A call site's outcome, waiting for the shell to drain it.
struct Pending {
    level: Level,
    title: String,
    detail: Option<String>,
    node: Option<NodeRef>,
}

thread_local! {
    // One queue per thread: the shell drains it on the UI thread, and each
    // test runs on its own thread, so a test never sees another's outcomes.
    static QUEUE: std::cell::RefCell<Vec<Pending>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn take_queue() -> Vec<Pending> {
    QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// Reports an outcome. The shell shows it on its next frame. Call it from the
/// UI thread (where the shell drains).
pub fn notify(level: Level, title: impl Into<String>, node: Option<NodeRef>) {
    QUEUE.with(|q| q.borrow_mut().push(Pending { level, title: title.into(), detail: None, node }));
}

/// Shorthand for [`notify`] with [`Level::Error`] and no node.
pub fn error(title: impl Into<String>) {
    notify(Level::Error, title, None);
}

/// Shorthand for [`notify`] with [`Level::Success`] and no node.
pub fn success(title: impl Into<String>) {
    notify(Level::Success, title, None);
}

/// The stack's rectangle for `n` live toasts, `expand` 0.0 collapsed to 1.0
/// fanned out, bottom edge at `bottom`, right edge at `right`.
fn stack_rect(theme: &Theme, n: usize, expand: f32, right: f32, bottom: f32) -> Rect {
    let h = theme.toast_height;
    let gap = theme.toast_peek;
    let collapsed = h + gap * n.min(PEEK + 1).saturating_sub(1) as f32;
    let fanned = if n == 0 { 0.0 } else { h * n as f32 + gap * (n - 1) as f32 };
    let height = collapsed + (fanned - collapsed) * expand;
    Rect::from_min_size(
        egui::pos2(right - theme.toast_width, bottom - height),
        egui::vec2(theme.toast_width, height),
    )
}

/// Paints the toast stack in the bottom-right corner and runs its timers.
/// Call once per frame after [`Toasts::drain`] and [`Toasts::tick`].
pub fn paint(ctx: &egui::Context, theme: &Theme, store: &mut Toasts) {
    if store.live.is_empty() {
        store.set_hovered(false);
        return;
    }
    let inset = theme.toast_inset;
    let screen = ctx.content_rect();
    let anim = theme.toast_anim_secs;
    let hovered_before = store.hovered();
    let expand = ctx.animate_bool_with_time(Id::new("shell/toasts/expand"), hovered_before, anim);
    let n = store.live.len();
    let newest_first: Vec<Toast> = store.live.iter().rev().cloned().collect();
    let mut dismiss: Option<u64> = None;
    let mut over_stack = false;

    let right = screen.right() - inset;
    let bottom = screen.bottom() - inset;
    let rect = stack_rect(theme, n, expand, right, bottom);
    Area::new(Id::new("shell/toasts"))
        .order(Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            let stack = ui.allocate_rect(rect, Sense::hover());
            over_stack = stack.hovered();
            let painter = ui.painter().clone();
            let h = theme.toast_height;
            let gap = theme.toast_peek;
            // Back to front: the oldest visible card first, the newest last.
            for (i, toast) in newest_first.iter().enumerate().rev() {
                let behind = i.min(PEEK + 1);
                if i > PEEK && expand < 0.5 {
                    continue;
                }
                let scale = 1.0 - theme.toast_shrink * i.min(PEEK) as f32 * (1.0 - expand);
                let width = theme.toast_width * scale;
                let top_collapsed = rect.bottom() - h - gap * behind as f32;
                let top_fanned = rect.bottom() - h - (h + gap) * i as f32;
                let top = top_collapsed + (top_fanned - top_collapsed) * expand;
                let inset_x = (theme.toast_width - width) / 2.0;
                let card = Rect::from_min_size(
                    egui::pos2(rect.left() + inset_x, top),
                    egui::vec2(width, h * scale),
                );
                let grow = ctx.animate_value_with_time(
                    Id::new(("shell/toasts/in", toast.id)),
                    1.0,
                    anim,
                );
                let fade = if i == 0 { grow } else { grow.min(1.0) };
                let level_colour = toast.level.colour(theme);
                painter.rect_filled(card, theme.corner_radius(), theme.panel.linear_multiply(fade));
                painter.rect_stroke(
                    card,
                    theme.corner_radius(),
                    theme.hairline_stroke(level_colour.linear_multiply(fade)),
                    egui::StrokeKind::Inside,
                );
                if i == 0 || expand >= 0.5 {
                    let text_x = card.left() + theme.pad as f32;
                    let title = painter.layout(
                        toast.title.clone(),
                        theme.font_body.clone(),
                        theme.ink.linear_multiply(fade),
                        (card.width() - 2.0 * theme.pad as f32).max(0.0),
                    );
                    painter.galley(egui::pos2(text_x, card.top() + theme.pad as f32), title, theme.ink);
                    if let Some(detail) = &toast.detail {
                        let line = painter.layout(
                            detail.clone(),
                            theme.font_small.clone(),
                            theme.ink_weak.linear_multiply(fade),
                            (card.width() - 2.0 * theme.pad as f32).max(0.0),
                        );
                        painter.galley(
                            egui::pos2(text_x, card.bottom() - theme.pad as f32 - line.size().y),
                            line,
                            theme.ink_weak,
                        );
                    }
                }
                // The close x, top right of each card that shows text.
                if i == 0 || expand >= 0.5 {
                    let x_rect = Rect::from_min_size(
                        egui::pos2(card.right() - h / 2.0, card.top()),
                        egui::vec2(h / 2.0, h / 2.0),
                    );
                    let resp = ui.interact(x_rect, Id::new(("shell/toasts/close", toast.id)), Sense::click());
                    painter.text(
                        x_rect.center(),
                        Align2::CENTER_CENTER,
                        "x",
                        theme.font_small.clone(),
                        if resp.hovered() { theme.ink } else { theme.ink_weak },
                    );
                    if resp.clicked() {
                        dismiss = Some(toast.id);
                    }
                }
            }
        });

    if let Some(id) = dismiss {
        store.dismiss(id);
    }
    store.set_hovered(over_stack);
    if n > 1 || expand > 0.0 {
        ctx.request_repaint();
    }
}

/// The log window: every entry, newest first, with its time and level dot.
/// Returns the node of the entry clicked this frame, if it has one (the
/// shell opens its graph and selects it).
pub fn log_window(ctx: &egui::Context, theme: &Theme, store: &mut Toasts) -> Option<NodeRef> {
    let mut picked: Option<NodeRef> = None;
    let mut open = store.log_open;
    gallery::overlay_window("shell/toast-log", tr("log").to_string())
        .open(&mut open)
        .show(ctx, |ui| {
            if store.log_len() == 0 {
                ui.label(RichText::new(tr("nothing logged yet")).font(theme.font_small.clone()).color(theme.ink_weak));
                return;
            }
            egui::ScrollArea::vertical().max_height(theme.toast_height * 10.0).show(ui, |ui| {
                for entry in store.log().rev() {
                    let secs = entry.at.max(0.0) as u64;
                    let stamp = format!("{:02}:{:02}", secs / 60, secs % 60);
                    let line = ui.horizontal(|ui| {
                        ui.label(RichText::new(stamp).font(theme.font_small.clone()).color(theme.ink_faint));
                        ui.label(RichText::new("●").font(theme.font_small.clone()).color(entry.level.colour(theme)));
                        let text = match &entry.detail {
                            Some(d) => format!("{} — {d}", entry.title),
                            None => entry.title.clone(),
                        };
                        let sense = if entry.node.is_some() { Sense::click() } else { Sense::hover() };
                        ui.add(egui::Label::new(RichText::new(text).font(theme.font_small.clone()).color(theme.ink)).sense(sense))
                    });
                    let resp = line.inner;
                    if resp.clicked() {
                        picked = entry.node.clone();
                    }
                }
            });
        });
    store.log_open = open;
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(s: &mut Toasts, level: Level, title: &str) -> u64 {
        s.push(0.0, level, title, None, None)
    }

    /// A toast is on screen for its level's lifetime, then gone; the log keeps it.
    #[test]
    fn toasts_expire_and_the_log_keeps_them() {
        let mut s = Toasts::default();
        push(&mut s, Level::Info, "saved");
        push(&mut s, Level::Error, "broke");
        s.tick(DEFAULT_SECS - 0.1);
        assert_eq!(s.live().len(), 2, "both still up before five seconds");
        s.tick(0.2);
        assert_eq!(s.live().len(), 1, "the info toast ran out at five seconds");
        assert_eq!(s.live()[0].title, "broke");
        s.tick(ERROR_SECS);
        assert!(s.live().is_empty(), "the error ran out at eight seconds");
        assert_eq!(s.log_len(), 2, "expired toasts stay in the log");
    }

    /// While the pointer is over the stack no countdown moves; leaving resumes them.
    #[test]
    fn hover_pauses_every_timer() {
        let mut s = Toasts::default();
        push(&mut s, Level::Info, "saved");
        s.set_hovered(true);
        s.tick(60.0);
        assert_eq!(s.live().len(), 1, "a hovered stack does not expire");
        s.set_hovered(false);
        s.tick(DEFAULT_SECS + 1.0);
        assert!(s.live().is_empty(), "the countdown picks up where it stopped");
    }

    /// The log holds at most [`LOG_CAP`] lines, dropping the oldest first.
    #[test]
    fn ring_buffer_caps_the_log_oldest_first() {
        let mut s = Toasts::default();
        for i in 0..(LOG_CAP + 25) {
            s.push(i as f64, Level::Info, format!("entry {i}"), None, None);
        }
        assert_eq!(s.log_len(), LOG_CAP);
        assert_eq!(s.log().next().map(|e| e.title.as_str()), Some("entry 25"));
        assert_eq!(s.log().last().map(|e| e.title.as_str()), Some(format!("entry {}", LOG_CAP + 24).as_str()));
    }

    /// A dismissed toast leaves the stack at once and keeps its log line.
    #[test]
    fn dismiss_removes_only_the_toast() {
        let mut s = Toasts::default();
        let id = push(&mut s, Level::Warn, "careful");
        s.dismiss(id);
        assert!(s.live().is_empty());
        assert_eq!(s.log_len(), 1);
    }

    /// Pushes carry a node reference into both the toast and the log line.
    #[test]
    fn node_reference_reaches_the_log() {
        let mut s = Toasts::default();
        let r = NodeRef { graph: "g1".into(), node: "n7".into() };
        s.push(1.5, Level::Error, "no wire", None, Some(r.clone()));
        let entry = s.log().next().expect("one line");
        assert_eq!(entry.node.as_ref(), Some(&r));
        assert_eq!(s.live()[0].node.as_ref(), Some(&r));
    }
}
