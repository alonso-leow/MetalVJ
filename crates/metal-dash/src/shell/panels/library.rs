//! `S.U11`: the library panel — a **browser**, two columns — in the
//! [`super::Region::Library`] region (`plan-sonnet.md` § S.U11,
//! `plan-opus.md` § O.12.e). Rebuilt 2026-09-15 (`paneles-inferiores.md`
//! § P1); `R22.2` (22-sept-2026) put the files in a grid of cards.
//!
//! **Scope, in two columns.** (Until 21-sept-2026 the studio's live-preview
//! grid and solo sat above them — the old HTML dashboard's `<img id="rack">`
//! ported here for want of any image consumer. They were never the
//! library's: the grid is now the picture of each `gen.*` on the nodes
//! canvas and in the `+ nodo` menu's hover, and the solo — the studio
//! bench's selected-model render, not the composed output the monitor
//! shows — is gone; the animation itself is the shared
//! [`crate::shell::preview::FloatingPreview`], `R22.1`.)
//! (1) A left column of *folders*: a search box, the current path with an `↑`, one level of
//! `GET /fs`, a `…` per folder and per file (`import` → `Shell::route_library_intents`, which
//! adds the file or folder to the open graph as a source; `remove`, and on the folder
//! in use `train curator`),
//! the curator status on that folder's row. (2) A right column of *files*: the same folder's one level, a
//! list of `theme.library_row` rows (2026-10-01; a grid of cards before) — icon, truncated name with
//! the full one on its tooltip, and a weak status line — where **the whole
//! card is the button**: a click on audio is `play` (adding it to the song
//! library first if it is not one), a click on a sequence is `load` with
//! `view`/`load` on right click, and video/image cards stay information
//! only. The old collections/index/movements sections are gone:
//! collections lives in the header now; a folder is indexed lazily (`P0`) and
//! the operator never sees an "index" step.
//!
//! **Rule 5.** Every value painted here is exactly what a route already
//! resolved — a clip's length, a song's digest status, a folder's contents.
//! Nothing here re-derives one.
//!
//! **Rule 6.** Every call goes through [`crate::call::Call`] —
//! `door.call`/`door.json` — never a handler body, and never a
//! `crate::call::<submodule>` path (gate 1). `/author/*` reaches
//! `Author::dispatch` through the door's own branch (`S.U11`), exactly the
//! split `main.rs::serve` already made.
//!
//! **What is deliberately not here — the leftover, `S.U11b`.** `curator.js`'s
//! tick-by-tick pick lane (`lanes`/`tickUnder`/`paint`/`goTick`/
//! `writeKeyframe`/`clearKeyframe`/`laneOp` and their pure-edit half) is a
//! second layout surface comparable in size to `nodes.rs`'s graph canvas,
//! not a form of buttons; the folder column paints only the curator's status
//! display. This repo has no file-dialog dependency, so there is no
//! "add by path" any more — an audio is added by browsing to it and
//! clicking its card.

use std::collections::HashMap;
use crate::shell::components::{filter_chips, icon_button};
use std::path::PathBuf;

use egui::{
    CentralPanel, Frame, Id, Panel, Pos2, Rect, Response, RichText, ScrollArea, Sense, Ui,
    UiBuilder, Vec2,
};
use serde_json::{json, Value};

use crate::call::{Call, Params};

use crate::shell::classes::Theme;
use crate::surface::LibraryIntent;
use crate::shell::components::{
    icon_toggle, paint_icon, paint_icon_stroked, panel_frame, header_edit, panel_head, search_field, section_heading, segmented,
    Icon,
};
use crate::shell::i18n::tr;
use crate::shell::media;
use crate::shell::preview::PreviewSubject;

/// Everything the library panel remembers between frames — form fields the
/// operator is mid-typing, a folder's cached file listing, and the one
/// piece of state `confirmish` (`shell.js`) needs that a door reply cannot
/// give it: which song, if any, is waiting on an inline confirm before
/// `POST /author/open` actually fires. `GET /state`, `GET /author/songs`
/// and `GET /author/curator/state` are *not* cached here — like
/// `lanes.rs`'s own `PanelState`, they are re-read fresh every frame this
/// panel paints, because a digest finishing or a folder's index job
/// advancing keeps moving whether or not this panel happens to be open.
/// A file or folder `import` was raised on (2026-10-10): `kind` is the
/// library's own word (`image|video|audio`) or `folder`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Import {
    pub path: String,
    pub kind: String,
}

pub struct PanelState {
    folder_query: String,
    file_query: String,
    /// The folder the right column lists — `$HOME` until the operator walks
    /// somewhere else. `fs` is its cached `GET /fs` reply, fetched once when
    /// `cwd` changes (never per frame, the same rule the old metraje column
    /// kept).
    cwd: PathBuf,
    fs: Option<Value>,
    files_note: Option<String>,
    /// The path field's text while it is being typed into; `None` shows
    /// `cwd` itself (2026-10-10: the path field replaced the `↑` and label).
    cwd_edit: Option<String>,

    song_note: Option<String>,
    /// `(name, audio)` of a song `play` was clicked for while a *different*
    /// song was already open — the inline stand-in for `shell.js`'s
    /// `confirmish`, which armed a two-click/two-second window instead. No
    /// route this shell can reach reports whether the currently open song
    /// has unsaved edits (that flag lives in the absorbed studio's own
    /// per-driver `dirty` counters, never serialised into any payload this
    /// panel reads), so this guards the coarser, always-safe condition:
    /// switching away from *any* other open song, not just a dirty one.
    confirm_open: Option<(String, String)>,
    add_note: Option<String>,

    curator_note: Option<String>,
    /// The card last clicked (2026-09-30): outlined in the grid, and raised
    /// once as `picked_file` for the properties panel to show.
    selected_path: Option<String>,
    picked_file: Option<crate::shell::panels::FileRef>,
    /// An `import` the `…` menu raised, drained once by the wiring (`mod.rs`).
    import: Option<Import>,

    /// Hooks the wiring (`paneles-inferiores.md` § P3) drains: one per
    /// intent the browser can raise that the lanes panel acts on. Set here,
    /// read in `shell/mod.rs`; nothing else writes them. `R22.7`: the song
    /// hook carries the **stem**, never the path — every ingest route
    /// (`/author/grade/matrix`, `/author/models/example`, …) looks the song
    /// up as `data/audio/{stem}.8hz.npy`, so a path here loads nothing.
    last_opened_song: Option<String>,
    sequence_to_show: Option<String>,
    sequence_to_load: Option<String>,
    /// A video card's 'add as layer' menu item (touch-friendly, the
    /// drag-free twin of dropping the card on the timeline). `Shell` drains it
    /// into the ingest lane, the same way a timeline drop lands.
    clip_to_layer: Option<crate::shell::panels::FileDrag>,

    /// The right column's route (2026-10-01): the folder's files, or every
    /// kept scene across all videos ([`super::scenes::route`]).
    scenes_open: bool,
    scenes: super::scenes::RouteState,

    /// The files column's filters (2026-10-08): show only favourites, and
    /// narrow to one kind. Both are view-only — they change what is listed,
    /// never what the door is asked for.
    fav_only: bool,
    /// `0` all, `1` audio, `2` video, `3` image, `4` sequence — the index of
    /// the kind row's own segment.
    kind_filter: usize,

    /// The simplified sequence route is the right column's third route
    /// (2026-10-15), beside `scenes_open`'s scene index.
    sequence_open: bool,

    /// The media board's pick, drawn over the whole browser (2026-10-15): an
    /// image is one frame, a video replays off [`Self::preview_clock`].
    preview: crate::shell::preview::FloatingPreview,
    /// `(clip key, the input time that clip began)` — a video seeks from its
    /// span's start off this clock; a new pick resets it.
    preview_clock: Option<(String, f64)>,

    /// The control surface's search dial (2026-10-28): `true` while it walks
    /// the folders column, `false` the files. Steps and a `load` wait here
    /// until the column they belong to paints.
    dial_folders: bool,
    dial_step: i32,
    dial_load: bool,
    /// The folder the dial rests on (`..` is the parent).
    dial_dir: Option<String>,
    /// The dial just moved: the row it rests on scrolls into view.
    dial_scroll: bool,
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState {
            folder_query: String::new(),
            file_query: String::new(),
            cwd: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/")),
            fs: None,
            files_note: None,
            cwd_edit: None,
            song_note: None,
            confirm_open: None,
            add_note: None,
            curator_note: None,
            selected_path: None,
            picked_file: None,
            import: None,
            last_opened_song: None,
            sequence_to_show: None,
            sequence_to_load: None,
            clip_to_layer: None,
            scenes_open: false,
            scenes: Default::default(),
            fav_only: false,
            kind_filter: 0,
            sequence_open: false,
            preview: crate::shell::preview::FloatingPreview::default(),
            preview_clock: None,
            dial_folders: false,
            dial_step: 0,
            dial_load: false,
            dial_dir: None,
            dial_scroll: false,
        }
    }
}

impl PanelState {
    /// Test-only: arm the confirm popover directly, the same trade
    /// `lanes::PanelState::set_trigger_id` makes for a `TextEdit` — a
    /// kittest test can then exercise cancel/confirm without depending on
    /// the exact pixels a synthetic click into a `menu_button` needs to
    /// land on.
    #[cfg(test)]
    pub(crate) fn set_confirm_open(&mut self, name: &str, audio: &str) {
        self.confirm_open = Some((name.to_string(), audio.to_string()));
    }

