//! The control surface: what turns a hardware controller (the FLkey Mini's
//! keys, pads and knobs, any class-compliant MIDI box) and the overlays grid
//! into one instrument.
//!
//! Pure on purpose — no door, no egui — so the call layer can validate the
//! stored shape on every build and the state machine is testable without a
//! window. The grid panel (`shell/panels/lanes.rs`) paints it and runs the
//! [`Out`]s it answers.
//!
//! - [`Layout`] is the stored half (`settings.json` → `grid_surface`): the
//!   grid's fixed columns × rows, the cells' order and pinned pages, the
//!   control cells ([`Role`]), the input bindings ([`Bind`]) and the
//!   recorded loops ([`Loop`]).
//! - **Pages are layers.** A binding names the page it lives on, so the same
//!   pad fires a different cell on every page — a `fn` button or the `fn`
//!   dial switches the whole set at once. A control cell's binding has no
//!   page: `rec`, `fn` and the dials answer on every page.
//! - [`Surface`] is the live half (never saved): what is held, what is
//!   frozen, the loop being recorded, the loops playing.
//! - **Sizes** are whole cells or the fractions `0.5` and `0.3` (a third) of
//!   one; the packer works in [`UNITS`] sub-cells per cell so both fit.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

/// Sub-cells per cell: halves are 3, thirds 2.
pub const UNITS: usize = 6;

/// The sizes a cell can take, `(w, h)` in cells.
pub const SPANS: [(f32, f32); 11] = [
    (1.0, 1.0),
    (2.0, 1.0),
    (1.0, 2.0),
    (2.0, 2.0),
    (1.0, 0.5),
    (1.0, 0.3),
    (0.5, 1.0),
    (0.3, 1.0),
    // Wide strips, for a piano that wants room.
    (4.0, 1.0),
    (4.0, 2.0),
    (8.0, 1.0),
];

/// `cells` in sub-cells (`0.3` is a third: 2 of 6).
pub fn units(cells: f32) -> usize {
    ((cells * UNITS as f32).round() as usize).max(1)
}

/// Whether `(w, h)` is one of [`SPANS`].
pub fn valid_span(w: f64, h: f64) -> bool {
    SPANS
        .iter()
        .any(|&(sw, sh)| (sw as f64 - w).abs() < 1e-3 && (sh as f64 - h).abs() < 1e-3)
}

/// `0.3` → `"0.3"`, `2` → `"2"` — a size's label.
pub fn span_label((w, h): (f32, f32)) -> String {
    let n = |v: f32| if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") };
    format!("{}×{}", n(w), n(h))
}

/// Loop lengths the speed dial steps through, in ticks.
pub const LOOP_TICKS: [usize; 8] = [2, 4, 8, 16, 32, 64, 128, 256];

/// A fresh recording's length when nothing has been dialled.
pub const DEFAULT_LOOP_TICKS: usize = 16;

/// How long a `fn` press must be held to act as a momentary shift (it springs
/// back on release) rather than a latch.
pub const FN_HOLD_SECS: f64 = 0.35;

pub const MIN_DIM: u8 = 1;
pub const MAX_DIM: u8 = 16;

/// A control cell — the hardware's non-content buttons, given a place in the
/// grid so the mouse reaches them too and MIDI can be bound to them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Role {
    /// Turning it lengthens or shortens the focused loop (or the next
    /// recording) one [`LOOP_TICKS`] notch per step.
    SpeedDial,
    /// Held while another button is held: freezes that button where it is.
    /// Tapped alone: starts recording a loop, tapped again closes it.
    Rec,
    /// Tapped: releases every frozen button. Held while a frozen button is
    /// pressed: releases that one.
    DropRec,
    /// Moves the library's selection, one entry per step.
    SearchDial,
    /// Loads the library's selection into the grid (in folder mode: walks
    /// into the selected folder).
    Load,
    /// Switches the search dial between folders and files.
    Folder,
    /// Goes to `page` (the next page when unset): held, it springs back on
    /// release; tapped, it stays.
    Fn {
        #[serde(default)]
        page: Option<usize>,
    },
    /// Steps the page, one per step.
    FnDial,
    /// 25 keys from `base` (MIDI note number) — each key is the same input a
    /// hardware key of that note is, so it fires what that note is bound to.
    Piano {
        #[serde(default = "default_piano_base")]
        base: u8,
    },
}

fn default_piano_base() -> u8 {
    48
}

/// How many keys a [`Role::Piano`] has.
pub const PIANO_KEYS: u8 = 25;

impl Role {
    /// Every kind the `+` menu offers, in its order.
    pub fn catalogue() -> Vec<Role> {
        vec![
            Role::Piano { base: default_piano_base() },
            Role::Rec,
            Role::DropRec,
            Role::SpeedDial,
            Role::Fn { page: None },
            Role::FnDial,
            Role::SearchDial,
            Role::Load,
            Role::Folder,
        ]
    }

