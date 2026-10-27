//! `R22.1`: the floating preview — the one picture of *what a thing draws*,
//! anchored to the top-right corner of whatever canvas is showing it.
//!
//! It was `panels/nodes.rs`'s `preview_pip`, a private copy that always
//! pumped `GET /preview.jpg` — the preview device's *composed* frame, every
//! animation of the graph mixed. the operator's ask (22-sept-2026) is narrower:
//! with a `gen.*` node focused, show **that node's** animation and nothing
//! else, and show nothing at all with any other selection. `R22.8` hangs the
//! same widget off the ingest lane to preview the authored sequence, which is
//! what makes it a shared widget rather than a canvas-local one.
//!
//! **The stale-texture bug this fixes.** [`frames::Frame`] remembers the
//! texture but not the query that asked for it; switching from one node to
//! another would paint the previous node's picture until the next fetch
//! landed. [`FloatingPreview`] keeps the key — `(route, query without w)` —
//! and drops the frame the moment it changes, so a new subject never paints
//! the old subject's pixels.

use egui::{Id, Pos2, Rect as EguiRect, Sense, Ui};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{lucide, Icon};
use crate::shell::frames::{live_gap, software_gpu, Fetcher, Frame, SOFT_PREVIEW_W};

/// One floating preview: the texture pump, whether a click has made it big,
/// the key of what it is currently showing, and the rect it painted last.
pub struct FloatingPreview {
    frame: Frame,
    big: bool,
    key: Option<(String, Vec<(String, String)>)>,
    /// The rect the picture occupied as of the last frame it painted — the
    /// host canvas reads it to know a press belongs to the picture and not
    /// to the graph underneath (egui hit-tests a press against the previous
    /// frame's rects, and so does this).
    rect: Option<EguiRect>,
    /// Whether each landed frame is histogrammed (the grade page's read).
    want_hist: bool,
    /// G3: the shared-device texture last presented (its frame count and size).
    gpu_landed: u64,
    gpu_size: Option<(u32, u32)>,
    /// P-G: what the picture is of — set by the host; with one, a bar under
    /// the picture names it and offers "Exportar cuadro".
    subject: Option<PreviewSubject>,
    /// The bar's rect as of the last frame (part of [`Self::hit_rect`]).
    bar: Option<EguiRect>,
    /// The bar's fps reading, and the `(when, landed)` mark it counts from.
    fps: f32,
    fps_mark: Option<(web_time::Instant, u64)>,
    /// The last export's message and when it was shown (the toast).
    toast: Option<(String, web_time::Instant)>,
    /// The last picture painted for the current key: kept on screen while a
    /// frame is Pending or a fetch fails.
    held: Option<(egui::TextureId, [usize; 2])>,
}

/// What a floating preview shows, for its bar: a label (name · kind), the
/// body `POST /frame/export` draws it from (`None` hides the camera), and
/// the output resolution/rate it really renders at (the bar falls back to
/// the landed frame's size and the measured fps).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreviewSubject {
    pub label: String,
    pub export: Option<serde_json::Value>,
    pub size: Option<(u32, u32)>,
    pub fps: Option<f32>,
}

/// How long the export toast stays up.
const TOAST: std::time::Duration = std::time::Duration::from_secs(4);
/// How often the bar's fps reading is recomputed.
const FPS_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

impl Default for FloatingPreview {
    fn default() -> Self {
        FloatingPreview {
            frame: Frame::default(),
            big: false,
            key: None,
            rect: None,
            want_hist: false,
            gpu_landed: 0,
            gpu_size: None,
            subject: None,
            bar: None,
            fps: 0.0,
            fps_mark: None,
            toast: None,
            held: None,
        }
    }
}

/// Opaque: the frame inside is a texture pump, not state worth printing.
impl std::fmt::Debug for FloatingPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FloatingPreview")
            .field("big", &self.big)
            .field("has_frame", &self.key.is_some())
            .finish()
    }
}

/// A clone restarts the pump (the frame keeps a fetch in flight, which is
/// not cloneable and not worth sharing). `IngestLane` derives `Clone` for
/// tests, and a copied lane has no business painting the original's pixels.
impl Clone for FloatingPreview {
    fn clone(&self) -> Self {
        FloatingPreview {
            big: self.big,
            ..Default::default()
        }
    }
}

impl FloatingPreview {
    /// Whether the preview is currently expanded to the host's full width —
    /// the canvas hides its floating bars then, and the kittest suite pins
    /// the toggle.
    pub fn big(&self) -> bool {
        self.big
    }

