//! Layout and hit-test, in Rust — so the day the UI stops being a browser
//! `<canvas>`, the editor does not get rewritten, only repainted.
//!
//! `the design notes` §5 makes the argument this module exists to
//! satisfy: *"the node editor is the most 'desktop application' piece of the
//! project... if the layout and the hit-test live in JS, the native version
//! means starting from scratch."* A browser client gets [`Primitive`]s to paint and
//! sends back pointer coordinates in graph space; a future native painter
//! gets the same `Layout` and paints it with `wgpu` instead. Neither one
//! computes a single port position itself.
//!
//! # The Resolume-Wire shape
//!
//! Each node is a **header band** (the title, centred; the primary output dot
//! on its right edge, a single image input's dot on its left edge) followed
//! by **body rows**: one per input port, one per parameter
//! ([`crate::params::node_params`]) with its `param:<name>` dot and a value
//! field, any output past the primary one as a right-aligned row, a swatch
//! row per colour parameter, and the model-tile area for a `gen.*` node. A
//! generator's whole row (6–20 floats) would make one node taller than the
//! canvas, so a long parameter list shows the first [`COLLAPSED_ROWS`] and a
//! `▸ n more` row that expands — the flag is persisted on the node
//! ([`crate::Node::expanded`], `#[serde(default)]`), so a graph file
//! remembers whether its author opened a node.
//!
//! Every primitive is **tagged**: a node box, a header band, a port dot, a
//! param dot, a value field, a swatch, a model tile or a cable (with its
//! endpoints and its type: image, control or param). A painter reads the tag
//! and draws; it never re-derives which rectangle is a row and which is a box.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::params::{self, Param, ParamKind};
use crate::{Graph, Node, NodeId, NodeKind, PortType};

/// The width of every node box, in graph units.
pub const NODE_W: f32 = 176.0;
/// The nominal height of a node — kept for the callers (`migrate.rs`) that
/// only space nodes out with it; the real box height follows its content.
pub const NODE_H: f32 = 56.0;
/// The header band's own height.
pub const HEADER_H: f32 = 26.0;
/// Height of one body row.
pub const ROW_H: f32 = 20.0;
/// Radius of a port or param dot.
pub const PORT_R: f32 = 6.0;
/// Width of a parameter's value field.
pub const FIELD_W: f32 = 52.0;
/// Side of a colour swatch painted in a node's body.
pub const SWATCH: f32 = 30.0;
/// How many rows are shown before a `▸ n more` row folds the rest away.
pub const COLLAPSED_ROWS: usize = 6;
/// How close a click has to land to a cable to count as hitting it, in graph
/// units (same units as `Node::pos`). The plan's own acceptance test is "a
/// click 2px from a cable hits the cable" — this has to be strictly greater
/// than 2 for that test to pass.
pub const EDGE_HIT_TOLERANCE: f32 = 4.0;
/// Width of an embedded `ai.*` chip drawn on its source (`ai.embed`).
pub const CHIP_W: f32 = 84.0;
/// Height of an embedded `ai.*` chip.
pub const CHIP_H: f32 = 18.0;
/// The gap between the chip and the top edge of its source.
pub const CHIP_GAP: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
    }
}

/// A cable, drawn as a cubic bezier: `p0` and `p3` are the port centers,
/// `p1`/`p2` are the two control points the plan promises (`§5`, "bezier with
/// two control points"), pulled horizontally toward each other so the
/// curve leaves each port level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Path {
    pub p0: Point,
    pub p1: Point,
    pub p2: Point,
    pub p3: Point,
}

impl Path {
    fn straight_pull(p0: Point, p3: Point) -> Path {
        let dx = ((p3.x - p0.x) * 0.5).abs().max(24.0);
        Path {
            p0,
            p1: Point { x: p0.x + dx, y: p0.y },
            p2: Point { x: p3.x - dx, y: p3.y },
            p3,
        }
    }

    /// Points sampled along the curve, for both drawing (a client with no
    /// bezier primitive can draw a polyline) and hit-testing.
    pub fn sample(&self, steps: usize) -> Vec<Point> {
        (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                let mt = 1.0 - t;
                let x = mt * mt * mt * self.p0.x
                    + 3.0 * mt * mt * t * self.p1.x
                    + 3.0 * mt * t * t * self.p2.x
                    + t * t * t * self.p3.x;
                let y = mt * mt * mt * self.p0.y
                    + 3.0 * mt * mt * t * self.p1.y
                    + 3.0 * mt * t * t * self.p2.y
                    + t * t * t * self.p3.y;
                Point { x, y }
            })
            .collect()
    }

    fn distance_to(&self, p: Point) -> f32 {
        let pts = self.sample(32);
        pts.windows(2)
            .map(|w| distance_point_to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min)
    }
}

fn distance_point_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0)
    };
    let cx = a.x + abx * t;
    let cy = a.y + aby * t;
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

/// A text anchor, tagged with what it names.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub x: f32,
    pub y: f32,
    pub content: String,
    pub tag: TextTag,
}

/// What a [`Text`] is: a node title, a row label (a port or parameter name), a
/// value readout, the `▸ n more` affordance, or nothing in particular.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextTag {
    Title,
    Row,
    Value,
    More,
    Plain,
}

/// A rectangle, tagged with what it is, so a painter never has to guess from
/// its size (the old `w >= NODE_W * 0.5` heuristic).
#[derive(Debug, Clone, PartialEq)]
pub struct TaggedRect {
    pub rect: Rect,
    pub tag: RectTag,
}

