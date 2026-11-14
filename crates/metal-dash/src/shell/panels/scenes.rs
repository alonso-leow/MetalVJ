//! 2026-10-01: video scenes in the shell — one set of widgets, two places.
//!
//! - [`gallery`]: the properties page of a selected video. Every scene its
//!   ingest cut (`GET /author/scenes?clip=`), poster and length, and per
//!   scene: **preview** (the scene loops in a picture under its row, through
//!   the editor's own `/author/video/seek` + `frame.jpg`), **save** under the
//!   name typed beside it, **favourite**, and a group picker.
//! - [`route`]: the library's Scenes route. Every saved or favourite scene
//!   across all videos (`GET /author/scenes`), filterable by group and by
//!   text; groups are created, renamed and deleted here; a row drags onto a
//!   source like a file does (a [`FileDrag`] with its `scene` span), and a
//!   click selects its video so the properties panel shows the gallery.
//!
//! Every write is one `POST /author/scenes` body (`call::author::scenes`),
//! the same the harness's `edit_scene` sends. The phone shell paints these
//! same functions — properties in its long-press overlay, the library in its
//! left sidebar — so nothing here asks how wide it is beyond wrapping.

use std::collections::BTreeSet;

use egui::{ColorImage, Id, RichText, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use crate::shell::components::{lucide, Icon};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::i18n::{tr, trf};
use crate::shell::media;

use super::{FileDrag, FileRef};

/// How long one listing is reused, in seconds. The listing is two small
/// files on disk, but on the external drive that is not a per-frame cost.
const LIST_TTL: f64 = 1.0;
/// How often a previewed scene asks for its next frame, in seconds — the
/// proxy's own eight frames a second (the base tick, derived).
const PREVIEW_STEP: f64 = metal_schema::TickRate::Base.seconds_per_tick();

/// The library's Scenes route, between frames.
#[derive(Default)]
pub struct RouteState {
    query: String,
    /// The group the list is narrowed to; `None` is every kept scene.
    group: Option<String>,
    /// The name field the group verbs read.
    draft: String,
    note: Option<String>,
}

// ---------------------------------------------------------------------------
// the door, cached
// ---------------------------------------------------------------------------

/// The cache generation: a write bumps it and every listing refetches.
fn generation(ui: &Ui) -> u64 {
    ui.ctx().data(|d| d.get_temp(Id::new("shell/scenes/generation"))).unwrap_or(0)
}

/// `GET /author/scenes` (with `clip`, or the whole book), cached for
/// [`LIST_TTL`] and dropped on any write.
fn listing(ui: &Ui, door: &dyn Call, clip: Option<&str>) -> Value {
    let id = Id::new(("shell/scenes/listing", clip.unwrap_or_default().to_string()));
    let now = ui.input(|i| i.time);
    let gen = generation(ui);
    let cached: Option<(Value, f64, u64)> = ui.ctx().data(|d| d.get_temp(id));
    if let Some((v, _, _)) = cached.filter(|(_, at, g)| now - at < LIST_TTL && *g == gen) {
        return v;
    }
    let query: Vec<(&str, String)> = clip.map(|c| vec![("clip", c.to_string())]).unwrap_or_default();
    let v = door
        .bytes("GET", "/author/scenes", &query)
        .ok()
        .and_then(|r| crate::json_memo::parse(&r.body).ok())
        .unwrap_or_else(|| json!({}));
    ui.ctx().data_mut(|d| d.insert_temp(id, (v.clone(), now, gen)));
    v
}

/// One `POST /author/scenes`; the error text on refusal.
fn post(ui: &Ui, door: &dyn Call, body: Value) -> Option<String> {
    let gen = generation(ui) + 1;
    ui.ctx().data_mut(|d| d.insert_temp(Id::new("shell/scenes/generation"), gen));
    door.json("POST", "/author/scenes", Some(body)).err().map(|(_, msg)| msg)
}

/// A scene row's own fields, as a body's base.
fn scene_body(op: &str, s: &Value) -> Value {
    json!({ "op": op, "id": s["id"], "clip": s["clip"], "start": s["start"], "end": s["end"], "poster": s["poster"] })
}

/// A poster as a texture, fetched once per path.
fn poster(ui: &Ui, door: &dyn Call, path: &str) -> Option<TextureHandle> {
    let id = Id::new(("shell/scenes/poster", path.to_string()));
    if let Some(tex) = ui.ctx().data(|d| d.get_temp::<Option<TextureHandle>>(id)) {
        return tex;
    }
    let tex = door
        .bytes("GET", "/author/scenes/poster.jpg", &[("path", path.to_string())])
        .ok()
        .and_then(|r| crate::shell::frames::decode_jpeg_rgb(&r.body))
        .map(|(w, h, rgb)| ui.ctx().load_texture(path, ColorImage::from_rgb([w as usize, h as usize], &rgb), TextureOptions::LINEAR));
    ui.ctx().data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// The poster at `width`, keeping its aspect; nothing when there is none.
fn thumb(ui: &mut Ui, door: &dyn Call, s: &Value, width: f32) -> Option<egui::Response> {
    let tex = poster(ui, door, s["poster"].as_str()?)?;
    let [w, h] = tex.size();
    let size = Vec2::new(width, width * h as f32 / w.max(1) as f32);
    Some(ui.add(egui::Image::from_texture(&tex).fit_to_exact_size(size).sense(Sense::click())))
}

/// While a scene row is dragged, the whole row follows the pointer as it reads
/// in the list: its poster and its name, painted through
/// [`crate::shell::components::drag_ghost`] like the library's file rows, not as
/// a bare text label.
#[allow(clippy::too_many_arguments)]
fn scene_drag_ghost(
    ui: &Ui,
    theme: &Theme,
    door: &dyn Call,
    s: &Value,
    id: &str,
    response: &egui::Response,
    width: f32,
    thumb_width: f32,
    name: &str,
) {
    if !response.dragged() {
        return;
    }
    let Some(pointer) = response.interact_pointer_pos() else {
        return;
    };
    crate::shell::components::drag_ghost(ui.ctx(), theme, Id::new(("shell/scenes/drag-ghost", id)), pointer, width, |ui| {
        ui.horizontal(|ui| {
            thumb(ui, door, s, thumb_width);
            ui.label(RichText::new(name).font(theme.font_small.clone()).color(theme.ink));
        });
    });
}

/// `0:03.0–0:06.0 · 3.0 s`.
fn span(s: &Value) -> String {
    let at = |t: f64| format!("{}:{:04.1}", (t / 60.0).floor() as u64, t % 60.0);
    let (start, end) = (s["start"].as_f64().unwrap_or(0.0), s["end"].as_f64().unwrap_or(0.0));
    format!("{}–{} · {:.1} s", at(start), at(end), end - start)
}

/// The groups a listing names, sorted.
fn group_names(v: &Value) -> Vec<String> {
    match &v["groups"] {
        Value::Array(a) => a.iter().filter_map(|g| g.as_str().map(str::to_string)).collect(),
        Value::Object(o) => o.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The group picker: one checkbox per group, filing or unfiling the scene.
fn group_picker(ui: &mut Ui, theme: &Theme, door: &dyn Call, s: &Value, groups: &[String]) -> Option<String> {
    let mine: Vec<&str> = s["groups"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut err = None;
    let label = if mine.is_empty() { tr("groups").to_string() } else { mine.join(", ") };
    ui.menu_button(RichText::new(label).font(theme.font_small.clone()), |ui| {
        if groups.is_empty() {
            ui.label(RichText::new(tr("no groups yet — make one in the library's scenes")).font(theme.font_small.clone()).color(theme.ink_faint));
        }
        for g in groups {
            let mut on = mine.contains(&g.as_str());
            if crate::shell::components::toggle_row(ui, theme, &g, &mut on).changed() {
                let mut body = scene_body("file", s);
                body["group"] = json!(g);
                body["on"] = json!(on);
                err = post(ui, door, body);
            }
        }
    });
    err
}

// ---------------------------------------------------------------------------
// the properties gallery
// ---------------------------------------------------------------------------

/// A scene row as a media-board clip: the whole video file with its
/// `[start, end)` span marked, so the sequence or a loop names the same scene.
fn scene_clip(s: &Value) -> media::Clip {
    let clip = s["clip"].as_str().unwrap_or_default();
    let name = std::path::Path::new(clip).file_name().and_then(|n| n.to_str()).unwrap_or(clip).to_string();
    media::Clip {
        path: clip.to_string(),
        name,
        kind: "video".to_string(),
        scene: Some(media::Scene {
            id: s["id"].as_str().unwrap_or_default().to_string(),
            start: s["start"].as_f64().unwrap_or(0.0),
            end: s["end"].as_f64().unwrap_or(0.0),
        }),
        seconds: 0.0,
        looping: false,
    }
}

/// Where `path`'s scene `id` sits on the media board, if at all.
fn board_index(ctx: &egui::Context, path: &str, id: &str) -> Option<usize> {
    media::board(ctx)
        .clips
        .iter()
        .position(|c| c.kind == "video" && c.path == path && c.scene.as_ref().is_some_and(|s| s.id == id))
}

/// The marked scenes of a listing, in listing order.
fn marked<'a>(scenes: &'a [Value], selected: &'a BTreeSet<String>) -> impl Iterator<Item = &'a Value> + 'a {
    scenes.iter().filter(move |s| selected.contains(s["id"].as_str().unwrap_or_default()))
}

/// The gallery's toolbar, under its count: how many scenes are marked, and the
/// three verbs over them — send to the sequence, export each as a clip, clear.
fn gallery_toolbar(ui: &mut Ui, theme: &Theme, door: &dyn Call, scenes: &[Value], selected: &mut BTreeSet<String>, note: &mut Option<String>) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(trf("{n} selected", &[("n", &selected.len())])).font(theme.font_small.clone()).color(theme.ink_weak));
        let any = !selected.is_empty();
        if ui.add_enabled(any, egui::Button::new(RichText::new(tr("send to sequence")).font(theme.font_small.clone()))).clicked() {
            media::set_clips(ui.ctx(), marked(scenes, selected).map(scene_clip).collect());
            media::set_cursor(ui.ctx(), 0);
        }
        if ui.add_enabled(any, egui::Button::new(RichText::new(tr("export clips")).font(theme.font_small.clone()))).clicked() {
            let mut paths = Vec::new();
            let mut first_err = None;
            for s in marked(scenes, selected) {
                let c = scene_clip(s);
                if let Some(body) = media::scene_export_body(&c, None) {
                    match door.json("POST", "/author/scenes/export", Some(body)) {
                        Ok(v) => {
                            if let Some(p) = v["path"].as_str() {
                                paths.push(p.to_string());
                            }
                        }
                        Err((_, msg)) => first_err = first_err.or(Some(msg)),
                    }
                }
            }
            *note = Some(first_err.unwrap_or_else(|| paths.join(", ")));
        }
        if ui.add_enabled(any, egui::Button::new(RichText::new(tr("clear selection")).font(theme.font_small.clone()))).clicked() {
            selected.clear();
        }
    });
}

