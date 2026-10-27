//! The canvas's press hit-test, answered on the UI thread from the `GET /graph`
//! reply the canvas already paints.
//!
//! The answer must be the one `GET /graph/hit` gives (`call::graph::hit_reply`
//! over `metal_graph::Layout::hit`). Every rule here is that rule, read off the
//! `primitives` the same reply carries, in the same priority order:
//!
//! 1. ports and param dots, in `(node, port, dir)` order — a port within
//!    [`PORT_R`] of the point wins;
//! 2. parameter value fields, then colour swatches;
//! 3. cables, within [`EDGE_HIT_TOLERANCE`] of their 32-step sampled curve,
//!    skipping any cable whose endpoint is an embedded chip;
//! 4. node boxes (inclusive edges), else empty.
//!
//! The primitives hold each rect's own corners, so nothing here re-lays out
//! the graph. If the reply does not describe the graph it claims to (a missing
//! primitive, a node count that disagrees), [`local_hit`] answers `None` and the
//! caller asks the door instead.

use std::collections::{BTreeMap, BTreeSet};

use metal_graph::layout::{EDGE_HIT_TOLERANCE, PORT_R};
use serde_json::{json, Value};

/// A port's key, as `Layout` keys it: `(node, port, out)`. `false` < `true`
/// is the same order `Dir::In < Dir::Out` gives.
type PortKey = (String, String, bool);

