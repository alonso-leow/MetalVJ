//! Which venue document a node edits: the room is **per node**, not a global
//! pick.
//!
//! The old venue canvas offered a dropdown over every venue on disk. That is
//! the stale list this module replaces: a `venue.fixtures` node owns *its*
//! room in `config.venue`, and every node that drives a room — the fixtures
//! node, each per-fixture `venue.fixture`, the `output.device` the picture
//! lands on — reads its venue from there or from the one its cables reach
//! (`venue_for`). Nothing is chosen twice.
//!
//! Two things make that survivable while the feature is young:
//!
//! - **A clean default.** A node with no venue gets one named after the graph
//!   (`col-show` → `col-show`), so a fresh collection never edits whichever
//!   venue happened to be projected last.
//! - **Adoption, not orphaning.** A graph whose cables already reach a
//!   `venue.surface` naming a venue the node's own config does not name is
//!   *adopted*: the node's config is pointed at the room it was already
//!   drawing, through `POST /graph/node {op:"config"}` — the one write every
//!   other node setting uses. A room the operator built in the editor last
//!   week stays reachable from the graph that used it.
//!
//! The panel still never reads a venue file: it asks `GET /venue` for the id
//! and paints that reply (`super::props_venue`), as it always did.

use serde_json::{json, Value};

use crate::call::Call;

use crate::shell::classes::Theme;
use crate::shell::components::{dropdown, label_row, note, section_heading};
use crate::shell::i18n::{tr, trf};
use super::props_venue::post_config;
use super::properties::surface;

/// The node kinds a venue is read off, in the order [`venue_for`] tries them:
/// the group node that drives the rig, one fixture, then the screen a picture
/// lands on.
const VENUE_KINDS: [&str; 3] = [
    metal_graph::VENUE_FIXTURES_KIND,
    metal_graph::VENUE_FIXTURE_KIND,
    "output.device",
];

/// [`VENUE_KINDS`] that may *own* a venue: only the group node. A fixture or
/// an output adopts the room it already reaches; it never invents one, or a
/// graph with three outputs would end up with three empty rooms.
const OWNER_KIND: &str = metal_graph::VENUE_FIXTURES_KIND;

/// The prefix a graph's default venue id gets: short enough to stay a valid
/// file stem, and recognisable in `data/venues/`.
const DEFAULT_PREFIX: &str = "venue-";

/// What a node's venue is, and how this frame knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeVenue {
    /// The venue id the node edits.
    pub id: String,
    /// Its `config.venue` already named it: nothing was written this frame.
    pub own: bool,
    /// The node adopted the venue it already reached (see the module note).
    pub adopted: bool,
}

/// The venue a graph is about, for a panel that only has the graph (no node
/// in particular): the group node's room, else the first room any node
/// reaches. `None` for a graph with no room at all.
pub fn venue_of_graph(gid: &str, graph: &Value) -> String {
    for kind in VENUE_KINDS {
        for n in nodes_of(graph) {
            if n["kind"].as_str() != Some(kind) {
                continue;
            }
            if let Some(id) = n["config"]["venue"].as_str().filter(|s| !s.is_empty()) {
                return id.to_string();
            }
        }
    }
    default_venue_id(gid)
}

/// The venue `node_id` edits, adopting or defaulting as the module note
/// describes. A write only ever happens when `own` is false and the node is
/// [`OWNER_KIND`]; every other caller gets the same answer without a write.
pub fn venue_for(door: &dyn Call, gid: &str, node_id: &str, node: &Value, graph: &Value) -> NodeVenue {
    let kind = node["kind"].as_str().unwrap_or_default();
    if let Some(id) = node["config"]["venue"].as_str().filter(|s| !s.is_empty()) {
        return NodeVenue {
            id: id.to_string(),
            own: true,
            adopted: false,
        };
    }
    // The room this node already reaches: a `venue.surface`'s own config on
    // a cable downstream (or upstream) of it. Adopting it keeps the scenes an
    // operator already built in this graph, instead of opening a blank room.
    if let Some(id) = reached_venue(graph, node_id) {
        if kind == OWNER_KIND {
            post_config(door, gid, node_id, json!({ "venue": id }));
        }
        return NodeVenue {
            id,
            own: false,
            adopted: true,
        };
    }
    let id = graph_venue(graph).unwrap_or_else(|| default_venue_id(gid));
    if kind == OWNER_KIND {
        post_config(door, gid, node_id, json!({ "venue": id }));
    }
    NodeVenue {
        id,
        own: false,
        adopted: false,
    }
}

