//! The restricted-feature overlay (`S.U18` companion, 15-sept-2026). One
//! client, one binary, two kinds of machine: the laptop with a compositor
//! and a filesystem, and the headless node — the review deployment, a
//! one-core VPS behind nginx — which has one writable path and no window to
//! spawn. Desktop-only controls are not silently dead there and are not
//! sold as features to the remote window: a locked control paints its own
//! lock badge, and its click opens a popup that says so.
//!
//! Layering obeys the directory's own rules: `classes` only — no `call`
//! surface, no I/O. The caller supplies whether the capability exists,
//! read off `GET /state`'s `capabilities` object; this module draws.
//! Spacing comes from the [`Theme`] (`item_gap`, `text_input_width`), so
//! gate 2 has a restyle drill to hold onto.

use egui::{Frame, Popup, Response, RichText, Ui};

use crate::shell::classes::Theme;
use crate::shell::components::{lucide_job, Icon};

/// What the instance says it can do, read off the `capabilities` object the
/// `transmit::state` payload carries. A field absent reads cautious
/// `false`: an older instance (or a `/state` stub without the object)
/// counts as more restricted, so a new client never trusts a desktop
/// capability it cannot confirm.
pub struct Capabilities {
    /// `current_exe --monitor` — a real window on a compositor, which a
    /// build without `gpu` cannot promise and a headless VPS has none of.
    pub windows: bool,
}

impl Capabilities {
    /// `false` on both when the payload says nothing (the cautious default).
    pub fn of(state: &serde_json::Value) -> Self {
        let caps = &state["capabilities"];
        Self {
            windows: caps["windows"].as_bool().unwrap_or(false),
        }
    }
}

/// The title the popup opens with. Named here so a kittest asserts a label
/// rather than a widget tree.
pub const LOCKED_TITLE: &str = "locked";
/// The call to action — honest about this being "coming soon", and naming the
/// free native build as the way the app runs today on a machine with a
/// window manager.
pub const LOCKED_CTA: &str = "coming soon — download the native app (free)";

/// The control-or-overlay seam, for one button at a time. `allowed` paints
/// the plain button and answers with the click that arrived; not allowed
/// paints the same button with a lock badge, and the click becomes the
/// question the popup answers instead of the write it would be elsewhere.
/// The popup is egui's own [`Popup`] anchored under the button: clicking
/// the button toggles it, clicking out closes it, nothing else to manage.
pub fn gated_button(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    allowed: bool,
    why: &'static str,
) -> Response {
    let text: egui::WidgetText = if allowed {
        RichText::new(label).into()
    } else {
        lucide_job(Icon::Lock, theme.font_small.size, label, theme.ink_weak).into()
    };
    let response = ui.button(text);
    if allowed {
        return response;
    }
    Popup::from_toggle_button_response(&response)
        .kind(egui::PopupKind::Menu)
        .frame(
            Frame::popup(ui.style())
                .stroke((theme.hairline, theme.phosphor_dim))
                .inner_margin(theme.pad as f32),
        )
        .show(|ui| {
            ui.set_min_width(theme.text_input_width);
            ui.label(lucide_job(Icon::Lock, theme.font_small.size, LOCKED_TITLE, theme.ink));
            ui.label(RichText::new(why).color(theme.ink_weak));
            ui.label(RichText::new(LOCKED_CTA).color(theme.phosphor));
        });
    response
}
