//! 2026-10-15: the simplified **sequence** — the library's third route.
//!
//! It is a read-only reproduction surface, not an authoring one: it reads the
//! shared [`crate::shell::media`] board (the image or video picks the library
//! and the video gallery published), lists them as a timeline of clips — one
//! row per clip, its span, a loop marker, the cursor highlighted — and
//! reproduces the current one in a [`FloatingPreview`]. A local clock advances
//! the cursor over the clips while playing, wrapping inside a looping clip and
//! otherwise stepping to the next.
//!
//! **`enable effects`** is the hand-off out of here: it flips the board's
//! effects switch and publishes the board's clips under
//! [`take_enable_effects`], which the shell drains to load them into the full
//! float-authoring timeline (the lanes' own lanes). This module owns no
//! authoring state — that is the intents' job, not this surface's.
//!
//! **Regla 5/6.** Every value it paints is one the board already holds, and
//! every call goes through [`Call`] (`media::seek`'s `POST /author/video/seek`).

use egui::{Context, Id, Pos2, Rect, RichText, Sense, StrokeKind, Ui, Vec2};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::i18n::{tr, trf};
use crate::shell::media::{self, Clip};
use crate::shell::preview::{FloatingPreview, PreviewSubject};

/// The hand-off the `enable effects` button publishes; the shell takes it with
/// [`take_enable_effects`].
const EFFECTS_ID: &str = "shell/sequence/enable-effects";
/// The local clock and the play flag, in egui memory between frames.
const CLOCK_ID: &str = "shell/sequence/clock";
const PLAYING_ID: &str = "shell/sequence/playing";

thread_local! {
    /// The sequence's one picture pump. A [`FloatingPreview`] keeps a fetch in
    /// flight and so is not `Send`/`Sync`; egui memory cannot hold it, and a
    /// `thread_local` is the same single-thread home the window paints in.
    static PREVIEW: std::cell::RefCell<FloatingPreview> =
        std::cell::RefCell::new(FloatingPreview::default());
}

/// The play clock: the clip it belongs to, the seconds into that clip, and the
/// input time it last advanced from.
#[derive(Clone, Default)]
struct Clock {
    key: String,
    t: f64,
    at: f64,
}

fn playing(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(Id::new(PLAYING_ID))).unwrap_or(false)
}

fn set_playing(ctx: &Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::new(PLAYING_ID), on));
}

/// The board clip's identity, so a changed pick restarts the clock.
fn clip_key(clip: &Clip) -> String {
    format!(
        "{}#{}",
        clip.path,
        clip.scene.as_ref().map(|s| s.id.as_str()).unwrap_or("")
    )
}

/// `0:03.0–0:06.0 · 3.0s`, the same reading the scenes gallery gives a clip.
fn span_text(clip: &Clip) -> String {
    let (a, b) = clip.span();
    let at = |t: f64| format!("{}:{:04.1}", (t / 60.0).floor() as u64, t.rem_euclid(60.0));
    format!("{}–{} · {:.1}s", at(a), at(b), clip.length())
}

/// The clips the `enable effects` button handed off, removed and returned once.
pub fn take_enable_effects(ctx: &Context) -> Option<Vec<Clip>> {
    ctx.data_mut(|d| d.remove_temp::<Vec<Clip>>(Id::new(EFFECTS_ID)))
}