/// What a [`TaggedRect`] is.
#[derive(Debug, Clone, PartialEq)]
pub enum RectTag {
    /// A node's whole box, following its content height. `node` is the
    /// node's id, so a client hit-testing the primitives can name the node.
    Node { node: NodeId },
    /// A node's header band.
    Header,
    /// A port dot.
    Port { node: NodeId, port: String, out: bool },
    /// A parameter's dot.
    ParamDot { node: NodeId, name: String },
    /// A parameter's value field.
    ValueField { node: NodeId, name: String },
    /// A colour swatch. `name` is the config key (`rgb`, `colors:0`, …) and
    /// `rgb` the resolved colour (0..1 per channel), so a painter fills it
    /// without re-reading the node's config.
    Swatch { node: NodeId, name: String, rgb: [f32; 3] },
    /// The model-tile area of a `gen.*` node.
    Tile { node: NodeId },
    /// The `▸ n more` row that expands the folded rows.
    More { node: NodeId },
    /// An embedded `ai.*` (`ai.embed`) drawn as a chip on its source. `node`
    /// is the owned AI, `source` the node it hangs off and `ai` its kind (the
    /// label a painter shows). The chip is not a free node: it has no box of
    /// its own, no hit as a node, and its wires stay hidden until `source` is
    /// selected.
    Chip { node: NodeId, source: NodeId, ai: String },
}

/// A cable, tagged with its endpoints and its type.
#[derive(Debug, Clone, PartialEq)]
pub struct TaggedPath {
    pub path: Path,
    pub edge: String,
    pub from: (NodeId, String),
    pub to: (NodeId, String),
    pub kind: EdgeKind,
}

/// What a cable carries: an image, a whole control port, or one variable bound
/// to one parameter (a param edge, [`crate::params::is_param_edge`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Image,
    Control,
    Param,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    Rect(TaggedRect),
    Path(TaggedPath),
    Text(Text),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dir {
    In,
    Out,
}