    /// The rect the picture painted last frame, if it painted one.
    pub fn rect(&self) -> Option<EguiRect> {
        self.rect
    }

    /// The picture plus its bar: what a press must not fall through.
    pub fn hit_rect(&self) -> Option<EguiRect> {
        match (self.rect, self.bar) {
            (Some(r), Some(b)) => Some(r.union(b)),
            (r, _) => r,
        }
    }

    /// P-G: name what the picture is of (or nothing, which hides the bar).
    pub fn set_subject(&mut self, subject: Option<PreviewSubject>) {
        self.subject = subject;
    }

    #[cfg(test)]
    /// The export toast's text while it is up.
    pub fn toast(&self) -> Option<&str> {
        self.toast.as_ref().map(|(m, _)| m.as_str())
    }

    #[cfg(test)]
    /// The bar's current fps reading (frames landed per second).
    pub fn fps(&self) -> f32 {
        self.fps
    }

    /// The bar under the picture: the camera button ("Exportar cuadro"),
    /// the subject's label, the landed frame's resolution and the fps.
    /// The camera posts `POST /frame/export` through the door (regla 9) and
    /// a toast over the bar shows where the PNG landed.
    fn paint_bar(&mut self, ui: &mut Ui, theme: &Theme, door: &dyn Call, host_rect: EguiRect) {
        use crate::shell::i18n::{tr, trf};
        self.bar = None;
        let (Some(subject), Some(rect)) = (self.subject.clone(), self.rect) else {
            return;
        };
        let landed = self.frame.landed() + self.gpu_landed;
        match self.fps_mark {
            Some((at, n)) if at.elapsed() >= FPS_WINDOW => {
                self.fps = (landed - n) as f32 / at.elapsed().as_secs_f32();
                self.fps_mark = Some((web_time::Instant::now(), landed));
            }
            None => self.fps_mark = Some((web_time::Instant::now(), landed)),
            _ => {}
        }
        let pad = f32::from(theme.pad);
        let h = theme.interact_size.y + pad;
        let top = (rect.max.y + pad).min(host_rect.max.y - h);
        let bar = EguiRect::from_min_size(Pos2::new(rect.min.x, top), egui::vec2(rect.width(), h));
        let painter = ui.painter_at(host_rect);
        painter.rect_filled(bar, theme.corner_radius(), theme.faded(theme.inset, theme.caption_alpha));
        let (w, hpx) = subject.size.unwrap_or_else(|| {
            self.gpu_size.unwrap_or_else(|| {
                self.frame
                    .current()
                    .map(|t| (t.size()[0] as u32, t.size()[1] as u32))
                    .unwrap_or_default()
            })
        });
        let fps = subject.fps.unwrap_or(self.fps);
        let readout = trf("{label} · {w}×{h} · {fps} fps",
            &[
                ("label", &subject.label),
                ("w", &w),
                ("h", &hpx),
                ("fps", &format!("{fps:.0}")),
            ],
        );
        let mut export = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar.shrink(pad * 0.5)), |ui| {
            ui.horizontal(|ui| {
                if subject.export.is_some() {
                    export = ui
                        .button(lucide(Icon::Camera, theme.font_small.size))
                        .on_hover_text(tr("export frame"))
                        .clicked();
                }
                ui.label(egui::RichText::new(readout).font(theme.font_small.clone()).color(theme.ink_weak));
            });
        });
        if let Some(body) = subject.export.clone().filter(|_| export) {
            let msg = match door.json("POST", "/frame/export", Some(body)) {
                Ok(v) => trf("saved {path}", &[("path", &v["path"].as_str().unwrap_or_default())]),
                Err((_, e)) => trf("export failed: {err}", &[("err", &e)]),
            };
            self.toast = Some((msg, web_time::Instant::now()));
        }
        if let Some((msg, at)) = &self.toast {
            if at.elapsed() < TOAST {
                let galley = painter.layout_no_wrap(msg.clone(), theme.font_small.clone(), theme.ink);
                let at_pos = Pos2::new(bar.min.x + pad, bar.min.y - galley.size().y - pad * 1.5);
                painter.rect_filled(
                    EguiRect::from_min_size(at_pos, galley.size()).expand(pad * 0.5),
                    theme.corner_radius(),
                    theme.faded(theme.inset, theme.caption_alpha),
                );
                painter.galley(at_pos, galley, theme.ink);
                ui.ctx().request_repaint_after(TOAST);
            } else {
                self.toast = None;
            }
        }
        self.bar = Some(bar);
    }

    /// Pumps `route` (with `query`, plus the `w` the paint width resolves
    /// to) and paints the frame over `host_rect`, top-right, at
    /// `theme.graph_preview_share` of the host's width — the host's whole
    /// width once clicked. A frame for a different key is dropped before
    /// the tick, never painted. Nothing paints until the route answers once.
    ///
    /// `host_rect` is the canvas the preview floats over, and also its clip
    /// rect: the picture never spills past the canvas it belongs to.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        door: &dyn Call,
        fetcher: Option<&Fetcher>,
        host_rect: EguiRect,
        route: &str,
        query: &[(&str, String)],
    ) {
        let width = if self.big {
            host_rect.width()
        } else {
            host_rect.width() * theme.graph_preview_share
        };
        let pad = f32::from(theme.pad);
        let right = host_rect.max.x - pad;
        if self.show_sized(ui, theme, door, fetcher, host_rect, right, width, route, query, None) {
            self.big = !self.big;
        }
        self.paint_bar(ui, theme, door, host_rect);
    }

    /// The before/after pair (2026-10-09): `self` paints the focused
    /// effect's own stage (after) at the top-right and `before` the picture
    /// feeding its `image` input just left of it, both captioned. A click on
    /// either toggles a split view where each takes half the canvas.
    #[allow(clippy::too_many_arguments)]
    pub fn show_compare(
        &mut self,
        before: &mut FloatingPreview,
        ui: &mut Ui,
        theme: &Theme,
        door: &dyn Call,
        fetcher: Option<&Fetcher>,
        host_rect: EguiRect,
        after: (&str, &[(&str, String)]),
        source: (&str, &[(&str, String)]),
    ) {
        let pad = f32::from(theme.pad);
        let width = if self.big {
            (host_rect.width() - pad * 3.0) * 0.5
        } else {
            host_rect.width() * theme.compare_share
        };
        let right = host_rect.max.x - pad;
        let mut clicked = self.show_sized(
            ui, theme, door, fetcher, host_rect, right, width, after.0, after.1,
            Some(crate::shell::i18n::tr("after")),
        );
        let left_edge = right - width;
        clicked |= before.show_sized(
            ui, theme, door, fetcher, host_rect, left_edge - pad, width, source.0, source.1,
            Some(crate::shell::i18n::tr("before")),
        );
        if clicked {
            self.big = !self.big;
        }
        self.paint_bar(ui, theme, door, host_rect);
    }

    /// Ask the picture for a histogram of each frame it lands (or stop).
    pub fn want_histogram(&mut self, on: bool) {
        self.want_hist = on;
        self.frame.want_histogram(on);
    }

    /// The last landed frame's histogram, when asked for.
    pub fn histogram(&self) -> Option<&crate::shell::frames::Histogram> {
        self.frame.histogram()
    }

    /// One picture `width` wide whose right edge sits at `right`, at the top
    /// of `host_rect` (its clip), with an optional caption in its corner.
    /// Returns whether it was clicked.
    #[allow(clippy::too_many_arguments)]
    fn show_sized(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        door: &dyn Call,
        fetcher: Option<&Fetcher>,
        host_rect: EguiRect,
        right: f32,
        width: f32,
        route: &str,
        query: &[(&str, String)],
        caption: Option<&str>,
    ) -> bool {
        let key = (
            route.to_string(),
            query
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<Vec<_>>(),
        );
        if self.key.as_ref() != Some(&key) {
            self.frame = Frame::default();
            self.frame.want_histogram(self.want_hist);
            self.held = None;
            self.key = Some(key.clone());
        }
        self.rect = None;

        // Ask for a frame the size it will be painted at (the route scales),
        // rounded so the query does not change every sub-pixel resize.
        let mut q: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        // Under a software GPU the picture is asked for at most
        // `SOFT_PREVIEW_W` wide; the rate is the same 60 fps cap everywhere
        // (`live_gap()`). The painted size is unchanged, only the work shrinks.
        let soft = software_gpu();
        let asked = if soft { width.min(SOFT_PREVIEW_W) } else { width };
        let gap = live_gap();
        q.push(("w".to_string(), (asked.max(1.0) as u32).to_string()));
        let refs: Vec<(&str, String)> = q.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let armed = pick_target(ui.ctx()).is_some();
        self.frame.want_sample(armed);
        // G3: a device frame is presented straight from the painter's texture
        // when the window shares its device; the grade's histogram keeps the
        // JPEG. The eyedropper reads its pixel off the same texture (2026-10-12).
        let full = EguiRect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        // G4: a node's own stage is a texture too (when its buffer stayed one).
        // G5: a source's and the author's solo pictures are textures too.
        let device_route = matches!(
            route,
            "/output.jpg" | "/preview.jpg" | "/node.jpg" | "/source.jpg" | "/author/render/solo.jpg"
        );
        let mut shared = None;
        let mut shared_key = None;
        let mut pending = false;
        if device_route {
            if let Some(f) = fetcher {
                let k = format!("{}?{:?}", route, key.1);
                match self.frame.tick_texture(ui.ctx(), f, &k, route, &refs, gap) {
                    crate::shell::present::Shown::Ready { id, size, landed } => {
                        self.gpu_landed = landed;
                        self.gpu_size = Some((size[0], size[1]));
                        shared = Some((id, [size[0] as usize, size[1] as usize]));
                        shared_key = Some(k);
                    }
                    crate::shell::present::Shown::Pending => {
                        if self.held.is_none() {
                            return false;
                        }
                        pending = true;
                    }
                    crate::shell::present::Shown::Unsupported => {}
                }
            }
        }
        let (tex_id, tex_size) = match shared {
            Some(s) => s,
            None if pending => self.held.unwrap_or_default(),
            None => {
                match self
                    .frame
                    .tick(ui.ctx(), door, fetcher, route, &refs, gap)
                    .cloned()
                {
                    Some(tex) => {
                        self.gpu_size = None;
                        (tex.id(), tex.size())
                    }
                    None => match self.held {
                        Some(h) => h,
                        None => return false,
                    },
                }
            }
        };
        self.held = Some((tex_id, tex_size));

        let size = tile_size(width, tex_size, full);
        let pad = f32::from(theme.pad);
        let rect = EguiRect::from_min_size(
            Pos2::new(right - size.x, host_rect.min.y + pad),
            size,
        );
        let painter = ui.painter_at(host_rect);
        painter.image(tex_id, rect, full, theme.image_tint);
        painter.rect_stroke(
            rect,
            theme.corner_radius(),
            theme.rule_bright_stroke(),
            egui::StrokeKind::Outside,
        );
        if let Some(caption) = caption {
            let at = rect.left_top() + egui::Vec2::splat(pad);
            let galley = painter.layout_no_wrap(caption.to_string(), theme.font_small.clone(), theme.ink);
            painter.rect_filled(
                EguiRect::from_min_size(at, galley.size()).expand(pad * 0.5),
                theme.corner_radius(),
                theme.faded(theme.inset, theme.caption_alpha),
            );
            painter.galley(at, galley, theme.ink);
        }
        self.rect = Some(rect);
        // One id per subject, so two previews in one frame (the nodes canvas
        // and the ingest lane, `R22.8`) never fight over the click.
        let id = Id::new(("shell/preview", &key));
        let response = ui.interact(rect, id, Sense::click());
        if armed && response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let (u, v) = ((pos.x - rect.min.x) / rect.width(), (pos.y - rect.min.y) / rect.height());
                match (shared_key, fetcher) {
                    // The picture is the device texture: its pixel is read there.
                    (Some(k), Some(f)) => f.presenter().request_pick(&k, u, v),
                    _ => {
                        if let Some(rgb) = self.frame.sample(u, v) {
                            ui.ctx().data_mut(|d| d.insert_temp(picked_id(), rgb));
                        }
                    }
                }
            }
            return false;
        }
        response.clicked()
    }
}

