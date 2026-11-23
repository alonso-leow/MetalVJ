//! The properties browser's reusable pieces: the search bar and status line
//! the collapsible sections share, the folder tree/chips helpers, and the
//! striped/sortable table both table sections and hook modules paint rows
//! with.
//!
//! Appearance follows VirtualDJ's browser (the reference image in the brief): a
//! folder tree of coloured icons, a dense table with sortable column headers
//! (the sorted column marked with an arrow), striped rows, used rows in an
//! accent token and unused rows in the weak ink, a search field with a count
//! above the table, and a coloured status line under it. Every colour and size
//! is a [`Theme`] token (gate 2).

use crate::shell::components::small_text;
use egui::{Id, RichText, Ui};

use crate::shell::classes::Theme;
use crate::shell::i18n::{tr, trf};

/// One table column: its header text and how wide its cells are laid out.
#[derive(Clone)]
pub(crate) struct Column {
    pub title: &'static str,
    pub width: f32,
}

impl Column {
    pub(crate) fn new(title: &'static str, width: f32) -> Self {
        Column { title, width }
    }
}

/// One table row: its cells, whether it is "used" (accent) or not, and any
/// key the caller needs back on click (a node id, a param name, an index).
#[derive(Clone)]
pub(crate) struct Row {
    pub cells: Vec<String>,
    pub used: bool,
    pub key: RowKey,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum RowKey {
    #[cfg(test)]
    None,
    Text(String),
}

/// A sortable, striped, dense table. Built by a folder (or a hook module) and
/// painted by [`Table::ui`]; the caller decides what a clicked row means.
pub(crate) struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// The column the rows are sorted by, and whether the sort is ascending.
    pub sort: usize,
    pub ascending: bool,
}

impl Table {
    pub(crate) fn new(columns: Vec<Column>, rows: Vec<Row>) -> Self {
        Table { columns, rows, sort: 0, ascending: true }
    }

    /// Sorts rows by `sort`'s cell (numeric when both parse, else text),
    /// reversing for `!ascending`. Stable, so equal keys keep their order.
    pub(crate) fn sort_rows(&mut self) {
        let at = self.sort;
        let asc = self.ascending;
        self.rows.sort_by(|a, b| {
            let (x, y) = (a.cells.get(at), b.cells.get(at));
            let ord = match (x, y) {
                (Some(x), Some(y)) => match (x.parse::<f64>(), y.parse::<f64>()) {
                    (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                    _ => x.cmp(y),
                },
                _ => std::cmp::Ordering::Equal,
            };
            if asc {
                ord
            } else {
                ord.reverse()
            }
        });
    }

    /// Paints the header row (each title a click that re-sorts) and the striped
    /// body. Returns the key of a clicked row, if any. `search` filters rows by
    /// a case-insensitive match on any cell.
    pub(crate) fn ui(&mut self, ui: &mut Ui, theme: &Theme, search: &str) -> Option<RowKey> {
        let needle = search.trim().to_lowercase();
        self.sort_rows();
        // Header: clickable titles; the sorted one shows an arrow.
        ui.horizontal(|ui| {
            for (i, col) in self.columns.iter().enumerate() {
                let arrow = if i == self.sort {
                    if self.ascending { " ▲" } else { " ▼" }
                } else {
                    ""
                };
                let label = format!("{}{arrow}", tr(col.title));
                let text = small_text(theme, label).color(theme.ink_weak);
                if ui.add_sized([col.width, theme.table_header_height], egui::Button::new(text).frame(false)).clicked() {
                    if self.sort == i {
                        self.ascending = !self.ascending;
                    } else {
                        self.sort = i;
                        self.ascending = true;
                    }
                }
            }
        });
        ui.separator();
        let mut clicked = None;
        for (i, row) in self.rows.iter().enumerate() {
            if !needle.is_empty()
                && !row.cells.iter().any(|c| c.to_lowercase().contains(&needle))
            {
                continue;
            }
            let ink = if row.used { theme.accent_vivid } else { theme.ink_weak };
            let striped = i % 2 == 1;
            let bg = if striped { theme.stripe } else { theme.panel };
            let resp = egui::Frame::new().fill(bg).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (c, col) in self.columns.iter().enumerate() {
                        let cell = row.cells.get(c).cloned().unwrap_or_default();
                        ui.add_sized(
                            [col.width, theme.table_row_height],
                            egui::Label::new(small_text(theme, cell).color(ink))
                                .sense(egui::Sense::click()),
                        );
                    }
                });
            });
            if resp.response.interact(egui::Sense::click()).clicked() {
                clicked = Some(row.key.clone());
            }
        }
        clicked
    }
}