    /// The wire tag (`speed-dial`, `piano`, …), also the stem of a new key.
    pub fn tag(&self) -> &'static str {
        match self {
            Role::SpeedDial => "speed-dial",
            Role::Rec => "rec",
            Role::DropRec => "drop-rec",
            Role::SearchDial => "search-dial",
            Role::Load => "load",
            Role::Folder => "folder",
            Role::Fn { .. } => "fn",
            Role::FnDial => "fn-dial",
            Role::Piano { .. } => "piano",
        }
    }

    /// The label a cell paints (English; the panel translates).
    pub fn label(&self) -> String {
        match self {
            Role::SpeedDial => "speed".into(),
            Role::Rec => "rec".into(),
            Role::DropRec => "drop rec".into(),
            Role::SearchDial => "search".into(),
            Role::Load => "load".into(),
            Role::Folder => "folders".into(),
            Role::Fn { page: Some(p) } => format!("fn {}", p + 1),
            Role::Fn { page: None } => "fn".into(),
            Role::FnDial => "fn dial".into(),
            Role::Piano { .. } => "piano".into(),
        }
    }

    /// Knobs: a value turns into steps rather than a press.
    pub fn is_dial(&self) -> bool {
        matches!(self, Role::SpeedDial | Role::SearchDial | Role::FnDial)
    }

    /// The size a fresh one takes.
    pub fn default_span(&self) -> (f32, f32) {
        match self {
            Role::Piano { .. } => (2.0, 1.0),
            _ => (1.0, 1.0),
        }
    }
}

/// One input bound to one cell. `page: None` answers on every page (control
/// cells); `Some(p)` only while page `p` is shown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bind {
    /// `note:<n>`, `cc:<n>`, `bend` ([`metal_midi::Msg::input`]).
    pub input: String,
    #[serde(default)]
    pub page: Option<usize>,
    pub cell: String,
    /// How a knob on this input reports itself.
    #[serde(default, skip_serializing_if = "is_absolute")]
    pub dial: DialMode,
}

fn is_absolute(m: &DialMode) -> bool {
    *m == DialMode::Absolute
}

/// [`metal_midi::DialMode`], with the stored spelling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DialMode {
    #[default]
    Absolute,
    Relative,
    Offset,
}

impl DialMode {
    pub const ALL: [DialMode; 3] = [DialMode::Absolute, DialMode::Relative, DialMode::Offset];

    pub fn midi(self) -> metal_midi::DialMode {
        match self {
            DialMode::Absolute => metal_midi::DialMode::Absolute,
            DialMode::Relative => metal_midi::DialMode::Relative,
            DialMode::Offset => metal_midi::DialMode::Offset,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DialMode::Absolute => "knob",
            DialMode::Relative => "encoder",
            DialMode::Offset => "encoder (offset)",
        }
    }
}

/// What a press did, as a loop stores it and the grid fires it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Gesture {
    /// Pressed (a hold button goes on).
    Down,
    /// Released (a hold button goes off).
    Up,
    /// A whole press of a toggle button.
    Click,
    /// A knob turned this many steps.
    Steps(i32),
    /// A fader or knob moved to this position, 0..=1.
    Value(f32),
}

/// One event of a recorded loop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoopEvent {
    /// Ticks from the loop's start.
    pub at: f64,
    pub cell: String,
    pub gesture: Gesture,
}

/// A recorded loop: the grid presses of one take, replayed every `ticks`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Loop {
    pub name: String,
    pub ticks: usize,
    #[serde(default)]
    pub events: Vec<LoopEvent>,
    #[serde(default)]
    pub playing: bool,
}

/// The stored half of the surface (`grid_surface` in settings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default = "default_cols")]
    pub cols: u8,
    #[serde(default = "default_rows")]
    pub rows: u8,
    /// Pages kept even when empty (a `fn` page waiting for its buttons).
    #[serde(default)]
    pub page_count: usize,
    /// Cell keys in the order the operator arranged them; a cell not named
    /// here follows, in the order the grid builds it.
    #[serde(default)]
    pub order: Vec<String>,
    /// Cells pinned to a page.
    #[serde(default)]
    pub pages: BTreeMap<String, usize>,
    #[serde(default)]
    pub roles: BTreeMap<String, Role>,
    #[serde(default)]
    pub binds: Vec<Bind>,
    #[serde(default)]
    pub loops: BTreeMap<String, Loop>,
    /// The next recording's length, in ticks.
    #[serde(default = "default_loop_ticks")]
    pub loop_ticks: usize,
}

fn default_cols() -> u8 {
    8
}
fn default_rows() -> u8 {
    4
}
fn default_loop_ticks() -> usize {
    DEFAULT_LOOP_TICKS
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            cols: default_cols(),
            rows: default_rows(),
            page_count: 0,
            order: Vec::new(),
            pages: BTreeMap::new(),
            roles: BTreeMap::new(),
            binds: Vec::new(),
            loops: BTreeMap::new(),
            loop_ticks: DEFAULT_LOOP_TICKS,
        }
    }
}

impl Layout {
    /// The stored value, read leniently: anything unreadable is the default.
    pub fn from_value(v: &serde_json::Value) -> Layout {
        serde_json::from_value::<Layout>(v.clone())
            .map(Layout::clamped)
            .unwrap_or_default()
    }