/// A selected video's scenes. Starts the clip's ingest when it has none yet.
pub(crate) fn gallery(ui: &mut Ui, theme: &Theme, door: &dyn Call, clip: &str) {
    let v = listing(ui, door, Some(clip));
    let scenes = v["scenes"].as_array().cloned().unwrap_or_default();
    ui.label(RichText::new(trf("scenes · {n}", &[("n", &scenes.len())])).font(theme.font_body.clone()).color(theme.ink));
    if v["pending"] == true {
        ui.label(RichText::new(tr("cutting scenes…")).font(theme.font_small.clone()).color(theme.ink_faint));
        crate::shell::frames::repaint_after(ui.ctx(), std::time::Duration::from_secs_f64(LIST_TTL));
        return;
    }

    // The marked scene ids live in egui memory, keyed by this clip, so a
    // selection survives the frame that made it and the row loop below.
    let selected_id = Id::new(("shell/scenes/selected", clip));
    let mut selected: BTreeSet<String> = ui.ctx().data(|d| d.get_temp(selected_id)).unwrap_or_default();
    let mut note = None;
    gallery_toolbar(ui, theme, door, &scenes, &mut selected, &mut note);

    let groups = group_names(&v);
    let preview_id = Id::new("shell/scenes/preview");
    let previewing: Option<String> = ui.ctx().data(|d| d.get_temp(preview_id)).flatten();
    let mut err = None;
    for s in &scenes {
        let id = s["id"].as_str().unwrap_or_default().to_string();
        let clip_path = s["clip"].as_str().unwrap_or_default();
        let file = std::path::Path::new(clip_path).file_name().and_then(|n| n.to_str()).unwrap_or(clip_path).to_string();
        let (start, end) = (s["start"].as_f64().unwrap_or(0.0), s["end"].as_f64().unwrap_or(0.0));
        ui.separator();
        let row = ui.horizontal_wrapped(|ui| {
            let mut on = selected.contains(&id);
            if crate::shell::components::toggle_row(ui, theme, "", &mut on).changed() {
                if on {
                    selected.insert(id.clone());
                } else {
                    selected.remove(&id);
                }
            }
            if thumb(ui, door, s, theme.overlay_cell.x).is_some_and(|r| r.clicked()) {
                toggle_preview(ui, preview_id, &id, previewing.as_deref());
            }
            ui.vertical(|ui| {
                ui.label(RichText::new(span(s)).font(theme.font_small.clone()).color(theme.ink_weak));
                ui.horizontal_wrapped(|ui| {
                    let playing = previewing.as_deref() == Some(id.as_str());
                    let label = if playing { tr("stop") } else { tr("preview") };
                    if ui.button(RichText::new(label).font(theme.font_small.clone())).clicked() {
                        toggle_preview(ui, preview_id, &id, previewing.as_deref());
                    }
                    let name_id = Id::new(("shell/scenes/name", id.clone()));
                    let mut name: String = ui
                        .ctx()
                        .data(|d| d.get_temp(name_id))
                        .unwrap_or_else(|| s["name"].as_str().unwrap_or_default().to_string());
                    ui.add(egui::TextEdit::singleline(&mut name).hint_text(tr("name")).desired_width(theme.text_input_width));
                    ui.ctx().data_mut(|d| d.insert_temp(name_id, name.clone()));
                    let saved = s["saved"] == true;
                    if ui.selectable_label(saved, RichText::new(tr("save")).font(theme.font_small.clone())).clicked() {
                        let mut body = scene_body("save", s);
                        // Clicking a saved scene with a new name renames it;
                        // with the same name it is unsaved.
                        let renamed = saved && name != s["name"].as_str().unwrap_or_default();
                        body["on"] = json!(!saved || renamed);
                        body["name"] = json!(name);
                        err = post(ui, door, body);
                    }
                    let fav = s["favourite"] == true;
                    if ui.selectable_label(fav, lucide(Icon::Star, theme.font_small.size)).on_hover_text(tr("favourite")).clicked() {
                        let mut body = scene_body("favourite", s);
                        body["on"] = json!(!fav);
                        err = post(ui, door, body);
                    }
                    if let Some(e) = group_picker(ui, theme, door, s, &groups) {
                        err = Some(e);
                    }
                    // A loop taken off this scene: already on the board it
                    // toggles, otherwise it is pushed and turned on.
                    let on_loop = board_index(ui.ctx(), clip_path, &id).is_some_and(|i| media::loop_of(ui.ctx(), i));
                    if ui.selectable_label(on_loop, RichText::new(tr("loop scene")).font(theme.font_small.clone())).clicked() {
                        match board_index(ui.ctx(), clip_path, &id) {
                            Some(i) => media::set_loop(ui.ctx(), i, !media::loop_of(ui.ctx(), i)),
                            None => {
                                media::push(ui.ctx(), scene_clip(s));
                                if let Some(i) = board_index(ui.ctx(), clip_path, &id) {
                                    media::set_loop(ui.ctx(), i, true);
                                }
                            }
                        }
                    }
                });
            });
        });
        // The whole row drags as the scene, exactly like the library's own
        // `scene_row`: onto the nodes canvas it pops the scene's node.
        let response = ui.interact(row.response.rect, ui.id().with(("shell/scenes/gallery-row", &id)), Sense::click_and_drag());
        response.dnd_set_drag_payload(FileDrag {
            name: file.clone(),
            path: clip_path.to_string(),
            kind: "video".to_string(),
            seconds: Some(end - start),
            scene: Some((id.clone(), start, end)),
        });
        let file_name = s["name"].as_str().filter(|n| !n.is_empty()).map_or_else(|| file.clone(), str::to_string);
        scene_drag_ghost(ui, theme, door, s, &id, &response, row.response.rect.width(), theme.overlay_cell.x, &file_name);
        if previewing.as_deref() == Some(id.as_str()) {
            preview(ui, theme, door, s);
        }
    }
    if let Some(e) = err {
        ui.label(RichText::new(e).font(theme.font_small.clone()).color(theme.ink_weak));
    }
    if let Some(n) = note {
        ui.label(RichText::new(n).font(theme.font_small.clone()).color(theme.ink_weak));
    }
    ui.ctx().data_mut(|d| d.insert_temp(selected_id, selected));
}