/// The graph's own `venue.fixtures` room, when one names it.
fn graph_venue(graph: &Value) -> Option<String> {
    nodes_of(graph)
        .find(|n| n["kind"].as_str() == Some(OWNER_KIND))
        .and_then(|n| n["config"]["venue"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn nodes_of(graph: &Value) -> impl Iterator<Item = &Value> {
    graph["graph"]["nodes"].as_array().into_iter().flatten()
}

/// The venue a `venue.surface` node anywhere in `graph` names, preferring one
/// the node reaches by cable. `None` when the graph names none.
pub fn reached_venue(graph: &Value, node_id: &str) -> Option<String> {
    let nodes = graph["graph"]["nodes"].as_array()?.as_slice();
    let surfaces: Vec<&Value> = nodes
        .iter()
        .filter(|n| n["kind"].as_str() == Some(metal_graph::VENUE_SURFACE_KIND))
        .filter(|n| {
            n["config"]["venue"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        })
        .collect();
    if surfaces.is_empty() {
        return None;
    }
    // Downstream first (the chain reads left to right), then anywhere.
    let reached = downstream(graph, node_id);
    surfaces
        .iter()
        .find(|n| n["id"].as_str().is_some_and(|id| reached.iter().any(|r| r == id)))
        .or_else(|| surfaces.first())
        .and_then(|n| n["config"]["venue"].as_str())
        .map(str::to_string)
}

/// Every node id reachable from `node_id` down the graph's edges.
fn downstream(graph: &Value, node_id: &str) -> Vec<String> {
    let edges = graph["graph"]["edges"].as_array().cloned().unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let mut queue = std::collections::VecDeque::from([node_id.to_string()]);
    while let Some(cur) = queue.pop_front() {
        for e in edges.iter().filter(|e| e["from"]["node"].as_str() == Some(cur.as_str())) {
            if let Some(to) = e["to"]["node"].as_str() {
                if !out.iter().any(|id| id == to) {
                    out.push(to.to_string());
                    queue.push_back(to.to_string());
                }
            }
        }
    }
    out
}

/// A fresh venue id for graph `gid`: `venue-<graph>`, made a valid file stem.
/// A graph whose id is already a venue's name keeps it — the room a collection
/// was built against is the room it edits.
pub fn default_venue_id(gid: &str) -> String {
    let stem: String = gid
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let stem = stem.trim_matches('-');
    if stem.is_empty() {
        return format!("{DEFAULT_PREFIX}graph");
    }
    format!("{DEFAULT_PREFIX}{stem}")
}

/// The venues on disk, as `(id, name)` — the list the add popup shows when
/// the operator reuses a room instead of starting one.
pub fn venues(door: &dyn Call) -> Vec<(String, String)> {
    surface::get_venue_list(door)
        .as_ref()
        .and_then(|v| v["venues"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| {
            let id = v["id"].as_str()?.to_string();
            let name = v["name"]
                .as_str()
                .filter(|n| !n.is_empty())
                .unwrap_or(&id)
                .to_string();
            Some((id, name))
        })
        .collect()
}

/// The venue document (its room, as `GET /venue` returns it) a path asks for:
/// a missing room is a `null` venue, which the popup paints as an empty room.
pub fn venue_detail(door: &dyn Call, id: &str) -> Option<Value> {
    let mut query = std::collections::HashMap::new();
    query.insert("id".to_string(), id.to_string());
    let params = crate::call::Params {
        query,
        body: Vec::new(),
        tail: None,
    };
    door.call("GET", "/venue", params)
        .filter(|reply| reply.status == 200)
        .and_then(|reply| crate::json_memo::parse(&reply.body).ok())
}

/// The venues this node's graph already edits, `(id, name)` from disk, with
/// the node's own first. Used by the map folder's read-out and the popup's
/// reuse list.
pub fn choices(door: &dyn Call, own: &str) -> Vec<(String, String)> {
    let mut out = venues(door);
    if !own.is_empty() && !out.iter().any(|(id, _)| id == own) {
        out.insert(0, (own.to_string(), own.to_string()));
    }
    out
}

/// The `Map` card shared by `venue.fixtures`, `venue.fixture` and
/// `output.device`: which venue document this node edits, and — for the node
/// that owns the room — how to paint it and where it starts. The venue is not
/// a dropdown over every room on the machine: it is this node's own, and the
/// note says when the node adopted the room its cables already reached.
///
/// `open` is set when the operator asked for the add popup (the screen and
/// projection cards); the popup belongs to the node, so the button lives here
/// beside the mapping rather than on the canvas.
pub fn map_card(
    ui: &mut egui::Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    open: &mut bool,
) {
    let venue = venue_for(door, gid, node_id, node, graph);
    let owner = node["kind"].as_str() == Some(OWNER_KIND);
    label_row(ui, theme, tr("room"), |ui| {
        ui.label(
            egui::RichText::new(&venue.id)
                .font(theme.font_small.clone())
                .color(theme.ink),
        )
    });
    if venue.adopted {
        note(ui, theme, tr("this room is the one the graph already used."));
    }
    if ui
        .button(egui::RichText::new(tr("paint the room")).font(theme.font_small.clone()))
        .on_hover_text(tr("screens, projections and fixtures of this room"))
        .clicked()
    {
        *open = true;
    }
    if !owner {
        return;
    }
    // The room this node owns: point it at one already on disk (a scene the
    // operator built last week), rather than starting an empty one.
    let mut pick = venue.id.clone();
    let known = choices(door, &venue.id);
    let options: Vec<(String, &str)> = known.iter().map(|(id, name)| (id.clone(), name.as_str())).collect();
    label_row(ui, theme, tr("edit room"), |ui| {
        dropdown(ui, theme, ("shell/venue/map", node_id), &mut pick, &options)
    });
    if pick != venue.id {
        post_config(door, gid, node_id, json!({ "venue": pick }));
    }
    let fresh = default_venue_id(gid);
    if fresh != venue.id
        && ui
            .button(egui::RichText::new(tr("new room")).font(theme.font_small.clone()))
            .on_hover_text(trf("edit {id} instead of this room", &[("id", &fresh)]))
            .clicked()
    {
        post_config(door, gid, node_id, json!({ "venue": fresh }));
    }
}

/// The mapping card: the room's own scene rows — screens, projectors and
/// fixtures — each named beside the graph node wired to it, and the nodes with
/// no row yet. This is the "mapping with the existing wired nodes" the
/// properties panel carries: the 3D popup *adds* things to the room, this
/// panel says which of the operator's real nodes drives each of them.
///
/// Below the room rows the card is the **input → output table** the plan wants:
///
/// - *entradas* — every `venue.surface` and `venue.fixture` node of the open
///   graph, and the `gen.*` generators it carries upstream (the engine's walk:
///   through effects, masks and further surfaces to each generator; bounded by
///   the node count, so a cycle cannot spin). One `gen → surface` glyph strip
///   per row.
/// - *salidas* — every surface, projector output and fixture of the room, the
///   sectors it is divided into (`venue.surface[i].sectors`, read defensively)
///   and the nodes wired to it. One `surface → device` strip per row.
///
/// The room may be one hardware output carrying several inputs (several
/// `venue.surface` sectors on one display), which the section's sub-label
/// says.
///
/// Reads `GET /venue?id=` and the open graph; every write is the node's own
/// `config` (`POST /graph/node`), the same route the rest of the panel uses.
pub fn mapping_card(ui: &mut egui::Ui, theme: &Theme, door: &dyn Call, gid: &str, graph: &Value) {
    let room = venue_of_graph(gid, graph);
    let detail = venue_detail(door, &room);
    let wire_kind = if is_device_graph(gid) { WireKind::Device } else { WireKind::Scan };
    let model = Mapping::read(graph, detail.as_ref(), wire_kind);
    room_rows(ui, theme, &model);
    ui.add_space(theme.section_gap);
    mapping_table(ui, theme, &model);
}

/// How the table decides which nodes are wired to a room row: a device graph
/// carries several `output.device` nodes (one per screen), so "wired" means
/// `config.device` names the row; everywhere else the row is named by the
/// node's own `config.surface`/`config.fixture`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireKind {
    Device,
    Scan,
}

/// One upstream generator a node carries: its id and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub kind: String,
}

/// One `venue.surface` (or `venue.fixture`) node and the generators behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputRow {
    pub node: String,
    pub kind: String,
    /// The surface/fixture of the room the node names, when it names one.
    pub row: Option<String>,
    pub sources: Vec<Source>,
}

/// One room row and what drives it: an output (surface / projector / display)
/// or a fixture.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputRow {
    pub id: String,
    pub name: String,
    pub kind: OutputKind,
    /// The output's own pixel grid, when the room divides it.
    pub sectors: Option<SectorGrid>,
    pub wired: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    Surface,
    Projector,
    Display,
    Fixture,
}

impl OutputKind {
    fn label(self) -> &'static str {
        match self {
            OutputKind::Surface => tr("surface"),
            OutputKind::Projector => tr("projector"),
            OutputKind::Display => tr("display"),
            OutputKind::Fixture => tr("fixture"),
        }
    }
}