/// A painted tile's size at `width` points, keeping the crop's own aspect
/// (read off the texture and the `uv`, so no aspect literal lives here).
/// Shared with `panels/nodes.rs`'s `+ nodo` hover tiles.
pub fn tile_size(width: f32, size: [usize; 2], uv: EguiRect) -> egui::Vec2 {
    let [tw, th] = size;
    let (w, h) = (tw as f32 * uv.width(), th as f32 * uv.height());
    if w <= 0.0 || h <= 0.0 {
        return egui::Vec2::splat(width);
    }
    egui::Vec2::new(width, width * h / w)
}

/// 2026-10-01: **one** floating preview. The sequence editor used to float a
/// second picture over the timeline; it now hands its picture to the nodes
/// canvas's preview through this slot (egui memory, one frame of grace), and
/// paints its own only while the canvas is not on screen (the tabbed layout).
#[derive(Clone, Debug, PartialEq)]
pub struct SequencePicture {
    pub route: String,
    pub query: Vec<(String, String)>,
    /// The timeline is the focused region: its picture wins over a node's.
    pub focused: bool,
    /// The editor's playback is running — the play control shows pause.
    pub playing: bool,
    /// P-G: the bar's subject for the sequence (name, export body, size).
    pub subject: Option<PreviewSubject>,
    /// The frame it was published on.
    frame: u64,
}