/// The simplified sequence, painting the media board: a timeline of the picked
/// clips, its controls, and the current clip's picture.
pub(crate) fn view(ui: &mut Ui, theme: &Theme, door: &dyn Call) {
    let ctx = ui.ctx().clone();
    let board = media::board(&ctx);

    ui.horizontal_wrapped(|ui| {
        let now_playing = playing(&ctx);
        let label = if now_playing { tr("pause clips") } else { tr("play clips") };
        if ui
            .button(RichText::new(label).font(theme.font_small.clone()))
            .clicked()
        {
            set_playing(&ctx, !now_playing);
        }
        if ui
            .button(RichText::new(tr("enable effects")).font(theme.font_small.clone()))
            .clicked()
        {
            media::set_effects(&ctx, true);
            ctx.data_mut(|d| d.insert_temp(Id::new(EFFECTS_ID), board.clips.clone()));
        }
        let clear = ui.add_enabled(
            !board.clips.is_empty(),
            egui::Button::new(RichText::new(tr("clear")).font(theme.font_small.clone())),
        );
        if clear.clicked() {
            media::clear(&ctx);
            ctx.data_mut(|d| d.remove_temp::<Clock>(Id::new(CLOCK_ID)));
            set_playing(&ctx, false);
        }
    });

    if board.clips.is_empty() {
        ui.label(
            RichText::new(tr("no clips — pick media in the library"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    }

    // The clock: advance the cursor over the clips while playing. A looping
    // clip wraps inside its own span; the last clip stops playback.
    let now = ui.input(|i| i.time);
    let clock_id = Id::new(CLOCK_ID);
    let mut clock: Clock = ctx.data(|d| d.get_temp(clock_id)).unwrap_or_default();
    let key = board.current().map(clip_key).unwrap_or_default();
    if clock.key != key {
        clock.key = key;
        clock.t = 0.0;
        clock.at = now;
    }
    let dt = (now - clock.at).max(0.0);
    clock.at = now;
    if playing(&ctx) {
        if let Some(clip) = board.current() {
            let len = clip.length();
            clock.t += dt;
            if clock.t >= len {
                if clip.looping || len <= 0.0 {
                    clock.t = if len > 0.0 { clock.t % len } else { 0.0 };
                } else if board.cursor + 1 < board.clips.len() {
                    media::set_cursor(&ctx, board.cursor + 1);
                    clock.t = 0.0;
                    clock.key.clear();
                } else {
                    clock.t = 0.0;
                    set_playing(&ctx, false);
                }
            }
        }
    }
    ctx.data_mut(|d| d.insert_temp(clock_id, clock.clone()));
    let board = media::board(&ctx);

    // The rows: one per clip, its span and loop marker, the cursor highlighted.
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = theme.library_gap;
        for (i, clip) in board.clips.iter().enumerate() {
            let marker = if clip.looping { " " } else { "" };
            let text = format!("{}{}   {}", clip.name, marker, span_text(clip));
            let selected = i == board.cursor;
            let label = RichText::new(text)
                .font(theme.font_small.clone())
                .color(if selected { theme.ink } else { theme.ink_weak });
            if ui.selectable_label(selected, label).clicked() {
                media::set_cursor(&ctx, i);
            }
        }
    });

    // The bar: the clips end to end, width proportional to length, the cursor
    // position marked.
    let (bar, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme.interact_size.y),
        Sense::hover(),
    );
    let total: f64 = board.clips.iter().map(Clip::length).sum();
    let count = board.clips.len() as f64;
    let painter = ui.painter();
    let mut x = bar.left();
    let mut cursor_x = bar.left();
    for (i, clip) in board.clips.iter().enumerate() {
        let frac = if total > 0.0 { clip.length() / total } else { 1.0 / count };
        let w = bar.width() * frac as f32;
        let seg = Rect::from_min_size(Pos2::new(x, bar.top()), Vec2::new(w, bar.height()));
        painter.rect_filled(
            seg,
            theme.corner_radius(),
            if i == board.cursor { theme.accent().color } else { theme.library_card },
        );
        painter.rect_stroke(seg, theme.corner_radius(), theme.rule_stroke(), StrokeKind::Inside);
        if i == board.cursor {
            let len = clip.length();
            let local = if len > 0.0 { clock.t.rem_euclid(len) } else { 0.0 };
            cursor_x = x + w * (local / len.max(f64::EPSILON)) as f32;
        }
        x += w;
    }
    painter.line_segment(
        [Pos2::new(cursor_x, bar.top()), Pos2::new(cursor_x, bar.bottom())],
        theme.accent(),
    );

    // The current clip's picture, over the rest of the panel.
    if let Some(clip) = media::current(&ctx) {
        if clip.kind == "image" || clip.kind == "video" {
            let (start, end) = clip.span();
            let len = (end - start).max(0.0);
            let local = if len > 0.0 { clock.t.rem_euclid(len) } else { 0.0 };
            media::seek(door, &clip, start + local);
            let (route, query) = media::preview_route(&clip);
            let query: Vec<(&str, String)> =
                query.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let subject = PreviewSubject {
                label: trf("{name} · {kind}", &[("name", &clip.name), ("kind", &clip.kind)]),
                export: None,
                size: None,
                fps: None,
            };
            let host = ui.max_rect();
            PREVIEW.with(|p| {
                let mut p = p.borrow_mut();
                p.set_subject(Some(subject));
                p.show(ui, theme, door, None, host, &route, &query);
            });
            if playing(&ctx) {
                crate::shell::frames::repaint_after(&ctx, crate::shell::frames::live_gap());
            }
        }
    }
}
