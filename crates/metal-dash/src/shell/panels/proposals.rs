//! What the AI proposes before it writes: a tool call waiting on the
//! operator (`awaiting_approval`) shown as the change it would make — in the
//! chat under the call and on the property it touches — with the same
//! accept/deny in both places (`POST /ai/chat/approve`). A node's property
//! values read `name  old → new`; a script's source reads as a line diff.
//!
//! The chat publishes the waiting calls each frame ([`publish`]); the
//! properties panel reads them ([`pending`]). Neither owns the other: the call
//! itself is the one source of truth, so a decision from either place lands in
//! both on the next frame.

use egui::{Context, Id, RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{ai_button, small_text};
use crate::shell::i18n::tr;

/// Calls older than this (seconds since the chat last published) are gone.
const FRESH: f64 = 2.0;

/// A tool call waiting for the operator.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// One property value a call would change.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub name: String,
    pub old: Option<String>,
    pub new: String,
}

fn key() -> Id {
    Id::new("shell/ai/pending")
}

/// Records the calls waiting right now (an empty list clears).
pub fn publish(ctx: &Context, calls: Vec<Pending>) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(key(), (now, calls)));
}

/// The calls waiting as of the last chat frame.
pub fn pending(ctx: &Context) -> Vec<Pending> {
    let now = ctx.input(|i| i.time);
    ctx.data(|d| d.get_temp::<(f64, Vec<Pending>)>(key()))
        .filter(|(at, _)| now - at <= FRESH)
        .map(|(_, calls)| calls)
        .unwrap_or_default()
}

/// Accept (`true`) or deny a waiting call.
pub fn decide(door: &dyn Call, id: &str, allow: bool) {
    let _ = door.json("POST", "/ai/chat/approve", Some(json!({ "call": id, "allow": allow })));
}

fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), |f| format!("{}", (f * 1000.0).round() / 1000.0)),
        other => other.to_string(),
    }
}

/// The node a call is about.
pub fn node_of(p: &Pending) -> Option<String> {
    let a = &p.args;
    a["node"]
        .as_str()
        .or_else(|| a["id"].as_str())
        .or_else(|| a["updates"][0]["node"].as_str())
        .map(str::to_string)
}

/// The property values a call would set on `node` (the node's own `config`
/// read for the old value when it has one under that name).
pub fn changes(p: &Pending, node: &str, current: &Value) -> Vec<Change> {
    let old_of = |name: &str| -> Option<String> {
        let c = &current["config"];
        if !c[name].is_null() {
            return Some(show(&c[name]));
        }
        c["floats"]
            .as_array()?
            .iter()
            .find(|f| f["name"].as_str() == Some(name))
            .map(|f| show(if f["value"].is_null() { &f["default"] } else { &f["value"] }))
    };
    let mut out = Vec::new();
    match p.name.as_str() {
        "set_params" if node_of(p).as_deref() == Some(node) => {
            if let Some(patch) = p.args["patch"].as_object() {
                for (k, v) in patch {
                    if let Some(floats) = v.as_object().filter(|_| k == "floats") {
                        for (fk, fv) in floats {
                            out.push(Change { name: fk.clone(), old: old_of(fk), new: show(fv) });
                        }
                    } else {
                        out.push(Change { name: k.clone(), old: old_of(k), new: show(v) });
                    }
                }
            }
        }
        "set_param" if p.args["node"].as_str() == Some(node) => {
            let name = p.args["name"].as_str().unwrap_or("?");
            out.push(Change { name: name.to_string(), old: old_of(name), new: show(&p.args["value"]) });
        }
        "set_param_batch" => {
            for u in p.args["updates"].as_array().into_iter().flatten() {
                if u["node"].as_str() == Some(node) {
                    let name = u["name"].as_str().unwrap_or("?");
                    out.push(Change { name: name.to_string(), old: old_of(name), new: show(&u["value"]) });
                }
            }
        }
        _ => {}
    }
    out
}

/// The proposed source of a `save_script` for `node`, if one waits.
pub fn script_source(p: &Pending, node: &str) -> Option<String> {
    (p.name == "save_script" && p.args["node"].as_str() == Some(node))
        .then(|| p.args["source"].as_str().map(str::to_string))
        .flatten()
}

/// How a diff line stands against the old text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Same,
    Add,
    Del,
}