    /// Why a posted value cannot be stored, if it cannot.
    pub fn check(v: &serde_json::Value) -> Result<(), String> {
        let l = serde_json::from_value::<Layout>(v.clone()).map_err(|e| e.to_string())?;
        for (name, n) in [("cols", l.cols), ("rows", l.rows)] {
            if !(MIN_DIM..=MAX_DIM).contains(&n) {
                return Err(format!("{name} must be {MIN_DIM}..={MAX_DIM}, got {n}"));
            }
        }
        if let Some(lp) = l.loops.values().find(|lp| lp.ticks == 0) {
            return Err(format!("loop {:?} has no length", lp.name));
        }
        Ok(())
    }

    fn clamped(mut self) -> Layout {
        self.cols = self.cols.clamp(MIN_DIM, MAX_DIM);
        self.rows = self.rows.clamp(MIN_DIM, MAX_DIM);
        self
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// A fresh key for a new `stem` cell (`rec-1`, `rec-2`, …).
    pub fn fresh_key(&self, stem: &str) -> String {
        (1..)
            .map(|n| format!("{stem}-{n}"))
            .find(|k| !self.roles.contains_key(k) && !self.loops.contains_key(k))
            .expect("an unbounded range has a free key")
    }

    /// The cell `input` fires on `page`: a binding on that page wins over one
    /// on every page.
    pub fn bound(&self, input: &str, page: usize) -> Option<&Bind> {
        self.binds
            .iter()
            .find(|b| b.input == input && b.page == Some(page))
            .or_else(|| self.binds.iter().find(|b| b.input == input && b.page.is_none()))
    }

    /// Binds `input` to `cell`, on `page` (or every page), replacing whatever
    /// that input fired there before.
    pub fn bind(&mut self, input: &str, page: Option<usize>, cell: &str) {
        self.binds.retain(|b| !(b.input == input && b.page == page));
        self.binds.push(Bind {
            input: input.to_string(),
            page,
            cell: cell.to_string(),
            dial: DialMode::Absolute,
        });
    }

    /// The bindings that fire `cell`.
    pub fn binds_of<'a>(&'a self, cell: &'a str) -> impl Iterator<Item = &'a Bind> + 'a {
        self.binds.iter().filter(move |b| b.cell == cell)
    }

    /// Forgets a cell everywhere: its role or loop, its bindings, its place.
    pub fn forget(&mut self, cell: &str) {
        self.roles.remove(cell);
        self.loops.remove(cell);
        self.binds.retain(|b| b.cell != cell);
        self.order.retain(|k| k != cell);
        self.pages.remove(cell);
    }

    /// `keys` in the stored order: the named ones first as the operator
    /// arranged them, then the rest as built.
    pub fn ordered(&self, keys: &[String]) -> Vec<usize> {
        let mut idx: Vec<usize> = Vec::with_capacity(keys.len());
        for k in &self.order {
            if let Some(i) = keys.iter().position(|x| x == k) {
                if !idx.contains(&i) {
                    idx.push(i);
                }
            }
        }
        let rest: Vec<usize> = (0..keys.len()).filter(|i| !idx.contains(i)).collect();
        idx.extend(rest);
        idx
    }

    /// Moves `cell` `delta` places in the order (the whole current order is
    /// written, so the move sticks).
    pub fn shift(&mut self, keys: &[String], cell: &str, delta: i32) {
        let mut order: Vec<String> = self.ordered(keys).into_iter().map(|i| keys[i].clone()).collect();
        let Some(at) = order.iter().position(|k| k == cell) else { return };
        let to = (at as i64 + delta as i64).clamp(0, order.len() as i64 - 1) as usize;
        let k = order.remove(at);
        order.insert(to, k);
        self.order = order;
    }
}

/// Where one cell landed: its page and its corner, in sub-cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub page: usize,
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// Where a cell may land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pin {
    /// Flows in order onto the first page with room.
    Flow,
    /// Lands on this page.
    Page(usize),
    /// Holds the same place on every page — the control cells, like the
    /// hardware buttons they stand for. Reported on page 0.
    Every,
}

