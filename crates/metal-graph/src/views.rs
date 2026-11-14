//! Gen views: one `gen.*` node, many perspectives.
//!
//! A gen node's `config.views` lists K views. Each view starts from the node's
//! single base row and changes it: a `vary` linear spread across the views, then
//! `add` (scaled by `amount`), then `set` (absolute). Every result is clamped to
//! the param's own min..max. A view with a downstream edge gets its own render
//! on the output port `view.<name>`.
//!
//! Parsing is lenient: a bad entry is skipped, never an error. The camera of a
//! view is read here only as data; resolving it into a 7-float block is the
//! venue's job.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Config, Edge, Endpoint, Graph, Node, NodeId};

/// The output-port prefix of a view on a gen node: `view.<name>`.
pub const VIEW_PORT_PREFIX: &str = "view.";
/// The float a gen node exposes to scale every view's variation at once.
pub const VIEW_AMOUNT_PARAM: &str = "view_amount";

/// Where a view's camera comes from. Untagged: the first shape that matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ViewCamera {
    /// `{"projector": "p1"}`: the venue projector's own camera.
    Projector { projector: String },
    /// `{"surface": "wall-front", "viewer": [x,y,z]}`: a surface seen from a viewer.
    Surface {
        surface: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        viewer: Option<[f32; 3]>,
    },
    /// `{"eye": [..], "target": [..], "fov": f}`: a literal camera.
    Eye { eye: [f32; 3], target: [f32; 3], fov: f32 },
}

/// One view: its name (the port is `view.<name>`), an optional camera and its
/// per-param edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenView {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<ViewCamera>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub add: BTreeMap<String, f32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set: BTreeMap<String, f32>,
}

/// A linear spread of one param across the views: view `k` of `K` gets
/// `amount * spread * (2k/(K-1) - 1)` added to the param.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vary {
    pub param: String,
    pub spread: f32,
}

/// A gen node's whole view block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Views {
    /// Scales every `vary` and `add`. `set` is absolute and ignores it.
    pub amount: f32,
    /// Venue metres map into model space as `(world - anchor) * scale`.
    pub anchor: [f32; 3],
    pub scale: f32,
    pub list: Vec<GenView>,
    pub vary: Vec<Vary>,
    /// The venue id the projector and surface cameras resolve against (JSON key
    /// `venue`). `None` or empty means the venue projected last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
}

impl Default for Views {
    fn default() -> Views {
        Views { amount: 1.0, anchor: [0.0; 3], scale: 1.0, list: Vec::new(), vary: Vec::new(), venue: None }
    }
}

/// The views of a gen node's config (`config["views"]`), or `None` when it has
/// none. Lenient: a malformed view or vary entry is skipped; a view whose name is
/// empty or already taken is dropped (the first one wins).
pub fn views_of(config: &Value) -> Option<Views> {
    let v = config.get("views")?.as_object()?;
    let num = |key: &str| v.get(key).and_then(Value::as_f64).map(|f| f as f32).filter(|f| f.is_finite());
    let mut out = Views::default();
    if let Some(a) = num("amount") {
        out.amount = a;
    }
    if let Some(s) = num("scale") {
        out.scale = s;
    }
    if let Some(a) = v.get("anchor").and_then(|a| serde_json::from_value::<[f32; 3]>(a.clone()).ok()) {
        out.anchor = a;
    }
    out.venue = v.get("venue").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let mut seen = HashSet::new();
    for item in v.get("list").and_then(Value::as_array).into_iter().flatten() {
        let Ok(view) = serde_json::from_value::<GenView>(item.clone()) else { continue };
        if view.name.is_empty() || !seen.insert(view.name.clone()) {
            continue;
        }
        out.list.push(view);
    }
    for item in v.get("vary").and_then(Value::as_array).into_iter().flatten() {
        let Ok(vary) = serde_json::from_value::<Vary>(item.clone()) else { continue };
        if !vary.param.is_empty() && vary.spread.is_finite() {
            out.vary.push(vary);
        }
    }
    Some(out)
}