/// A line diff (longest common subsequence), lines in reading order.
pub fn diff_lines(old: &str, new: &str) -> Vec<(Tag, String)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push((Tag::Same, a[i].to_string()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push((Tag::Del, a[i].to_string()));
            i += 1;
        } else {
            out.push((Tag::Add, b[j].to_string()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| (Tag::Del, l.to_string())));
    out.extend(b[j..].iter().map(|l| (Tag::Add, l.to_string())));
    out
}

/// A diff as painted lines: added in the AI blue on its wash, removed struck
/// through in the weak ink, unchanged lines dimmed (and elided far from a change).
pub fn diff_view(ui: &mut Ui, theme: &Theme, lines: &[(Tag, String)]) {
    const CONTEXT: usize = 2;
    let near = |at: usize| {
        lines[at.saturating_sub(CONTEXT)..(at + CONTEXT + 1).min(lines.len())].iter().any(|(t, _)| *t != Tag::Same)
    };
    let mut skipped = false;
    for (at, (tag, line)) in lines.iter().enumerate() {
        if *tag == Tag::Same && !near(at) {
            skipped = true;
            continue;
        }
        if std::mem::take(&mut skipped) {
            ui.label(small_text(theme, "…").color(theme.ink_faint));
        }
        let mono = theme.font_mono(theme.font_small.size);
        let text = match tag {
            Tag::Add => RichText::new(format!("+ {line}")).color(theme.ai).background_color(theme.faded(theme.ai, 28)),
            Tag::Del => RichText::new(format!("- {line}")).color(theme.ink_weak).strikethrough(),
            Tag::Same => RichText::new(format!("  {line}")).color(theme.ink_faint),
        };
        ui.add(egui::Label::new(text.font(mono)).wrap());
    }
}

/// The accept/deny pair for a waiting call.
pub fn decision_row(ui: &mut Ui, theme: &Theme, door: &dyn Call, id: &str) {
    ui.horizontal(|ui| {
        if ai_button(ui, theme, &tr("accept")).clicked() {
            decide(door, id, true);
        }
        if ai_button(ui, theme, &tr("deny")).clicked() {
            decide(door, id, false);
        }
    });
}

/// `name  old → new` rows for a list of changes, the new value in the AI blue.
pub fn change_rows(ui: &mut Ui, theme: &Theme, changes: &[Change]) {
    for c in changes {
        let mut job = egui::text::LayoutJob::default();
        let f = |color| egui::TextFormat::simple(theme.font_small.clone(), color);
        job.append(&c.name, 0.0, f(theme.ink));
        if let Some(old) = &c.old {
            job.append(&format!("  {old}"), 0.0, f(theme.ink_weak));
            job.append("  →", 0.0, f(theme.ink_faint));
        }
        job.append(&format!("  {}", c.new), 0.0, f(theme.ai));
        ui.label(job);
    }
}

/// The review bar at the top of a node's properties: every waiting call that
/// changes this node, its values and accept/deny. Nothing is painted when
/// nothing waits.
pub fn review_bar(ui: &mut Ui, theme: &Theme, door: &dyn Call, node_id: &str, node: &Value) {
    for p in pending(ui.ctx()) {
        let rows = changes(&p, node_id, node);
        if rows.is_empty() {
            continue;
        }
        crate::shell::components::payload_cell(ui, theme, None, |ui| {
            ui.vertical(|ui| {
                ui.label(small_text(theme, tr("the AI proposes")).color(theme.ai).strong());
                change_rows(ui, theme, &rows);
                decision_row(ui, theme, door, &p.id);
            });
        });
        ui.add_space(theme.section_gap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_marks_added_and_removed_lines() {
        let d = diff_lines("a\nb\nc", "a\nB\nc\nd");
        assert_eq!(
            d,
            vec![
                (Tag::Same, "a".into()),
                (Tag::Del, "b".into()),
                (Tag::Add, "B".into()),
                (Tag::Same, "c".into()),
                (Tag::Add, "d".into()),
            ]
        );
    }

    #[test]
    fn set_params_lists_each_patched_value_with_the_old_one() {
        let p = Pending {
            id: "c1".into(),
            name: "set_params".into(),
            args: json!({ "id": "fx-3", "patch": { "intensity": 0.2 } }),
        };
        let node = json!({ "config": { "intensity": 0.8 } });
        assert_eq!(
            changes(&p, "fx-3", &node),
            vec![Change { name: "intensity".into(), old: Some("0.8".into()), new: "0.2".into() }]
        );
        assert!(changes(&p, "fx-9", &node).is_empty());
    }

    #[test]
    fn a_save_script_proposes_its_source_for_that_node_only() {
        let p = Pending {
            id: "c2".into(),
            name: "save_script".into(),
            args: json!({ "node": "s-1", "source": "x = 1" }),
        };
        assert_eq!(script_source(&p, "s-1").as_deref(), Some("x = 1"));
        assert_eq!(script_source(&p, "s-2"), None);
    }
}