/// A surface's sectors: `rows` × `cols` of equal `ratio`, with `points`-long
/// outlines each when they carry a drawn shape.
#[derive(Clone, Debug, PartialEq)]
pub struct SectorGrid {
    pub rows: u32,
    pub cols: u32,
    pub ratio: f32,
    pub points: Option<usize>,
}

impl SectorGrid {
    /// How many sectors the surface is divided into.
    pub fn count(&self) -> usize {
        (self.rows * self.cols) as usize
    }
}

/// Everything the mapping card paints, read once so a test can hold it to a
/// graph and a room reply without a door.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mapping {
    pub inputs: Vec<InputRow>,
    pub outputs: Vec<OutputRow>,
}

impl Mapping {
    /// Read `graph` and `detail` (`GET /venue?id=`, `None` for a missing room)
    /// into the two lists. Never panics on any shape: a missing `graph` key is
    /// no nodes, a missing room is no outputs.
    pub fn read(graph: &Value, detail: Option<&Value>, wire: WireKind) -> Mapping {
        let nodes: Vec<&Value> = nodes_of(graph).collect();
        let ids: Vec<&str> = nodes.iter().filter_map(|n| n["id"].as_str()).collect();
        let edges = graph["graph"]["edges"].as_array().cloned().unwrap_or_default();
        let inputs = nodes
            .iter()
            .filter(|n| {
                matches!(
                    n["kind"].as_str(),
                    Some(metal_graph::VENUE_SURFACE_KIND) | Some(metal_graph::VENUE_FIXTURE_KIND)
                )
            })
            .map(|n| {
                let kind = n["kind"].as_str().unwrap_or_default().to_string();
                let id = n["id"].as_str().unwrap_or_default().to_string();
                let row = if kind == metal_graph::VENUE_SURFACE_KIND {
                    n["config"]["surface"].as_str().filter(|s| !s.is_empty())
                } else {
                    n["config"]["fixture"].as_str().filter(|s| !s.is_empty())
                }
                .map(str::to_string);
                InputRow {
                    sources: upstream_generators(&id, &nodes, &edges, ids.len()),
                    node: id,
                    kind,
                    row,
                }
            })
            .collect();
        let wired = |key: &str, id: &str| -> Vec<String> {
            nodes
                .iter()
                .filter(|n| n["config"][key].as_str() == Some(id))
                .filter_map(|n| n["id"].as_str().map(str::to_string))
                .collect()
        };
        let mut outputs: Vec<OutputRow> = Vec::new();
        let venue = detail.and_then(|d| d.get("venue"));
        for s in venue.and_then(|v| v["surface"].as_array()).into_iter().flatten() {
            let Some(id) = s["id"].as_str() else {
                continue;
            };
            outputs.push(OutputRow {
                id: id.to_string(),
                name: s["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
                kind: OutputKind::Surface,
                sectors: sectors_of(s),
                wired: wired("surface", id),
            });
        }
        for p in venue.and_then(|v| v["projectors"].as_array()).into_iter().flatten() {
            let id = p["id"].as_str().unwrap_or("?");
            let out = p["output"].as_str().unwrap_or_default();
            let wired = if wire == WireKind::Device && !out.is_empty() {
                wired("device", out)
            } else {
                Vec::new()
            };
            outputs.push(OutputRow {
                id: id.to_string(),
                name: p["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
                kind: OutputKind::Projector,
                sectors: None,
                wired,
            });
        }
        if !venue.is_some_and(|v| v["surface"].as_array().is_some_and(|s| !s.is_empty())) {
            for d in venue.and_then(|v| v["displays"].as_array()).into_iter().flatten() {
                let Some(id) = d["id"].as_str() else {
                    continue;
                };
                outputs.push(OutputRow {
                    id: id.to_string(),
                    name: d["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
                    kind: OutputKind::Display,
                    sectors: None,
                    wired: if wire == WireKind::Device { wired("device", id) } else { Vec::new() },
                });
            }
        }
        for f in venue.and_then(|v| v["fixture"].as_array()).into_iter().flatten() {
            let Some(id) = f["id"].as_str() else {
                continue;
            };
            outputs.push(OutputRow {
                id: id.to_string(),
                name: f["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
                kind: OutputKind::Fixture,
                sectors: None,
                wired: wired("fixture", id),
            });
        }
        Mapping { inputs, outputs }
    }

    /// The sources a surface node carries, by node id (the `salidas` strip's
    /// left half).
    pub fn sources_of(&self, row: &str) -> Vec<&Source> {
        self.inputs
            .iter()
            .filter(|i| i.row.as_deref() == Some(row))
            .flat_map(|i| i.sources.iter())
            .collect()
    }
}

/// A surface's sector grid out of the venue reply, every field defaulted: a
/// drawn grid carries `points`, a projected one a count; anything else is one
/// undivided sector.
fn sectors_of(surface: &Value) -> Option<SectorGrid> {
    let s = surface.get("sectors")?;
    let points = s["points"]
        .as_array()
        .map(|p| p.len())
        .or_else(|| s["points"].as_u64().map(|n| n as usize));
    let draw = |key: &str| s[key].as_u64().unwrap_or(0) as u32;
    let mut rows = draw("rows");
    let mut cols = draw("cols");
    let ratio = s["ratio"].as_f64().unwrap_or(1.0) as f32;
    if rows == 0 || cols == 0 {
        // A drawn grid's outline (`points`, `n/2 + 1` corners) or an explicit
        // `count` says how many; with neither the surface is one sector.
        let drawn = points.and_then(|n| u32::try_from(n / 2 + 1).ok());
        let count = drawn
            .or_else(|| s["count"].as_u64().map(|n| n as u32))
            .unwrap_or(1)
            .max(1);
        if rows == 0 && cols == 0 {
            cols = count;
            rows = 1;
        } else if rows == 0 {
            rows = count.div_ceil(cols.max(1));
        } else {
            cols = count.div_ceil(rows.max(1));
        }
    }
    Some(SectorGrid {
        rows,
        cols,
        ratio: if ratio.is_finite() && ratio > 0.0 { ratio } else { 1.0 },
        points,
    })
}

/// Whether `gid` names a device graph (one `output.device` per screen).
fn is_device_graph(gid: &str) -> bool {
    gid.starts_with("device-")
}

/// [`mapping_card`]'s existing half: the room's rows beside the nodes wired to
/// them, and the nodes that name none yet.
fn room_rows(ui: &mut egui::Ui, theme: &Theme, model: &Mapping) {
    let mut any = false;
    for kind in [
        OutputKind::Surface,
        OutputKind::Projector,
        OutputKind::Display,
        OutputKind::Fixture,
    ] {
        let rows: Vec<&OutputRow> = model.outputs.iter().filter(|o| o.kind == kind).collect();
        if rows.is_empty() {
            continue;
        }
        section_heading(ui, theme, tr(kind.label()));
        for row in rows {
            label_row(ui, theme, &row.name, |ui| {
                let text = if row.wired.is_empty() {
                    tr("— no surface node —").to_string()
                } else {
                    row.wired.join(", ")
                };
                ui.label(
                    egui::RichText::new(text)
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                )
            });
        }
        any = true;
    }
    if !any {
        note(ui, theme, tr("— this room is empty —"));
        return;
    }
    let orphans: usize = model
        .inputs
        .iter()
        .filter(|i| i.row.is_none())
        .count();
    if orphans > 0 {
        note(
            ui,
            theme,
            &trf("{n} wired nodes still pick no row of this room", &[("n", &orphans.to_string())]),
        );
    }
}

/// The entry → exit table: the inputs each surface carries, and the outputs
/// with their sectors and wired nodes.
fn mapping_table(ui: &mut egui::Ui, theme: &Theme, model: &Mapping) {
    section_heading(ui, theme, tr("entradas"));
    note(ui, theme, tr("the generator each surface carries, upstream"));
    if model.inputs.is_empty() {
        note(ui, theme, tr("— no surface or fixture node in this graph —"));
    }
    for row in &model.inputs {
        let sources = if row.sources.is_empty() {
            tr("— no generator —").to_string()
        } else {
            row.sources.iter().map(|s| s.id.clone()).collect::<Vec<_>>().join(", ")
        };
        // `gen → surface node`: the generator on the left, the node that
        // carries it on the right (its room row when it names one).
        let node = match &row.row {
            Some(r) => format!("{}{r}", strip_arrow()),
            None => format!("{}{}", strip_arrow(), row.node),
        };
        label_row(ui, theme, &row.node, |ui| {
            ui.label(
                egui::RichText::new(format!("{sources}{node}"))
                    .font(theme.font_small.clone())
                    .color(theme.ink),
            )
        });
    }
    ui.add_space(theme.section_gap);
    section_heading(ui, theme, tr("salidas"));
    note(ui, theme, tr("now we can have from one hardware output several inputs"));
    if model.outputs.is_empty() {
        note(ui, theme, tr("— this room is empty —"));
        return;
    }
    for row in &model.outputs {
        let sectors = row
            .sectors
            .as_ref()
            .map(|s| trf("{n} sectors", &[("n", &s.count().to_string())]))
            .unwrap_or_else(|| tr("1 sector").to_string());
        let wired = if row.wired.is_empty() {
            tr("— no node wired —").to_string()
        } else {
            row.wired.join(", ")
        };
        label_row(ui, theme, &row.name, |ui| {
            // `surface → device · sectors · node`: the operator reads which
            // input reaches which output at a glance.
            ui.label(
                egui::RichText::new(format!(
                    "{}{}{} · {sectors} · {wired}",
                    row.id,
                    strip_arrow(),
                    row.kind.label(),
                ))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            )
        });
    }
}

/// The arrow and separators the shell's own tables already use (`→`, `·`),
/// as a string the strips share.
pub fn strip_arrow() -> &'static str {
    " → "
}

/// The upstream walk behind [`Mapping::read`]: from `node`, through effects,
/// masks, further surfaces and composition nodes, to every `gen.*` it carries.
/// Bounded by the node count (like the engine's own walk), so a cycle in a
/// file an older build wrote cannot spin it; visited nodes are skipped.
fn upstream_generators(node: &str, nodes: &[&Value], edges: &[Value], limit: usize) -> Vec<Source> {
    let kind_of = |id: &str| -> Option<&str> {
        nodes
            .iter()
            .find(|n| n["id"].as_str() == Some(id))
            .and_then(|n| n["kind"].as_str())
    };
    let mut out: Vec<Source> = Vec::new();
    let mut seen: Vec<String> = vec![node.to_string()];
    let mut queue = std::collections::VecDeque::from([node.to_string()]);
    let mut steps = 0usize;
    while let Some(cur) = queue.pop_front() {
        steps += 1;
        if steps > limit + 1 {
            break;
        }
        if let Some(kind) = kind_of(&cur) {
            if kind.starts_with("gen.") {
                if !out.iter().any(|s| s.id == cur) {
                    out.push(Source {
                        id: cur.clone(),
                        kind: kind.to_string(),
                    });
                }
                continue;
            }
        }
        for e in edges.iter().filter(|e| e["to"]["node"].as_str() == Some(cur.as_str())) {
            if let Some(from) = e["from"]["node"].as_str() {
                if !seen.iter().any(|s| s == from) {
                    seen.push(from.to_string());
                    queue.push_back(from.to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph(nodes: Value, edges: Value) -> Value {
        json!({ "graph": { "nodes": nodes, "edges": edges } })
    }

    /// A graph with no venue node and no surface: the node's room is named
    /// after the graph, not after whatever was projected last.
    #[test]
    fn a_fresh_graph_names_its_room_after_itself() {
        assert_eq!(default_venue_id("col-show"), "venue-col-show");
        assert_eq!(default_venue_id(""), "venue-graph");
        assert_eq!(default_venue_id("../etc"), "venue-etc");
        let g = graph(json!([]), json!([]));
        assert_eq!(venue_of_graph("device-hdmi", &g), "venue-device-hdmi");
    }

    /// Three outputs and one room: they all read that one room, and none of
    /// them invents another.
    #[test]
    fn every_screen_of_a_graph_reads_the_same_room() {
        let g = graph(
            json!([
                { "id": "v", "kind": "venue.fixtures", "config": { "venue": "bar" } },
                { "id": "o1", "kind": "output.device" },
                { "id": "o2", "kind": "output.device" }
            ]),
            json!([]),
        );
        assert_eq!(venue_of_graph("col-show", &g), "bar");
        assert_eq!(reached_venue(&g, "o1"), None);
        assert_eq!(graph_venue(&g).as_deref(), Some("bar"));
    }

    /// A graph built before this batch reaches its room through a
    /// `venue.surface`; the room is adopted, not replaced by an empty one.
    #[test]
    fn a_graph_already_wired_to_a_room_adopts_it() {
        let g = graph(
            json!([
                { "id": "v", "kind": "venue.fixtures" },
                { "id": "s", "kind": "venue.surface", "config": { "venue": "living-room" } },
                { "id": "o", "kind": "output.device" }
            ]),
            json!([
                { "from": { "node": "v", "port": "image" }, "to": { "node": "s", "port": "image" } },
                { "from": { "node": "s", "port": "image" }, "to": { "node": "o", "port": "image" } }
            ]),
        );
        assert_eq!(reached_venue(&g, "v").as_deref(), Some("living-room"));
        assert_eq!(reached_venue(&g, "s").as_deref(), Some("living-room"));
    }

    /// Two generators into one surface: the input row lists both, and the
    /// output row for that surface carries whichever node names it.
    #[test]
    fn two_generators_into_one_surface() {
        let g = json!({ "graph": {
            "nodes": [
                { "id": "g1", "kind": "gen.turing" },
                { "id": "g2", "kind": "gen.noise" },
                { "id": "mix", "kind": "compose.layers" },
                { "id": "s", "kind": "venue.surface", "config": { "surface": "wall-1" } }
            ],
            "edges": [
                { "from": { "node": "g1", "port": "image" }, "to": { "node": "mix", "port": "image" } },
                { "from": { "node": "g2", "port": "image" }, "to": { "node": "mix", "port": "image" } },
                { "from": { "node": "mix", "port": "image" }, "to": { "node": "s", "port": "image" } }
            ]
        }});
        let room = json!({ "venue": { "surface": [{ "id": "wall-1", "name": "Wall" }] } });
        let m = Mapping::read(&g, Some(&room), WireKind::Scan);
        assert_eq!(m.inputs.len(), 1);
        let mut ids: Vec<&str> = m.inputs[0].sources.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["g1", "g2"]);
        let wall = m.outputs.iter().find(|o| o.id == "wall-1").expect("the wall is a row");
        assert_eq!(wall.wired, vec!["s"]);
    }

    /// One generator into two surfaces: both input rows carry it, and each
    /// output names its own node.
    #[test]
    fn one_generator_into_two_surfaces() {
        let g = json!({ "graph": {
            "nodes": [
                { "id": "g", "kind": "gen.pattern" },
                { "id": "fx", "kind": "fx.vignette" },
                { "id": "s1", "kind": "venue.surface", "config": { "surface": "a" } },
                { "id": "s2", "kind": "venue.surface", "config": { "surface": "b" } }
            ],
            "edges": [
                { "from": { "node": "g", "port": "image" }, "to": { "node": "fx", "port": "image" } },
                { "from": { "node": "fx", "port": "image" }, "to": { "node": "s1", "port": "image" } },
                { "from": { "node": "g", "port": "image" }, "to": { "node": "s2", "port": "image" } }
            ]
        }});
        let room = json!({ "venue": { "surface": [{ "id": "a" }, { "id": "b" }] } });
        let m = Mapping::read(&g, Some(&room), WireKind::Scan);
        assert_eq!(m.inputs.len(), 2);
        for row in &m.inputs {
            assert_eq!(row.sources.len(), 1, "{row:?}");
            assert_eq!(row.sources[0].id, "g");
        }
        assert_eq!(m.outputs.len(), 2);
        assert_eq!(m.sources_of("a")[0].id, "g");
        assert_eq!(m.sources_of("b")[0].id, "g");
    }

    /// A cycle upstream is bounded by the node count: the walk terminates and
    /// still lists the generator it reaches.
    #[test]
    fn a_cycle_upstream_is_bounded() {
        let g = json!({ "graph": {
            "nodes": [
                { "id": "g", "kind": "gen.turing" },
                { "id": "a", "kind": "fx.shake" },
                { "id": "b", "kind": "fx.blur" },
                { "id": "s", "kind": "venue.surface", "config": { "surface": "w" } }
            ],
            "edges": [
                { "from": { "node": "g", "port": "image" }, "to": { "node": "a", "port": "image" } },
                { "from": { "node": "a", "port": "image" }, "to": { "node": "b", "port": "image" } },
                { "from": { "node": "b", "port": "image" }, "to": { "node": "a", "port": "image" } },
                { "from": { "node": "a", "port": "image" }, "to": { "node": "s", "port": "image" } }
            ]
        }});
        let m = Mapping::read(&g, None, WireKind::Scan);
        assert_eq!(m.inputs.len(), 1);
        assert_eq!(m.inputs[0].sources.len(), 1);
        assert_eq!(m.inputs[0].sources[0].id, "g");
    }

    /// The three shapes the card must survive: no graph at all, no room, and
    /// a room with no rows.
    #[test]
    fn the_mapping_never_panics_on_an_empty_graph_or_room() {
        let empty = Mapping::read(&Value::Null, None, WireKind::Scan);
        assert!(empty.inputs.is_empty() && empty.outputs.is_empty());
        let bare = json!({ "graph": { "nodes": [], "edges": [] } });
        assert!(Mapping::read(&bare, None, WireKind::Scan).outputs.is_empty());
        let room = json!({ "venue": { "surface": [], "projectors": [], "fixture": [] } });
        let m = Mapping::read(&bare, Some(&room), WireKind::Device);
        assert!(m.inputs.is_empty() && m.outputs.is_empty());
        // A fully missing node list behaves like an empty one.
        let no_nodes = json!({});
        assert!(Mapping::read(&no_nodes, Some(&room), WireKind::Scan).outputs.is_empty());
    }

    /// A surface's sector grid is read defensively: points, counts and a
    /// missing key all resolve to a positive count.
    #[test]
    fn sectors_are_read_defensively() {
        assert_eq!(sectors_of(&json!({ "sectors": { "points": [0, 0, 1, 0] } })).unwrap().count(), 3);
        assert_eq!(sectors_of(&json!({ "sectors": { "rows": 1, "cols": 4 } })).unwrap().count(), 4);
        assert_eq!(sectors_of(&json!({ "sectors": { "count": 2 } })).unwrap().count(), 2);
        assert_eq!(sectors_of(&json!({ "sectors": {} })).unwrap().count(), 1);
        assert_eq!(sectors_of(&json!({})), None);
        assert_eq!(sectors_of(&json!([])).map(|_| ()), None);
    }

    /// A node whose config already names a venue owns it: nothing is written
    /// and the panel paints that room.
    #[test]
    fn a_nodes_own_venue_is_not_rewritten() {
        let node = json!({ "id": "v", "kind": "venue.fixtures", "config": { "venue": "desk" } });
        let g = graph(json!([node.clone()]), json!([]));
        let door = crate::shell::FakeDoor::new();
        let v = venue_for(&door, "col-show", "v", &node, &g);
        assert_eq!(v.id, "desk");
        assert!(v.own);
        assert!(!v.adopted);
    }
}