fn toggle_preview(ui: &Ui, key: Id, id: &str, current: Option<&str>) {
    let next = (current != Some(id)).then(|| id.to_string());
    ui.ctx().data_mut(|d| d.insert_temp(key, next));
}

/// The scene looping in a picture: the clock wraps inside `[start, end)` and
/// each step seeks the editor's decoder there and reads its frame.
fn preview(ui: &mut Ui, theme: &Theme, door: &dyn Call, s: &Value) {
    let (start, end) = (s["start"].as_f64().unwrap_or(0.0), s["end"].as_f64().unwrap_or(0.0));
    let clip = s["clip"].as_str().unwrap_or_default();
    let key = Id::new("shell/scenes/preview-frame");
    let now = ui.input(|i| i.time);
    let last: Option<(f64, Option<TextureHandle>)> = ui.ctx().data(|d| d.get_temp(key));
    let mut tex = last.as_ref().and_then(|(_, t)| t.clone());
    if last.as_ref().is_none_or(|(at, _)| now - at >= PREVIEW_STEP) {
        let t = start + now.rem_euclid((end - start).max(PREVIEW_STEP));
        let _ = door.json("POST", "/author/video/seek", Some(json!({ "path": clip, "t": t })));
        if let Some((w, h, rgb)) = door
            .bytes("GET", "/author/video/frame.jpg", &[("path", clip.to_string())])
            .ok()
            .and_then(|r| crate::shell::frames::decode_jpeg_rgb(&r.body))
        {
            let image = ColorImage::from_rgb([w as usize, h as usize], &rgb);
            // Reuse the held texture: a fresh `load_texture` each step created
            // and freed a GPU texture every step.
            match tex.as_mut() {
                Some(held) => held.set(image, TextureOptions::LINEAR),
                None => tex = Some(ui.ctx().load_texture("shell/scenes/preview", image, TextureOptions::LINEAR)),
            }
        }
        ui.ctx().data_mut(|d| d.insert_temp(key, (now, tex.clone())));
    }
    crate::shell::frames::repaint_after(ui.ctx(), std::time::Duration::from_secs_f64(PREVIEW_STEP));
    match tex {
        Some(tex) => {
            let [w, h] = tex.size();
            let width = ui.available_width();
            ui.add(egui::Image::from_texture(&tex).fit_to_exact_size(Vec2::new(width, width * h as f32 / w.max(1) as f32)));
        }
        None => {
            ui.label(RichText::new(tr("decoding…")).font(theme.font_small.clone()).color(theme.ink_faint));
        }
    }
}