/// Applies view `k` of `views.list.len()` to `row` in place. `names[i]` is the
/// param name of `row[i]`; `mins`/`maxs` are the same length. Order: `vary`,
/// then `add` (scaled by `amount`), then `set` (absolute). Each touched value is
/// clamped to its param's range. Unknown names are ignored; a `k` past the list
/// changes nothing.
pub fn apply_view(
    row: &mut [f32],
    names: &[&str],
    mins: &[f32],
    maxs: &[f32],
    views: &Views,
    k: usize,
) {
    let count = views.list.len();
    if k >= count {
        return;
    }
    let step = |i: usize, delta: f32, absolute: bool, row: &mut [f32]| {
        let next = if absolute { delta } else { row[i] + delta };
        // `max`/`min` rather than `clamp`: clamp panics when min > max.
        row[i] = next.max(mins[i]).min(maxs[i]);
    };
    let position = |name: &str| names.iter().position(|n| *n == name);

    // Spread position of view k in -1..=1; 0 for a single view.
    let t = if count == 1 { 0.0 } else { 2.0 * k as f32 / (count - 1) as f32 - 1.0 };
    for vary in &views.vary {
        if let Some(i) = position(&vary.param) {
            step(i, views.amount * vary.spread * t, false, row);
        }
    }
    let view = &views.list[k];
    for (name, delta) in &view.add {
        if let Some(i) = position(name) {
            step(i, views.amount * delta, false, row);
        }
    }
    for (name, value) in &view.set {
        if let Some(i) = position(name) {
            step(i, *value, true, row);
        }
    }
}

/// Whether `kind`/`port` is a view output: a `gen.*` node's `view.<name>` with a
/// non-empty name.
pub fn is_view_port(kind: &str, port: &str) -> bool {
    kind.starts_with("gen.") && port.strip_prefix(VIEW_PORT_PREFIX).is_some_and(|n| !n.is_empty())
}

/// One clone made by [`expand_views`]: `clone` renders view `index` (of
/// `count`) of the gen node `source`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewClone {
    pub clone: NodeId,
    pub source: NodeId,
    pub index: usize,
    pub count: usize,
}