fn sequence_id() -> Id {
    Id::new("shell/preview/sequence")
}

fn canvas_id() -> Id {
    Id::new("shell/preview/canvas-frame")
}

fn toggle_id() -> Id {
    Id::new("shell/preview/play-toggle")
}

/// The sequence editor's picture this frame.
pub fn publish_sequence(
    ctx: &egui::Context,
    route: &str,
    query: Vec<(String, String)>,
    focused: bool,
    playing: bool,
    subject: Option<PreviewSubject>,
) {
    let frame = ctx.cumulative_frame_nr();
    let pic = SequencePicture { route: route.to_string(), query, focused, playing, subject, frame };
    ctx.data_mut(|d| d.insert_temp(sequence_id(), pic));
}

/// The sequence picture published this frame or the last one.
pub fn sequence(ctx: &egui::Context) -> Option<SequencePicture> {
    let now = ctx.cumulative_frame_nr();
    ctx.data(|d| d.get_temp::<SequencePicture>(sequence_id())).filter(|p| p.frame + 1 >= now)
}

/// The nodes canvas marks itself painted, so the timeline knows its picture
/// has somewhere to go.
pub fn mark_canvas(ctx: &egui::Context) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(canvas_id(), frame));
}

/// Whether the nodes canvas painted this frame or the last one.
pub fn canvas_showing(ctx: &egui::Context) -> bool {
    let now = ctx.cumulative_frame_nr();
    ctx.data(|d| d.get_temp::<u64>(canvas_id())).is_some_and(|f| f + 1 >= now)
}