/// The search field and the row count above the table.
pub(crate) fn search_bar(ui: &mut Ui, theme: &Theme, search: &mut String, count: usize) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(search)
                .hint_text(tr("search"))
                .desired_width(theme.text_input_width),
        );
        ui.label(
            RichText::new(trf("{n} rows", &[("n", &count)]))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    });
}

/// The status line under the content: the last note or refusal, else a hint,
/// in the accent token VirtualDJ's green line wears.
pub(crate) fn status_line(ui: &mut Ui, theme: &Theme, note: Option<&str>) {
    let (text, colour) = match note {
        Some(n) => (n.to_string(), theme.ok),
        None => (tr("select a row to act; headers sort").to_string(), theme.ink_faint),
    };
    ui.add(egui::Label::new(small_text(theme, text).color(colour)).wrap());
}

#[cfg(test)]
/// A folder row in the tree: its label (an English literal marked with
/// `tr_key`), a small coloured dot (the family icon), the count when it has
/// one, and whether it is the open folder.
pub(crate) struct FolderRow {
    pub label: &'static str,
    pub count: Option<usize>,
    pub open: bool,
}

#[cfg(test)]
/// Paints the left tree. Returns the label of a folder the operator clicked.
pub(crate) fn tree(ui: &mut Ui, theme: &Theme, root: &str, folders: &[FolderRow]) -> Option<&'static str> {
    let mut clicked = None;
    ui.vertical(|ui| {
        ui.label(
            RichText::new(crate::shell::i18n::tr_dyn(root))
                .font(theme.font_heading.clone())
                .color(theme.ink),
        );
        for f in folders {
            let count = f.count.map(|c| format!(" ({c})")).unwrap_or_default();
            let text = format!("{}{count}", tr(f.label));
            let colour = if f.open { theme.accent_vivid } else { theme.ink_weak };
            if ui
                .selectable_label(f.open, small_text(theme, text).color(colour))
                .clicked()
            {
                clicked = Some(f.label);
            }
        }
    });
    clicked
}

#[cfg(test)]
/// The narrow-width strip of folder chips that replaces the tree (the
/// threshold is a theme token). Returns the label of a clicked chip.
pub(crate) fn chips(ui: &mut Ui, theme: &Theme, folders: &[FolderRow]) -> Option<&'static str> {
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        for f in folders {
            let count = f.count.map(|c| format!(" {c}")).unwrap_or_default();
            let text = format!("{}{count}", tr(f.label));
            let colour = if f.open { theme.accent_vivid } else { theme.ink_weak };
            if ui
                .selectable_label(f.open, small_text(theme, text).color(colour))
                .clicked()
            {
                clicked = Some(f.label);
            }
        }
    });
    clicked
}

/// Formats a live value for a cell, `—` when absent.
pub(crate) fn fmt(v: Option<f64>) -> String {
    match v {
        Some(v) => format!("{v:.3}"),
        None => "—".to_string(),
    }
}

/// The id salt every table folder uses, so two folders never share widget ids.
pub(crate) fn table_id(folder: &str) -> Id {
    Id::new("shell/region/properties/table").with(folder)
}