#[derive(Clone, Copy)]
struct Box4 {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Box4 {
    /// `Rect::contains`: both edges count.
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    fn json(&self) -> Value {
        json!([self.x, self.y, self.w, self.h])
    }
}

/// The hit at `(x, y)` in graph space, in `GET /graph/hit`'s reply shape, or
/// `None` when `reply` (a `GET /graph` reply) cannot answer for its own graph.
pub fn local_hit(reply: &Value, x: f32, y: f32) -> Option<Value> {
    let prims = reply["primitives"].as_array()?;
    let nodes = reply["graph"]["nodes"].as_array()?;

    let mut ports: BTreeMap<PortKey, [f32; 2]> = BTreeMap::new();
    let mut fields: Vec<(String, String, Box4)> = Vec::new();
    let mut swatches: Vec<(String, String, Box4)> = Vec::new();
    let mut paths: Vec<(String, String, String, [[f32; 2]; 4])> = Vec::new();
    let mut chips: BTreeSet<String> = BTreeSet::new();
    let mut boxes: Vec<(String, Box4)> = Vec::new();

    for p in prims {
        match (p["type"].as_str()?, p["tag"].as_str()) {
            ("rect", Some("node")) => boxes.push((str_of(p, "node")?, rect_of(p)?)),
            ("rect", Some("port")) => {
                let key = (str_of(p, "node")?, str_of(p, "port")?, p["out"].as_bool()?);
                ports.insert(key, centre_of(p)?);
            }
            // The dot is a port named `param:<name>` (`params::param_port`),
            // an input, inserted after the node's own ports — the order
            // `Layout::layout` inserts them in.
            ("rect", Some("param_dot")) => {
                let name = str_of(p, "name")?;
                let key = (str_of(p, "node")?, format!("param:{name}"), false);
                ports.insert(key, centre_of(p)?);
            }
            ("rect", Some("value_field")) => {
                fields.push((str_of(p, "node")?, str_of(p, "name")?, rect_of(p)?));
            }
            ("rect", Some("swatch")) => {
                swatches.push((str_of(p, "node")?, str_of(p, "name")?, rect_of(p)?));
            }
            ("rect", Some("chip")) => {
                chips.insert(str_of(p, "node")?);
            }
            ("path", _) => {
                let pt = |k: &str| -> Option<[f32; 2]> {
                    Some([p[k][0].as_f64()? as f32, p[k][1].as_f64()? as f32])
                };
                paths.push((
                    str_of(p, "edge")?,
                    str_of(&p["from"], "node")?,
                    str_of(&p["to"], "node")?,
                    [pt("p0")?, pt("p1")?, pt("p2")?, pt("p3")?],
                ));
            }
            _ => {}
        }
    }

    // The reply must describe every free node it lists: one node box per
    // node that is not an embedded chip. Anything else is a stale or partial
    // reply, and only the door can answer for the disk.
    let free = nodes.iter().filter(|n| n["embedded_in"].is_null()).count();
    if boxes.len() != free {
        return None;
    }

    // Ports first (any port, a param dot included).
    for ((node, port, out), pt) in &ports {
        let d = ((x - pt[0]).powi(2) + (y - pt[1]).powi(2)).sqrt();
        if d <= PORT_R {
            return Some(json!({
                "hit": "port",
                "node": node,
                "port": port,
                "dir": if *out { "out" } else { "in" },
                "point": port_point(&ports, node, port).map(|p| json!([p[0], p[1]])),
            }));
        }
    }

    for (node, name, r) in &fields {
        if r.contains(x, y) {
            let dot = port_point(&ports, node, &format!("param:{name}"));
            return Some(json!({
                "hit": "param",
                "node": node,
                "name": name,
                "point": dot.map(|p| json!([p[0], p[1]])),
            }));
        }
    }

    for (node, name, r) in &swatches {
        if r.contains(x, y) {
            return Some(json!({ "hit": "swatch", "node": node, "name": name }));
        }
    }

    for (edge, from, to, c) in &paths {
        // A cable into or out of an embedded chip is not hit where it is not drawn.
        if chips.contains(from) || chips.contains(to) {
            continue;
        }
        if curve_distance(x, y, c) <= EDGE_HIT_TOLERANCE {
            return Some(json!({ "hit": "edge", "edge": edge }));
        }
    }

    for (node, r) in &boxes {
        if r.contains(x, y) {
            return Some(json!({ "hit": "node", "node": node, "rect": r.json() }));
        }
    }

    Some(json!({ "hit": "empty" }))
}

/// `Layout::port_point`: the first entry for `(node, port)` in key order,
/// whichever direction it has.
fn port_point(ports: &BTreeMap<PortKey, [f32; 2]>, node: &str, port: &str) -> Option<[f32; 2]> {
    ports
        .iter()
        .find(|((n, p, _), _)| n == node && p == port)
        .map(|(_, pt)| *pt)
}

/// `Path::distance_to`: the distance from `(x, y)` to the curve's 32-step
/// polyline. The arithmetic is `metal_graph::layout::Path::sample` and
/// `distance_point_to_segment`, term for term, so the f32 results agree.
fn curve_distance(x: f32, y: f32, c: &[[f32; 2]; 4]) -> f32 {
    let [p0, p1, p2, p3] = *c;
    let steps: usize = 32;
    let pts: Vec<[f32; 2]> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let mt = 1.0 - t;
            let px = mt * mt * mt * p0[0]
                + 3.0 * mt * mt * t * p1[0]
                + 3.0 * mt * t * t * p2[0]
                + t * t * t * p3[0];
            let py = mt * mt * mt * p0[1]
                + 3.0 * mt * mt * t * p1[1]
                + 3.0 * mt * t * t * p2[1]
                + t * t * t * p3[1];
            [px, py]
        })
        .collect();
    pts.windows(2)
        .map(|w| segment_distance(x, y, w[0], w[1]))
        .fold(f32::INFINITY, f32::min)
}

fn segment_distance(px: f32, py: f32, a: [f32; 2], b: [f32; 2]) -> f32 {
    let abx = b[0] - a[0];
    let aby = b[1] - a[1];
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((px - a[0]) * abx + (py - a[1]) * aby) / len2).clamp(0.0, 1.0)
    };
    let cx = a[0] + abx * t;
    let cy = a[1] + aby * t;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v[key].as_str().map(str::to_string)
}

