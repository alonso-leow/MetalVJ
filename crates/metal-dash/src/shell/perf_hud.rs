//! The F12 performance HUD (2026-10-09): `crate::perf`'s last window painted
//! over the shell — frame rate and percentiles, how much of each frame the UI
//! thread spent inside the door, and which `(thread, route)` pairs hold or
//! wait on the shared lock the most. Off by default; F12 toggles it. The
//! same numbers answer `GET /perf`.

use std::sync::Arc;

use egui::{Id, RichText, Ui};
use serde_json::Value;

use super::classes::Theme;
use super::i18n::tr;

/// How often the HUD re-reads the snapshot, in seconds.
const REFRESH: f64 = 0.5;

fn open_id() -> Id {
    Id::new("shell/perf-hud/open")
}

/// The last snapshot, re-read at most every [`REFRESH`] seconds. Shared, so
/// the badge and the HUD read it each frame without deep-copying the JSON.
fn snapshot(ctx: &egui::Context) -> Arc<Value> {
    let now = ctx.input(|i| i.time);
    let key = Id::new("shell/perf-hud/snapshot");
    match ctx.data(|d| d.get_temp::<(f64, Arc<Value>)>(key)) {
        Some((at, v)) if now - at < REFRESH => v,
        _ => {
            let v = Arc::new(crate::perf::snapshot());
            ctx.data_mut(|d| d.insert_temp(key, (now, Arc::clone(&v))));
            v
        }
    }
}

/// The always-on frame rate and frame time, painted inline in the header's
/// right cluster — never a floating area, so it cannot cover the AI chat
/// sidebar. Re-read at most every [`REFRESH`] seconds.
pub fn badge(ui: &mut Ui, theme: &Theme) {
    let ctx = ui.ctx().clone();
    let snap = snapshot(&ctx);
    crate::shell::frames::repaint_after(&ctx, std::time::Duration::from_secs_f64(REFRESH));
    let f = |v: &Value| v.as_f64().unwrap_or(0.0);
    ui.label(
        RichText::new(format!(
            "{:.0} fps · {:.1} ms",
            f(&snap["fps"]),
            f(&snap["frame_ms"]["p50"]),
        ))
        .font(theme.font_small.clone())
        .color(theme.ink_weak),
    );
}

/// Toggles on F12 and paints the HUD while open.
pub fn paint(ui: &mut Ui, theme: &Theme) {
    let ctx = ui.ctx().clone();
    let mut open = ctx.data(|d| d.get_temp::<bool>(open_id())).unwrap_or(false);
    if ctx.input(|i| i.key_pressed(egui::Key::F12)) {
        open = !open;
        ctx.data_mut(|d| d.insert_temp(open_id(), open));
    }
    if !open {
        return;
    }
    let snap = snapshot(&ctx);
    crate::shell::frames::repaint_after(&ctx, std::time::Duration::from_secs_f64(REFRESH));
    let f = |v: &Value| v.as_f64().unwrap_or(0.0);
    egui::Area::new(Id::new("shell/perf-hud"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_BOTTOM, egui::Vec2::ZERO)
        .interactable(false)
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                let line = |ui: &mut Ui, text: String, strong: bool| {
                    ui.label(
                        RichText::new(text)
                            .font(theme.font_small.clone())
                            .color(if strong { theme.ink } else { theme.ink_weak }),
                    );
                };
                line(
                    ui,
                    format!(
                        "{} {:.0} fps · {} p50 {:.1} p95 {:.1} max {:.0} ms · {} {}",
                        tr("perf"),
                        f(&snap["fps"]),
                        tr("frame"),
                        f(&snap["frame_ms"]["p50"]),
                        f(&snap["frame_ms"]["p95"]),
                        f(&snap["frame_ms"]["max"]),
                        snap["slow_frames"].as_u64().unwrap_or(0),
                        tr("slow"),
                    ),
                    true,
                );
                line(
                    ui,
                    format!(
                        "ui {:.1} · paint {:.1} · door {:.1} ms {}",
                        f(&snap["ui_ms"]),
                        f(&snap["paint_ms"]),
                        f(&snap["door_ms"]),
                        tr("per frame"),
                    ),
                    false,
                );
                line(ui, tr("UI pass — ms per frame · max").to_string(), true);
                for sp in snap["spans"].as_array().into_iter().flatten().take(8) {
                    line(
                        ui,
                        format!(
                            "{} — {:.1} · {:.0}",
                            sp["span"].as_str().unwrap_or("?"),
                            f(&sp["ms_per_frame"]),
                            f(&sp["max_ms"]),
                        ),
                        false,
                    );
                }
                line(ui, tr("thread · route — calls/s · wait · run · max ms · lock share").to_string(), true);
                for c in snap["calls"].as_array().into_iter().flatten().take(10) {
                    line(
                        ui,
                        format!(
                            "{} — {:.0}/s · {:.1} · {:.1} · {:.0} · {:.0}%",
                            c["call"].as_str().unwrap_or("?"),
                            f(&c["per_s"]),
                            f(&c["wait_ms"]),
                            f(&c["run_ms"]),
                            f(&c["max_ms"]),
                            f(&c["busy"]) * 100.0,
                        ),
                        false,
                    );
                }
            });
        });
}