// ---------------------------------------------------------------------------
// the library's Scenes route
// ---------------------------------------------------------------------------

/// Every kept scene across all videos. Returns the video a click picked.
pub(crate) fn route(ui: &mut Ui, theme: &Theme, door: &dyn Call, state: &mut RouteState) -> Option<FileRef> {
    let v = listing(ui, door, None);
    let groups = group_names(&v);
    if state.group.as_ref().is_some_and(|g| !groups.contains(g)) {
        state.group = None;
    }

    // The filter: every kept scene, the favourites, or one group.
    ui.horizontal_wrapped(|ui| {
        let shown = match &state.group {
            None => tr("all scenes").to_string(),
            Some(g) => g.clone(),
        };
        crate::shell::components::combo_box(theme, Id::new("shell/region/library/scenes/group"))
            .selected_text(shown)
            .show_ui(ui, |ui| {
                if ui.selectable_label(state.group.is_none(), tr("all scenes")).clicked() {
                    state.group = None;
                }
                for g in &groups {
                    if ui.selectable_label(state.group.as_ref() == Some(g), g).clicked() {
                        state.group = Some(g.clone());
                    }
                }
            });
        ui.add(egui::TextEdit::singleline(&mut state.query).hint_text(tr("search scenes…")).desired_width(theme.text_input_width));
    });

    // The group verbs, over one name field: new, and — with a group picked
    // — rename to it, or delete the group (never its favourites).
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(&mut state.draft).hint_text(tr("group name")).desired_width(theme.text_input_width));
        let named = !state.draft.trim().is_empty();
        if ui.add_enabled(named, egui::Button::new(RichText::new(tr("new group")).font(theme.font_small.clone()))).clicked() {
            state.note = post(ui, door, json!({ "op": "group.create", "group": state.draft.trim() }));
            state.draft.clear();
        }
        if let Some(g) = state.group.clone() {
            if ui.add_enabled(named, egui::Button::new(RichText::new(tr("rename group")).font(theme.font_small.clone()))).clicked() {
                let to = state.draft.trim().to_string();
                state.note = post(ui, door, json!({ "op": "group.rename", "group": g, "to": to }));
                if state.note.is_none() {
                    state.group = Some(to);
                    state.draft.clear();
                }
            }
            if ui.button(RichText::new(tr("delete group")).font(theme.font_small.clone())).clicked() {
                state.note = post(ui, door, json!({ "op": "group.delete", "group": g }));
                state.group = None;
            }
        }
    });

    let in_group: Option<Vec<String>> = state.group.as_ref().map(|g| {
        v["groups"][g.as_str()].as_array().map(|a| a.iter().filter_map(|i| i.as_str().map(str::to_string)).collect()).unwrap_or_default()
    });
    let q = state.query.trim().to_lowercase();
    let rows: Vec<&Value> = v["scenes"]
        .as_array()
        .map(|a| a.iter().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|s| in_group.as_ref().is_none_or(|ids| ids.iter().any(|i| s["id"] == i.as_str())))
        .filter(|s| {
            q.is_empty()
                || s["name"].as_str().unwrap_or_default().to_lowercase().contains(&q)
                || s["clip"].as_str().unwrap_or_default().to_lowercase().contains(&q)
        })
        .collect();
    if rows.is_empty() {
        ui.label(RichText::new(tr("no saved or favourite scenes")).font(theme.font_small.clone()).color(theme.ink_weak));
    }

    let mut picked = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = theme.library_gap;
        for s in rows {
            if let Some(f) = scene_row(ui, theme, door, s) {
                picked = Some(f);
            }
        }
    });
    if let Some(note) = &state.note {
        ui.label(RichText::new(note.clone()).font(theme.font_small.clone()).color(theme.ink_weak));
    }
    picked
}