fn rect_of(p: &Value) -> Option<Box4> {
    Some(Box4 {
        x: p["x"].as_f64()? as f32,
        y: p["y"].as_f64()? as f32,
        w: p["w"].as_f64()? as f32,
        h: p["h"].as_f64()? as f32,
    })
}

/// A dot's centre as the primitive carries it (`cx`/`cy`).
fn centre_of(p: &Value) -> Option<[f32; 2]> {
    Some([p["cx"].as_f64()? as f32, p["cy"].as_f64()? as f32])
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::{Endpoint, Graph, NodeKind};

    fn catalog() -> Vec<NodeKind> {
        let text = metal_vfs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json"),
        )
        .expect("the repo schema.json reads");
        NodeKind::catalog(&metal_schema::Manifest::parse(&text).expect("the repo schema.json parses"))
    }

    /// Asserts, at every point, that the local answer is the server's (the
    /// same `hit_reply` over the same layout), and returns the variants seen.
    fn assert_matches_server(g: &Graph, cat: &[NodeKind], points: &[[f32; 2]]) -> BTreeSet<String> {
        let reply = crate::graph_reply(g, cat);
        let mut seen = BTreeSet::new();
        for &[x, y] in points {
            let server = crate::graph_hit_reply(g, cat, x, y);
            let local = local_hit(&reply, x, y)
                .unwrap_or_else(|| panic!("a fresh reply must answer locally at ({x}, {y})"));
            assert_eq!(local, server, "hit at ({x}, {y})");
            seen.insert(server["hit"].as_str().unwrap_or_default().to_string());
        }
        seen
    }

    fn at(c: &[[f32; 2]; 4], t: f32, axis: usize) -> f32 {
        let mt = 1.0 - t;
        mt * mt * mt * c[0][axis] + 3.0 * mt * mt * t * c[1][axis] + 3.0 * mt * t * t * c[2][axis] + t * t * t * c[3][axis]
    }

    /// A probe set derived from the reply itself: a grid over the node boxes
    /// (padded), every node corner, every port and param dot centre with its
    /// 6-unit ring, and points on each cable's curve just inside and just
    /// outside the 4-unit tolerance.
    fn probe_points(reply: &Value) -> Vec<[f32; 2]> {
        let prims = reply["primitives"].as_array().unwrap();
        let f = |v: &Value| v.as_f64().unwrap() as f32;
        let mut pts = Vec::new();
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in prims.iter().filter(|p| p["tag"] == "node") {
            let (x, y, w, h) = (f(&p["x"]), f(&p["y"]), f(&p["w"]), f(&p["h"]));
            pts.extend([[x, y], [x + w, y], [x, y + h], [x + w, y + h]]);
            lo = [lo[0].min(x), lo[1].min(y)];
            hi = [hi[0].max(x + w), hi[1].max(y + h)];
        }
        let mut x = lo[0] - 20.0;
        while x <= hi[0] + 20.0 {
            let mut y = lo[1] - 20.0;
            while y <= hi[1] + 20.0 {
                pts.push([x, y]);
                y += 1.5;
            }
            x += 1.5;
        }
        for p in prims {
            if p["tag"] == "port" || p["tag"] == "param_dot" {
                let (cx, cy) = (f(&p["cx"]), f(&p["cy"]));
                for d in [0.0, 5.99, 6.01, -6.0, -6.01] {
                    pts.push([cx + d, cy]);
                    pts.push([cx, cy + d]);
                }
            }
            if p["type"] == "path" {
                let c: [[f32; 2]; 4] = ["p0", "p1", "p2", "p3"].map(|k| [f(&p[k][0]), f(&p[k][1])]);
                for i in 0..=8 {
                    let t = i as f32 / 8.0;
                    let (bx, by) = (at(&c, t, 0), at(&c, t, 1));
                    for d in [3.9, 4.1, -3.9, -4.1] {
                        pts.push([bx, by + d]);
                        pts.push([bx + d, by]);
                    }
                }
            }
        }
        pts
    }

    /// Two overlapping nodes: the second box covers part of the first.
    fn overlapping() -> Graph {
        let mut g = Graph::new("overlap", "overlap", metal_graph::Domain::Live);
        let a = g.add_node("a", "gen.turing", [0.0, 0.0]);
        let b = g.add_node("b", "fx.shake", [60.0, 30.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new(&b, "image"));
        g
    }

    /// Two nodes apart with a cable between them, so the curve crosses empty canvas.
    fn cable() -> Graph {
        let mut g = Graph::new("cable", "cable", metal_graph::Domain::Live);
        let a = g.add_node("a", "gen.turing", [0.0, 0.0]);
        let b = g.add_node("b", "fx.shake", [300.0, 140.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new(&b, "image"));
        g
    }

    /// A colour node (swatch) and an effect with parameter rows (value fields).
    fn colour_and_params() -> Graph {
        let mut g = Graph::new("swatch", "swatch", metal_graph::Domain::Live);
        let c = g.add_node("c", metal_graph::COLOR_KIND, [0.0, 0.0]);
        if let Some(n) = g.node_mut(&c) {
            n.config = metal_graph::Config::from_json_str(r#"{"rgb":[1.0,0.5,0.2]}"#).expect("a colour config parses");
        }
        g.add_node("f", "fx.shake", [260.0, 0.0]);
        g
    }

    /// An embedded chip: a free node whose box is not drawn as a node.
    fn with_chip() -> Graph {
        let mut g = Graph::new("chip", "chip", metal_graph::Domain::Live);
        let a = g.add_node("a", "gen.turing", [0.0, 0.0]);
        let c = g.add_node("c", "fx.shake", [0.0, 0.0]);
        if let Some(n) = g.node_mut(&c) {
            n.embedded_in = Some(a.clone());
        }
        g
    }

    #[test]
    fn local_hit_matches_the_server_on_every_fixture() {
        let cat = catalog();
        let mut seen = BTreeSet::new();
        for g in [overlapping(), cable(), colour_and_params(), with_chip()] {
            let pts = probe_points(&crate::graph_reply(&g, &cat));
            seen.extend(assert_matches_server(&g, &cat, &pts));
        }
        for variant in ["node", "port", "edge", "empty", "param", "swatch"] {
            assert!(seen.contains(variant), "the fixtures should exercise {variant}; saw {seen:?}");
        }
    }

    #[test]
    fn empty_graph_and_empty_canvas_answer_empty() {
        let cat = catalog();
        let none = Graph::new("none", "none", metal_graph::Domain::Live);
        let reply = crate::graph_reply(&none, &cat);
        assert_eq!(local_hit(&reply, 10.0, 10.0), Some(json!({ "hit": "empty" })));

        let seen = assert_matches_server(&overlapping(), &cat, &[[-500.0, -500.0], [900.0, 900.0]]);
        assert_eq!(seen, BTreeSet::from(["empty".to_string()]));
    }

    #[test]
    fn a_reply_that_does_not_describe_its_graph_falls_back() {
        let cat = catalog();
        let mut reply = crate::graph_reply(&overlapping(), &cat);
        assert!(local_hit(&reply, 0.0, 0.0).is_some());

        // Two free nodes in the graph, one box in the reply: stale or partial.
        let prims = reply["primitives"].as_array_mut().unwrap();
        let at = prims.iter().position(|p| p["tag"] == "node").unwrap();
        prims.remove(at);
        assert_eq!(local_hit(&reply, 0.0, 0.0), None);

        // A bare graph with no primitives is never answered as empty.
        let bare = json!({ "graph": reply["graph"].clone() });
        assert_eq!(local_hit(&bare, 0.0, 0.0), None);
    }
}