impl Dir {
    pub fn is_out(self) -> bool {
        matches!(self, Dir::Out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Node(NodeId),
    Port(NodeId, String, Dir),
    /// A parameter's value field or dot — the Resolume-Wire "play this row"
    /// target. `name` is the parameter's name.
    Param(NodeId, String),
    /// A colour swatch in a node's body (2026-10-08) — a `color` node's `rgb`
    /// or a `palette` node's `colors:<i>`. `name` is the config key, so the
    /// panel can open a picker seeded from it and post it back.
    Swatch(NodeId, String),
    Edge(String),
    Empty,
}

/// One parameter's resolved geometry.
#[derive(Debug, Clone)]
pub struct ParamGeom {
    pub dot: Point,
    pub field: Rect,
    pub value: Option<f32>,
    /// The `x`-ordered list of swatches this parameter paints, if it is a
    /// colour slot holding a colour.
    pub swatches: Vec<(String, [f32; 3])>,
    /// The config's palette rows, when this is a palette-carrying node.
    pub palette: Vec<Vec<[f32; 3]>>,
}

/// One node's resolved geometry.
struct NodeGeom {
    rect: Rect,
    header: Rect,
    /// Port points by `(port, out)` — a port on the header or a body row.
    ports: BTreeMap<(String, bool), Point>,
    /// Parameter rows by name, in row order.
    params: Vec<(String, ParamGeom)>,
    /// The model-tile area, for a `gen.*` node.
    tile: Option<Rect>,
    /// The `▸ n more` row, when the node's rows are folded.
    more: Option<Rect>,
    /// How many parameter rows the `more` row folds away.
    hidden: usize,
    /// `true` when a port dot sits on the header band rather than a body row.
    header_ports: Vec<(String, bool)>,
}

/// The resolved geometry of one graph: every node's box, every port's point,
/// every edge's curve. Built once per graph version by [`Graph::layout`] and
/// handed to a painter and to [`Layout::hit`] — neither of which recomputes
/// anything.
pub struct Layout {
    node_rects: BTreeMap<NodeId, Rect>,
    port_points: BTreeMap<(NodeId, String, Dir), Point>,
    edge_paths: BTreeMap<String, Path>,
    /// Per edge: its endpoints and its type, so a tagged cable can be painted
    /// from the id the layout was built with.
    edge_tags: BTreeMap<String, ((NodeId, String), (NodeId, String), EdgeKind)>,
    node_labels: BTreeMap<NodeId, String>,
    nodes: BTreeMap<NodeId, NodeGeom>,
    /// The embedded `ai.*` chips (`ai.embed`), by owned node id: the chip's
    /// rect, its source and its kind. A chip is painted on its source and is
    /// never a free node — see [`RectTag::Chip`].
    chips: BTreeMap<NodeId, (Rect, NodeId, String)>,
}

fn config_json(node: &Node) -> Value {
    serde_json::from_str(&node.config.to_json_str()).unwrap_or(Value::Null)
}

/// One number-triple off a JSON array, when it is three finite numbers.
fn triple(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    let x = a.first()?.as_f64()?;
    let y = a.get(1)?.as_f64()?;
    let z = a.get(2)?.as_f64()?;
    Some([x as f32, y as f32, z as f32])
}

/// The colour swatches a node's config carries: `rgb: [r,g,b]` (a `color`
/// node) or `colors: [[r,g,b], …]` (a `palette` node). Names are the config
/// keys so a painter can tag its fill back to the datum.
fn config_swatches(config: &Value) -> Vec<(String, [f32; 3])> {
    if let Some(rgb) = config.get("rgb").and_then(triple) {
        return vec![("rgb".to_string(), rgb)];
    }
    if let Some(colors) = config.get("colors").and_then(Value::as_array) {
        return colors
            .iter()
            .enumerate()
            .filter_map(|(i, c)| triple(c).map(|rgb| (format!("colors:{i}"), rgb)))
            .collect();
    }
    Vec::new()
}

/// The rect one colour swatch paints at — the geometry [`Layout::primitives`]
/// emits and [`Layout::hit`] tests, so a click lands where the chip is drawn.
fn swatch_rect(geom: &NodeGeom, pg: &ParamGeom, i: usize) -> Rect {
    Rect {
        x: geom.rect.x + PORT_R + SWATCH * i as f32,
        y: pg.dot.y - SWATCH * 0.45,
        w: SWATCH,
        h: SWATCH * 0.9,
    }
}

/// A body row of a node, before it is given a `y`.
enum Row {
    Port { name: String, out: bool },
    Param { name: String },
    More,
    Tile,
}

impl Row {
    /// A copy without the owned name being consumed twice — `fixed` rows are
    /// built once and then copied into the visible list.
    fn shallow_clone(&self) -> Row {
        match self {
            Row::Port { name, out } => Row::Port { name: name.clone(), out: *out },
            Row::Param { name } => Row::Param { name: name.clone() },
            Row::More => Row::More,
            Row::Tile => Row::Tile,
        }
    }
}

/// The height a node's box needs for `rows` body rows (a header plus the
/// rows), never less than a single header.
fn box_height(rows: usize) -> f32 {
    HEADER_H + ROW_H * rows.max(1) as f32
}

/// The index of the one input that sits on the header's left edge: the first
/// image input of a node that takes exactly one image (an effect's `image`, a
/// mask's `stroke`). A generator's `control` is a body row, and a node with
/// two image inputs keeps them all in the body. `None` when no single image
/// input owns the header.
fn header_input(inputs: &[crate::Port]) -> Option<usize> {
    let images: Vec<usize> = inputs
        .iter()
        .enumerate()
        .filter(|(_, p)| p.ty == PortType::Image)
        .map(|(i, _)| i)
        .collect();
    if images.len() == 1 {
        Some(images[0])
    } else {
        None
    }
}

/// The index of the output on the header's right edge: the first image output
/// if there is one (a generator's render), else the first output.
fn primary_output(outputs: &[crate::Port]) -> Option<usize> {
    if outputs.is_empty() {
        return None;
    }
    outputs.iter().position(|p| p.ty == PortType::Image).or(Some(0))
}

impl Graph {
    /// Computes layout from each node's own `pos` (already authored — see
    /// `Node::pos`) plus the catalog's port lists and each node's parameters
    /// ([`params::node_params`]), which decide how many rows a node's box
    /// carries and where they sit.
    pub fn layout(&self, catalog: &[NodeKind]) -> Layout {
        let by_kind: BTreeMap<&str, &NodeKind> =
            catalog.iter().map(|k| (k.kind.as_str(), k)).collect();

        let mut node_rects = BTreeMap::new();
        let mut port_points = BTreeMap::new();
        let mut node_labels = BTreeMap::new();
        let mut nodes = BTreeMap::new();

        for n in &self.nodes {
            // An embedded node (`Node::embedded_in`) is not a free node: it
            // draws as a chip on its source and never takes part in the
            // layout or the hit-test as a box. It is left out here and its
            // chip is built below, once every free node's box is known.
            if n.embedded_in.is_some() {
                continue;
            }
            let kind = by_kind.get(n.kind.as_str()).copied();
            let params = params::node_params(n, kind);
            let (geom, label) = node_geom(n, kind, &params);
            node_labels.insert(n.id.clone(), label);
            node_rects.insert(n.id.clone(), geom.rect);
            for ((port, out), point) in &geom.ports {
                let dir = if *out { Dir::Out } else { Dir::In };
                port_points.insert((n.id.clone(), port.clone(), dir), *point);
            }
            for (name, pg) in &geom.params {
                port_points.insert((n.id.clone(), params::param_port(name), Dir::In), pg.dot);
            }
            nodes.insert(n.id.clone(), geom);
        }

        // The embedded `ai.*` chips (`ai.embed`), and the port points their
        // cables land on. A chip's ports live here, never in `port_points`, so
        // [`Layout::hit`] cannot answer a free-node port where a chip sits —
        // "excluded from layout and hit-testing as a free node".
        let mut chips: BTreeMap<NodeId, (Rect, NodeId, String)> = BTreeMap::new();
        let mut chip_ports: BTreeMap<(NodeId, String, Dir), Point> = BTreeMap::new();
        for n in &self.nodes {
            let Some(source) = n.embedded_in.as_deref() else {
                continue;
            };
            let Some(src) = nodes.get(source) else {
                continue;
            };
            let rect = chip_rect(src.rect);
            let mid = Point { x: rect.x, y: rect.y + rect.h / 2.0 };
            let mid_out = Point { x: rect.x + rect.w, y: rect.y + rect.h / 2.0 };
            if let Some(kind) = by_kind.get(n.kind.as_str()) {
                for p in &kind.inputs {
                    chip_ports.insert((n.id.clone(), p.name.clone(), Dir::In), mid);
                }
                for p in &kind.outputs {
                    chip_ports.insert((n.id.clone(), p.name.clone(), Dir::Out), mid_out);
                }
            }
            chips.insert(n.id.clone(), (rect, source.to_string(), n.kind.clone()));
        }

        let mut edge_paths = BTreeMap::new();
        let mut edge_tags = BTreeMap::new();
        for e in &self.edges {
            // A binding's `to.port` is `param:<name>`; find that node's row by
            // name and take the row's dot. Anything else is an ordinary input
            // port, keyed by its own name — a chip's port when the node is an
            // embedded `ai.*`.
            let to_point = params::param_name(&e.to.port)
                .and_then(|name| {
                    nodes
                        .get(&e.to.node)
                        .and_then(|g| g.params.iter().find(|(n, _)| n == name))
                        .map(|(_, pg)| pg.dot)
                })
                .or_else(|| {
                    nodes
                        .get(&e.to.node)
                        .and_then(|g| port_point_of(g, &e.to.port, Dir::In))
                })
                .or_else(|| {
                    chip_ports
                        .get(&(e.to.node.clone(), e.to.port.clone(), Dir::In))
                        .copied()
                });
            // A binding's `from.port` is `<port>.<var>` (`control.rms`), so the
            // port dot is keyed by the *port* half; an ordinary cable names
            // the port whole. Either way the layout never guesses a position.
            let from_port = params::split_var_port(&e.from.port)
                .map(|(port, _)| port)
                .unwrap_or(e.from.port.as_str());
            let from_point = nodes
                .get(&e.from.node)
                .and_then(|g| port_point_of(g, from_port, Dir::Out))
                .or_else(|| {
                    port_points
                        .get(&(e.from.node.clone(), from_port.to_string(), Dir::Out))
                        .copied()
                })
                .or_else(|| {
                    chip_ports
                        .get(&(e.from.node.clone(), from_port.to_string(), Dir::Out))
                        .copied()
                });
            if let (Some(p0), Some(p3)) = (from_point, to_point) {
                edge_paths.insert(e.id.clone(), Path::straight_pull(p0, p3));
                edge_tags.insert(
                    e.id.clone(),
                    (
                        (e.from.node.clone(), e.from.port.clone()),
                        (e.to.node.clone(), e.to.port.clone()),
                        self.edge_kind(catalog, e),
                    ),
                );
            }
            // A dangling edge (deleted node, renamed port) simply gets no
            // path — nothing to draw, nothing to hit-test, no panic.
        }

        Layout { node_rects, port_points, edge_paths, edge_tags, node_labels, nodes, chips }
    }

    /// The kind of one edge's cable: a binding, or the type of the output port
    /// it leaves (`image` vs `control`).
    fn edge_kind(&self, catalog: &[NodeKind], e: &crate::Edge) -> EdgeKind {
        if params::is_param_edge(e) {
            return EdgeKind::Param;
        }
        let ty = self
            .node(&e.from.node)
            .and_then(|n| catalog.iter().find(|k| k.kind == n.kind))
            .and_then(|k| k.outputs.iter().find(|p| p.name == e.from.port))
            .map(|p| p.ty);
        match ty {
            Some(PortType::Image) => EdgeKind::Image,
            _ => EdgeKind::Control,
        }
    }
}

/// One node's box, header, port dots, parameter rows, tile and `more` row.
fn node_geom(n: &Node, kind: Option<&NodeKind>, params: &[Param]) -> (NodeGeom, String) {
    let x = n.pos[0];
    let y = n.pos[1];
    let label = if n.label.is_empty() { n.kind.clone() } else { n.label.clone() };
    let config = config_json(n);

    let inputs = kind.map(|k| k.inputs.as_slice()).unwrap_or(&[]);
    let outputs = kind.map(|k| k.outputs.as_slice()).unwrap_or(&[]);

    let header_in = header_input(inputs);
    let primary_out = primary_output(outputs);
    let has_tile =
        n.kind.starts_with("gen.") && outputs.iter().any(|p| p.ty == PortType::Image);

    let colour_params: Vec<&Param> =
        params.iter().filter(|p| p.kind == ParamKind::Colour).collect();
    let float_params: Vec<&Param> =
        params.iter().filter(|p| p.kind == ParamKind::Float).collect();
    let swatches = config_swatches(&config);
    // A colour node with a plain `rgb` swatch but no declared colour param
    // (a `color` kind another agent is adding) still gets one row for it.
    let extra_swatch_rows = swatches.len().saturating_sub(colour_params.len());

    // The `fixed` rows — input ports, extra outputs and colour swatches — are
    // always shown; only the (possibly long) float-parameter list folds.
    let mut fixed: Vec<Row> = Vec::new();
    let mut header_ports: Vec<(String, bool)> = Vec::new();
    for (i, p) in inputs.iter().enumerate() {
        if Some(i) == header_in {
            header_ports.push((p.name.clone(), false));
            continue;
        }
        fixed.push(Row::Port { name: p.name.clone(), out: false });
    }
    for (i, p) in outputs.iter().enumerate() {
        if Some(i) == primary_out {
            header_ports.push((p.name.clone(), true));
            continue;
        }
        fixed.push(Row::Port { name: p.name.clone(), out: true });
    }
    for p in &colour_params {
        fixed.push(Row::Param { name: p.name.clone() });
    }
    for _ in 0..extra_swatch_rows {
        fixed.push(Row::Param { name: "rgb".to_string() });
    }

    let folded = float_params.len() > COLLAPSED_ROWS && !n.expanded;
    let shown_params = if folded { COLLAPSED_ROWS } else { float_params.len() };

    // Assemble the visible rows: fixed rows, then the shown float params,
    // then a `more` row when folded, then the tile.
    let mut rows: Vec<Row> = Vec::new();
    rows.extend(fixed.iter().map(Row::shallow_clone));
    for p in float_params.iter().take(shown_params) {
        rows.push(Row::Param { name: p.name.clone() });
    }
    let more_index = if folded {
        let i = rows.len();
        rows.push(Row::More);
        Some(i)
    } else {
        None
    };
    let tile_index = if has_tile {
        let i = rows.len();
        rows.push(Row::Tile);
        Some(i)
    } else {
        None
    };

    let visible = rows.len().max(1);
    let rect = Rect { x, y, w: NODE_W, h: box_height(visible) };
    let header = Rect { x, y, w: NODE_W, h: HEADER_H };

    let mut ports: BTreeMap<(String, bool), Point> = BTreeMap::new();
    let mut param_geoms: Vec<(String, ParamGeom)> = Vec::new();
    let mut tile = None;
    let mut more = None;

    // The header's own dots.
    for (name, out) in &header_ports {
        let point = if *out {
            Point { x: x + NODE_W, y: y + HEADER_H / 2.0 }
        } else {
            Point { x, y: y + HEADER_H / 2.0 }
        };
        ports.insert((name.clone(), *out), point);
    }

    let mut swatch_at = 0usize;
    for (row_i, row) in rows.iter().enumerate() {
        let row_y = y + HEADER_H + ROW_H * row_i as f32;
        let cy = row_y + ROW_H / 2.0;
        match row {
            Row::Port { name, out } => {
                let point = if *out { Point { x: x + NODE_W, y: cy } } else { Point { x, y: cy } };
                ports.insert((name.clone(), *out), point);
            }
            Row::Param { name } => {
                let is_colour = colour_params.iter().any(|p| p.name == *name);
                let (value, swatch_list) = if is_colour {
                    (None, swatches.iter().skip(swatch_at).take(1).cloned().collect())
                } else if let Some(p) = float_params.iter().find(|p| p.name == *name) {
                    (p.value.or(Some(p.default)), Vec::new())
                } else {
                    (None, swatches.iter().skip(swatch_at).take(1).cloned().collect())
                };
                swatch_at += 1;
                param_geoms.push((
                    name.clone(),
                    ParamGeom {
                        dot: Point { x, y: cy },
                        field: Rect {
                            x: x + NODE_W - FIELD_W - PORT_R,
                            y: row_y + ROW_H * 0.15,
                            w: FIELD_W,
                            h: ROW_H * 0.7,
                        },
                        value,
                        swatches: swatch_list,
                        palette: Vec::new(),
                    },
                ));
            }
            Row::More => {
                debug_assert_eq!(Some(row_i), more_index);
                more = Some(Rect { x, y: row_y, w: NODE_W, h: ROW_H });
            }
            Row::Tile => {
                debug_assert_eq!(Some(row_i), tile_index);
                tile = Some(Rect {
                    x: x + PORT_R,
                    y: row_y + 2.0,
                    w: NODE_W - PORT_R * 2.0,
                    h: ROW_H * 1.6,
                });
            }
        }
    }

    (NodeGeom { rect, header, ports, params: param_geoms, tile, more, hidden: float_params.len() - shown_params, header_ports }, label)
}

/// The dot rectangle around a port center.
fn dot_rect(p: Point) -> Rect {
    Rect { x: p.x - PORT_R, y: p.y - PORT_R, w: PORT_R * 2.0, h: PORT_R * 2.0 }
}

/// The chip an embedded `ai.*` draws on its source: a small band above the
/// source's top-right corner, clear of its header so it reads as attached to
/// the card rather than part of it.
fn chip_rect(src: Rect) -> Rect {
    Rect {
        x: src.x + src.w - CHIP_W,
        y: src.y - CHIP_H - CHIP_GAP,
        w: CHIP_W,
        h: CHIP_H,
    }
}

/// One node's port dot by name and direction — a header dot or a body row,
/// they live in the same map. `None` when the node has no such port.
fn port_point_of(geom: &NodeGeom, port: &str, dir: Dir) -> Option<Point> {
    geom.ports.get(&(port.to_string(), dir.is_out())).copied()
}

/// A compact fixed-point number for a value field ("0", "0.5", "3").
fn format_number(v: f32) -> String {
    if (v - v.round()).abs() < 0.001 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

impl Layout {
    pub fn primitives(&self) -> Vec<Primitive> {
        let mut out = Vec::new();
        for (id, geom) in &self.nodes {
            let label = self.node_labels.get(id).cloned().unwrap_or_default();
            out.push(Primitive::Rect(TaggedRect { rect: geom.rect, tag: RectTag::Node { node: id.clone() } }));
            out.push(Primitive::Rect(TaggedRect { rect: geom.header, tag: RectTag::Header }));
            out.push(Primitive::Text(Text {
                x: geom.rect.x + NODE_W / 2.0,
                y: geom.rect.y + HEADER_H / 2.0,
                content: label,
                tag: TextTag::Title,
            }));
            for ((port, out_dir), p) in &geom.ports {
                out.push(Primitive::Rect(TaggedRect {
                    rect: dot_rect(*p),
                    tag: RectTag::Port { node: id.clone(), port: port.clone(), out: *out_dir },
                }));
                // A body-row port carries its own name; the header's dots do
                // not (the title is the node).
                if !geom.header_ports.contains(&(port.clone(), *out_dir)) {
                    let label_x = if *out_dir {
                        geom.rect.x + NODE_W - PORT_R - 6.0
                    } else {
                        geom.rect.x + PORT_R + 6.0
                    };
                    out.push(Primitive::Text(Text {
                        x: label_x,
                        y: p.y,
                        content: port.clone(),
                        tag: TextTag::Row,
                    }));
                }
            }
            for (name, pg) in &geom.params {
                out.push(Primitive::Rect(TaggedRect {
                    rect: dot_rect(pg.dot),
                    tag: RectTag::ParamDot { node: id.clone(), name: name.clone() },
                }));
                out.push(Primitive::Rect(TaggedRect {
                    rect: pg.field,
                    tag: RectTag::ValueField { node: id.clone(), name: name.clone() },
                }));
                out.push(Primitive::Text(Text {
                    x: geom.rect.x + PORT_R + 6.0,
                    y: pg.dot.y,
                    content: name.clone(),
                    tag: TextTag::Row,
                }));
                if let Some(v) = pg.value {
                    out.push(Primitive::Text(Text {
                        x: pg.field.x + 4.0,
                        y: pg.field.y + pg.field.h / 2.0,
                        content: format_number(v),
                        tag: TextTag::Value,
                    }));
                }
                for (i, (key, rgb)) in pg.swatches.iter().enumerate() {
                    out.push(Primitive::Rect(TaggedRect {
                        rect: swatch_rect(geom, pg, i),
                        tag: RectTag::Swatch { node: id.clone(), name: key.clone(), rgb: *rgb },
                    }));
                }
            }
            if let Some(t) = geom.tile {
                out.push(Primitive::Rect(TaggedRect { rect: t, tag: RectTag::Tile { node: id.clone() } }));
            }
            if let Some(m) = geom.more {
                out.push(Primitive::Rect(TaggedRect { rect: m, tag: RectTag::More { node: id.clone() } }));
                out.push(Primitive::Text(Text {
                    x: m.x + PORT_R + 6.0,
                    y: m.y + m.h / 2.0,
                    content: format!("\u{25b8} {} more", geom.hidden),
                    tag: TextTag::More,
                }));
            }
        }
        for (id, (rect, source, ai)) in &self.chips {
            out.push(Primitive::Rect(TaggedRect {
                rect: *rect,
                tag: RectTag::Chip { node: id.clone(), source: source.clone(), ai: ai.clone() },
            }));
        }
        for (id, path) in &self.edge_paths {
            // An edge that reached here has both endpoints; the tag carries
            // them and the kind so a painter styles a param cable differently.
            let (from, to, kind) = self.edge_tags[id].clone();
            out.push(Primitive::Path(TaggedPath {
                path: *path,
                edge: id.clone(),
                from,
                to,
                kind,
            }));
        }
        out
    }

    /// Ports first (small targets, and the plan requires clicking a port's
    /// edge to hit the port and not the node behind it), then a value field,
    /// then a param dot, then cables (within [`EDGE_HIT_TOLERANCE`]), then the
    /// node body, else [`Hit::Empty`].
    ///
    /// A parameter has **two** targets of its one row, and they answer
    /// differently (N): the value field is a [`Hit::Param`] — the drag that
    /// edits a held value — and the dot is a [`Hit::Port`] carrying the
    /// `param:<name>` port, so a cable from an output can land on the row and
    /// bind it. The dot is checked in the port sweep (it lives in
    /// `port_points` like any other port) and the field right after.
    pub fn hit(&self, x: f32, y: f32) -> Hit {
        let p = Point { x, y };

        // Ports — a param dot included, under its `param:<name>` spelling, so
        // a cable drop on a row resolves to the same `Port` answer a drop on
        // an image input does. A `Param` hit (the field) is answered below.
        for ((node, port, dir), pt) in &self.port_points {
            let d = ((p.x - pt.x).powi(2) + (p.y - pt.y).powi(2)).sqrt();
            if d <= PORT_R {
                return Hit::Port(node.clone(), port.clone(), *dir);
            }
        }

        for (node, geom) in &self.nodes {
            for (name, pg) in &geom.params {
                if pg.field.contains(p) {
                    return Hit::Param(node.clone(), name.clone());
                }
            }
        }

        // Colour swatches (2026-10-08) — checked after the value fields (a
        // swatch sits on the row's left, clear of the field) and before the
        // cables and the node box, so a click on the chip edits the colour
        // and a click elsewhere on the row still drags the value.
        for (node, geom) in &self.nodes {
            for (_name, pg) in &geom.params {
                for (i, (key, _rgb)) in pg.swatches.iter().enumerate() {
                    if swatch_rect(geom, pg, i).contains(p) {
                        return Hit::Swatch(node.clone(), key.clone());
                    }
                }
            }
        }

        for (id, path) in &self.edge_paths {
            // An embedded `ai.*`'s wires stay hidden until its source is
            // selected, so they are never hit where they would be drawn —
            // the same "not a free node" rule the chip's box follows.
            if let Some((from, to, _)) = self.edge_tags.get(id) {
                if self.chips.contains_key(&from.0) || self.chips.contains_key(&to.0) {
                    continue;
                }
            }
            if path.distance_to(p) <= EDGE_HIT_TOLERANCE {
                return Hit::Edge(id.clone());
            }
        }

        for (id, rect) in &self.node_rects {
            if rect.contains(p) {
                return Hit::Node(id.clone());
            }
        }

        Hit::Empty
    }

    pub fn node_rect(&self, id: &str) -> Option<Rect> {
        self.node_rects.get(id).copied()
    }

    /// Whether node `id`'s rows are folded behind a `▸ n more` row.
    pub fn node_folded(&self, id: &str) -> bool {
        self.nodes.get(id).is_some_and(|g| g.more.is_some())
    }

    /// Where a cable would leave/enter `(node, port)` — Set D2's reason
    /// for existing: the browser client only paints and posts pointer
    /// coordinates, so *something* server-side has to answer "where is this
    /// port" per node per port, not just as an undifferentiated dot in
    /// [`primitives`](Layout::primitives). A port name is unique within a
    /// node regardless of direction, so there is at most one match.
    pub fn port_point(&self, node: &str, port: &str) -> Option<Point> {
        self.port_points.iter().find(|((n, p, _), _)| n == node && p == port).map(|(_, pt)| *pt)
    }

    /// The cable `id` draws as, for the same reason [`port_point`](Layout::port_point)
    /// exists: a client painting from a tagged (node/edge id -> geometry)
    /// export needs this looked up by id, not fished out of the flat,
    /// untagged list [`primitives`](Layout::primitives) returns.
    pub fn edge_path(&self, id: &str) -> Option<Path> {
        self.edge_paths.get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Domain, Endpoint, Graph, NodeFamily, Port, PortType};

    fn catalog() -> Vec<NodeKind> {
        // Through the same constructor `NodeKind::catalog` uses, so the
        // `menu`/`domains` fields R22.5a added stay consistent here too.
        vec![
            NodeKind::new(
                "gen.turing",
                NodeFamily::Generator,
                "Turing",
                vec![Port::new("control", PortType::Control)],
                vec![Port::new("image", PortType::Image), Port::new("row", PortType::Control)],
            ),
            NodeKind::new(
                "output.device",
                NodeFamily::OutputDevice,
                "Output",
                vec![Port::new("image", PortType::Image), Port::new("control", PortType::Control)],
                vec![],
            ),
        ]
    }

    #[test]
    fn clicking_on_a_port_hits_the_port_not_the_node() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let layout = g.layout(&catalog());
        let rect = layout.node_rect(&gen).unwrap();
        // The primary output port sits on the header's right edge.
        let px = rect.x + rect.w;
        let py = rect.y + HEADER_H / 2.0;
        match layout.hit(px, py) {
            Hit::Port(node, port, dir) => {
                assert_eq!(node, gen);
                assert_eq!(port, "image");
                assert!(dir.is_out());
            }
            other => panic!("expected a port hit, got {other:?}"),
        }
    }

    #[test]
    fn clicking_2px_from_a_cable_hits_the_cable() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [300.0, 0.0]);
        let edge_id = g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        let layout = g.layout(&catalog());
        let path = layout.edge_paths.get(&edge_id).expect("edge has a path");
        // Midpoint of the curve, nudged 2 graph units perpendicular (here,
        // straight up) — well clear of any port.
        let mid = path.sample(2)[1];
        match layout.hit(mid.x, mid.y - 2.0) {
            Hit::Edge(id) => assert_eq!(id, edge_id),
            other => panic!("expected an edge hit 2px from the cable, got {other:?}"),
        }
    }