/// Packs `(w, h)` spans (sub-cells) on `cols × rows` pages (sub-cells too).
/// Cells on every page are placed first and their places kept free on every
/// page; pinned cells land on their page next; the rest flow, first-fit in
/// reading order, from page 0 — a cell that fits nowhere opens the next page.
/// A pin that does not fit flows instead.
pub fn pack(spans: &[(usize, usize)], pins: &[Pin], cols: usize, rows: usize) -> Vec<Placed> {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let mut placed: Vec<Option<Placed>> = vec![None; spans.len()];
    let fit = |(w, h): (usize, usize)| (w.clamp(1, cols), h.clamp(1, rows));
    let first_fit = |occ: &[bool], w: usize, h: usize| -> Option<(usize, usize)> {
        (0..=rows - h).find_map(|y| {
            (0..=cols - w)
                .find(|&x| (y..y + h).all(|r| (x..x + w).all(|c| !occ[r * cols + c])))
                .map(|x| (x, y))
        })
    };
    // Every page starts from the template the cells on every page leave.
    let mut template = vec![false; cols * rows];
    for (i, (&span, pin)) in spans.iter().zip(pins).enumerate() {
        if *pin != Pin::Every {
            continue;
        }
        let (w, h) = fit(span);
        if let Some((x, y)) = first_fit(&template, w, h) {
            for r in y..y + h {
                for c in x..x + w {
                    template[r * cols + c] = true;
                }
            }
            placed[i] = Some(Placed { page: 0, x, y, w, h });
        }
    }
    let mut pages: Vec<Vec<bool>> = Vec::new();
    let take = |pages: &mut Vec<Vec<bool>>, page: usize, x: usize, y: usize, w: usize, h: usize| {
        while pages.len() <= page {
            pages.push(template.clone());
        }
        for r in y..y + h {
            for c in x..x + w {
                pages[page][r * cols + c] = true;
            }
        }
    };
    for (i, (&span, pin)) in spans.iter().zip(pins).enumerate() {
        let Pin::Page(page) = *pin else { continue };
        let (w, h) = fit(span);
        while pages.len() <= page {
            pages.push(template.clone());
        }
        if let Some((x, y)) = first_fit(&pages[page], w, h) {
            take(&mut pages, page, x, y, w, h);
            placed[i] = Some(Placed { page, x, y, w, h });
        }
    }
    let mut page = 0;
    for (i, &span) in spans.iter().enumerate() {
        if placed[i].is_some() {
            continue;
        }
        let (w, h) = fit(span);
        loop {
            if pages.len() <= page {
                pages.push(template.clone());
            }
            if let Some((x, y)) = first_fit(&pages[page], w, h) {
                take(&mut pages, page, x, y, w, h);
                placed[i] = Some(Placed { page, x, y, w, h });
                break;
            }
            // A page the cells on every page fill leaves no room anywhere:
            // the cell overlaps rather than paging forever.
            if !template.contains(&false) || first_fit(&template, w, h).is_none() {
                placed[i] = Some(Placed { page, x: 0, y: 0, w, h });
                break;
            }
            page += 1;
        }
    }
    placed.into_iter().map(|p| p.expect("every span is placed")).collect()
}

/// What the library should do, raised by a control cell and drained by the
/// library panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryIntent {
    Step(i32),
    ToggleFolders,
    Load,
}

/// What the grid must do after a gesture.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Fire this content cell.
    Fire { cell: String, gesture: Gesture },
    /// Show this page.
    Page(usize),
    /// Step the page.
    PageStep(i32),
    Library(LibraryIntent),
    /// Set a loop's length (`None`: the next recording's).
    LoopTicks { key: Option<String>, ticks: usize },
    /// A recording started.
    RecStart,
    /// A recording closed: store it as a loop cell.
    RecStop(Loop),
    /// Flip whether a loop plays.
    LoopToggle(String),
}

#[derive(Clone, Debug, PartialEq)]
struct Recording {
    started: f64,
    ticks: usize,
    events: Vec<LoopEvent>,
}

/// The live half of the surface: what is held, frozen, recording, playing.
#[derive(Debug, Default)]
pub struct Surface {
    held: BTreeSet<String>,
    frozen: BTreeSet<String>,
    /// `rec` is down; `true` once it froze something (so its release does
    /// not also start a recording).
    rec_down: Option<bool>,
    drop_down: Option<bool>,
    recording: Option<Recording>,
    /// `(fn cell, pressed at, page to return to)`.
    fn_hold: Option<(String, f64, usize)>,
    /// The loop the speed dial lengthens: the last one touched.
    pub focus_loop: Option<String>,
    /// Each playing loop's playhead, in ticks.
    playheads: HashMap<String, f64>,
    last_tick: Option<f64>,
    dials: HashMap<String, metal_midi::Dial>,
    /// Notes held on any input, for the piano to light.
    pub notes: BTreeSet<u8>,
}

impl Surface {
    pub fn is_frozen(&self, cell: &str) -> bool {
        self.frozen.contains(cell)
    }