/// Expands every wired view of every gen node into a clone node, so the paint
/// path needs no change: the source keeps its row ports, and each clone
/// `<source>~<view>` is a copy of the source (kind, position, config minus
/// `views`, plus `view_of`) that renders one view. The clone id is
/// `<source>~<view>`, and each copied inbound edge is `<edge id>~<view>`.
///
/// The clones exist only at frame time: this runs on the copy of the graph the
/// device render compiles, never on the authored graph, so nothing saves them
/// to disk or into the dashboard's shared state.
///
/// A view is wired when an edge
/// leaves the source on its `view.<name>` port; that edge is moved to the clone's
/// `image` port. Every edge into the source (control, `tex.*`, `param:`) is
/// duplicated onto the clone as `<edge id>~<view>`. Edges out of the source's
/// other ports stay on the source. Unwired views make no clone. A graph with no
/// `view.` edges left is returned unchanged, so running it twice is a no-op.
pub fn expand_views(g: &mut Graph) -> Vec<ViewClone> {
    // Read every source first; the graph is only mutated below.
    let sources: Vec<(NodeId, Views)> = g
        .nodes
        .iter()
        .filter(|n| n.kind.starts_with("gen."))
        .filter_map(|n| {
            let cfg: Value = serde_json::from_str(&n.config.to_json_str()).ok()?;
            Some((n.id.clone(), views_of(&cfg)?))
        })
        .collect();
    let mut made = Vec::new();
    let mut new_nodes: Vec<Node> = Vec::new();
    let mut new_edges: Vec<Edge> = Vec::new();
    for (source, views) in sources {
        let count = views.list.len();
        for (index, view) in views.list.iter().enumerate() {
            let port = format!("{VIEW_PORT_PREFIX}{}", view.name);
            let wired = g.edges.iter().any(|e| e.from.node == source && e.from.port == port);
            if !wired {
                continue;
            }
            let clone_id = format!("{source}~{}", view.name);
            if g.node(&clone_id).is_some() || new_nodes.iter().any(|n| n.id == clone_id) {
                continue;
            }
            let Some(src) = g.node(&source).cloned() else { continue };
            let mut cfg: serde_json::Map<String, Value> =
                serde_json::from_str::<Value>(&src.config.to_json_str())
                    .ok()
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
            cfg.remove("views");
            cfg.insert("view_of".into(), json!({"node": source, "view": view.name}));
            let mut clone = src.clone();
            clone.id = clone_id.clone();
            clone.config = Config::from_value(Value::Object(cfg));
            new_nodes.push(clone);
            // The inbound edges of the source, copied onto the clone.
            for e in g.edges.iter().filter(|e| e.to.node == source) {
                new_edges.push(Edge::new(
                    format!("{}~{}", e.id, view.name),
                    e.from.clone(),
                    Endpoint::new(clone_id.clone(), e.to.port.clone()),
                ));
            }
            // The view's own outgoing edges now read the clone's image.
            for e in g.edges.iter_mut() {
                if e.from.node == source && e.from.port == port {
                    e.from = Endpoint::new(clone_id.clone(), "image");
                }
            }
            made.push(ViewClone { clone: clone_id, source: source.clone(), index, count });
        }
    }
    g.nodes.extend(new_nodes);
    g.edges.extend(new_edges);
    made
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg() -> Value {
        json!({
            "views": {
                "amount": 1.0,
                "anchor": [0, 1.6, 4],
                "scale": 2.0,
                "list": [
                    {"name": "left", "camera": {"projector": "p1"}},
                    {"name": "front", "camera": {"surface": "wall-front", "viewer": [0, 1.6, 4]}},
                    {"name": "right", "camera": {"eye": [1, 0.6, 3], "target": [0, 0, 0], "fov": 50},
                     "add": {"hue": 0.1}, "set": {"seed": 3}}
                ],
                "vary": [{"param": "hue", "spread": 0.3}, {"param": "speed", "spread": 0.2}]
            }
        })
    }

    fn views(list: Vec<GenView>, vary: Vec<Vary>, amount: f32) -> Views {
        Views { amount, list, vary, ..Views::default() }
    }

    fn named(name: &str) -> GenView {
        GenView { name: name.into(), camera: None, add: BTreeMap::new(), set: BTreeMap::new() }
    }

    #[test]
    fn parses_the_plan_shape_with_every_camera_form() {
        let v = views_of(&cfg()).expect("has views");
        assert_eq!(v.amount, 1.0);
        assert_eq!(v.anchor, [0.0, 1.6, 4.0]);
        assert_eq!(v.scale, 2.0);
        let names: Vec<&str> = v.list.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["left", "front", "right"]);
        assert_eq!(v.list[0].camera, Some(ViewCamera::Projector { projector: "p1".into() }));
        assert_eq!(
            v.list[1].camera,
            Some(ViewCamera::Surface { surface: "wall-front".into(), viewer: Some([0.0, 1.6, 4.0]) })
        );
        assert!(matches!(v.list[2].camera, Some(ViewCamera::Eye { fov, .. }) if fov == 50.0));
        assert_eq!(v.list[2].add.get("hue"), Some(&0.1));
        assert_eq!(v.list[2].set.get("seed"), Some(&3.0));
        assert_eq!(v.vary.len(), 2);
    }

    #[test]
    fn no_views_block_is_none_and_defaults_fill_the_rest() {
        assert!(views_of(&json!({"floats": {}})).is_none());
        let v = views_of(&json!({"views": {"list": [{"name": "a"}]}})).unwrap();
        assert_eq!((v.amount, v.scale, v.anchor), (1.0, 1.0, [0.0; 3]));
    }

    #[test]
    fn a_bad_entry_is_skipped_not_an_error() {
        let v = views_of(&json!({"views": {
            "list": [{"name": "ok"}, {"camera": {}}, {"name": ""}, {"name": "bad", "add": {"x": "nope"}}],
            "vary": [{"spread": 1.0}, {"param": "h", "spread": 0.5}]
        }}))
        .unwrap();
        assert_eq!(v.list.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["ok"]);
        assert_eq!(v.vary, vec![Vary { param: "h".into(), spread: 0.5 }]);
    }

    #[test]
    fn duplicate_view_names_keep_the_first() {
        let v = views_of(&json!({"views": {"list": [
            {"name": "a", "add": {"x": 1.0}}, {"name": "a", "add": {"x": 2.0}}, {"name": "b"}
        ]}}))
        .unwrap();
        assert_eq!(v.list.len(), 2);
        assert_eq!(v.list[0].add.get("x"), Some(&1.0));
        assert_eq!(v.list[1].name, "b");
    }

    #[test]
    fn vary_is_a_symmetric_spread_across_three_views() {
        let vy = views(
            vec![named("a"), named("b"), named("c")],
            vec![Vary { param: "hue".into(), spread: 0.3 }],
            1.0,
        );
        let names = ["hue"];
        let (mins, maxs) = ([-10.0], [10.0]);
        let mut offsets = Vec::new();
        for k in 0..3 {
            let mut row = [0.5];
            apply_view(&mut row, &names, &mins, &maxs, &vy, k);
            offsets.push(row[0] - 0.5);
        }
        assert!((offsets[0] + 0.3).abs() < 1e-6, "{offsets:?}");
        assert!(offsets[1].abs() < 1e-6, "{offsets:?}");
        assert!((offsets[2] - 0.3).abs() < 1e-6, "{offsets:?}");
    }

    #[test]
    fn vary_with_one_view_is_no_offset() {
        let vy = views(vec![named("only")], vec![Vary { param: "hue".into(), spread: 0.9 }], 1.0);
        let mut row = [0.5];
        apply_view(&mut row, &["hue"], &[0.0], &[1.0], &vy, 0);
        assert_eq!(row[0], 0.5);
    }

    #[test]
    fn add_is_scaled_by_amount_and_set_is_not() {
        let mut v = named("a");
        v.add.insert("hue".into(), 0.4);
        v.set.insert("seed".into(), 3.0);
        let vy = views(vec![v], vec![], 0.5);
        let mut row = [0.1, 0.0];
        apply_view(&mut row, &["hue", "seed"], &[-1.0, 0.0], &[1.0, 10.0], &vy, 0);
        assert!((row[0] - 0.3).abs() < 1e-6, "add 0.4 at amount 0.5 is 0.2: {row:?}");
        assert_eq!(row[1], 3.0, "set is absolute at any amount");
    }

    #[test]
    fn set_wins_over_add_and_vary_on_the_same_param() {
        let mut v = named("a");
        v.add.insert("hue".into(), 0.4);
        v.set.insert("hue".into(), 0.9);
        let vy = views(vec![v], vec![Vary { param: "hue".into(), spread: 0.2 }], 1.0);
        let mut row = [0.1];
        apply_view(&mut row, &["hue"], &[0.0], &[1.0], &vy, 0);
        assert_eq!(row[0], 0.9);
    }

    #[test]
    fn results_are_clamped_to_the_params_range() {
        let mut v = named("a");
        v.add.insert("hue".into(), 5.0);
        v.set.insert("seed".into(), -4.0);
        // Two views, so view 1 sits at the top of the spread (+spread).
        let vy = views(vec![named("x"), v], vec![Vary { param: "speed".into(), spread: 9.0 }], 1.0);
        let mut row = [0.5, 0.5, 0.5];
        apply_view(&mut row, &["hue", "seed", "speed"], &[0.0, 0.0, 0.0], &[1.0, 1.0, 1.0], &vy, 1);
        assert_eq!(row, [1.0, 0.0, 1.0]);
    }

    #[test]
    fn unknown_names_and_out_of_range_k_change_nothing() {
        let mut v = named("a");
        v.add.insert("nope".into(), 1.0);
        let vy = views(vec![v], vec![Vary { param: "gone".into(), spread: 1.0 }], 1.0);
        let mut row = [0.5];
        apply_view(&mut row, &["hue"], &[0.0], &[1.0], &vy, 0);
        assert_eq!(row[0], 0.5);
        apply_view(&mut row, &["hue"], &[0.0], &[1.0], &vy, 7);
        assert_eq!(row[0], 0.5);
    }

    #[test]
    fn view_ports_are_gen_only_with_a_nonempty_name() {
        assert!(is_view_port("gen.chamber", "view.left"));
        assert!(!is_view_port("gen.chamber", "view."));
        assert!(!is_view_port("gen.chamber", "image"));
        assert!(!is_view_port("fx.shake", "view.left"));
        assert!(!is_view_port("gen.chamber", "tex.skin"));
    }

    fn expand_graph() -> Graph {
        use crate::{Domain, Endpoint};
        let mut g = Graph::new("g", "g", Domain::Live);
        let src = g.add_node("gen", "gen.chamber", [1.0, 2.0]);
        let node = g.node_mut(&src).unwrap();
        node.config = Config::from_json_str(
            &json!({"floats": {"hue": 0.5}, "views": {"list": [
                {"name": "left", "camera": {"projector": "p1"}},
                {"name": "right", "set": {"seed": 3}}
            ], "vary": [{"param": "hue", "spread": 0.2}]}}).to_string(),
        )
        .unwrap();
        let ctl = g.add_node("ctl", "control.knob", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [5.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&ctl, "value"), Endpoint::new(&src, "param:hue"));
        g.add_edge(Endpoint::new(&src, "view.left"), Endpoint::new(&fx, "image"));
        g.add_edge(Endpoint::new(&src, "row"), Endpoint::new(&out, "image"));
        g
    }

    #[test]
    fn only_the_wired_view_gets_a_clone_and_its_edge_moves() {
        let mut g = expand_graph();
        let made = expand_views(&mut g);
        assert_eq!(made, vec![ViewClone { clone: "gen-0~left".into(), source: "gen-0".into(), index: 0, count: 2 }]);
        let clone = g.node("gen-0~left").expect("clone pushed");
        assert_eq!(clone.kind, "gen.chamber");
        assert_eq!(clone.pos, [1.0, 2.0]);
        assert!(clone.config.get("views").is_none());
        assert_eq!(clone.config.get("view_of"), Some(&json!({"node": "gen-0", "view": "left"})));
        assert!(g.node("gen-0~right").is_none(), "unwired view has no clone");
        // The view edge now comes from the clone's image.
        assert!(g.edges.iter().any(|e| e.from == Endpoint::new("gen-0~left", "image") && e.to.node == "fx-0"));
        // The row edge stays on the source.
        assert!(g.edges.iter().any(|e| e.from == Endpoint::new("gen-0", "row") && e.to.node == "out-0"));
    }

    #[test]
    fn inbound_edges_are_duplicated_onto_the_clone() {
        let mut g = expand_graph();
        expand_views(&mut g);
        let dup = g.edges.iter().find(|e| e.to == Endpoint::new("gen-0~left", "param:hue")).expect("param copied");
        assert_eq!(dup.from, Endpoint::new("ctl-0", "value"));
        assert_ne!(dup.id, g.edges.iter().find(|e| e.to == Endpoint::new("gen-0", "param:hue")).unwrap().id);
        assert_eq!(dup.id, format!("{}~left", g.edges.iter().find(|e| e.to.node == "gen-0" && e.to.port == "param:hue").unwrap().id));
    }

    #[test]
    fn expanding_twice_is_a_no_op() {
        let mut g = expand_graph();
        expand_views(&mut g);
        let (nodes, edges) = (g.nodes.len(), g.edges.len());
        assert!(expand_views(&mut g).is_empty());
        assert_eq!((g.nodes.len(), g.edges.len()), (nodes, edges));
    }
}