fn canvas_rect_id() -> Id {
    Id::new("shell/preview/canvas-rect")
}

/// The canvas's own rect, stamped with the frame it painted on — so a rect
/// left over from a canvas that is no longer on screen (a hidden tab, a
/// delegated layout's inactive half) reads as absent, exactly the one-frame
/// grace [`mark_canvas`]/[`canvas_showing`] keep. The biblioteca's preview
/// reads this to float over the nodes canvas rather than over its own panel.
pub fn set_canvas_rect(ctx: &egui::Context, rect: egui::Rect) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(canvas_rect_id(), (frame, rect)));
}

/// The nodes canvas's rect this frame or the last one, if it painted.
pub fn canvas_rect(ctx: &egui::Context) -> Option<egui::Rect> {
    let now = ctx.cumulative_frame_nr();
    ctx.data(|d| d.get_temp::<(u64, egui::Rect)>(canvas_rect_id()))
        .filter(|(f, _)| *f + 1 >= now)
        .map(|(_, r)| r)
}

/// The eyedropper: the `(graph, node)` whose key colour is being picked, or
/// `None`. While set, a click on a picture reads its colour instead of
/// toggling the picture's size.
pub fn set_pick_target(ctx: &egui::Context, target: Option<(String, String)>) {
    ctx.data_mut(|d| d.insert_temp(pick_target_id(), target));
}

/// The node the eyedropper is armed for, if any.
pub fn pick_target(ctx: &egui::Context) -> Option<(String, String)> {
    ctx.data(|d| d.get_temp::<Option<(String, String)>>(pick_target_id())).flatten()
}

/// The window's device read the pixel a click asked for (`present.rs`).
pub fn deliver_picked(ctx: &egui::Context, rgb: [u8; 3]) {
    ctx.data_mut(|d| d.insert_temp(picked_id(), rgb));
}

/// The colour a click picked, waiting for its node's page to take it.
pub fn take_picked(ctx: &egui::Context) -> Option<[u8; 3]> {
    ctx.data_mut(|d| d.remove_temp::<[u8; 3]>(picked_id()))
}

fn pick_target_id() -> Id {
    Id::new("shell/preview/pick-target")
}

fn picked_id() -> Id {
    Id::new("shell/preview/picked")
}

/// The preview's play control was pressed; the timeline drains it.
pub fn request_play_toggle(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(toggle_id(), true));
}

