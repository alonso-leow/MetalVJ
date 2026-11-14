//! Imported desk scenes in the shell: the scenes gallery's sections (the same
//! overlay as "new node" and settings, [`crate::shell::gallery`]) and the
//! properties page of a scene clip on the lanes.
//!
//! Sections: first by what a scene does (movement, strobe, colour, static,
//! other — `metal_stage::scene::tags`), then one per show folder (the first
//! path part of the desk folder). A card's kind is `scene.<slug>`.
//!
//! Every read and write goes through the door: `GET /author/stage/scenes`,
//! `GET /author/stage/scene`, `GET /author/stage/scene/clip`, `POST
//! /author/stage/scene`.

use std::collections::{BTreeMap, HashMap};

use egui::{Id, RichText, Ui};
use serde_json::Value;

use crate::call::{Call, Params};
use crate::shell::classes::Theme;
use crate::shell::components::{field_row, icon_button, number_row, section, slider_row, subtitle, toggle_row, Icon};
use crate::shell::gallery::{Item, Section, STRIP};
use crate::shell::i18n::{tr, tr_dyn, trf};
use crate::shell::lanes::{IngestLane, EDITOR_RATE};
use crate::shell::panels::lanes::SCENE_KIND_PREFIX;
use metal_stage::scene::Scene;

/// How long one listing is reused, in seconds.
const LIST_TTL: f64 = 2.0;

/// The tag sections, in order, with their icons.
const TAGS: [(&str, Icon); 5] =
    [("movement", Icon::Radar), ("strobe", Icon::Spark), ("colour", Icon::Palette), ("static", Icon::Lamp), ("other", Icon::Other)];

fn get(door: &dyn Call, path: &str, query: &[(&str, &str)]) -> Option<Value> {
    let q: HashMap<String, String> = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let reply = door.call("GET", path, Params { query: q, body: Vec::new(), tail: None })?;
    (reply.status == 200).then(|| serde_json::from_slice(&reply.body).ok()).flatten()
}

/// The listing, cached for [`LIST_TTL`] in the context's memory.
fn listing(ctx: &egui::Context, door: &dyn Call) -> Vec<Value> {
    let id = Id::new("shell/stage-scenes/list");
    let now = ctx.input(|i| i.time);
    if let Some((at, rows)) = ctx.data(|d| d.get_temp::<(f64, std::sync::Arc<Vec<Value>>)>(id)) {
        if now - at < LIST_TTL {
            return rows.as_ref().clone();
        }
    }
    let rows = get(door, "/author/stage/scenes", &[]).and_then(|v| v["scenes"].as_array().cloned()).unwrap_or_default();
    ctx.data_mut(|d| d.insert_temp(id, (now, std::sync::Arc::new(rows.clone()))));
    rows
}

fn forget_listing(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<(f64, std::sync::Arc<Vec<Value>>)>(Id::new("shell/stage-scenes/list")));
}