/// One kept scene: poster, name (or the clip's file name), span, `★`; the
/// whole row drags as the scene and clicks as its video.
fn scene_row(ui: &mut Ui, theme: &Theme, door: &dyn Call, s: &Value) -> Option<FileRef> {
    let clip = s["clip"].as_str().unwrap_or_default();
    let file = std::path::Path::new(clip).file_name().and_then(|n| n.to_str()).unwrap_or(clip).to_string();
    let name = s["name"].as_str().filter(|n| !n.is_empty()).map_or_else(|| file.clone(), str::to_string);
    let (start, end) = (s["start"].as_f64().unwrap_or(0.0), s["end"].as_f64().unwrap_or(0.0));
    let star = if s["favourite"] == true { "★ " } else { "" };
    let inner = ui.horizontal(|ui| {
        thumb(ui, door, s, theme.library_row * theme.overlay_cell.x / theme.overlay_cell.y.max(1.0));
        ui.label(RichText::new(format!("{star}{name}")).font(theme.font_small.clone()).color(theme.ink));
        ui.label(RichText::new(span(s)).font(theme.font_small.clone()).color(theme.ink_faint));
    });
    let id = s["id"].as_str().unwrap_or_default().to_string();
    let response = ui.interact(inner.response.rect, ui.id().with(("shell/region/library/scene", &id)), Sense::click_and_drag());
    response.dnd_set_drag_payload(FileDrag {
        name: name.clone(),
        path: clip.to_string(),
        kind: "video".to_string(),
        seconds: Some(end - start),
        scene: Some((id.clone(), start, end)),
    });
    scene_drag_ghost(ui, theme, door, s, &id, &response, inner.response.rect.width(), theme.scene_thumb_width(), &name);
    response.clicked().then(|| FileRef { name: file, path: clip.to_string(), kind: "video".to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    /// Dragging a scene row leaves a ghost that carries the scene's name as a
    /// label: the row's own parts, not a bare painted text.
    #[test]
    fn a_dragged_scene_row_shows_its_name_as_a_ghost_label() {
        let door = crate::shell::FakeDoor::new();
        let theme = Theme::phosphor();
        let probe = theme.overlay_cell;
        let s = json!({ "id": "s1", "name": "Alpha scene", "clip": "/clips/a.mp4", "start": 0.0, "end": 3.0 });
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            let _ = scene_row(ui, &theme, &door, &s);
        });
        harness.run();
        let at = harness.get_by_label("Alpha scene").rect().center();
        harness.drag_at(at);
        harness.run();
        harness.hover_at(at + probe);
        harness.run();
        assert_eq!(
            harness.get_all_by_label("Alpha scene").count(),
            2,
            "the row and its drag ghost both carry the name"
        );
    }
}
