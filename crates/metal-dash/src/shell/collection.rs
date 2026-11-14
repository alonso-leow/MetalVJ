//! Which screen a node on the open canvas draws on.
//!
//! The canvas edits a **collection** (`metal_graph::collection`): its
//! `output.device` nodes each name a screen in `config.device`. A panel that
//! needs a device — a preview, the venue, a mask — asks here: the screen a
//! `device-<id>` graph is named after, or else the screen of the output the
//! node feeds (the first one when it feeds several or none).

use serde_json::Value;

/// The screen for `node` in the graph reply `graph` (`GET /graph`'s body) open
/// under `gid`. `node` is `None` for "the graph as a whole".
pub fn device_of(gid: &str, graph: &Value, node: Option<&str>) -> Option<String> {
    if let Some(device) = gid.strip_prefix("device-") {
        return Some(device.to_string());
    }
    let nodes = graph["graph"]["nodes"].as_array()?;
    let outputs: Vec<(&str, &str)> = nodes
        .iter()
        .filter(|n| n["kind"].as_str() == Some("output.device"))
        .filter_map(|n| Some((n["id"].as_str()?, n["config"]["device"].as_str().filter(|d| !d.is_empty())?)))
        .collect();
    let device_of_output = |id: &str| outputs.iter().find(|(o, _)| *o == id).map(|(_, d)| d.to_string());
    if let Some(start) = node {
        if let Some(own) = device_of_output(start) {
            return Some(own);
        }
        // Down the cables to the first output with a screen.
        let edges = graph["graph"]["edges"].as_array().cloned().unwrap_or_default();
        let mut queue = std::collections::VecDeque::from([start.to_string()]);
        let mut seen = std::collections::HashSet::new();
        while let Some(cur) = queue.pop_front() {
            if !seen.insert(cur.clone()) {
                continue;
            }
            for e in edges.iter().filter(|e| e["from"]["node"].as_str() == Some(cur.as_str())) {
                if let Some(to) = e["to"]["node"].as_str() {
                    if let Some(d) = device_of_output(to) {
                        return Some(d);
                    }
                    queue.push_back(to.to_string());
                }
            }
        }
    }
    outputs.first().map(|(_, d)| d.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn show() -> Value {
        json!({ "graph": {
            "nodes": [
                { "id": "gen-0", "kind": "gen.turing" },
                { "id": "fx-0", "kind": "fx.shake" },
                { "id": "out-0", "kind": "output.device", "config": { "device": "A" } },
                { "id": "out-1", "kind": "output.device", "config": { "device": "B" } }
            ],
            "edges": [
                { "from": { "node": "gen-0", "port": "image" }, "to": { "node": "out-0", "port": "image" } },
                { "from": { "node": "fx-0", "port": "image" }, "to": { "node": "out-1", "port": "image" } }
            ]
        } })
    }

    #[test]
    fn a_device_graph_is_named_after_its_screen() {
        assert_eq!(device_of("device-HDMI", &json!({}), None).as_deref(), Some("HDMI"));
    }

    #[test]
    fn a_node_draws_on_the_screen_of_the_output_it_feeds() {
        assert_eq!(device_of("col-show", &show(), Some("gen-0")).as_deref(), Some("A"));
        assert_eq!(device_of("col-show", &show(), Some("fx-0")).as_deref(), Some("B"));
        assert_eq!(device_of("col-show", &show(), Some("out-1")).as_deref(), Some("B"));
    }

    #[test]
    fn an_unwired_node_falls_back_to_the_first_screen() {
        let g = show();
        assert_eq!(device_of("col-show", &g, Some("nowhere")).as_deref(), Some("A"));
        assert_eq!(device_of("col-show", &g, None).as_deref(), Some("A"));
    }
}