    /// Test-only: point the browser at a folder without walking to it, the
    /// same trade `set_confirm_open` above makes for a form the test does
    /// not want to simulate clicks into.
    #[cfg(test)]
    /// The rows of the folder listing the panel last read (`GET /fs`), for the
    /// chat's `@` completion; empty until the library has been painted.
    pub(crate) fn mention_files(&self) -> Vec<Value> {
        self.fs.as_ref().and_then(|f| f["files"].as_array().cloned()).unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn set_fs_for_test(&mut self, fs: Value) {
        self.fs = Some(fs);
    }

    pub(crate) fn set_cwd(&mut self, path: &str) {
        self.cwd = PathBuf::from(path);
        self.fs = None;
    }

    /// Test-only: arm the files column's view filters without clicking the
    /// star and the kind row — the same trade `set_cwd` makes for the walk.
    #[cfg(test)]
    pub(crate) fn set_filters(&mut self, fav_only: bool, kind_filter: usize) {
        self.fav_only = fav_only;
        self.kind_filter = kind_filter;
    }

    /// Test-only readbacks for the three wiring hooks — `shell/mod.rs`'s
    /// `route_library_intents` is what consumes them in the shipped app.
    #[cfg(test)]
    pub(crate) fn last_opened_song(&self) -> Option<&str> {
        self.last_opened_song.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn sequence_to_show(&self) -> Option<&str> {
        self.sequence_to_show.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn sequence_to_load(&self) -> Option<&str> {
        self.sequence_to_load.as_deref()
    }

    /// Test-only setters for the hooks, so the wiring tests can raise one
    /// without a full browse-then-click through the browser.
    #[cfg(test)]
    pub(crate) fn set_last_opened_song_for_test(&mut self, song: &str) {
        self.last_opened_song = Some(song.to_string());
    }

    #[cfg(test)]
    pub(crate) fn set_sequence_to_load_for_test(&mut self, name: &str) {
        self.sequence_to_load = Some(name.to_string());
    }

    /// Takes the "an audio was opened" hook (`P3`): `Shell::route_library_intents`
    /// is the one reader, so the intent fires once.
    pub(crate) fn take_last_opened_song(&mut self) -> Option<String> {
        self.last_opened_song.take()
    }

    pub(crate) fn take_sequence_to_show(&mut self) -> Option<String> {
        self.sequence_to_show.take()
    }

    /// Takes the video a card asked to add as a timeline layer.
    pub(crate) fn take_clip_to_layer(&mut self) -> Option<crate::shell::panels::FileDrag> {
        self.clip_to_layer.take()
    }

    /// Takes the file a card click picked — `Shell` turns it into
    /// `Selection::File`, so the properties panel follows the library too.
    /// Takes the `import` a `…` menu raised; `Shell::route_library_intents`
    /// is the one reader, so it runs once.
    pub(crate) fn take_import(&mut self) -> Option<Import> {
        self.import.take()
    }

    /// Shows a line under the files column — how an `import` that could
    /// not run says why (no open graph, a refused engine call).
    pub(crate) fn set_note(&mut self, note: String) {
        self.add_note = Some(note);
    }

    pub(crate) fn take_picked_file(&mut self) -> Option<crate::shell::panels::FileRef> {
        self.picked_file.take()
    }

    /// TASK 3: dismiss the media preview when focus leaves the library —
    /// drops the preview's subject and its clock (and so its pumped frame),
    /// so `preview_overlay` stops painting even if a board clip lingers on
    /// the frame the focus moved. `Shell::ui` is the one caller.
    pub(crate) fn dismiss_preview(&mut self) {
        self.preview = crate::shell::preview::FloatingPreview::default();
        self.preview_clock = None;
    }

    pub(crate) fn take_sequence_to_load(&mut self) -> Option<String> {
        self.sequence_to_load.take()
    }
}

#[cfg(test)]
impl PanelState {
    /// Test-only: raise an `import` the way a `…` menu does.
    pub(crate) fn raise_import_for_test(&mut self, path: &str, kind: &str) {
        self.import = Some(Import { path: path.to_string(), kind: kind.to_string() });
    }

    /// Test-only: the line under the files column.
    pub(crate) fn note_for_test(&self) -> Option<String> {
        self.add_note.clone()
    }
}

/// Paint the whole `library` region and report the outer frame as a
/// clickable [`Response`] — the same contract [`super::nodes::paint`] and
/// [`super::lanes::paint`] keep.
pub fn paint(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    focused: bool,
) -> Response {
    let _span = crate::perf::Span::new("library");
    for intent in super::pads::take_library_intents(ui.ctx()) {
        match intent {
            LibraryIntent::Step(n) => state.dial_step += n,
            LibraryIntent::ToggleFolders => {
                state.dial_folders = !state.dial_folders;
                state.dial_step = 0;
            }
            LibraryIntent::Load => state.dial_load = true,
        }
    }
    ui.push_id(Id::new(super::Region::Library.id()), |ui| {
        // One outline, chosen once: the panel's own hairline rule at rest, the
        // accent when this region has the focus. `panel_frame` bakes its rule
        // stroke in; selecting the stroke here in one pass keeps the focused
        // panel to the same single-outline shape a form field has, rather than
        // a rule with a second stroke laid over the same border.
        let frame = panel_frame(theme).stroke(if focused {
            theme.accent()
        } else {
            theme.rule_stroke()
        });
        let inner = frame.show(ui, |ui| {
            ui.expand_to_include_rect(ui.max_rect());
            let full_rect = ui.max_rect();
            let response = ui
                .vertical(|ui| {
                // The click-to-focus catcher between the passive heading and
                // every interactive row — `nodes.rs`'s `toolbar` doc comment
                // has the full finding (egui gives an overlapping click to
                // whichever sensing widget was added most recently, so a
                // catcher painted last would win every click in this panel).
                // Each column now carries its own header bar over its own list
                // (folders left, files right), so the click catcher still goes
                // in before them: the search fields win their own clicks, the
                // bars and the rows around them fall through.
                let bg = ui.interact(full_rect, ui.id().with("click-bg"), Sense::click());
                // Two columns, the same resizable `Panel::left` +
                // `CentralPanel` split the region rows use (`regions.rs`):
                // folders left, files right, each with its own scroll and the
                // theme's library padding (`R22.2`), so neither list touches
                // the region's edge.
                let row_width = ui.available_width();
                let left_width =
                    Theme::clamp_split(row_width, theme.library_share, theme.region_min.x);
                Panel::left(Id::new("shell/region/library/dirs"))
                    .resizable(true)
                    .default_size(left_width)
                    .min_size(theme.region_min.x)
                    .max_size(row_width - theme.region_min.x)
                    .frame(Frame::new().inner_margin(theme.library_pad))
                    .show(ui, |ui| {
                        // The folders column names the region itself, so
                        // `get_by_label("biblioteca")` meets one widget.
                        panel_head(ui, theme, super::Region::Library.title(), |ui| {
                            search_field(
                                ui,
                                theme,
                                &mut state.folder_query,
                                tr("search folder…"),
                            );
                        });
                        ScrollArea::vertical()
                            .id_salt("shell/region/library/dirs/scroll")
                            .show(ui, |ui| dirs_column(ui, theme, door, state));
                    });
                CentralPanel::default_margins()
                    .frame(Frame::new().inner_margin(theme.library_pad))
                    .show(ui, |ui| {
                        // A distinct label, so the folders header stays the one
                        // `library` a test reaches.
                        panel_head(ui, theme, "library · files", |ui| {
                            search_field(
                                ui,
                                theme,
                                &mut state.file_query,
                                tr("search file…"),
                            );
                        });
                        ScrollArea::vertical()
                            .id_salt("shell/region/library/files/scroll")
                            .show(ui, |ui| files_column(ui, theme, door, state));
                    });
                bg
                })
                .inner;
            if !state.scenes_open && !state.sequence_open {
                preview_overlay(ui, theme, door, state, full_rect);
            }
            response
        });
        inner.inner
    })
    .inner
}

/// Draws the media board's current pick over the whole browser (2026-10-15):
/// the image a card selected, or the video seeking to a fresh time each frame
/// — a still is one frame, a video wraps its span and asks for the next.
fn preview_overlay(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    full_rect: Rect,
) {
    let Some(clip) = media::current(ui.ctx()) else {
        return;
    };
    if clip.kind != "image" && clip.kind != "video" {
        return;
    }
    let now = ui.input(|i| i.time);
    let key = format!(
        "{}#{}",
        clip.path,
        clip.scene.as_ref().map(|s| s.id.as_str()).unwrap_or("")
    );
    let started = match &state.preview_clock {
        Some((k, at)) if k == &key => *at,
        _ => {
            state.preview_clock = Some((key, now));
            now
        }
    };
    if clip.kind == "video" {
        let (start, end) = clip.span();
        let len = (end - start).max(0.0);
        let t = if len > 0.0 {
            start + (now - started).rem_euclid(len)
        } else {
            start
        };
        media::seek(door, &clip, t);
        crate::shell::frames::repaint_after(ui.ctx(), crate::shell::frames::live_gap());
    }
    let (route, query) = media::preview_route(&clip);
    let query: Vec<(&str, String)> = query
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    state.preview.set_subject(Some(PreviewSubject {
        label: format!("{} · {}", clip.name, clip.kind),
        export: None,
        size: None,
        fps: None,
    }));
    // TASK 1: when the nodes canvas is on screen, float the preview over it —
    // a foreground `Area` fixed at the canvas, clipped to it, so a file picked
    // in the library reads as belonging to the graph canvas. With no canvas
    // (a delegated/hidden layout) it falls back to the in-panel painting the
    // overlay always had. Either way it is painted, never laid out, so the
    // library's own content never shifts.
    match crate::shell::preview::canvas_rect(ui.ctx()) {
        Some(canvas) => {
            let _ = egui::Area::new(Id::new("shell/region/library/preview-overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(canvas.min)
                .show(ui.ctx(), |ui| {
                    ui.set_clip_rect(canvas);
                    state.preview.show(ui, theme, door, None, canvas, &route, &query);
                });
        }
        None => {
            state.preview.show(ui, theme, door, None, full_rect, &route, &query);
        }
    }
}

/// The left column: the file system's folders, one level — `GET /fs` of
/// `state.cwd`. A search box, the current path as an editable field, an
/// "indexing…" line while the lazy open's `Job` runs, one [`file_like_row`]
/// per folder (its `…` menu: `import`/`remove`, and on the
/// folder in use `train curator`), and the curator's status on the
/// folder in use.
fn dirs_column(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    refresh_fs(door, state);

    let state_val = get(door, "GET", "/state", &[]).unwrap_or_else(|| json!({}));
    let reel = &state_val["reel"];
    let open_name = reel["library"]["name"].as_str().map(str::to_string);

    path_field(ui, theme, state);

    // The lazy open's job, while it runs — the number is the `Job`'s own
    // (rule 5), never a local guess.
    if let Some(job) = reel
        .get("job")
        .filter(|j| j["done"].as_bool() != Some(true))
    {
        let pct = job["progress"].as_f64().unwrap_or(0.0) * 100.0;
        ui.label(
            RichText::new(format!("indexing… {pct:.0}%"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }

    // A dropped clip's proxy being built, beside the indexing line — same
    // coarse phase, a different pipeline. The first running job is the one
    // line the library shows (the `step` is the `Job`'s own, rule 5).
    if let Some(job) = state_val["ingest"]
        .as_array()
        .and_then(|jobs| jobs.iter().find(|j| j["done"].as_bool() != Some(true)))
    {
        let step = job["step"].as_str().unwrap_or("ingesting");
        let pct = job["progress"].as_f64().unwrap_or(0.0) * 100.0;
        ui.label(
            RichText::new(format!("{step}… {pct:.0}%"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }

    let dirs = state
        .fs
        .as_ref()
        .and_then(|f| f["dirs"].as_array().cloned())
        .unwrap_or_default();
    let q = state.folder_query.trim().to_lowercase();
    let shown: Vec<String> = dirs
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .filter(|name| q.is_empty() || name.to_lowercase().contains(&q))
        .collect();

    if shown.is_empty() {
        let text = if dirs.is_empty() {
            "no folders"
        } else {
            "no folders match the search"
        };
        ui.label(
            RichText::new(text)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }
    // The curator belongs to the folder in use (2026-09-30): its trained
    // status rides on that folder's row and `train` is a secondary option
    // in the folder's `…` menu.
    let curator = if open_name.is_some() {
        door.json("GET", "/author/curator/state", None).ok()
    } else {
        None
    };
    let trained = curator.as_ref().map(curator_status);
    // The folder in use leads, under its own heading only when there is one
    // (2026-10-10: no `folders` heading over the rest). Every row is the
    // same [`file_like_row`] the files column paints.
    let in_use: Vec<&String> = shown
        .iter()
        .filter(|n| open_name.as_deref() == Some(n.as_str()))
        .collect();
    let others: Vec<&String> = shown
        .iter()
        .filter(|n| open_name.as_deref() != Some(n.as_str()))
        .collect();
    // The search dial, while it walks folders: `..` then the rows in the
    // order they paint; `load` walks into the one it rests on.
    if state.dial_folders {
        let mut entries: Vec<String> = Vec::new();
        if state.cwd.parent().is_some() {
            entries.push("..".to_string());
        }
        entries.extend(in_use.iter().chain(&others).map(|n| n.to_string()));
        if let Some(to) = dial_move(&entries, state.dial_dir.as_deref(), std::mem::take(&mut state.dial_step)) {
            state.dial_dir = Some(to);
            state.dial_scroll = true;
        }
        if std::mem::take(&mut state.dial_load) {
            match state.dial_dir.take().as_deref() {
                Some("..") => {
                    if let Some(up) = state.cwd.parent().map(|p| p.to_path_buf()) {
                        state.cwd = up;
                    }
                }
                Some(name) => state.cwd = state.cwd.join(name),
                None => {}
            }
            state.fs = None;
            state.files_note = None;
        }
        if state.cwd.parent().is_some() {
            let dialled = state.dial_dir.as_deref() == Some("..");
            let (response, rect) = file_like_row(ui, theme, Icon::ArrowUp, "..", None, false, None);
            dial_mark(ui, theme, state, &response, rect, dialled);
            if response.clicked() {
                if let Some(up) = state.cwd.parent().map(|p| p.to_path_buf()) {
                    state.cwd = up;
                    state.fs = None;
                    state.files_note = None;
                }
            }
        }
    }
    if !in_use.is_empty() {
        section_heading(ui, theme, "in use");
        for name in &in_use {
            dir_row(ui, theme, door, state, name, true, trained.as_deref());
        }
    }
    for name in &others {
        dir_row(ui, theme, door, state, name, false, trained.as_deref());
    }
    if let Some(note) = state.curator_note.clone() {
        ui.label(
            RichText::new(note)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }
}

/// The current path as one editable field, the width of the search box above
/// it and outlined in the bright rule so it reads as the place the column
/// shows (2026-10-20: the accent stroke was the phosphor red, which read as an
/// error beside the neutral search boxes; a neutral single outline replaced
/// it). Enter (or leaving the field with changed text) walks to the typed path
/// and drops the cached `GET /fs`; an empty or unchanged entry only reverts.
fn path_field(ui: &mut Ui, theme: &Theme, state: &mut PanelState) {
    let shown = state.cwd.display().to_string();
    let mut text = state.cwd_edit.clone().unwrap_or_else(|| shown.clone());
    let parent = state.cwd.parent().map(|p| p.to_path_buf());
    let response = ui
        .horizontal(|ui| {
            // `back`: one level up, the way the field's own path walks.
            let back = ui
                .add_enabled_ui(parent.is_some(), |ui| icon_button(ui, theme, Icon::ArrowLeft, false, tr("back")))
                .inner;
            if back.clicked() {
                if let Some(up) = parent {
                    state.cwd = up;
                    state.fs = None;
                    state.files_note = None;
                    state.cwd_edit = None;
                }
            }
            // The same one-line field as the search boxes above (`header_edit`),
            // so the location reads as one of the panel's inputs.
            ui.add(header_edit(theme, &mut text, tr("path")))
        })
        .inner;
    if response.lost_focus() {
        let typed = text.trim();
        if !typed.is_empty() && typed != shown {
            state.cwd = PathBuf::from(typed);
            state.fs = None;
            state.files_note = None;
        }
        state.cwd_edit = None;
    } else if response.changed() || response.has_focus() {
        state.cwd_edit = Some(text);
    }
}

/// One folder of the left column: its name (a click walks into it) and the
/// `…` menu (`import`/`remove`, and `train curator`
/// on the folder in use). The row is [`file_like_row`], so it lines up with
/// the file rows on the right.
fn dir_row(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    name: &str,
    on: bool,
    trained: Option<&str>,
) {
    let subtitle = match (on, trained) {
        (true, Some(t)) => Some(format!("in use · {t}")),
        (true, None) => Some("in use".to_string()),
        _ => None,
    };
    let path = state.cwd.join(name).to_string_lossy().into_owned();
    let tag = row_tag(door, &path);
    let (response, rect) =
        file_like_row(ui, theme, Icon::Folder, name, subtitle.as_deref(), false, tag);
    let dialled = state.dial_folders && state.dial_dir.as_deref() == Some(name);
    dial_mark(ui, theme, state, &response, rect, dialled);
    // A folder drags too: dropped on the canvas it becomes a `reel.batch`
    // over what it holds.
    response.dnd_set_drag_payload(crate::shell::panels::FileDrag {
        name: name.to_string(),
        path: path.clone(),
        kind: "folder".to_string(),
        seconds: None,
        scene: None,
    });
    drag_ghost_row(ui, theme, &response, rect, Icon::Folder, name, subtitle.as_deref(), tag);
    favourite_star(ui, theme, rect, &crate::shell::favorites::file_entry("folder", name, &path));
    if response.clicked() {
        state.cwd = state.cwd.join(name);
        state.fs = None;
        state.files_note = None;
    }
    row_menu(ui, theme, rect, &response, |ui| {
        if ui
            .button(RichText::new("import").font(theme.font_small.clone()))
            .clicked()
        {
            state.import = Some(Import {
                path: state.cwd.join(name).to_string_lossy().into_owned(),
                kind: "folder".to_string(),
            });
            ui.close();
        }
        colour_submenu(ui, theme, door, &path);
        if on
            && ui
                .button(RichText::new("remove").font(theme.font_small.clone()))
                .clicked()
        {
            let _ = door.json("POST", "/reel", Some(json!({ "op": "close" })));
            ui.close();
        }
        if on {
            ui.separator();
            if ui
                .button(RichText::new("train curator").font(theme.font_small.clone()))
                .clicked()
            {
                state.curator_note = Some(train_curator(door, name));
                ui.close();
            }
        }
    });
}

/// One row of either column (2026-10-10): a folder and a file look the same.
/// The row is the full width of its column and [`Theme::library_card_h`] tall;
/// a hover fill, a painted [`Icon`] in a badge at the left, the title (truncated
/// with the whole one on its tooltip) and an optional weak subtitle. The
/// right [`Theme::row_menu_width`] is left free for [`row_menu`], so every
/// row's `…` lands at the same x. Returns the row's response (the whole row
/// is the button) and its rect, for the caller to paint over or hang the
/// menu on.
pub(crate) fn file_like_row(
    ui: &mut Ui,
    theme: &Theme,
    icon: Icon,
    title: &str,
    subtitle: Option<&str>,
    previewing: bool,
    tag: Option<usize>,
) -> (Response, Rect) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, theme.library_card_h), Sense::hover());
    let response = ui.interact(
        rect,
        ui.id().with(("shell/region/library/row", icon, title)),
        Sense::click_and_drag(),
    );
    // TASK 2: the row whose file is being previewed reads as the preview's
    // own — an accent wash (the theme's selection/hover deep) and a left
    // accent bar, reused from the same tokens the rest of the row uses. The
    // content below paints on top of both.
    let fill = if previewing {
        theme.phosphor_deep
    } else if response.hovered() {
        theme.library_card_hover
    } else {
        // Resting rows sit darker than the panel (the theme's inset), so the
        // hover wash above reads as the lighter step.
        theme.inset
    };
    ui.painter().rect_filled(rect, theme.library_card_corner(), fill);
    if previewing {
        ui.painter().rect_filled(
            Rect::from_min_size(
                rect.left_top(),
                Vec2::new(theme.accent_stroke, rect.height()),
            ),
            theme.library_card_corner(),
            theme.phosphor,
        );
    }

    let size = theme.library_badge_size;
    let slot = Rect::from_min_size(
        Pos2::new(
            rect.left() + theme.library_card_pad.x,
            rect.center().y - size.y / 2.0,
        ),
        size,
    );
    ui.painter()
        .rect_filled(slot, theme.library_card_corner(), theme.library_badge);
    // The tag tints the badge's glyph, and (below) the title. `paint_icon`
    // takes no colour, so the glyph's stroke is re-sourced by handing it a
    // copy of the theme with only `ink_weak` — the one token its
    // `icon_stroke` reads — replaced by the tag's own preset.
    let tint = tag.map(|idx| theme.tag_colours()[idx]);
    match tint {
        Some(colour) => {
            let mut tinted = theme.clone();
            tinted.ink_weak = colour;
            paint_icon(ui.painter(), slot, icon, &tinted);
        }
        None => paint_icon(ui.painter(), slot, icon, theme),
    }

    // The text lives in a child ui that stops short of the menu column, so
    // a long title truncates there and never runs under the `…`. The block's
    // height is measured, not guessed, so it sits exactly mid-row: the same
    // space above the title as below the subtitle (or the title alone).
    let title_h = ui.ctx().fonts_mut(|f| f.row_height(&theme.font_body));
    let block_h = match subtitle {
        Some(_) => title_h + theme.library_detail_gap + ui.ctx().fonts_mut(|f| f.row_height(&theme.font_small)),
        None => title_h,
    };
    let top = (rect.center().y - block_h / 2.0).max(rect.top());
    let text = Rect::from_min_max(
        Pos2::new(slot.right() + theme.item_gap.x, top),
        Pos2::new(rect.right() - theme.row_menu_width, (top + block_h).min(rect.bottom())),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(text)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );
    child.spacing_mut().item_spacing.y = theme.library_detail_gap;
    child
        .add(
            egui::Label::new(
                RichText::new(title)
                    .font(theme.font_body.clone())
                    .color(tint.unwrap_or(theme.ink)),
            )
            .truncate()
            .selectable(false)
            .show_tooltip_when_elided(false),
        )
        .on_hover_text(title);
    if let Some(sub) = subtitle {
        child.add(
            egui::Label::new(
                RichText::new(sub)
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            )
            .truncate()
            .selectable(false),
        );
    }
    (response, rect)
}

/// While a row is dragged, the whole row follows the pointer — icon, title and
/// subtitle, as it reads in the list — not just its name.
fn drag_ghost_row(
    ui: &Ui,
    theme: &Theme,
    response: &Response,
    rect: Rect,
    icon: Icon,
    title: &str,
    subtitle: Option<&str>,
    tag: Option<usize>,
) {
    if !response.dragged() {
        return;
    }
    let Some(pointer) = response.interact_pointer_pos() else {
        return;
    };
    crate::shell::components::drag_ghost(
        ui.ctx(),
        theme,
        Id::new("shell/region/library/drag-ghost"),
        pointer,
        rect.width(),
        |ui| {
            file_like_row(ui, theme, icon, title, subtitle, false, tag);
        },
    );
}

/// The `…` menu of a row, in its fixed right column so every row's button
/// sits at the same x. Frameless and weak until the row is hovered. The
/// child ui is placed, not laid out, so the row's own allocation is the only
/// space the row takes.
/// A row's favourite star (`shell/favorites.rs`): a click queues the file or
/// folder for the global store, and the star shows whether it is one. It sits
/// just left of the `…` column, so every row's star lands at the same x.
/// Side of a row's corner button (`★`, `…`), in points.
const CORNER_SIDE: f32 = 20.0;

/// The top-right corner buttons of a library row: `index` 0 is the rightmost
/// (`…`), 1 sits left of it (`★`). Both share one top edge and one size, so
/// every row lines them up.
fn corner_slot(rect: Rect, index: usize) -> Rect {
    let pad = 4.0;
    let x = rect.right() - pad - CORNER_SIDE - index as f32 * (CORNER_SIDE + pad);
    Rect::from_min_size(Pos2::new(x, rect.top() + pad), Vec2::splat(CORNER_SIDE))
}

fn favourite_star(ui: &mut Ui, theme: &Theme, rect: Rect, entry: &crate::shell::favorites::Entry) {
    let slot = corner_slot(rect, 1);
    let fav = crate::shell::favorites::is_favourite(ui.ctx(), &entry.key);
    let response = ui.interact(slot, ui.id().with(("shell/library/star", &entry.key)), Sense::click());
    let ink = if fav { theme.accent_vivid } else { theme.ink_weak };
    ui.painter().rect_filled(slot, theme.library_card_corner(), theme.library_badge);
    paint_icon_stroked(ui.painter(), slot, Icon::Star, theme, theme.line_stroke(ink));
    if response.on_hover_text(tr("favourite")).clicked() {
        crate::shell::favorites::request_toggle(ui.ctx(), entry.clone());
    }
}

fn row_menu(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    row: &Response,
    add_contents: impl FnOnce(&mut Ui),
) {
    let mut child = ui.new_child(UiBuilder::new().max_rect(corner_slot(rect, 0)));
    egui::containers::menu::MenuButton::from_button(
        egui::Button::new(
            RichText::new("…")
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        )
        .min_size(Vec2::splat(CORNER_SIDE))
        .fill(theme.library_badge),
    )
    .ui(&mut child, add_contents);
}

// ---------------------------------------------------------------------------
// colour tags — the per-file/per-folder `file_tags` map in `POST /settings`
// ---------------------------------------------------------------------------

/// The colour tag (an index into [`Theme::tag_colours`]) stored for `path`,
/// or `None` when the path carries no tag. Read through the door (never the
/// file), so a row's tint and its `color` submenu always agree.
fn row_tag(door: &dyn Call, path: &str) -> Option<usize> {
    tags_of(door).get(path).and_then(Value::as_u64).map(|n| n as usize)
}

/// The whole `file_tags` object off `GET /settings` — empty on a fresh
/// install or a read the door refused. A write merges into this (read,
/// change one path, post the whole map), so tagging one row never clears
/// another's tag.
fn tags_of(door: &dyn Call) -> serde_json::Map<String, Value> {
    get(door, "GET", "/settings", &[])
        .and_then(|v| v.get("file_tags").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

/// Set (or clear, with `None`) the colour tag on one path: read the map,
/// change the one entry, and post the whole `file_tags` back — the route
/// replaces the key as a whole (`grid_spans`'s own contract), so the merge
/// happens here. The reply is discarded; the next frame's read is the
/// feedback.
fn set_tag(door: &dyn Call, path: &str, idx: Option<usize>) {
    let mut tags = tags_of(door);
    match idx {
        Some(i) => {
            tags.insert(path.to_string(), json!(i));
        }
        None => {
            tags.remove(path);
        }
    }
    let _ = door.json("POST", "/settings", Some(json!({ "file_tags": tags })));
}

/// The `color` submenu a row's `…` carries: one swatch per preset in
/// [`Theme::tag_colours`], then `none` to clear the tag. A swatch is the
/// colour itself (a `●` painted in it), so there is no colour name to
/// translate — no more text than `import`/`remove` carry.
fn colour_submenu(ui: &mut Ui, theme: &Theme, door: &dyn Call, path: &str) {
    ui.menu_button(RichText::new("color").font(theme.font_small.clone()), |ui| {
        let colours = theme.tag_colours();
        for (idx, colour) in colours.iter().enumerate() {
            if ui
                .button(RichText::new("●").font(theme.font_body.clone()).color(*colour))
                .clicked()
            {
                set_tag(door, path, Some(idx));
                ui.close();
            }
        }
        ui.separator();
        if ui
            .button(RichText::new("none").font(theme.font_small.clone()))
            .clicked()
        {
            set_tag(door, path, None);
            ui.close();
        }
    });
}

/// Fetches `GET /fs` for `state.cwd` when there is no cached reply — once per
/// folder change, never per frame. A failed read leaves an empty listing and
/// a note, so the panel does not retry every frame.
fn refresh_fs(door: &dyn Call, state: &mut PanelState) {
    if state.fs.is_some() {
        return;
    }
    let path = state.cwd.to_string_lossy().into_owned();
    match get(door, "GET", "/fs", &[("path", path)]) {
        Some(v) => {
            state.fs = Some(v);
            state.files_note = None;
        }
        None => {
            state.fs = Some(json!({ "dirs": [], "files": [] }));
            state.files_note = Some("could not read the folder".to_string());
        }
    }
}

// ---------------------------------------------------------------------------
// songs — `/author/songs`, `/author/digest`, `/author/open`, `/author/reveal`
// ---------------------------------------------------------------------------

/// The status words `shell.js`'s `STATUS_HINT` shows — copied
/// rather than derived, the same way `lanes.rs`'s `ROUTER_MODES` hardcodes
/// four closed strings no route enumerates.
fn status_hint(status: &str) -> &'static str {
    match status {
        "undigested" => "not digested",
        "queued" => "queued",
        "digesting" => "digesting…",
        "digested" => "ready",
        "in progress" => "in progress",
        "failed" => "failed",
        _ => "?",
    }
}

/// The right column: the current folder's files, one level — `GET /fs` of
/// `state.cwd`, unioned with `GET /author/songs` so an audio that is already
/// a song shows its digest status. A search box, the inline `confirm_open`
/// popover, then a grid of fixed [`file_card`]s (`R22.2`), as many columns
/// as fit — no button row any more; the card is the button. Built on the
/// old `songs_section`, which it replaces.
fn files_column(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut PanelState) {
    // The scenes and sequence routes live in their own places now; the
    // column is the folder's files.
    state.scenes_open = false;
    state.sequence_open = false;
    if let Some((name, audio)) = state.confirm_open.clone() {
        let mut frame = panel_frame(theme);
        frame = frame.stroke(theme.accent());
        frame.show(ui, |ui| {
            ui.label(
                RichText::new(format!("switch to {name}? another song is open"))
                    .font(theme.font_small.clone())
                    .color(theme.ink),
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(RichText::new("confirm").font(theme.font_small.clone()))
                    .clicked()
                {
                    match door.json(
                        "POST",
                        "/author/open",
                        Some(json!({ "song": audio, "audio": audio })),
                    ) {
                        Ok(_) => {
                            state.song_note = Some(format!("opened {name}"));
                            state.last_opened_song = Some(name.clone());
                        }
                        Err((_, msg)) => state.song_note = Some(msg),
                    }
                    state.confirm_open = None;
                }
                if ui
                    .button(RichText::new("cancel").font(theme.font_small.clone()))
                    .clicked()
                {
                    state.confirm_open = None;
                }
            });
        });
    }

    // The list's own filters (2026-10-08): a favourites-only toggle and a
    // kind row. Both are view state kept in `PanelState`, so they survive a
    // folder walk; neither changes the route the column reads.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = theme.item_gap;
        if icon_toggle(ui, theme, Icon::Star, state.fav_only, tr("favourites")).clicked() {
            state.fav_only = !state.fav_only;
        }
        // Chips: nothing picked is "all" (kind 0); clicking the picked one clears it.
        let mut kind = (state.kind_filter != 0).then_some(state.kind_filter);
        filter_chips(
            ui,
            theme,
            &mut kind,
            &[(1, tr("audio")), (2, tr("video")), (3, tr("image")), (4, tr("sequence"))],
        );
        state.kind_filter = kind.unwrap_or(0);

    });

    let songs_val = door
        .json("GET", "/author/songs", None)
        .ok()
        .unwrap_or_else(|| json!({}));
    let songs = songs_val["songs"].as_array().cloned().unwrap_or_default();
    let any_open = songs.iter().any(|s| s["open"].as_bool() == Some(true));
    if songs
        .iter()
        .any(|s| matches!(s["status"].as_str(), Some("queued") | Some("digesting")))
    {
        // `watchDigest` (`shell.js`): keep repainting while at least one song
        // is mid-digest, so its status line advances without operator input
        // — the door call above is a same-process function call, not a
        // network poll.
        crate::shell::frames::repaint_after(ui.ctx(), std::time::Duration::from_secs(1));
    }
    // A file is "already a song" when its stem is a name `GET /author/songs`
    // knows (rule 5: the digest status is the library's, never re-derived).
    let song_for = |file: &str| {
        let stem = std::path::Path::new(file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file);
        songs
            .iter()
            .find(|s| s["name"].as_str() == Some(stem))
            .cloned()
    };

    // The folder's files, plus the movements library's own entries as
    // `sequence` rows: movements all live in one file
    // (`data/library/movements.json`), so a sequence has to be named by its
    // movement for `view`/`load` to mean anything. The raw `.json` row
    // `/fs` reports as a `sequence` is dropped, so it is not listed twice;
    // the movement's own `favourite` rides along for the card's status line.
    let mut all = state
        .fs
        .as_ref()
        .and_then(|f| f["files"].as_array().cloned())
        .unwrap_or_default();
    all.retain(|f| f["kind"] != "sequence");
    if let Some(listing) = get(door, "GET", "/author/models/library", &[]) {
        for m in listing["movements"].as_array().cloned().unwrap_or_default() {
            if let Some(name) = m["name"].as_str() {
                all.push(json!({
                    "file": name,
                    "kind": "sequence",
                    "favourite": m["favourite"],
                }));
            }
        }
    }

    let fq = state.file_query.trim().to_lowercase();
    let shown: Vec<&Value> = all
        .iter()
        .filter(|f| {
            let name_ok = fq.is_empty()
                || f["file"]
                    .as_str()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&fq);
            let fav_ok = !state.fav_only || f["favourite"].as_bool() == Some(true);
            let kind_ok = match state.kind_filter {
                1 => f["kind"].as_str() == Some("audio"),
                2 => f["kind"].as_str() == Some("video"),
                3 => f["kind"].as_str() == Some("image"),
                4 => f["kind"].as_str() == Some("sequence"),
                _ => true,
            };
            name_ok && fav_ok && kind_ok
        })
        .collect();

    if all.is_empty() {
        ui.label(
            RichText::new("no files in this folder")
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    } else if shown.is_empty() {
        ui.label(
            RichText::new("no files match the search")
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }

    // The search dial, while it walks files: it moves the pick (the same one a
    // click makes), and `load` hands the pick to the overlays grid.
    if !state.dial_folders {
        let entries: Vec<String> = shown
            .iter()
            .map(|f| state.cwd.join(f["file"].as_str().unwrap_or_default()).to_string_lossy().to_string())
            .collect();
        if let Some(to) = dial_move(&entries, state.selected_path.as_deref(), std::mem::take(&mut state.dial_step)) {
            if let Some(f) = shown.iter().find(|f| state.cwd.join(f["file"].as_str().unwrap_or_default()).to_string_lossy() == to) {
                let name = f["file"].as_str().unwrap_or_default().to_string();
                let kind = f["kind"].as_str().unwrap_or("other").to_string();
                state.picked_file = Some(crate::shell::panels::FileRef { name, path: to.clone(), kind });
            }
            state.selected_path = Some(to);
            state.dial_scroll = true;
        }
        if std::mem::take(&mut state.dial_load) {
            let picked = state.selected_path.clone();
            if let Some(f) = shown.iter().find(|f| {
                Some(state.cwd.join(f["file"].as_str().unwrap_or_default()).to_string_lossy().to_string()) == picked
            }) {
                let name = f["file"].as_str().unwrap_or_default().to_string();
                super::pads::set_loaded(
                    ui.ctx(),
                    crate::shell::panels::FileDrag {
                        path: state.cwd.join(&name).to_string_lossy().to_string(),
                        name,
                        kind: f["kind"].as_str().unwrap_or("other").to_string(),
                        seconds: f["seconds"].as_f64(),
                        scene: None,
                    },
                );
            }
        }
    }

    // The files: one [`file_card`] row each, in a single list (2026-10-10:
    // the grid view is gone).
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = theme.library_gap;
        for f in &shown {
            let song = song_for(f["file"].as_str().unwrap_or_default());
            file_card(ui, theme, f, song.as_ref(), any_open, door, state);
        }
    });

    if let Some(note) = state.add_note.clone() {
        ui.label(
            RichText::new(note)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }
    if let Some(note) = state.song_note.clone() {
        ui.label(
            RichText::new(note)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }
}

/// The sequence's menu entries, `view` and `load` — shared by the row's
/// `…` button and its right-click menu, so the two cannot drift apart.
fn sequence_menu(ui: &mut Ui, theme: &Theme, file: &str, state: &mut PanelState) {
    if ui
        .button(RichText::new("view").font(theme.font_small.clone()))
        .clicked()
    {
        state.sequence_to_show = Some(file.to_string());
        ui.close();
    }
    if ui
        .button(RichText::new("load").font(theme.font_small.clone()))
        .clicked()
    {
        state.sequence_to_load = Some(file.to_string());
        ui.close();
    }
}

/// One file of the browser: its **whole row is the button** (`R22.2`) — a
/// [`file_like_row`] with the file's kind [`Icon`], its status line as the
/// subtitle, and the same `…` column a folder row has. Clicking is by kind:
/// audio runs [`play_song`], a sequence loads (`load`) and its `…`/right click
/// offers `view`/`load`; video and image stay information only — and every
/// row drags onto the canvas, the overlays grid or the sequence timeline.
fn file_card(
    ui: &mut Ui,
    theme: &Theme,
    f: &Value,
    song: Option<&Value>,
    any_open: bool,
    door: &dyn Call,
    state: &mut PanelState,
) -> Response {
    let file = f["file"].as_str().unwrap_or_default();
    let kind = f["kind"].as_str().unwrap_or("other");
    let icon = match kind {
        "audio" => Icon::Audio,
        "video" => Icon::Video,
        "image" => Icon::Image,
        "sequence" => Icon::Sequence,
        _ => Icon::File,
    };

    // The subtitle's values, off the same route values the old right-hand
    // status was: the digest status on audio, the length on video and image,
    // `★` on a favourite sequence. The kind itself is the icon now.
    let status: Option<String> = match kind {
        "audio" => {
            let st = song
                .and_then(|s| s["status"].as_str())
                .unwrap_or("undigested");
            (!matches!(st, "queued" | "digesting")).then(|| status_hint(st).to_string())
        }
        "video" | "image" => f["seconds"].as_f64().map(|s| format!("{s:.0}s")),
        "sequence" if f["favourite"].as_bool().unwrap_or(false) => Some("★".to_string()),
        _ => None,
    };
    // TASK 2: the file this row names is the one the media board is previewing
    // (the same `media::current` `preview_overlay` draws), so the row that owns
    // the floating picture is tinted and barred.
    let path = state.cwd.join(file).to_string_lossy().to_string();
    let previewing = matches!(kind, "image" | "video")
        && media::current(ui.ctx()).is_some_and(|c| c.path == path);
    let tag = row_tag(door, &path);
    let (response, rect) = file_like_row(ui, theme, icon, file, status.as_deref(), previewing, tag);
    if matches!(kind, "image" | "video" | "audio") {
        favourite_star(ui, theme, rect, &crate::shell::favorites::file_entry(kind, file, &path));
    }
    // An `files` card starts a drag carrying its own path and kind. The
    // card is still a button (`R22.2`) — a short click is the `play`/`load`
    // below; egui's own drag threshold tells the two apart, no code of ours.
    // The label follows the pointer while the drag is in the air.
    response.dnd_set_drag_payload(crate::shell::panels::FileDrag {
        name: file.to_string(),
        path: state.cwd.join(file).to_string_lossy().to_string(),
        kind: kind.to_string(),
        seconds: f["seconds"].as_f64(),
        scene: None,
    });
    drag_ghost_row(ui, theme, &response, rect, icon, file, status.as_deref(), tag);
    let open = song.is_some_and(|s| s["open"].as_bool() == Some(true));
    let picked = state.selected_path.as_deref() == Some(path.as_str());
    if picked && !state.dial_folders && std::mem::take(&mut state.dial_scroll) {
        response.scroll_to_me(None);
    }
    if response.clicked() {
        state.selected_path = Some(path.clone());
        state.picked_file = Some(crate::shell::panels::FileRef {
            name: file.to_string(),
            path: path.clone(),
            kind: kind.to_string(),
        });
    }

    // The selected/open edge is stroked on top of the row's own fill.
    if open || picked {
        ui.painter().rect_stroke(
            rect,
            theme.library_card_corner(),
            if open {
                theme.accent()
            } else {
                theme.card_picked_stroke()
            },
            egui::StrokeKind::Inside,
        );
    }

    match kind {
        "audio" => {
            if response.clicked() {
                let path = state.cwd.join(file);
                play_song(door, state, file, &path, song, any_open);
            }
            import_menu(ui, theme, rect, &response, &path, kind, None, door, state);
        }
        "sequence" => {
            if response.clicked() {
                state.sequence_to_load = Some(file.to_string());
            }
            response.context_menu(|ui| sequence_menu(ui, theme, file, state));
            row_menu(ui, theme, rect, &response, |ui| {
                sequence_menu(ui, theme, file, state)
            });
        }
        // The sources a graph can take: `import` lives in their `…` menu.
        // A video's menu also offers `add as layer` — touch has no drag that
        // is easy to reach from a finger, so it makes the same move a drop on
        // the timeline makes.
        "image" | "video" => {
            let layer = (kind == "video").then(|| crate::shell::panels::FileDrag {
                name: file.to_string(),
                path: state.cwd.join(file).to_string_lossy().to_string(),
                kind: kind.to_string(),
                seconds: f["seconds"].as_f64(),
                scene: None,
            });
            if response.clicked() {
                // The pick the overlay previews: one clip, the cursor on it.
                let clip = media::Clip {
                    path: path.clone(),
                    name: file.to_string(),
                    kind: kind.to_string(),
                    scene: None,
                    seconds: f["seconds"].as_f64().unwrap_or(0.0),
                    looping: false,
                };
                media::clear(ui.ctx());
                media::push(ui.ctx(), clip);
                media::set_cursor(ui.ctx(), 0);
            }
            import_menu(ui, theme, rect, &response, &path, kind, layer, door, state)
        }
        _ => {}
    }
    response
}

/// The `…` menu of a file a graph can take (image, video, audio): `import`.
fn import_menu(
    ui: &mut Ui,
    theme: &Theme,
    rect: Rect,
    response: &Response,
    path: &str,
    kind: &str,
    layer: Option<crate::shell::panels::FileDrag>,
    door: &dyn Call,
    state: &mut PanelState,
) {
    row_menu(ui, theme, rect, response, |ui| {
        if let Some(layer) = &layer {
            if ui
                .button(RichText::new(tr("add as layer")).font(theme.font_small.clone()))
                .clicked()
            {
                state.clip_to_layer = Some(layer.clone());
                ui.close();
            }
        }
        if ui
            .button(RichText::new("import").font(theme.font_small.clone()))
            .clicked()
        {
            state.import = Some(Import {
                path: path.to_string(),
                kind: kind.to_string(),
            });
            ui.close();
        }
        colour_submenu(ui, theme, door, path);
    });
}

/// `play` on an audio row: add it to the song library if it is not one yet,
/// queue its digest when it is a known song that never landed a matrix, then
/// open it — with the inline confirm when another song is already open. On a
/// real open, `last_opened_song` (the **stem**, `R22.7`) is set for the wiring
/// (`P3`) to flip lanes to its own state.
fn play_song(
    door: &dyn Call,
    state: &mut PanelState,
    file: &str,
    path: &std::path::Path,
    song: Option<&Value>,
    any_open: bool,
) {
    let stem = std::path::Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file)
        .to_string();
    let status = song
        .and_then(|s| s["status"].as_str())
        .unwrap_or("undigested");
    if song.is_none() {
        match add_song(door, path) {
            Ok(_) => state.add_note = Some(format!("added {file} — digesting")),
            Err((_, msg)) => {
                state.add_note = Some(msg);
                return;
            }
        }
    } else if matches!(status, "undigested" | "failed") {
        // `R22.7` B2: a song the library already knows but never digested
        // has no matrix to load, so the open alone would leave the columns
        // empty. Queue the digest before opening — the same
        // `POST /author/digest` the folder's own retry uses.
        if get(
            door,
            "POST",
            "/author/digest",
            &[("name", file.to_string())],
        )
        .is_none()
        {
            state.add_note = Some(format!("could not request the digest of {file}"));
        }
    }
    if any_open {
        // The open itself waits for the confirm popover, which carries the
        // same `audio` value it posts.
        state.confirm_open = Some((stem, file.to_string()));
        return;
    }
    match door.json("POST", "/author/open", Some(json!({ "audio": file }))) {
        Ok(_) => {
            state.song_note = Some(format!("opened {file}"));
            state.last_opened_song = Some(stem);
        }
        Err((_, msg)) => state.song_note = Some(msg),
    }
}

/// `addAudio` (`shell.js`), by the local path the browser already knows:
/// its bytes are read straight off disk (the door runs in-process — there
/// is no socket for a multipart body to cross) and posted in one chunk
/// (`last=1`) to `POST /author/add`, the same route the browser's own
/// chunked upload calls. This repo has no file-dialog dependency, so there
/// is no "type a path" field any more — the operator browses to the file
/// and clicks its card.
fn add_song(door: &dyn Call, path: &std::path::Path) -> Result<Value, (u16, String)> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let bytes = metal_vfs::read(path)
        .map_err(|e| (404, format!("could not read {}: {e}", path.display())))?;
    post_bytes(
        door,
        "/author/add",
        &[("name", name), ("last", "1".to_string())],
        bytes,
    )
}

// ---------------------------------------------------------------------------
// curator — read-only half: library/scene list and trained status
// ---------------------------------------------------------------------------

/// The curator's status in a few words, off `GET /author/curator/state`
/// (rule 5): `entrenado · N picks` or `not trained`.
fn curator_status(state: &Value) -> String {
    if state["trained"].as_bool().unwrap_or(false) {
        let picks = state["picks"].as_u64().unwrap_or(0);
        format!("trained · {picks} picks")
    } else {
        "not trained".to_string()
    }
}

/// `POST /author/curator/train` for the folder in use — the `⋮` menu's
/// secondary `train curator`. Returns the note the folder column shows.
fn train_curator(door: &dyn Call, library: &str) -> String {
    match door.json(
        "POST",
        "/author/curator/train",
        Some(json!({ "library": library })),
    ) {
        Ok(reply) => {
            let songs = reply["songs"].as_u64().unwrap_or(0);
            format!("{} · {songs} songs", curator_status(&reply))
        }
        Err((_, msg)) => msg,
    }
}

// ---------------------------------------------------------------------------
// the door, three small shapes — GET with a query, POST with only a query
// (no JSON body), POST with a raw byte body plus a query
// ---------------------------------------------------------------------------

/// Where the search dial lands: `steps` entries on from `at` (a dial with no
/// resting place starts before the first), clamped to the list. `None` when
/// it did not move.
fn dial_move(entries: &[String], at: Option<&str>, steps: i32) -> Option<String> {
    if steps == 0 || entries.is_empty() {
        return None;
    }
    let from = at.and_then(|a| entries.iter().position(|e| e == a)).map_or(-1, |i| i as i64);
    let to = if from < 0 && steps > 0 { steps as i64 - 1 } else { from + steps as i64 };
    entries.get(to.clamp(0, entries.len() as i64 - 1) as usize).cloned()
}

/// The search dial's outline on the folder it rests on, scrolled into view
/// the frame it moved there.
fn dial_mark(ui: &Ui, theme: &Theme, state: &mut PanelState, response: &Response, rect: Rect, dialled: bool) {
    if !dialled {
        return;
    }
    ui.painter().rect_stroke(rect, theme.library_card_corner(), theme.card_picked_stroke(), egui::StrokeKind::Inside);
    if std::mem::take(&mut state.dial_scroll) {
        response.scroll_to_me(None);
    }
}

fn get(door: &dyn Call, method: &str, path: &str, query: &[(&str, String)]) -> Option<Value> {
    let mut q = HashMap::new();
    for (k, v) in query {
        q.insert((*k).to_string(), v.clone());
    }
    let params = Params {
        query: q,
        body: Vec::new(),
        tail: None,
    };
    match door.call(method, path, params) {
        Some(reply) if reply.status == 200 => crate::json_memo::parse(&reply.body).ok(),
        _ => None,
    }
}

/// A POST with a raw byte body plus a query string — `/author/add`'s own
/// shape (`params.body` is the file's bytes, `name`/`last` are query
/// params), which `Call::json` cannot express either, for the same reason.
fn post_bytes(
    door: &dyn Call,
    path: &str,
    query: &[(&str, String)],
    body: Vec<u8>,
) -> Result<Value, (u16, String)> {
    let mut q = HashMap::new();
    for (k, v) in query {
        q.insert((*k).to_string(), v.clone());
    }
    let params = Params {
        query: q,
        body,
        tail: None,
    };
    match door.call("POST", path, params) {
        Some(reply) if reply.status == 200 => crate::json_memo::parse(&reply.body)
            .map_err(|e| (500, format!("{path} answered a body that is not JSON: {e}"))),
        Some(reply) => Err((
            reply.status,
            String::from_utf8_lossy(&reply.body).into_owned(),
        )),
        None => Err((404, format!("POST {path} matched no route"))),
    }
}

#[cfg(test)]
mod tests {

    /// The search dial walks a list: a dial with no resting place starts at
    /// the first entry, then moves by its steps, clamped at both ends.
    #[test]
    fn the_search_dial_steps_through_the_list_and_stops_at_its_ends() {
        let e: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(super::dial_move(&e, None, 1).as_deref(), Some("a"));
        assert_eq!(super::dial_move(&e, Some("a"), 1).as_deref(), Some("b"));
        assert_eq!(super::dial_move(&e, Some("b"), 5).as_deref(), Some("c"));
        assert_eq!(super::dial_move(&e, Some("b"), -9).as_deref(), Some("a"));
        assert_eq!(super::dial_move(&e, Some("gone"), -1).as_deref(), Some("a"));
        assert_eq!(super::dial_move(&e, Some("a"), 0), None);
        assert_eq!(super::dial_move(&[], None, 1), None);
    }

    use super::*;
    use crate::shell::FakeDoor;
    use crate::shell::Shell;
    use egui_kittest::kittest::Queryable;

    /// The same harness the properties tests build (`library.rs`'s own
    /// section lives in the same full grid), kept here beside the panel.
    fn library_harness(door: Box<dyn Call>) -> egui_kittest::Harness<'static, Shell> {
        let theme = Theme::phosphor();
        let size = egui::Vec2::new(
            crate::shell::DESKTOP_VIEWPORT.0 as f32,
            crate::shell::DESKTOP_VIEWPORT.1 as f32,
        );
        let harness = egui_kittest::Harness::builder()
            .with_size(size)
            .build_ui_state(
                |ui, shell: &mut Shell| shell.ui(ui),
                Shell::new(theme.clone(), door),
            );
        theme.apply(&harness.ctx);
        harness
    }

    /// [`library_harness`] with the browser pointed at a scripted `/music`
    /// and the three routes a card paints from already answered — the four
    /// `R22.2` card tests all need the same replies, so they share this.
    fn scripted_harness(
        files: Value,
        songs: Value,
        movements: Value,
    ) -> (egui_kittest::Harness<'static, Shell>, std::rc::Rc<FakeDoor>) {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/fs", json!({ "dirs": [], "files": files }));
        recorder.respond("GET", "/author/songs", json!({ "songs": songs }));
        recorder.respond(
            "GET",
            "/author/models/library",
            json!({ "movements": movements }),
        );
        let mut harness = library_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.state_mut().library_state_mut().set_cwd("/music");
        (harness, recorder)
    }

    #[test]
    fn status_hint_covers_every_status_audio_songs_can_report() {
        for s in [
            "undigested",
            "queued",
            "digesting",
            "digested",
            "in progress",
            "failed",
        ] {
            assert_ne!(status_hint(s), "?", "{s} should have a Spanish label");
        }
    }

    /// `R22.7` B1: every ingest route keys the song by its stem
    /// (`data/audio/{stem}.8hz.npy`), so the hook must carry the stem even
    /// when the browser hands `play` a full file name.
    #[test]
    fn play_hands_lanes_the_song_stem_not_the_path() {
        let door = FakeDoor::new();
        let mut state = PanelState::default();
        let song = serde_json::json!({ "name": "a", "status": "digested" });
        play_song(
            &door,
            &mut state,
            "a.mp3",
            std::path::Path::new("/music/a.mp3"),
            Some(&song),
            false,
        );
        assert_eq!(state.last_opened_song(), Some("a"));
        assert!(
            !door
                .calls
                .borrow()
                .iter()
                .any(|(m, p, _)| m == "POST" && p == "/author/digest"),
            "a digested song must not be queued again"
        );
    }

    /// `R22.7` B2: a song the library knows but never digested has no
    /// matrix, so `play` queues its digest before opening it.
    #[test]
    fn play_on_an_undigested_song_asks_for_a_digest() {
        let door = FakeDoor::new();
        let mut state = PanelState::default();
        let song = serde_json::json!({ "name": "a", "status": "undigested" });
        play_song(
            &door,
            &mut state,
            "a.mp3",
            std::path::Path::new("/music/a.mp3"),
            Some(&song),
            false,
        );
        let calls = door.calls.borrow();
        let digest = calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p == "/author/digest")
            .expect("an undigested song is queued for a digest");
        assert_eq!(
            digest.2.query.get("name").map(String::as_str),
            Some("a.mp3")
        );
        assert!(
            calls
                .iter()
                .any(|(m, p, _)| m == "POST" && p == "/author/open"),
            "the open still fires"
        );
    }

    /// `R22.2`: the `play` button is gone — the card itself is the button.
    /// The files column's view filters (2026-10-08): the kind row and the
    /// favourites star narrow what is listed without changing any route.
    #[test]
    fn kind_and_favourite_filters_narrow_the_list() {
        let (mut harness, _) = scripted_harness(
            json!([
                { "file": "a.mp3", "kind": "audio" },
                { "file": "b.mp4", "kind": "video" }
            ]),
            json!([]),
            json!([{ "name": "drop", "favourite": true }]),
        );
        harness.run();
        harness.get_by_label("a.mp3");
        harness.get_by_label("b.mp4");
        harness.get_by_label("drop");

        harness
            .state_mut()
            .library_state_mut()
            .set_filters(false, 2);
        harness.run();
        assert!(harness.query_by_label("a.mp3").is_none());
        assert!(harness.query_by_label("b.mp4").is_some());

        harness.state_mut().library_state_mut().set_filters(true, 0);
        harness.run();
        assert!(harness.query_by_label("b.mp4").is_none());
        assert!(harness.query_by_label("drop").is_some());
    }

    #[test]
    fn no_play_button_is_painted() {
        let (mut harness, _) = scripted_harness(
            json!([{ "file": "a.mp3", "kind": "audio" }]),
            json!([{ "name": "a", "status": "digested" }]),
            json!([]),
        );
        harness.run();
        harness.get_by_label("a.mp3");
        assert!(
            harness.query_by_label("play").is_none(),
            "the card is the button; no `play` label should paint"
        );
    }

    /// Clicking an audio card runs `play`: the song is already in the
    /// library and digested, so the click opens it with `POST /author/open`.
    #[test]
    fn clicking_an_audio_card_opens_it() {
        let (mut harness, recorder) = scripted_harness(
            json!([{ "file": "a.mp3", "kind": "audio" }]),
            json!([{ "name": "a", "status": "digested" }]),
            json!([]),
        );
        harness.run();
        harness.get_by_label("a.mp3").click();
        harness.run();
        assert!(
            recorder
                .calls
                .borrow()
                .iter()
                .any(|(m, p, _)| m == "POST" && p == "/author/open"),
            "clicking an audio card should post the open"
        );
    }

    /// Clicking a sequence card is `load`: the movement loads into the
    /// lanes scratch through `route_library_intents`, the same path the
    /// old `load` button took.
    #[test]
    fn clicking_a_sequence_card_loads_it() {
        let (mut harness, recorder) = scripted_harness(
            json!([]),
            json!([]),
            json!([{ "name": "drop", "favourite": true }]),
        );
        recorder.respond(
            "GET",
            "/author/models/movement",
            json!({ "name": "drop", "model": "geoterrain", "ticks": [[0.1, 0.2]] }),
        );
        harness.run();
        harness.get_by_label("drop").click();
        harness.run();
        harness.run();
        assert_eq!(
            harness.state_mut().lanes_state_mut().mv.name,
            "drop",
            "clicking a sequence card should load it into the scratch"
        );
    }

    /// Right click on a sequence card offers `view` — the verb the old
    /// button carried, now the card's context menu.
    #[test]
    fn right_click_on_a_sequence_offers_ver() {
        let (mut harness, _) = scripted_harness(
            json!([]),
            json!([]),
            json!([{ "name": "drop", "favourite": true }]),
        );
        harness.run();
        harness.get_by_label("drop").click_secondary();
        harness.run();
        harness.get_by_label("view");
    }

    /// 2026-10-10: a folder row and file rows with long and short names
    /// reserve the same right column, so their `…` buttons share one x and
    /// the title length never moves a menu.
    #[test]
    fn row_menu_buttons_line_up_across_folder_and_file_rows() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond("GET", "/fs", json!({ "dirs": ["x"], "files": [] }));
        recorder.respond("GET", "/author/songs", json!({ "songs": [] }));
        recorder.respond(
            "GET",
            "/author/models/library",
            json!({ "movements": [
                { "name": "s" },
                { "name": "a-much-longer-sequence-name-that-has-to-be-cut-short-in-the-row" }
            ] }),
        );
        let mut harness = library_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.state_mut().library_state_mut().set_cwd("/music");
        harness.run();

        // The folder sits in the left column and the files in the right one,
        // so their `…` cannot share an absolute x. What must match is the
        // fixed column each row reserves: the two file rows (short and long
        // title) share an x, and every button is the same width.
        let rects: Vec<egui::Rect> = harness
            .get_all_by_label("…")
            .map(|n| n.rect())
            .collect();
        assert_eq!(rects.len(), 3, "one `…` on the folder and on each sequence");
        let (folder, short, long) = (rects[0], rects[1], rects[2]);
        assert!(
            (short.left() - long.left()).abs() < 0.5,
            "a short and a long title should leave the `…` at one x, got {short:?} vs {long:?}"
        );
        assert!(
            (folder.width() - short.width()).abs() < 0.5,
            "the folder's `…` should be the same width, got {folder:?} vs {short:?}"
        );
    }

    /// `import` on an image, with a graph open: the source is added and
    /// configured, an `fx.forge` (passthrough) is added right of it, and the
    /// two are wired image-out → image-in — the same calls the canvas drop
    /// makes, plus the runtime.
    #[test]
    fn importar_on_an_image_adds_a_source_and_a_passthrough_forge() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        recorder.respond_seq(
            "POST",
            "/graph/node",
            json!({ "node": "s1", "graph": { "nodes": [{ "id": "s1", "kind": "still", "pos": [0.0, 0.0] }], "edges": [] } }),
        );
        recorder.respond_seq(
            "POST",
            "/graph/node",
            json!({ "graph": { "nodes": [{ "id": "s1", "kind": "still", "pos": [0.0, 0.0] }], "edges": [] } }),
        );
        recorder.respond_seq(
            "POST",
            "/graph/node",
            json!({ "node": "f1", "graph": { "nodes": [{ "id": "s1", "kind": "still", "pos": [0.0, 0.0] }, { "id": "f1", "kind": "fx.forge", "pos": [200.0, 0.0] }], "edges": [] } }),
        );
        let mut harness = library_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.state_mut().nodes_state_mut().set_selected_graph_for_test("g1");
        harness.state_mut().library_state_mut().raise_import_for_test("/pics/a.png", "image");
        harness.run();

        let calls = recorder.calls.borrow();
        let posts: Vec<(&str, Value)> = calls
            .iter()
            .filter(|(m, p, _)| m == "POST" && (p == "/graph/node" || p == "/graph/edge"))
            .map(|(_, p, params)| (p.as_str(), serde_json::from_slice(&params.body).unwrap()))
            .collect();
        let ops: Vec<(&str, &str)> = posts
            .iter()
            .map(|(p, b)| (*p, b["op"].as_str().unwrap_or_default()))
            .collect();
        assert_eq!(
            ops,
            vec![
                ("/graph/node", "add"),
                ("/graph/node", "config"),
                ("/graph/node", "add"),
                ("/graph/node", "config"),
                ("/graph/edge", "add"),
            ],
            "source add+config, forge add+config, then the edge"
        );
        assert_eq!(posts[0].1["kind"], "still");
        assert_eq!(posts[1].1["config"]["file"], "/pics/a.png");
        assert_eq!(posts[2].1["kind"], "fx.forge");
        assert_eq!(posts[3].1["config"]["shader"], "presets/passthrough");
        assert_eq!(posts[4].1["from"], json!({ "node": "s1", "port": "image" }));
        assert_eq!(posts[4].1["to"], json!({ "node": "f1", "port": "image" }));
    }

    /// No open graph: nothing is posted, and the library says why.
    #[test]
    fn importar_without_an_open_graph_says_so_and_posts_nothing() {
        let recorder = std::rc::Rc::new(FakeDoor::new());
        let mut harness = library_harness(Box::new(std::rc::Rc::clone(&recorder)));
        harness.state_mut().library_state_mut().raise_import_for_test("/pics/a.png", "image");
        harness.run();
        assert!(
            !recorder.calls.borrow().iter().any(|(m, p, _)| m == "POST" && p.starts_with("/graph")),
            "no graph is open, so no graph write"
        );
        assert_eq!(
            harness.state_mut().library_state_mut().note_for_test().as_deref(),
            Some("open a graph to import")
        );
    }

    /// A tag write merges into `file_tags` — it reads the stored map, changes
    /// the one path, and posts the whole map back — so tagging one row never
    /// clears another's tag, and clearing one leaves the rest.
    #[test]
    fn setting_a_tag_merges_into_the_file_tags_map_and_clear_removes_one() {
        // Tagging a second path keeps the first.
        let tagged = std::rc::Rc::new(FakeDoor::new());
        tagged.respond("GET", "/settings", json!({ "file_tags": { "/music/a.mp3": 2 } }));
        set_tag(&*tagged, "/music/clips", Some(4));
        let posts: Vec<Value> = tagged
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/settings")
            .map(|(_, _, params)| serde_json::from_slice(&params.body).unwrap())
            .collect();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0]["file_tags"]["/music/a.mp3"], json!(2), "the other tag survives");
        assert_eq!(posts[0]["file_tags"]["/music/clips"], json!(4));

        // Clearing one path leaves the others.
        let cleared = std::rc::Rc::new(FakeDoor::new());
        cleared.respond(
            "GET",
            "/settings",
            json!({ "file_tags": { "/music/a.mp3": 2, "/music/clips": 4 } }),
        );
        set_tag(&*cleared, "/music/a.mp3", None);
        let posts: Vec<Value> = cleared
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/settings")
            .map(|(_, _, params)| serde_json::from_slice(&params.body).unwrap())
            .collect();
        assert_eq!(posts[0]["file_tags"], json!({ "/music/clips": 4 }));
    }
}