fn item(row: &Value) -> Option<Item> {
    let slug = row["slug"].as_str()?;
    let secs = row["length_ms"].as_f64().unwrap_or(0.0) / 1000.0;
    Some(Item {
        kind: format!("{SCENE_KIND_PREFIX}{slug}"),
        label: row["name"].as_str().unwrap_or(slug).to_string(),
        family: format!("{} · {secs:.1} s · {}", row["folder"].as_str().unwrap_or(""), row["steps"].as_u64().unwrap_or(0)),
        group: row["folder"].as_str().map(show_of).unwrap_or_default().to_string(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        tiles: [None; STRIP],
        starred: false,
        favourite: false,
    })
}

/// The show a desk folder belongs to: its first path part.
pub fn show_of(folder: &str) -> &str {
    folder.split('/').next().unwrap_or(folder).trim()
}

/// Groups listing rows into the gallery's divisions: tag sections, then one
/// per show. Pure, so the grouping is testable without a door.
pub fn sections_of(rows: &[Value]) -> Vec<Section> {
    let mut out: Vec<Section> = TAGS
        .iter()
        .map(|(tag, icon)| Section {
            icon: *icon,
            label: tr_dyn(tag),
            items: rows
                .iter()
                .filter(|r| r["tags"].as_array().is_some_and(|t| t.iter().any(|x| x.as_str() == Some(tag))))
                .filter_map(item)
                .collect(),
        })
        .collect();
    let mut shows: BTreeMap<String, Vec<Item>> = BTreeMap::new();
    for r in rows {
        if let Some(i) = item(r) {
            shows.entry(show_of(r["folder"].as_str().unwrap_or("")).to_string()).or_default().push(i);
        }
    }
    out.extend(shows.into_iter().map(|(label, items)| Section { icon: Icon::Folder, label, items }));
    out
}

/// The scenes gallery's sections (`import scene` on the lanes).
pub fn gallery_sections(ctx: &egui::Context, door: &dyn Call) -> Vec<Section> {
    sections_of(&listing(ctx, door))
}

// ---------------------------------------------------------------------------
// properties
// ---------------------------------------------------------------------------

/// The scene being edited, kept in the context's memory per slug until saved.
fn draft_id(slug: &str) -> Id {
    Id::new(("shell/stage-scenes/draft", slug.to_string()))
}

fn load(ui: &Ui, door: &dyn Call, slug: &str) -> Option<Scene> {
    if let Some(s) = ui.data(|d| d.get_temp::<std::sync::Arc<Scene>>(draft_id(slug))) {
        return Some(s.as_ref().clone());
    }
    let s: Scene = serde_json::from_value(get(door, "/author/stage/scene", &[("slug", slug)])?).ok()?;
    ui.data_mut(|d| d.insert_temp(draft_id(slug), std::sync::Arc::new(s.clone())));
    Some(s)
}

/// The properties page of the active lanes clip when it is a scene: the
/// clip (length, copy, paste) and the scene itself (name, folder, steps,
/// every fixture's channels), saved back to the scene file and re-read into
/// the clip.
pub fn section_for_clip(ui: &mut Ui, theme: &Theme, door: &dyn Call, ingest: &mut IngestLane) {
    let Some(slug) = ingest.scene.clone() else { return };
    let open_id = Id::new("shell/stage-scenes/open");
    let mut open: [bool; 2] = ui.data(|d| d.get_temp(open_id)).unwrap_or([true, false]);

    section(ui, theme, &tr("clip"), &mut open[0]);
    if open[0] {
        if let Some((a, b)) = crate::shell::lanes::clip_bounds(ingest.bounds, &ingest.keyframes) {
            let mut secs = (b - a) as f32 / EDITOR_RATE;
            if number_row(ui, theme, &tr("length"), &mut secs, 0.1..=600.0, " s", 2, true).changed() {
                let ticks = (secs * EDITOR_RATE).round().max(1.0) as usize;
                ingest.resize_clip(ingest.active, ticks);
            }
        }
        ui.horizontal(|ui| {
            if icon_button(ui, theme, Icon::Copy, false, &tr("copy the clip")).clicked() {
                ingest.copy_clip(ingest.active);
            }
            if icon_button(ui, theme, Icon::Paste, false, &tr("paste at the playhead")).clicked() {
                ingest.paste_clip();
            }
            if icon_button(ui, theme, Icon::Revert, false, &tr("reload the scene into the clip")).clicked() {
                reload_clip(door, ingest, &slug);
            }
        });
    }

    let Some(mut scene) = load(ui, door, &slug) else {
        field_row(ui, theme, &tr("scene"), &slug);
        return;
    };
    let before = scene.clone();
    section(ui, theme, &tr("scene"), &mut open[1]);
    if open[1] {
        crate::shell::components::label_row(ui, theme, &tr("name"), |ui| ui.text_edit_singleline(&mut scene.name));
        crate::shell::components::label_row(ui, theme, &tr("folder"), |ui| ui.text_edit_singleline(&mut scene.folder));
        for (i, step) in scene.steps.iter_mut().enumerate() {
            subtitle(ui, theme, &trf("step {n}", &[("n", &(i + 1))]));
            let mut secs = step.length as f32 / 1000.0;
            if number_row(ui, theme, &tr("length"), &mut secs, 0.0..=600.0, " s", 2, true).changed() {
                step.length = (secs * 1000.0).round() as u32;
            }
            for f in &mut step.fixtures {
                let head = format!("{} · {}", f.model, f.id);
                egui::CollapsingHeader::new(RichText::new(head).font(theme.font_small.clone()))
                    .id_salt(("scene-fx", i, f.id))
                    .show(ui, |ui| {
                        for (name, v) in f.channels.iter_mut() {
                            slider_row(ui, theme, name, &mut v.norm, 0.0..=1.0, true);
                            toggle_row(ui, theme, &tr("fade"), &mut v.fade);
                        }
                    });
            }
        }
        let dirty = scene != before;
        ui.horizontal(|ui| {
            if icon_button(ui, theme, Icon::Save, dirty, &tr("save the scene")).clicked() {
                let body = serde_json::to_vec(&scene).unwrap_or_default();
                let ok = door
                    .call("POST", "/author/stage/scene", Params { query: HashMap::new(), body, tail: None })
                    .is_some_and(|r| r.status == 200);
                ingest.note = Some(if ok { tr("scene saved") } else { tr("the scene was not saved") }.to_string());
                if ok {
                    forget_listing(ui.ctx());
                    reload_clip(door, ingest, &slug);
                }
            }
            if icon_button(ui, theme, Icon::Undo, false, &tr("discard the changes")).clicked() {
                ui.data_mut(|d| d.remove::<std::sync::Arc<Scene>>(draft_id(&slug)));
            }
        });
    }
    if scene != before {
        ui.data_mut(|d| d.insert_temp(draft_id(&slug), std::sync::Arc::new(scene)));
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
}

/// Re-reads the scene's keys into the active clip, kept at its mark-in and
/// stretched to its current length.
fn reload_clip(door: &dyn Call, ingest: &mut IngestLane, slug: &str) {
    let Some(clip) = get(door, "/author/stage/scene/clip", &[("slug", slug)]) else { return };
    let Some((a, b)) = crate::shell::lanes::clip_bounds(ingest.bounds, &ingest.keyframes) else { return };
    let i = ingest.active;
    ingest.replace_scene_clip(i, a, &clip);
    ingest.resize_clip(i, b - a);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sections_are_tags_then_shows() {
        let rows = vec![
            json!({"slug": "a", "name": "A", "folder": "Show1/x", "length_ms": 500, "steps": 2, "tags": ["movement", "colour"]}),
            json!({"slug": "b", "name": "B", "folder": "Show2", "length_ms": 500, "steps": 1, "tags": ["static"]}),
        ];
        let s = sections_of(&rows);
        assert_eq!(s.len(), TAGS.len() + 2);
        assert_eq!(s[0].items.len(), 1);
        assert_eq!(s[0].items[0].kind, "scene.a");
        assert_eq!(s[2].items.len(), 1);
        assert_eq!(s[3].items[0].kind, "scene.b");
        assert_eq!(s[TAGS.len()].label, "Show1");
        assert_eq!(s[TAGS.len() + 1].label, "Show2");
    }
}