    pub fn frozen_count(&self) -> usize {
        self.frozen.len()
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// How far into the recording, 0..=1.
    pub fn recording_progress(&self, now: f64, spt: f64) -> Option<f32> {
        let r = self.recording.as_ref()?;
        Some((((now - r.started) / spt) / r.ticks as f64).clamp(0.0, 1.0) as f32)
    }

    /// Whether a control cell is lit: `rec` while recording or held, `drop
    /// rec` while held, `fn` while it holds the page.
    pub fn role_lit(&self, key: &str, role: &Role) -> bool {
        match role {
            Role::Rec => self.recording.is_some() || self.rec_down.is_some(),
            Role::DropRec => self.drop_down.is_some(),
            Role::Fn { .. } => self.fn_hold.as_ref().is_some_and(|(k, _, _)| k == key),
            _ => false,
        }
    }

    /// How far a playing loop's head is through it, 0..=1.
    pub fn loop_phase(&self, key: &str, lp: &Loop) -> Option<f32> {
        let head = self.playheads.get(key)?;
        Some((head / lp.ticks.max(1) as f64).clamp(0.0, 1.0) as f32)
    }

    /// A knob's raw value on `input` → steps, remembering each knob's last
    /// position.
    pub fn dial_steps(&mut self, input: &str, mode: DialMode, raw: u8) -> i32 {
        let dial = self.dials.entry(input.to_string()).or_insert_with(|| metal_midi::Dial::new(mode.midi()));
        dial.mode = mode.midi();
        dial.step(raw)
    }

    /// One gesture on a control cell.
    pub fn role(&mut self, layout: &Layout, key: &str, role: &Role, g: Gesture, now: f64, page: usize) -> Vec<Out> {
        let mut out = Vec::new();
        match (role, g) {
            (Role::Rec, Gesture::Down) => {
                // Hold another button, then press rec: that button freezes.
                let held: Vec<String> = self.held.iter().cloned().collect();
                let froze = !held.is_empty();
                self.frozen.extend(held);
                self.rec_down = Some(froze);
            }
            (Role::Rec, Gesture::Up) => {
                if self.rec_down.take() == Some(false) {
                    out.extend(self.toggle_recording(layout, now));
                }
            }
            (Role::Rec, Gesture::Click) => {
                out.extend(self.role(layout, key, role, Gesture::Down, now, page));
                out.extend(self.role(layout, key, role, Gesture::Up, now, page));
            }
            (Role::DropRec, Gesture::Down) => self.drop_down = Some(false),
            (Role::DropRec, Gesture::Up) => {
                if self.drop_down.take() == Some(false) {
                    out.extend(self.release_all());
                }
            }
            (Role::DropRec, Gesture::Click) => out.extend(self.release_all()),
            (Role::SpeedDial, Gesture::Steps(n)) => {
                let target = self
                    .focus_loop
                    .clone()
                    .filter(|k| layout.loops.contains_key(k) && self.recording.is_none());
                let current = match &target {
                    Some(k) => layout.loops[k].ticks,
                    None => self.recording.as_ref().map_or(layout.loop_ticks, |r| r.ticks),
                };
                let ticks = step_ticks(current, n);
                if let Some(r) = self.recording.as_mut() {
                    r.ticks = ticks;
                }
                out.push(Out::LoopTicks { key: target, ticks });
            }
            (Role::SearchDial, Gesture::Steps(n)) => out.push(Out::Library(LibraryIntent::Step(n))),
            (Role::Load, Gesture::Down | Gesture::Click) => out.push(Out::Library(LibraryIntent::Load)),
            (Role::Folder, Gesture::Down | Gesture::Click) => out.push(Out::Library(LibraryIntent::ToggleFolders)),
            (Role::Fn { page: to }, Gesture::Down) => {
                self.fn_hold = Some((key.to_string(), now, page));
                out.push(Out::Page(to.unwrap_or(page + 1)));
            }
            (Role::Fn { .. }, Gesture::Up) => {
                if let Some((k, at, back)) = self.fn_hold.take() {
                    if k == key && now - at >= FN_HOLD_SECS {
                        out.push(Out::Page(back));
                    }
                }
            }
            (Role::Fn { page: to }, Gesture::Click) => out.push(Out::Page(to.unwrap_or(page + 1))),
            (Role::FnDial, Gesture::Steps(n)) => out.push(Out::PageStep(n)),
            _ => {}
        }
        out
    }

    /// One gesture on a content cell, through the freeze/record gate.
    pub fn content(&mut self, cell: &str, g: Gesture, now: f64, spt: f64) -> Vec<Out> {
        // `drop rec` held + a frozen button: release that one.
        if let Some(used) = self.drop_down.as_mut() {
            if matches!(g, Gesture::Down | Gesture::Click) && self.frozen.contains(cell) {
                *used = true;
                self.frozen.remove(cell);
                return vec![Out::Fire { cell: cell.to_string(), gesture: Gesture::Up }];
            }
        }
        if let Some(froze) = self.rec_down.as_mut() {
            match g {
                // A held button freezes after its press lands: it stays on.
                Gesture::Down => {
                    *froze = true;
                    self.frozen.insert(cell.to_string());
                    self.held.insert(cell.to_string());
                    return self.fire(cell, g, now, spt);
                }
                // A toggle freezes where it is, without flipping.
                Gesture::Click => {
                    *froze = true;
                    self.frozen.insert(cell.to_string());
                    return Vec::new();
                }
                _ => {}
            }
        }
        match g {
            Gesture::Down => {
                self.held.insert(cell.to_string());
            }
            Gesture::Up => {
                self.held.remove(cell);
            }
            _ => {}
        }
        if self.frozen.contains(cell) {
            return Vec::new();
        }
        self.fire(cell, g, now, spt)
    }

    /// A loop cell was pressed: it plays or stops, and the speed dial now
    /// lengthens it.
    pub fn loop_pressed(&mut self, key: &str) -> Vec<Out> {
        self.focus_loop = Some(key.to_string());
        self.playheads.remove(key);
        vec![Out::LoopToggle(key.to_string())]
    }

    fn fire(&mut self, cell: &str, gesture: Gesture, now: f64, spt: f64) -> Vec<Out> {
        if let Some(r) = self.recording.as_mut() {
            r.events.push(LoopEvent { at: (now - r.started) / spt, cell: cell.to_string(), gesture });
        }
        vec![Out::Fire { cell: cell.to_string(), gesture }]
    }

    fn release_all(&mut self) -> Vec<Out> {
        let frozen = std::mem::take(&mut self.frozen);
        frozen
            .into_iter()
            .filter(|c| !self.held.contains(c))
            .map(|cell| Out::Fire { cell, gesture: Gesture::Up })
            .collect()
    }

    fn toggle_recording(&mut self, layout: &Layout, now: f64) -> Vec<Out> {
        match self.recording.take() {
            Some(r) => vec![Out::RecStop(close(layout, r))],
            None => {
                self.recording = Some(Recording { started: now, ticks: layout.loop_ticks.max(1), events: Vec::new() });
                vec![Out::RecStart]
            }
        }
    }

    /// Once a frame: closes a recording that has run its length, and plays
    /// every playing loop's events that fall since the last frame.
    pub fn tick(&mut self, layout: &Layout, now: f64, spt: f64) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some(r) = &self.recording {
            if (now - r.started) / spt >= r.ticks as f64 {
                let r = self.recording.take().expect("checked");
                out.push(Out::RecStop(close(layout, r)));
            }
        }
        let dt = self.last_tick.replace(now).map_or(0.0, |t| ((now - t) / spt).max(0.0));
        self.playheads.retain(|k, _| layout.loops.get(k).is_some_and(|l| l.playing));
        for (key, lp) in layout.loops.iter().filter(|(_, l)| l.playing && l.ticks > 0) {
            let len = lp.ticks as f64;
            let Some(from) = self.playheads.get(key).copied() else {
                // A loop that just started fires what sits on its downbeat.
                self.playheads.insert(key.clone(), 0.0);
                out.extend(window(lp, -f64::EPSILON, 0.0).map(fire_of));
                continue;
            };
            let to = from + dt;
            out.extend(window(lp, from, to.min(len)).map(fire_of));
            if to >= len {
                let wrapped = (to - len) % len;
                out.extend(window(lp, -f64::EPSILON, wrapped).map(fire_of));
                self.playheads.insert(key.clone(), wrapped);
            } else {
                self.playheads.insert(key.clone(), to);
            }
        }
        out
    }
}