pub fn take_play_toggle(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(toggle_id())).unwrap_or(false)
}

/// The play/pause control, inside a painted preview `rect` at its foot (the
/// picture may fill the whole canvas, so below it is not always on screen);
/// true when clicked. Added after the picture's own click, so it wins.
pub fn play_control(ui: &mut Ui, theme: &Theme, rect: EguiRect, playing: bool) -> bool {
    let pad = f32::from(theme.pad);
    let at = EguiRect::from_min_size(
        Pos2::new(rect.min.x + pad, rect.max.y - pad - theme.interact_size.y),
        egui::Vec2::new(rect.width() - pad - pad, theme.interact_size.y),
    );
    let label = if playing {
        crate::shell::i18n::tr("⏸ pause sequence")
    } else {
        crate::shell::i18n::tr(" play sequence")
    };
    let mut clicked = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
        clicked = ui.button(egui::RichText::new(label).font(theme.font_small.clone())).clicked();
    });
    clicked
}

fn histogram_id() -> Id {
    Id::new("shell/preview/histogram")
}

/// 2026-10-09: the focused grade's stage histogram, handed from the nodes
/// canvas (which pumps the picture) to the grade page (which paints it).
pub fn publish_histogram(ctx: &egui::Context, h: &crate::shell::frames::Histogram) {
    let frame = ctx.cumulative_frame_nr();
    let boxed = std::sync::Arc::new(*h);
    ctx.data_mut(|d| d.insert_temp(histogram_id(), (frame, boxed)));
}

/// The histogram published this frame or the last few, if any.
pub fn histogram(ctx: &egui::Context) -> Option<std::sync::Arc<crate::shell::frames::Histogram>> {
    let now = ctx.cumulative_frame_nr();
    ctx.data(|d| d.get_temp::<(u64, std::sync::Arc<crate::shell::frames::Histogram>)>(histogram_id()))
        .filter(|(f, _)| f + HIST_GRACE >= now)
        .map(|(_, h)| h)
}

/// How many frames a published histogram stays readable — the properties
/// panel may paint before the canvas in a frame.
const HIST_GRACE: u64 = 3;

/// P-G: output `device`'s resolution as `GET /state` answers it — what an
/// export writes, so the bar shows it rather than the preview's size.
pub fn output_size(door: &dyn Call, device: &str) -> Option<(u32, u32)> {
    let state = door.json("GET", "/state", None).ok()?;
    let out = state["outputs"].as_array()?.iter().find(|o| o["id"].as_str() == Some(device))?;
    Some((out["width"].as_u64()? as u32, out["height"].as_u64()? as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::FakeDoor;

    fn jpeg() -> Vec<u8> {
        let mut out = Vec::new();
        let enc = jpeg_encoder::Encoder::new(&mut out, 90);
        enc.encode(&[200u8; 16 * 16 * 3], 16, 16, jpeg_encoder::ColorType::Rgb).unwrap();
        out
    }

    fn run(door: &FakeDoor, a: &mut FloatingPreview, b: &mut FloatingPreview) {
        let theme = Theme::phosphor();
        let mut h = egui_kittest::Harness::new_ui(|ui| {
            let host = ui.max_rect();
            a.show_compare(b, ui, &theme, door, None, host, ("/node.jpg", &[]), ("/source.jpg", &[]));
        });
        h.run();
    }

    /// A fetch that fails after one good frame keeps the picture on screen,
    /// and one failing side never hides the other.
    #[test]
    fn a_failed_fetch_keeps_the_last_picture() {
        let door = FakeDoor::new();
        door.respond_bytes("GET", "/node.jpg", "image/jpeg", jpeg());
        door.respond_bytes("GET", "/source.jpg", "image/jpeg", jpeg());
        let (mut a, mut b) = (FloatingPreview::default(), FloatingPreview::default());
        run(&door, &mut a, &mut b);
        assert!(a.rect().is_some() && b.rect().is_some());
        door.respond_error("GET", "/node.jpg", 503, "busy");
        run(&door, &mut a, &mut b);
        assert!(a.rect().is_some(), "after keeps its last frame");
        assert!(b.rect().is_some());
        door.respond_error("GET", "/source.jpg", 503, "busy");
        let mut fresh = FloatingPreview::default();
        run(&door, &mut fresh, &mut b);
        assert!(fresh.rect().is_none(), "never landed: nothing to paint");
        assert!(b.rect().is_some(), "before survives the after failing");
    }
}