    #[test]
    fn clicking_far_from_everything_is_empty() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let layout = g.layout(&catalog());
        assert_eq!(layout.hit(10_000.0, 10_000.0), Hit::Empty);
    }

    /// An embedded node (`Node::embedded_in`, `ai.embed`) is not a free node:
    /// it gets no box and cannot be hit where its box would have been.
    #[test]
    fn an_embedded_node_is_not_laid_out_as_a_free_node() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let ai = g.add_node("ai", "gen.turing", [400.0, 0.0]);
        g.node_mut(&ai).unwrap().embedded_in = Some(gen.clone());
        let layout = g.layout(&catalog());
        assert!(layout.node_rect(&gen).is_some(), "the source is a free node");
        assert!(layout.node_rect(&ai).is_none(), "the embedded node has no box");
        assert_eq!(layout.hit(420.0, 20.0), Hit::Empty, "and cannot be hit as a free node");
    }

    /// The embedded `ai.*` draws as a chip on its source, and both of its
    /// wires resolve — so a selected source can reveal them — even though the
    /// chip is not a free node and its wires are never hit as cables.
    #[test]
    fn an_embedded_ai_is_a_chip_on_its_source_with_resolvable_wires() {
        let cat = vec![
            NodeKind::new(
                "audio.live",
                NodeFamily::Audio,
                "Audio",
                vec![],
                vec![Port::new("control", PortType::Control)],
            ),
            NodeKind::new(
                "gen.turing",
                NodeFamily::Generator,
                "Turing",
                vec![Port::new("control", PortType::Control)],
                vec![Port::new("image", PortType::Image)],
            ),
            NodeKind::new(
                crate::AI_COLOURIST_KIND,
                NodeFamily::Ai,
                "Colourist",
                vec![Port::new("control", PortType::Control)],
                vec![Port::new("control", PortType::Control)],
            ),
        ];
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [200.0, 0.0]);
        let ai = g.add_node("ai", crate::AI_COLOURIST_KIND, [0.0, 0.0]);
        g.node_mut(&ai).unwrap().embedded_in = Some(gen.clone());
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&ai, "control"));
        g.add_edge(Endpoint::new(&ai, "control"), Endpoint::new(&gen, "control"));

        let layout = g.layout(&cat);
        let chip = layout
            .primitives()
            .into_iter()
            .find_map(|p| match p {
                Primitive::Rect(TaggedRect { rect, tag: RectTag::Chip { node, source, ai } }) => {
                    Some((rect, node, source, ai))
                }
                _ => None,
            })
            .expect("the embedded ai draws a chip");
        assert_eq!(chip.1, ai);
        assert_eq!(chip.2, gen);
        assert_eq!(chip.3, crate::AI_COLOURIST_KIND);
        assert!(layout.node_rect(&ai).is_none(), "the chip is not a free box");
        assert_eq!(layout.edge_paths.len(), 2, "both wires resolve so a selected source can show them");
        // The chip sits above the source's top-right corner.
        let src = layout.node_rect(&gen).unwrap();
        assert!(chip.0.y < src.y, "the chip is above the source");
    }

    #[test]
    fn a_dangling_edge_produces_no_path_and_does_not_panic() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new("missing", "image"));
        let layout = g.layout(&catalog());
        assert!(layout.edge_paths.is_empty());
    }

    // -- D2 -------------------------------------------------------------

    #[test]
    fn port_point_finds_the_same_point_a_client_would_need_to_paint_a_dot_at() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let layout = g.layout(&catalog());
        let rect = layout.node_rect(&gen).unwrap();
        let p = layout.port_point(&gen, "image").expect("gen.turing has an image output");
        assert_eq!(p.x, rect.x + rect.w, "the output port sits on the right edge");
        assert!(layout.port_point(&gen, "no-such-port").is_none());
        assert!(layout.port_point("no-such-node", "image").is_none());
    }

    #[test]
    fn edge_path_looks_up_by_the_edge_id_a_client_was_handed() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [300.0, 0.0]);
        let edge_id = g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        let layout = g.layout(&catalog());
        assert!(layout.edge_path(&edge_id).is_some());
        assert!(layout.edge_path("no-such-edge").is_none());
    }

    // -- the Wire body: rows, params, swatches, tiles ------------------

    /// The `real` catalog off the repo's own manifest, so a generator's
    /// parameter rows are the manifest's.
    fn real_catalog() -> Vec<NodeKind> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        let manifest = metal_schema::Manifest::parse(&metal_vfs::read_to_string(path).expect("schema.json"))
            .expect("parses");
        NodeKind::catalog(&manifest)
    }

    /// A node's box grows with its rows: a generator (6+ floats) is taller
    /// than a lone header, and its output dot sits on the header.
    #[test]
    fn a_nodes_box_follows_its_content_height() {
        let catalog = real_catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [300.0, 0.0]);
        let layout = g.layout(&catalog);
        let gen_rect = layout.node_rect(&gen).unwrap();
        assert!(gen_rect.h > HEADER_H, "a generator's body has rows: {}", gen_rect.h);
        let out = layout.port_point(&gen, "image").unwrap();
        assert_eq!(out.y, gen_rect.y + HEADER_H / 2.0, "the render leaves on the header");
        // An effect is single-in/single-out: both dots on the header, and its
        // `amount` is a body row.
        let fx_rect = layout.node_rect(&fx).unwrap();
        let image_in = layout.port_point(&fx, "image").unwrap();
        assert_eq!(image_in.x, fx_rect.x, "the effect's image input is on the left edge");
        assert_eq!(image_in.y, fx_rect.y + HEADER_H / 2.0, "and on the header");
    }

    /// A parameter's two targets answer apart (N): the value field is the
    /// [`Hit::Param`] a drag edits, the dot is a [`Hit::Port`] with the
    /// `param:<name>` spelling a cable binds through.
    #[test]
    fn a_hit_on_a_value_field_is_param_and_on_a_param_dot_is_the_param_port() {
        let catalog = real_catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let fx = g.add_node("fx", "fx.vignette", [0.0, 0.0]);
        let layout = g.layout(&catalog);
        let geom = layout.nodes.get(&fx).unwrap();
        let (name, pg) = geom.params.first().expect("fx.vignette has an amount");
        assert_eq!(name, "amount");
        // The value field answers `Param`.
        match layout.hit(pg.field.x + pg.field.w / 2.0, pg.field.y + pg.field.h / 2.0) {
            Hit::Param(node, got) => {
                assert_eq!(node, fx);
                assert_eq!(got, "amount");
            }
            other => panic!("a value-field hit is a Param, got {other:?}"),
        }
        // The dot answers a `Port` carrying the parameter port, so a cable
        // dropped on the row resolves to the same shape a port drop does.
        match layout.hit(pg.dot.x, pg.dot.y) {
            Hit::Port(node, port, dir) => {
                assert_eq!(node, fx);
                assert_eq!(port, params::param_port("amount"));
                assert!(!dir.is_out(), "a parameter is an input");
            }
            other => panic!("a param-dot hit is the param port, got {other:?}"),
        }
    }

    /// A binding ends on its parameter's row, not on some port stub: the
    /// param edge's path terminates at the target row's dot.
    #[test]
    fn a_param_edge_ends_on_its_rows_dot() {
        let catalog = real_catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let fx = g.add_node("fx", "fx.vignette", [300.0, 0.0]);
        let edge = crate::bind::bind(
            &mut g,
            &fx,
            "amount",
            Some((&audio, "control", "rms")),
            &catalog,
        )
        .unwrap()
        .expect("a binding mints an edge");
        let layout = g.layout(&catalog);
        let path = layout.edge_path(&edge).expect("a param edge draws");
        let geom = layout.nodes.get(&fx).unwrap();
        let (_, pg) = geom.params.iter().find(|(n, _)| n == "amount").unwrap();
        assert_eq!((path.p3.x, path.p3.y), (pg.dot.x, pg.dot.y), "ends on the row's dot");
        assert!(layout.port_point(&fx, &params::param_port("amount")).is_some());
    }

    /// A generator's tile area is laid out, so the canvas can paint the model
    /// picture inside its box.
    #[test]
    fn a_generator_has_a_tile_area() {
        let catalog = real_catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let layout = g.layout(&catalog);
        assert!(layout.nodes.get(&gen).unwrap().tile.is_some(), "a generator carries its model tile");
    }

    /// A colour config paints a swatch in the body, tagged with its key.
    #[test]
    fn a_colour_config_paints_a_swatch() {
        let catalog = real_catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let id = g.add_node("color", "gen.turing", [0.0, 0.0]);
        g.node_mut(&id).unwrap().config =
            crate::Config::from_json_str(r#"{"rgb": [1.0, 0.0, 0.0]}"#).unwrap();
        let layout = g.layout(&catalog);
        let has_swatch = layout.primitives().into_iter().any(|p| {
            matches!(
                p,
                Primitive::Rect(TaggedRect { tag: RectTag::Swatch { name, rgb, .. }, .. })
                    if name == "rgb" && rgb == [1.0, 0.0, 0.0]
            )
        });
        assert!(has_swatch, "a node holding `rgb` paints one swatch");
    }

    /// A click on a swatch hits its own key (2026-10-08), so the canvas can
    /// open a picker seeded from the colour the chip shows.
    #[test]
    fn a_swatch_click_hits_its_own_key() {
        let catalog = real_catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let id = g.add_node("color", "gen.turing", [0.0, 0.0]);
        g.node_mut(&id).unwrap().config =
            crate::Config::from_json_str(r#"{"rgb": [1.0, 0.0, 0.0]}"#).unwrap();
        let layout = g.layout(&catalog);
        let rect = layout
            .primitives()
            .into_iter()
            .find_map(|p| match p {
                Primitive::Rect(t) => match t.tag {
                    RectTag::Swatch { .. } => Some(t.rect),
                    _ => None,
                },
                _ => None,
            })
            .expect("a swatch rect");
        match layout.hit(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0) {
            Hit::Swatch(node, key) => {
                assert_eq!(node, id);
                assert_eq!(key, "rgb");
            }
            other => panic!("expected a swatch hit, got {other:?}"),
        }
    }

    /// A node's rows stack in order under the header, one `ROW_H` apart: a
    /// parameter's dot and field sit on its own row, the dot on the left edge
    /// and the field's right side inside the box, and each later row is a
    /// `ROW_H` below the one before. A right-aligned extra output (a
    /// generator's `row`) sits on the right edge of its row.
    #[test]
    fn rows_stack_one_row_height_apart_with_the_dot_left_and_field_inside() {
        let catalog = real_catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let fx = g.add_node("fx", "fx.vignette", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [300.0, 0.0]);
        let layout = g.layout(&catalog);

        // fx.vignette's single `amount` row: dot on the left edge, one row
        // below the header's centre; the field's right edge inside the box.
        let rect = layout.node_rect(&fx).unwrap();
        let geom = layout.nodes.get(&fx).unwrap();
        let (_, pg) = geom.params.first().unwrap();
        assert_eq!(pg.dot.x, rect.x, "a param dot sits on the box's left edge");
        assert!(pg.dot.y >= rect.y + HEADER_H, "the row is below the header");
        assert_eq!(
            (pg.dot.y - (rect.y + HEADER_H)) % ROW_H,
            ROW_H / 2.0,
            "a row's centre is a half-row into its band"
        );
        assert!(
            pg.field.x + pg.field.w <= rect.x + rect.w,
            "the value field stays inside the box's right edge"
        );
        assert!(pg.field.contains(Point { x: pg.field.x + 1.0, y: pg.field.y + 1.0 }));

        // The generator's `row` output is a body row on the right edge, and
        // its first parameter row is above it (rows are assembled in order,
        // extra outputs before float params — see `node_geom`).
        let gen_rect = layout.node_rect(&gen).unwrap();
        let row = layout.port_point(&gen, "row").expect("the generator's row output is laid out");
        assert_eq!(row.x, gen_rect.x + gen_rect.w, "an extra output sits on the right edge");
        assert!(row.y >= gen_rect.y + HEADER_H, "it is a body row, not the header");
    }

    /// A generator's long row folds behind a `▸ n more` row, and the persisted
    /// `expanded` flag unfolds it.
    #[test]
    fn a_long_param_list_folds_and_the_expanded_flag_unfolds_it() {
        let catalog = real_catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let params = params::node_params(g.node(&gen).unwrap(), catalog.iter().find(|k| k.kind == "gen.turing"));
        assert!(params.len() > COLLAPSED_ROWS, "turing's row is longer than the fold");
        let folded = g.layout(&catalog);
        assert!(folded.node_folded(&gen));
        let short = folded.node_rect(&gen).unwrap().h;
        g.node_mut(&gen).unwrap().expanded = true;
        let open = g.layout(&catalog);
        assert!(!open.node_folded(&gen));
        assert!(open.node_rect(&gen).unwrap().h > short, "expanding shows more rows");
    }
}