fn window(lp: &Loop, from: f64, to: f64) -> impl Iterator<Item = &LoopEvent> {
    lp.events.iter().filter(move |e| e.at > from && e.at <= to)
}

fn fire_of(e: &LoopEvent) -> Out {
    Out::Fire { cell: e.cell.clone(), gesture: e.gesture }
}

/// A closed recording as a loop: named in order, playing at once.
fn close(layout: &Layout, r: Recording) -> Loop {
    let n = layout.loops.len() + 1;
    let events = r.events.into_iter().filter(|e| e.at < r.ticks as f64).collect();
    Loop { name: format!("loop {n}"), ticks: r.ticks, events, playing: true }
}

/// `current` moved `n` notches along [`LOOP_TICKS`].
pub fn step_ticks(current: usize, n: i32) -> usize {
    let at = LOOP_TICKS
        .iter()
        .position(|&t| t >= current)
        .unwrap_or(LOOP_TICKS.len() - 1) as i64;
    LOOP_TICKS[(at + n as i64).clamp(0, LOOP_TICKS.len() as i64 - 1) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPT: f64 = 0.125;

    #[test]
    fn spans_are_the_eight_sizes_in_sixths() {
        assert_eq!(units(1.0), 6);
        assert_eq!(units(0.5), 3);
        assert_eq!(units(0.3), 2);
        assert_eq!(units(2.0), 12);
        assert!(valid_span(1.0, 0.3) && valid_span(0.5, 1.0) && valid_span(2.0, 2.0));
        assert!(!valid_span(0.3, 0.3) && !valid_span(3.0, 1.0));
        assert_eq!(span_label((1.0, 0.3)), "1×0.3");
        assert_eq!(span_label((2.0, 1.0)), "2×1");
    }

    #[test]
    fn halves_and_thirds_share_a_cell() {
        // Two half-width cells sit side by side in one cell; three third-height
        // strips stack in one.
        let spans = [(3, 6), (3, 6), (6, 2), (6, 2), (6, 2)];
        let placed = pack(&spans, &[Pin::Flow; 5], 12, 6);
        assert_eq!((placed[0].x, placed[1].x), (0, 3));
        assert_eq!((placed[2].x, placed[2].y), (6, 0));
        assert_eq!((placed[3].y, placed[4].y), (2, 4));
        assert!(placed.iter().all(|p| p.page == 0));
    }

    #[test]
    fn pins_land_first_and_the_rest_flows_with_new_pages() {
        let spans = [(6, 6), (6, 6), (6, 6)];
        // One-cell pages: the pinned one owns page 1, the others fill 0 then 2.
        let placed = pack(&spans, &[Pin::Flow, Pin::Page(1), Pin::Flow], 6, 6);
        assert_eq!(placed.iter().map(|p| p.page).collect::<Vec<_>>(), vec![0, 1, 2]);
        // A span wider than the page is clamped to it.
        let placed = pack(&[(12, 6)], &[Pin::Flow], 6, 6);
        assert_eq!(placed[0].w, 6);
    }

    #[test]
    fn cells_on_every_page_keep_their_place_free_on_every_page() {
        // Two cells per page; the control cell takes the first slot of each.
        let spans = [(6, 6), (6, 6), (6, 6), (6, 6)];
        let pins = [Pin::Flow, Pin::Every, Pin::Flow, Pin::Flow];
        let placed = pack(&spans, &pins, 12, 6);
        assert_eq!((placed[1].page, placed[1].x), (0, 0));
        assert_eq!((placed[0].page, placed[0].x), (0, 6));
        assert_eq!((placed[2].page, placed[2].x), (1, 6));
        assert_eq!((placed[3].page, placed[3].x), (2, 6));
        // A page the control cells fill leaves nothing to page into.
        let placed = pack(&[(6, 6), (6, 6)], &[Pin::Every, Pin::Flow], 6, 6);
        assert_eq!(placed[1].page, 0);
    }

    #[test]
    fn a_binding_on_the_page_wins_over_one_on_every_page() {
        let mut l = Layout::default();
        l.bind("note:60", None, "rec-1");
        l.bind("note:60", Some(2), "sw:a");
        assert_eq!(l.bound("note:60", 2).unwrap().cell, "sw:a");
        assert_eq!(l.bound("note:60", 0).unwrap().cell, "rec-1");
        l.bind("note:60", Some(2), "sw:b");
        assert_eq!(l.binds.len(), 2, "rebinding replaces");
        l.forget("sw:b");
        assert_eq!(l.bound("note:60", 2).unwrap().cell, "rec-1");
    }

    #[test]
    fn order_shifts_and_unnamed_cells_follow() {
        let keys: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let mut l = Layout::default();
        assert_eq!(l.ordered(&keys), vec![0, 1, 2]);
        l.shift(&keys, "c", -2);
        assert_eq!(l.ordered(&keys), vec![2, 0, 1]);
        l.order = vec!["b".into(), "gone".into()];
        assert_eq!(l.ordered(&keys), vec![1, 0, 2]);
    }

    #[test]
    fn the_layout_round_trips_and_bad_dims_are_refused() {
        let mut l = Layout::default();
        l.roles.insert("fn-1".into(), Role::Fn { page: Some(1) });
        l.roles.insert("piano-1".into(), Role::Piano { base: 48 });
        let v = l.to_value();
        assert_eq!(v["roles"]["fn-1"], serde_json::json!({ "kind": "fn", "page": 1 }));
        assert_eq!(Layout::from_value(&v), l);
        assert!(Layout::check(&v).is_ok());
        assert!(Layout::check(&serde_json::json!({ "cols": 0 })).is_err());
        assert!(Layout::check(&serde_json::json!({ "rows": 40 })).is_err());
        assert_eq!(Layout::from_value(&serde_json::json!("junk")), Layout::default());
    }

    #[test]
    fn rec_held_with_a_held_button_freezes_it_and_its_release_is_swallowed() {
        let l = Layout::default();
        let mut s = Surface::default();
        assert_eq!(s.content("sw:strobe", Gesture::Down, 0.0, SPT).len(), 1);
        s.role(&l, "rec-1", &Role::Rec, Gesture::Down, 0.1, 0);
        assert!(s.is_frozen("sw:strobe"));
        assert!(s.content("sw:strobe", Gesture::Up, 0.2, SPT).is_empty(), "frozen: stays on");
        let out = s.role(&l, "rec-1", &Role::Rec, Gesture::Up, 0.3, 0);
        assert!(out.is_empty(), "a rec that froze something does not also record");
        assert!(!s.is_recording());
    }

    #[test]
    fn rec_held_then_a_press_freezes_too_and_a_toggle_freezes_without_flipping() {
        let l = Layout::default();
        let mut s = Surface::default();
        s.role(&l, "rec-1", &Role::Rec, Gesture::Down, 0.0, 0);
        let down = s.content("sw:hold", Gesture::Down, 0.1, SPT);
        assert_eq!(down, vec![Out::Fire { cell: "sw:hold".into(), gesture: Gesture::Down }]);
        assert!(s.content("sw:toggle", Gesture::Click, 0.1, SPT).is_empty());
        s.role(&l, "rec-1", &Role::Rec, Gesture::Up, 0.2, 0);
        assert!(s.content("sw:hold", Gesture::Up, 0.3, SPT).is_empty());
        assert!(s.content("sw:toggle", Gesture::Click, 0.4, SPT).is_empty(), "a frozen toggle is locked");
        assert_eq!(s.frozen_count(), 2);
    }

    #[test]
    fn drop_rec_releases_every_freeze_or_just_the_pressed_one() {
        let l = Layout::default();
        let mut s = Surface::default();
        s.role(&l, "rec-1", &Role::Rec, Gesture::Down, 0.0, 0);
        s.content("a", Gesture::Down, 0.0, SPT);
        s.content("b", Gesture::Down, 0.0, SPT);
        s.role(&l, "rec-1", &Role::Rec, Gesture::Up, 0.0, 0);
        s.content("a", Gesture::Up, 0.1, SPT);
        s.content("b", Gesture::Up, 0.1, SPT);
        // Held drop rec + a: only a lets go.
        s.role(&l, "drop-1", &Role::DropRec, Gesture::Down, 0.2, 0);
        assert_eq!(s.content("a", Gesture::Down, 0.2, SPT), vec![Out::Fire { cell: "a".into(), gesture: Gesture::Up }]);
        assert!(s.role(&l, "drop-1", &Role::DropRec, Gesture::Up, 0.3, 0).is_empty());
        assert!(s.is_frozen("b") && !s.is_frozen("a"));
        // A tap lets go of the rest.
        let out = s.role(&l, "drop-1", &Role::DropRec, Gesture::Click, 0.4, 0);
        assert_eq!(out, vec![Out::Fire { cell: "b".into(), gesture: Gesture::Up }]);
        assert_eq!(s.frozen_count(), 0);
    }

    #[test]
    fn a_rec_tap_records_a_loop_that_closes_at_its_length_and_replays() {
        let mut l = Layout { loop_ticks: 8, ..Layout::default() };
        let mut s = Surface::default();
        assert_eq!(s.role(&l, "rec-1", &Role::Rec, Gesture::Click, 0.0, 0), vec![Out::RecStart]);
        s.content("sw:a", Gesture::Click, 0.25, SPT); // tick 2
        s.content("sw:b", Gesture::Click, 0.5, SPT); // tick 4
        assert!(s.tick(&l, 0.9, SPT).is_empty());
        let out = s.tick(&l, 1.0, SPT); // tick 8: closed
        let Some(Out::RecStop(lp)) = out.first() else { panic!("closed: {out:?}") };
        assert_eq!((lp.ticks, lp.events.len(), lp.playing), (8, 2, true));
        l.loops.insert("loop-1".into(), lp.clone());
        // Playback: the first frame positions the head, then ticks 2 and 4
        // fire as the head passes them, and again after the wrap.
        assert!(s.tick(&l, 1.0, SPT).is_empty());
        assert_eq!(s.tick(&l, 1.3, SPT).len(), 1); // past tick 2
        assert_eq!(s.tick(&l, 1.6, SPT).len(), 1); // past tick 4
        assert!(s.tick(&l, 1.9, SPT).is_empty());
        assert_eq!(s.tick(&l, 2.3, SPT).len(), 1, "wraps to tick 2.4: a fires again");
    }

    #[test]
    fn the_speed_dial_steps_the_focused_loop_or_the_next_recording() {
        let mut l = Layout::default();
        let mut s = Surface::default();
        assert_eq!(
            s.role(&l, "sd", &Role::SpeedDial, Gesture::Steps(1), 0.0, 0),
            vec![Out::LoopTicks { key: None, ticks: 32 }]
        );
        l.loops.insert("loop-1".into(), Loop { name: "loop 1".into(), ticks: 8, events: vec![], playing: true });
        s.loop_pressed("loop-1");
        assert_eq!(
            s.role(&l, "sd", &Role::SpeedDial, Gesture::Steps(-2), 0.0, 0),
            vec![Out::LoopTicks { key: Some("loop-1".into()), ticks: 2 }]
        );
        assert_eq!(step_ticks(256, 3), 256);
        assert_eq!(step_ticks(5, 0), 8, "an off-ladder length snaps up");
    }

    #[test]
    fn fn_held_springs_back_and_fn_tapped_latches() {
        let l = Layout::default();
        let mut s = Surface::default();
        let role = Role::Fn { page: Some(3) };
        assert_eq!(s.role(&l, "fn-1", &role, Gesture::Down, 0.0, 1), vec![Out::Page(3)]);
        assert_eq!(s.role(&l, "fn-1", &role, Gesture::Up, 1.0, 3), vec![Out::Page(1)]);
        s.role(&l, "fn-1", &role, Gesture::Down, 2.0, 1);
        assert!(s.role(&l, "fn-1", &role, Gesture::Up, 2.1, 3).is_empty(), "a tap stays");
        assert_eq!(s.role(&l, "fd", &Role::FnDial, Gesture::Steps(-1), 0.0, 0), vec![Out::PageStep(-1)]);
        assert_eq!(
            s.role(&l, "f2", &Role::Fn { page: None }, Gesture::Click, 0.0, 4),
            vec![Out::Page(5)]
        );
    }

    #[test]
    fn library_controls_raise_their_intents() {
        let l = Layout::default();
        let mut s = Surface::default();
        assert_eq!(
            s.role(&l, "s", &Role::SearchDial, Gesture::Steps(2), 0.0, 0),
            vec![Out::Library(LibraryIntent::Step(2))]
        );
        assert_eq!(s.role(&l, "l", &Role::Load, Gesture::Down, 0.0, 0), vec![Out::Library(LibraryIntent::Load)]);
        assert_eq!(
            s.role(&l, "f", &Role::Folder, Gesture::Click, 0.0, 0),
            vec![Out::Library(LibraryIntent::ToggleFolders)]
        );
    }
}
