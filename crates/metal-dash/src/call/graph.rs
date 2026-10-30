//! D1/B4/B5: the graph editor's mutation surface and its sixteen effects.
//! None of these touch `State` — a graph lives on disk under `data/graphs/`
//! and is read fresh on every request (see `main.rs::load_graph`'s own
//! note on why, next to `outputs`/`collections`/`sequences`/`reels`, which
//! do not) — so every handler here ignores `Ctx::state`, with two
//! exceptions: `graph_node`'s `"mode"` op (O.4) reads `state.collections` to
//! refuse a mode pointing at a collection that does not exist, and
//! `graph_get` (S.AL.1) attaches each `out.sacn` node's live wire status
//! (`state.wire_sockets`) — the one piece of a reply this file returns that
//! is not fully determined by the file on disk, because it is not about the
//! graph at all, it is about the running engine's own socket for a node in
//! it.

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

pub fn graphs(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(json!({ "graphs": crate::list_graphs() }))
}

pub fn graph_get(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(id) = params.param("id") else {
        return Err(CallError::Bad("which graph".into()));
    };
    let g = match crate::load_graph(id) {
        Ok(g) => g,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let catalog = match crate::node_catalog() {
        Ok(c) => c,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let mut reply = crate::graph_reply(&g, &catalog);
    // S.AL.1: the properties panel's "last sent N ms ago" for an `out.sacn`
    // node needs a live read of the running engine's own socket cache
    // (rule 4 — never a second, cached copy of it) — attached here, fresh
    // on every request, rather than the panel keeping any state of its own.
    if let Some(nodes) = reply["graph"]["nodes"].as_array_mut() {
        for n in nodes.iter_mut() {
            if !n["kind"].as_str().is_some_and(metal_perform::wire::is_net_dmx) {
                continue;
            }
            let Some(node_id) = n["id"].as_str().map(str::to_string) else {
                continue;
            };
            if let Some(wire) = metal_perform::wire::status_json(&ctx.state.wire_sockets, &node_id) {
                n["wire"] = wire;
            }
        }
    }
    listen_issues(&mut reply, &g, ctx.sim);
    Ok(reply)
}

/// Plan "Audio input is a node", Design 6: an `audio.live` whose source will
/// not open is red — `listen` on the node (the pool's live readout, read
/// fresh, rule 4) — and a lint warning, so a graph with a dead input says so
/// where the operator is looking. A warning, not an error: the graph is
/// fine, the room's cable is not.
fn listen_issues(reply: &mut Value, g: &metal_graph::Graph, pool: &crate::listeners::Listeners) {
    let status = pool.status();
    if let Some(nodes) = reply["graph"]["nodes"].as_array_mut() {
        for n in nodes.iter_mut().filter(|n| n["kind"] == json!(crate::listeners::AUDIO_LIVE)) {
            let Some(s) = status.iter().find(|s| s.graph == g.id && Some(s.node.as_str()) == n["id"].as_str()) else {
                continue;
            };
            n["listen"] = json!({
                "real": s.real,
                "describe": s.describe,
                "bpm": s.bpm,
                "dropped": s.dropped,
                "error": s.error,
            });
        }
    }
    let Some(issues) = reply["issues"].as_array_mut() else { return };
    for s in status.iter().filter(|s| s.graph == g.id) {
        if let Some(e) = &s.error {
            issues.push(json!({
                "severity": "warning",
                "node": s.node,
                "message": format!("audio source {:?} did not open: {e}", s.want.as_pick()),
            }));
        }
    }
}

pub fn graph_hit(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(id) = params.param("id") else {
        return Err(CallError::Bad("which graph".into()));
    };
    let g = match crate::load_graph(id) {
        Ok(g) => g,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let catalog = match crate::node_catalog() {
        Ok(c) => c,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let x = params.num("x", 0.0);
    let y = params.num("y", 0.0);
    Ok(hit_reply(&g.layout(&catalog), x, y))
}

/// The `GET /graph/hit` answer for a press at `(x, y)` in graph space: which
/// thing `Layout::hit` resolves, in the reply's shape. Its own function so the
/// shell's local hit-test (`shell::graph_hit`) is checked against this exact
/// code, not a copy of it.
pub fn hit_reply(layout: &metal_graph::layout::Layout, x: f32, y: f32) -> Value {
    match layout.hit(x, y) {
        metal_graph::Hit::Node(id) => {
            let rect = layout.node_rect(&id);
            json!({ "hit": "node", "node": id, "rect": rect.map(|r| json!([r.x, r.y, r.w, r.h])) })
        }
        metal_graph::Hit::Port(node, port, dir) => {
            let point = layout.port_point(&node, &port);
            json!({
                "hit": "port",
                "node": node,
                "port": port,
                "dir": if dir.is_out() { "out" } else { "in" },
                "point": point.map(|p| json!([p.x, p.y])),
            })
        }
        // N: a parameter's value field *or* its dot both answer `param` — the
        // "play this row" target and the drop target a cable binds into. The
        // panel learns which row from `name`, and where it sits from `point`
        // (the dot) so a param-to-param stretch has an origin without the
        // panel owning any geometry.
        metal_graph::Hit::Param(node, name) => {
            let point = layout.port_point(&node, &metal_graph::params::param_port(&name));
            json!({
                "hit": "param",
                "node": node,
                "name": name,
                "point": point.map(|p| json!([p.x, p.y])),
            })
        }
        metal_graph::Hit::Swatch(node, name) => {
            json!({ "hit": "swatch", "node": node, "name": name })
        }
        metal_graph::Hit::Edge(id) => json!({ "hit": "edge", "edge": id }),
        metal_graph::Hit::Empty => json!({ "hit": "empty" }),
    }
}

pub fn graph_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let catalog = match crate::node_catalog() {
        Ok(c) => c,
        Err(e) => return Err(CallError::Bad(e)),
    };
    match body["op"].as_str().unwrap_or_default() {
        "create" => {
            let Some(id) = body["id"].as_str().filter(|s| !s.is_empty()) else {
                return Err(CallError::Bad("which id".into()));
            };
            if metal_vfs::exists(&crate::graph_path(id)) {
                return Err(CallError::Bad(format!("{id:?} already exists")));
            }
            let name = body["name"].as_str().unwrap_or(id).to_string();
            let domain = match body["domain"].as_str() {
                Some("offline") => metal_graph::Domain::Offline,
                _ => metal_graph::Domain::Live,
            };
            // There is one live collection (`col-main`): a screen is an
            // `output.device` node in it, so a second live canvas would be a
            // second copy of the show. Offline graphs are unaffected.
            if domain == metal_graph::Domain::Live && id != metal_graph::collection::CENTRAL_ID {
                return Err(CallError::Bad(format!(
                    "there is one live collection ({}): add {id:?}'s nodes to it, and a screen as an output node there",
                    metal_graph::collection::CENTRAL_ID
                )));
            }
            let mut g = metal_graph::Graph::new(id, &name, domain);
            // A collection can start with its first screen: one output node
            // naming it (`metal_graph::collection`).
            if let Some(device) = body["device"].as_str().filter(|d| !d.is_empty()) {
                let out = g.add_node("out", "output.device", [800.0, 0.0]);
                if let Some(node) = g.node_mut(&out) {
                    metal_graph::collection::bind_node(node, device);
                }
            }
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        // O.8: one owner per graph. `take` refuses a graph another peer
        // owns; `release` only by the owner; `owner` reports. A graph with
        // no owner is free, as before.
        op @ ("take" | "release" | "owner") => {
            let Some(id) = body["id"].as_str().filter(|s| !s.is_empty()) else {
                return Err(CallError::Bad("which id".into()));
            };
            let dir = crate::graphs_dir(&crate::output::data_root());
            let me = metal_graph::owner::local_identity();
            match op {
                "take" => metal_graph::owner::take(&dir, id, &me).map(|_| ()),
                "release" => metal_graph::owner::release(&dir, id, &me),
                _ => Ok(()),
            }
            .map_err(|r| CallError::Bad(r.to_string()))?;
            Ok(serde_json::from_str(&metal_graph::owner::status_json(&dir, id, &me)).unwrap_or(Value::Null))
        }
        "delete" => {
            let Some(id) = body["id"].as_str() else {
                return Err(CallError::Bad("which id".into()));
            };
            crate::check_graph_owner_at(&crate::output::data_root(), id).map_err(CallError::Bad)?;
            match metal_vfs::remove_file(crate::graph_path(id)) {
                Ok(()) => Ok(json!({ "deleted": id })),
                Err(e) => Err(CallError::Bad(format!("{id:?}: {e}"))),
            }
        }
        other => Err(CallError::Bad(format!("no such op: {other}"))),
    }
}

/// F9: the device graph a bare `venue.fixture` `add` lands in — the active
/// venue's first projector's device graph. `None` when the body is not a
/// `venue.fixture` add or the venue has no projector: the caller then needs an
/// explicit `graph`.
fn fixture_default_graph(body: &Value) -> Option<String> {
    if body["kind"].as_str() != Some(metal_graph::VENUE_FIXTURE_KIND) {
        return None;
    }
    let root = crate::output::data_root();
    let id = metal_perform::venue::last(&root)?;
    let v = metal_perform::venue::load(&root, &id).ok()?;
    let output = v.projectors.first()?.output.clone();
    Some(crate::device_graph_id(&output))
}

/// F9: where a new `venue.fixture` node lands when the caller gives no `pos`:
/// just under the venue/group node it is a view of, else under the device's
/// output node.
fn fixture_node_spot(g: &metal_graph::Graph) -> (f32, f32) {
    let anchor = g
        .nodes
        .iter()
        .find(|n| n.kind == metal_graph::VENUE_FIXTURES_KIND)
        .or_else(|| g.nodes.iter().find(|n| n.kind == "output.device"));
    match anchor {
        Some(n) => (n.pos[0], n.pos[1] + 140.0),
        None => (0.0, 0.0),
    }
}

/// F9: wire the device's compose output — whatever already feeds
/// `output.device`'s `image` — into `node`'s `image`, so its default `frame`
/// colour works at once. A no-op when the device has no compose source yet, or
/// the node is already fed.
fn autowire_fixture_image(g: &mut metal_graph::Graph, node: &str) {
    let source = g
        .nodes
        .iter()
        .find(|n| n.kind == "output.device")
        .and_then(|out| {
            g.edges
                .iter()
                .find(|e| e.to.node == out.id && e.to.port == "image")
                .map(|e| e.from.clone())
        });
    if let Some(src) = source {
        if !g.edges.iter().any(|e| e.to.node == node && e.to.port == "image") {
            g.add_edge(src, metal_graph::Endpoint::new(node, "image"));
        }
    }
}

/// The video paths a `reel.batch` config names, resolved: an absolute path as
/// is, a relative one under the data root (the same resolve `reel_batch` uses).
/// It walks `cfg["items"]` (an array of `{kind, path, …}`) and keeps only each
/// entry whose `kind == "video"` and whose `path` is a non-empty string —
/// stills (`image`) and scenes/groups have no proxy to build, so they are
/// skipped.
fn reel_batch_paths(cfg: &Value, root: &std::path::Path) -> Vec<String> {
    cfg.get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|i| i.get("kind").and_then(Value::as_str) == Some("video"))
                .filter_map(|i| i.get("path").and_then(Value::as_str))
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let p = std::path::Path::new(p);
                    let resolved = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
                    resolved.to_string_lossy().into_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Rule 6: a clip reaching the graph is a clip reaching ingest, whoever put it
/// there — the shell's paste and the AI's tool both POST this route. A
/// `reel.clip` node's `file` (or `clip`) path is queued for its proxy build;
/// a `reel.batch` node queues each video its `items` names ([`reel_batch_paths`]),
/// with stills and scene/group entries left alone because they have no proxy.
fn enqueue_clip_ingest(g: &metal_graph::Graph, id: &str) {
    let Some(n) = g.node(id) else { return };
    if n.kind != "reel.clip" && n.kind != "reel.batch" {
        return;
    }
    let cfg: Value = serde_json::from_str(&n.config.to_json_str()).unwrap_or(Value::Null);
    if n.kind == "reel.batch" {
        for path in reel_batch_paths(&cfg, &crate::output::data_root()) {
            crate::call::author::ingest::enqueue(&path);
        }
        return;
    }
    if let Some(path) = cfg.get("file").or_else(|| cfg.get("clip")).and_then(Value::as_str) {
        crate::call::author::ingest::enqueue(path);
    }
}

pub fn graph_node(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    // F9: a `venue.fixture` `add` may omit `graph`; the device graph of the
    // active venue's projector is the one the node is a view of, so the AI
    // tool `add_fixture_node` needs no graph argument.
    let gid: String = match body["graph"].as_str() {
        Some(g) => g.to_string(),
        None => fixture_default_graph(&body).ok_or_else(|| CallError::Bad("which graph".into()))?,
    };
    let mut g = match crate::load_graph(&gid) {
        Ok(g) => g,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let catalog = match crate::node_catalog() {
        Ok(c) => c,
        Err(e) => return Err(CallError::Bad(e)),
    };
    match body["op"].as_str().unwrap_or_default() {
        "add" => {
            let Some(kind) = body["kind"].as_str() else {
                return Err(CallError::Bad("which kind".into()));
            };
            if !catalog.iter().any(|k| k.kind == kind) {
                return Err(CallError::Bad(format!("no such node kind: {kind}")));
            }
            // F9: a `venue.fixture` node is placed near the venue/group node
            // and gets its `image` autowired from the device's own compose
            // output (whatever already feeds `output.device.image`), so the
            // default `frame` colour works the moment it is added. The
            // explicit `pos` still wins when the caller gives one.
            let has_pos = body["pos"].as_array().is_some_and(|p| p.len() == 2);
            let (x, y) = if has_pos {
                (body["pos"][0].as_f64().unwrap_or(0.0) as f32, body["pos"][1].as_f64().unwrap_or(0.0) as f32)
            } else if kind == metal_graph::VENUE_FIXTURE_KIND {
                fixture_node_spot(&g)
            } else {
                (0.0, 0.0)
            };
            let prefix = kind.split('.').next().unwrap_or("n");
            let id = g.add_node(prefix, kind, [x, y]);
            if let Some(seed) = metal_perform::script_preset::seed_config(kind) {
                if let (Some(n), Ok(cfg)) = (g.node_mut(&id), serde_json::to_string(&seed)) {
                    if let Ok(cfg) = metal_graph::Config::from_json_str(&cfg) {
                        n.config = cfg;
                    }
                }
            }
            // F9: a fresh `venue.fixture` starts from its declared defaults
            // (`colour_from: frame`, bus `priority`/`mode`), so a new node
            // changes nothing visible until it is pointed at a row.
            if kind == metal_graph::VENUE_FIXTURE_KIND {
                if let Some(n) = g.node_mut(&id) {
                    if let Ok(cfg) = metal_graph::Config::from_json_str(
                        &metal_graph::params::venue_fixture_default_config().to_string(),
                    ) {
                        n.config = cfg;
                    }
                }
            }
            // F9: the caller's own `config` object (the pointer `{venue,
            // fixture}` and, for the AI, `colour_from`) merges over the seed.
            if let Some(extra) = body.get("config").and_then(|c| c.as_object()) {
                if let Some(n) = g.node_mut(&id) {
                    let mut cur: serde_json::Map<String, Value> =
                        serde_json::from_str(&n.config.to_json_str()).unwrap_or_default();
                    for (k, v) in extra {
                        cur.insert(k.clone(), v.clone());
                    }
                    if let Ok(cfg) = metal_graph::Config::from_json_str(&Value::Object(cur).to_string()) {
                        n.config = cfg;
                    }
                }
            }
            if kind == metal_graph::VENUE_FIXTURE_KIND {
                autowire_fixture_image(&mut g, &id);
            }
            enqueue_clip_ingest(&g, &id);
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            let mut reply = crate::graph_reply(&g, &catalog);
            reply["node"] = json!(id);
            Ok(reply)
        }
        "move" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let Some(n) = g.node_mut(id) else {
                return Err(CallError::Bad(format!("no such node: {id}")));
            };
            n.pos = [
                body["pos"][0].as_f64().unwrap_or(0.0) as f32,
                body["pos"][1].as_f64().unwrap_or(0.0) as f32,
            ];
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        "remove" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            // An embedded `ai.*` is owned by its source: deleting the source
            // deletes it and its wires too (`ai.embed`). A detached one
            // (`embedded_in = None`) is a normal node and stays.
            let owned: Vec<String> = g
                .nodes
                .iter()
                .filter(|n| n.embedded_in.as_deref() == Some(id))
                .map(|n| n.id.clone())
                .collect();
            g.remove_node(id);
            for o in owned {
                g.remove_node(&o);
            }
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        "flag" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let field = body["field"].as_str().unwrap_or_default().to_string();
            match field.as_str() {
                // `S.AK` point 2: the same one-entry-batch mutator
                // `POST /switch {op: on|off}` calls, so a single node's
                // bypass and a switch's set/unset can never drift into two
                // copies of "what does turning a node off mean".
                "bypass" => {
                    let Some(current) = g.node(id).map(|n| n.bypass) else {
                        return Err(CallError::Bad(format!("no such node: {id}")));
                    };
                    let want_bypass = body["on"].as_bool().unwrap_or(!current);
                    let entries = [crate::switch::SwitchEntry::node(id.to_string(), !want_bypass)];
                    let now = ctx.state.time();
                    ctx.state.transitions.stamp(&g, &entries, now);
                    crate::switch::apply(&mut g, &entries);
                }
                "solo" => {
                    let Some(n) = g.node_mut(id) else {
                        return Err(CallError::Bad(format!("no such node: {id}")));
                    };
                    n.solo = body["on"].as_bool().unwrap_or(!n.solo);
                }
                other => {
                    return Err(CallError::Bad(format!(
                        "no such flag: {other} — expected bypass or solo"
                    )))
                }
            }
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        "duplicate" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let Some(src) = g.nodes.iter().find(|n| n.id == id).cloned() else {
                return Err(CallError::Bad(format!("no such node: {id}")));
            };
            let prefix = src.kind.split('.').next().unwrap_or("n");
            let new_id = g.add_node(prefix, &src.kind, [src.pos[0] + 24.0, src.pos[1] + 24.0]);
            if let Some(n) = g.node_mut(&new_id) {
                n.config = src.config.clone();
                n.label = src.label.clone();
            }
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            let mut reply = crate::graph_reply(&g, &catalog);
            reply["node"] = json!(new_id);
            Ok(reply)
        }
        "layer" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let Some(n) = g.node_mut(id) else {
                return Err(CallError::Bad(format!("no such node: {id}")));
            };
            if !n.kind.starts_with("gen.") {
                return Err(CallError::Bad(format!(
                    "{id} is a {}, not a gen.* node — layer config only applies to generators",
                    n.kind
                )));
            }
            let mut cfg = crate::node_layer::NodeLayerConfig::from_node(n);
            if let Some(v) = body.get("blend") {
                cfg.blend = match v {
                    Value::Null => None,
                    Value::String(s) => match crate::layers::Blend::parse(s) {
                        Some(b) => Some(b),
                        None => return Err(CallError::Bad(format!("no such blend: {s}"))),
                    },
                    _ => return Err(CallError::Bad("blend must be a string or null".into())),
                };
            }
            if let Some(v) = body.get("z") {
                cfg.z = match v {
                    Value::Null => None,
                    _ => match v.as_i64() {
                        Some(z) => Some(z as i32),
                        None => return Err(CallError::Bad("z must be an integer or null".into())),
                    },
                };
            }
            if let Some(v) = body.get("sector") {
                cfg.sector = match v {
                    Value::Null => None,
                    Value::Object(_) => {
                        let (Some(x), Some(y), Some(w), Some(h)) = (
                            v["x"].as_f64(),
                            v["y"].as_f64(),
                            v["w"].as_f64(),
                            v["h"].as_f64(),
                        ) else {
                            return Err(CallError::Bad("sector needs x/y/w/h".into()));
                        };
                        Some(
                            crate::layers::Sector {
                                x: x as f32,
                                y: y as f32,
                                w: w as f32,
                                h: h as f32,
                            }
                            .clamped(),
                        )
                    }
                    _ => return Err(CallError::Bad("sector must be an object or null".into())),
                };
            }
            if let Some(v) = body.get("mask") {
                cfg.mask = match v {
                    Value::Null => None,
                    Value::String(s) => Some(s.clone()),
                    _ => return Err(CallError::Bad("mask must be a string or null".into())),
                };
            }
            if let Some(v) = body.get("opacity") {
                cfg.opacity = match v {
                    Value::Null => None,
                    _ => match v.as_f64() {
                        Some(o) => Some(o as f32),
                        None => {
                            return Err(CallError::Bad("opacity must be a number or null".into()))
                        }
                    },
                };
            }
            // The generator's loop choice (`loop`, the loops design notes T7) is not a
            // layer field: keep it across the rewrite.
            n.config = cfg
                .to_config()
                .carrying(&n.config, "loop")
                .carrying(&n.config, metal_perform::drive::CONFIG_KEY);
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        // O.4's writer. `main.rs::graph_device_mode` reads a `stack.mode`
        // node's config as `layers::DeviceMode::describe`'s JSON, one step
        // before the layer list exists; this is the only route that writes
        // it, in that exact vocabulary (`DeviceMode::parse` on the way in,
        // `describe` on the way out), so the node, this op and the
        // operator's dropdown (`POST /layers { op: "mode" }`) cannot spell a
        // mode three ways. Same shape as `"layer"` above — one op, one node
        // kind, refuses every other — and the same `no such collection`
        // guard `apply_layers_op`'s dropdown arm keeps: a graph pointing at
        // a collection that does not exist is caught here, at selection
        // time, not as a black frame with an audience watching.
        "mode" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let Some(kind) = body["kind"].as_str() else {
                return Err(CallError::Bad(
                    "which mode — automix, disabled, own or collection".into(),
                ));
            };
            let Some(mode) = crate::layers::DeviceMode::parse(kind, body["collection"].as_str())
            else {
                return Err(CallError::Bad(
                    "automix, disabled, own or collection (with `collection`: id)".into(),
                ));
            };
            if let crate::layers::DeviceMode::Collection(ref cid) = mode {
                if ctx.state.collections.find(cid).is_none() {
                    return Err(CallError::Bad(format!("no such collection: {cid}")));
                }
            }
            let Some(n) = g.node(id) else {
                return Err(CallError::Bad(format!("no such node: {id}")));
            };
            if !accepts_device_mode(&n.kind) {
                return Err(CallError::Bad(format!(
                    "{} is a {}, not a composition node — the device mode lives on that \
                     node and nowhere else",
                    id, n.kind
                )));
            }
            // LAY.7b: the node is rewritten *in place* as the compose kind the
            // mode maps to (a legacy `stack.mode` node is converted, keeping
            // its id), and `Disabled` retires it — §2.4's bridge table,
            // applied by the one writer `main.rs::set_graph_device_mode_at`
            // rather than spelled a second time here.
            crate::set_graph_device_mode_at(&mut g, Some(id), &mode).map_err(CallError::Bad)?;
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        // S.W: write any node's free-form config. The text panel posts the
        // whole text object (or one `fx.text.*` amount); this merges it over
        // whatever is there so a partial write cannot erase a field it did
        // not mean to touch, and the reader's `#[serde(default)]` fills the
        // rest.
        //
        // `S.AL.3`: `floats` is one key with two shapes. An *array* is the
        // pre-existing, heavier op — redeclaring a `script.*` node's floats
        // themselves — and stays a plain merge below. An *object*
        // (`{"<name>": value}`), on a `script.*` node only, is the
        // properties panel's slider turning a knob in real time: it goes
        // through `metal_graph::script_floats::apply_floats_patch`, the same
        // validated writer `metal_agent::harness::Harness::set_params` calls
        // for the AI's route — one real writer, not two that could drift on
        // what counts as in range (rule 4).
        "config" | "text" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            // A generator's `floats` object (`{"<float>": value | null}`) is
            // its held floats — the same one validated writer `op: "param"`
            // and the AI's `set_params` go through (`metal_graph::params::
            // set_held`), one float at a time, never a blind merge that would
            // drop the floats this patch does not name.
            let gen_floats = body
                .get("config")
                .and_then(|c| c.get("floats"))
                .or_else(|| body.get("floats"))
                .and_then(Value::as_object)
                .filter(|_| g.node(id).is_some_and(|n| n.kind.starts_with("gen.")))
                .cloned();
            for (name, value) in gen_floats.iter().flatten() {
                let value = held_value(value)?;
                metal_graph::params::set_held(&mut g, id, name, value, &catalog).map_err(CallError::Bad)?;
            }
            let Some(n) = g.node_mut(id) else {
                return Err(CallError::Bad(format!("no such node: {id}")));
            };
            let kind = n.kind.clone();
            let mut current: serde_json::Map<String, Value> =
                serde_json::from_str::<Value>(&n.config.to_json_str())
                    .ok()
                    .and_then(|v| v.as_object().cloned())
                    .unwrap_or_default();
            let patch: serde_json::Map<String, Value> =
                if let Some(obj) = body.get("config").and_then(|c| c.as_object()) {
                    obj.clone()
                } else if let Some(obj) = body.as_object() {
                    obj.iter()
                        .filter(|(k, _)| {
                            k.as_str() != "graph" && k.as_str() != "op" && k.as_str() != "node"
                        })
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                } else {
                    serde_json::Map::new()
                };
            for (k, v) in patch {
                if k == "floats" && kind.starts_with("gen.") && v.is_object() {
                    // Written above, held float by held float.
                    continue;
                }
                if k == "floats" && metal_graph::is_script_kind(&kind) && v.is_object() {
                    let mut cfg_value = Value::Object(current.clone());
                    metal_graph::script_floats::apply_floats_patch(
                        &mut cfg_value,
                        v.as_object().expect("checked is_object above"),
                    )
                    .map_err(CallError::Bad)?;
                    current = cfg_value.as_object().cloned().expect("still an object");
                } else {
                    current.insert(k, v);
                }
            }
            n.config = metal_graph::Config::from_json_str(&Value::Object(current).to_string())
                .map_err(|e| CallError::Bad(e.to_string()))?;
            enqueue_clip_ingest(&g, id);
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        // Bindings (`metal_graph::bind`): `{op: "bind", graph, node, param,
        // from: {node, port, var} | null}` replaces — or, with `null`, removes —
        // the one edge driving `node`'s parameter `param`. The reply carries
        // the new edge's id (`null` after an unbind).
        "bind" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let Some(param) = body["param"].as_str() else {
                return Err(CallError::Bad("which param".into()));
            };
            let from = match body.get("from") {
                None => {
                    return Err(CallError::Bad(
                        "`from`: {node, port, var} — or null to unbind".into(),
                    ))
                }
                Some(Value::Null) => None,
                Some(f) => match (f["node"].as_str(), f["port"].as_str(), f["var"].as_str()) {
                    (Some(n), Some(p), Some(v)) => Some((n, p, v)),
                    _ => {
                        return Err(CallError::Bad(
                            "`from` needs node, port and var — or null to unbind".into(),
                        ))
                    }
                },
            };
            let edge = metal_graph::bind::bind(&mut g, id, param, from, &catalog).map_err(CallError::Bad)?;
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            let mut reply = crate::graph_reply(&g, &catalog);
            reply["edge"] = json!(edge);
            Ok(reply)
        }
        // One write for a parameter's held value: `{op: "param", graph, node,
        // name, value: number | null}` — an effect's fixed amount, a text
        // effect's amount, a script float's value, a generator's held float
        // (`null` releases it). `metal_graph::params::set_held` is the writer;
        // an unknown name or an out-of-range value is refused by name.
        "param" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let Some(name) = body["name"].as_str() else {
                return Err(CallError::Bad("which parameter (`name`)".into()));
            };
            let Some(value) = body.get("value") else {
                return Err(CallError::Bad("`value`: a number, or null to release".into()));
            };
            let value = held_value(value)?;
            metal_graph::params::set_held(&mut g, id, name, value, &catalog).map_err(CallError::Bad)?;
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        // A batch of held values: `{op: "params", graph, updates: [{node,
        // name, value: number | null}]}`. Every update goes through the one
        // validated writer (`metal_graph::params::set_held_all`), all or
        // nothing, and the graph is saved once — a scene press sets 20 floats
        // in one write, not 20.
        "params" => {
            let Some(list) = body["updates"].as_array() else {
                return Err(CallError::Bad("`updates`: a list of {node, name, value}".into()));
            };
            let mut updates = Vec::with_capacity(list.len());
            for u in list {
                let Some(node) = u["node"].as_str() else {
                    return Err(CallError::Bad("each update names its `node`".into()));
                };
                let Some(name) = u["name"].as_str() else {
                    return Err(CallError::Bad("each update names its `name`".into()));
                };
                let value = held_value(u.get("value").unwrap_or(&Value::Null))?;
                updates.push((node.to_string(), name.to_string(), value));
            }
            metal_graph::params::set_held_all(&mut g, &updates, &catalog).map_err(CallError::Bad)?;
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        // Surface sources (plan "Surface sources and less-friction
        // authoring", step 2). `attach` wires an image source into one image
        // input of a node: an existing node (`from`), a new one of `kind`
        // placed to the node's left, or nothing (`detach`). Whatever fed that
        // port is replaced. `{graph, node, port, kind?, from?, config?}`.
        "attach" => {
            let Some(node) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let Some(port) = body["port"].as_str() else {
                return Err(CallError::Bad("which port".into()));
            };
            let Some(target) = g.node(node).map(|n| (n.kind.clone(), n.pos)) else {
                return Err(CallError::Bad(format!("no such node: {node}")));
            };
            if !port_is(&catalog, &target.0, port, Side::Input) {
                return Err(CallError::Bad(format!("{} has no image input {port:?}", target.0)));
            }
            let kind = body["kind"].as_str().filter(|s| !s.is_empty());
            let from = body["from"].as_str().filter(|s| !s.is_empty());
            // The sources that fed this port before: one left with no outgoing
            // edge once the port is rewired is an orphan and goes with it.
            let previous: Vec<String> = g
                .edges
                .iter()
                .filter(|e| e.to.node == node && e.to.port == port)
                .map(|e| e.from.node.clone())
                .collect();
            g.edges.retain(|e| !(e.to.node == node && e.to.port == port));
            let source = match (from, kind) {
                (Some(f), _) => {
                    let Some(src) = g.node(f).map(|n| n.kind.clone()) else {
                        return Err(CallError::Bad(format!("no such node: {f}")));
                    };
                    if !port_is(&catalog, &src, "image", Side::Output) {
                        return Err(CallError::Bad(format!("{src} has no image output")));
                    }
                    Some(f.to_string())
                }
                (None, Some(k)) => {
                    if !port_is(&catalog, k, "image", Side::Output) {
                        return Err(CallError::Bad(format!("{k} is not a source with an image output")));
                    }
                    let pos = spot_near(&g, target.1, -220.0);
                    Some(add_kind(&mut g, k, pos, body.get("config"))?)
                }
                (None, None) => None,
            };
            if let Some(src) = &source {
                crate::add_edge_checked(
                    &mut g,
                    metal_graph::Endpoint::new(src, "image"),
                    metal_graph::Endpoint::new(node, port),
                    &catalog,
                )
                .map_err(|errors| CallError::Bad(errors.join("; ")))?;
            }
            for prev in previous {
                let orphan = Some(prev.as_str()) != from
                    && source.as_deref() != Some(prev.as_str())
                    && g.edges.iter().all(|e| e.from.node != prev);
                if orphan {
                    g.remove_node(&prev);
                }
            }
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            let mut reply = crate::graph_reply(&g, &catalog);
            reply["node"] = json!(source);
            Ok(reply)
        }
        // `insert` puts `kind` between `after` and every consumer of its
        // image: each edge leaving `(after, image)` now leaves the new node,
        // and `(after, image) → (new, image)` is added. `{graph, after, kind}`.
        "insert" => {
            let Some(after) = body["after"].as_str() else {
                return Err(CallError::Bad("which after".into()));
            };
            let Some(kind) = body["kind"].as_str() else {
                return Err(CallError::Bad("which kind".into()));
            };
            let Some((after_kind, after_pos)) = g.node(after).map(|n| (n.kind.clone(), n.pos)) else {
                return Err(CallError::Bad(format!("no such node: {after}")));
            };
            if !port_is(&catalog, &after_kind, "image", Side::Output) {
                return Err(CallError::Bad(format!("{after_kind} has no image output")));
            }
            if !port_is(&catalog, kind, "image", Side::Input) || !port_is(&catalog, kind, "image", Side::Output) {
                return Err(CallError::Bad(format!("{kind} is not an image-to-image node")));
            }
            let pos = spot_near(&g, after_pos, 220.0);
            let new = add_kind(&mut g, kind, pos, None)?;
            // Move the consumers first, so the new link below is the only
            // edge that still leaves `after`.
            for e in g.edges.iter_mut().filter(|e| e.from.node == after && e.from.port == "image") {
                e.from.node = new.clone();
            }
            crate::add_edge_checked(
                &mut g,
                metal_graph::Endpoint::new(after, "image"),
                metal_graph::Endpoint::new(&new, "image"),
                &catalog,
            )
            .map_err(|errors| CallError::Bad(errors.join("; ")))?;
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            let mut reply = crate::graph_reply(&g, &catalog);
            reply["node"] = json!(new);
            Ok(reply)
        }
        // Plan "AI embedded in the source's properties" (A1): the reaction is
        // a setting in the source, not a node the operator wires by hand.
        // `{op: "ai.embed", graph, node, ai, on}`. `on` creates the `ai.*`
        // node with `embedded_in = Some(source)` and the one chain
        // `audio → ai → source.control`; `off` is `detach` — the owned node
        // keeps its wires and becomes a normal, free node (config is a reading
        // of the graph, never a second state — rule 4).
        "ai.embed" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let Some(ai) = body["ai"].as_str() else {
                return Err(CallError::Bad("which ai".into()));
            };
            let Some(source) = g.node(id) else {
                return Err(CallError::Bad(format!("no such node: {id}")));
            };
            let source_kind = source.kind.clone();
            let source_pos = source.pos;
            let on = body["on"].as_bool().unwrap_or(true);

            if !on {
                for n in g.nodes.iter_mut() {
                    if n.embedded_in.as_deref() == Some(id) && n.kind == ai {
                        n.embedded_in = None;
                    }
                }
                if let Err(e) = crate::save_graph(&g) {
                    return Err(CallError::Bad(e));
                }
                return Ok(crate::graph_reply(&g, &catalog));
            }

            // Which AIs a source offers comes from its kind (plan A1.3):
            // every `gen.*` offers `ai.colourist`. `ai.router`/`ai.curator`
            // are not built yet — they slot in here when they are, and no
            // caller should invent their names before then.
            let offered = offered_ai_for(&source_kind);
            if !offered.contains(&ai) {
                return Err(CallError::Bad(format!("{source_kind} does not offer {ai}")));
            }
            if !catalog.iter().any(|k| k.kind == ai) {
                return Err(CallError::Bad(format!("no such node kind: {ai}")));
            }
            // One owned node per (source, ai): a repeat is a no-op, so the
            // toggle cannot mint a second chip. The reply still names the one
            // that is already there.
            if let Some(existing) = g
                .nodes
                .iter()
                .find(|n| n.embedded_in.as_deref() == Some(id) && n.kind == ai)
                .map(|n| n.id.clone())
            {
                if let Err(e) = crate::save_graph(&g) {
                    return Err(CallError::Bad(e));
                }
                let mut reply = crate::graph_reply(&g, &catalog);
                reply["node"] = json!(existing);
                return Ok(reply);
            }

            let audio = embed_audio_node(&mut g, id);
            let ai_id = g.add_node("ai", ai, [source_pos[0] + 24.0, source_pos[1] + 24.0]);
            if let Some(n) = g.node_mut(&ai_id) {
                n.embedded_in = Some(id.to_string());
                // A2: sensible defaults — no manual config. Pin the newest
                // trained weights for the model this reaction drives, as the
                // *name* the runtime's `ai_weights` named branch resolves
                // (`<data_root>/reels/<name>.json`), or `null` when there is
                // none: the runtime then falls back to the downstream model's
                // own default and says "untrained", exactly as today. No
                // second copy of any state — the name is a reading of the
                // directory the trainer writes.
                if ai == metal_graph::AI_COLOURIST_KIND {
                    let model = source_kind.strip_prefix("gen.").unwrap_or("");
                    let weights = newest_weights(&crate::output::data_root(), ai, model);
                    n.config =
                        metal_graph::Config::from_json_str(&json!({ "weights": weights }).to_string())
                            .map_err(|e| CallError::Bad(e.to_string()))?;
                }
            }
            // The one chain the reaction is. The source's own `control`
            // cable (from the audio node the chain reuses) is folded into
            // `ai`, so the port is never driven twice.
            g.edges.retain(|e| !(e.to.node == id && e.to.port == "control"));
            g.add_edge(
                metal_graph::Endpoint::new(&audio, "control"),
                metal_graph::Endpoint::new(&ai_id, "control"),
            );
            g.add_edge(
                metal_graph::Endpoint::new(&ai_id, "control"),
                metal_graph::Endpoint::new(id, "control"),
            );
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            let mut reply = crate::graph_reply(&g, &catalog);
            reply["node"] = json!(ai_id);
            Ok(reply)
        }
        // `load script…` (2026-10-10): a `.wgsl` file becomes the `shader`
        // of a node. The file is read here, not in the shell (the shell does
        // no I/O); the naga gate checks it, it is saved as
        // `data/shaders/<stem>.wgsl`, and the node's config names it — the
        // reply is the graph, so the shell needs no second call.
        "shader" => {
            let Some(id) = body["node"].as_str() else {
                return Err(CallError::Bad("which node".into()));
            };
            let Some(path) = body["path"].as_str() else {
                return Err(CallError::Bad("which file".into()));
            };
            if g.node(id).is_none() {
                return Err(CallError::Bad(format!("no such node: {id}")));
            }
            let stem = std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let wgsl = std::fs::read_to_string(path)
                .map_err(|e| CallError::Bad(format!("{path}: {e}")))?;
            save_runtime_shader(&stem, &wgsl)?;
            if let Some(n) = g.node_mut(id) {
                let mut cfg: serde_json::Map<String, Value> =
                    serde_json::from_str::<Value>(&n.config.to_json_str())
                        .ok()
                        .and_then(|v| v.as_object().cloned())
                        .unwrap_or_default();
                cfg.insert("shader".into(), json!(stem));
                n.config = metal_graph::Config::from_json_str(&Value::Object(cfg).to_string())
                    .map_err(|e| CallError::Bad(e.to_string()))?;
            }
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            let mut reply = crate::graph_reply(&g, &catalog);
            reply["shader"] = json!(stem);
            Ok(reply)
        }
        other => Err(CallError::Bad(format!("no such op: {other}"))),
    }
}

/// Which side of a node a port is on, for [`port_is`].
#[derive(Clone, Copy)]
enum Side {
    Input,
    Output,
}

/// Whether `kind` (a catalog kind) has an image port named `port` on `side`.
/// An unknown kind has no ports.
fn port_is(catalog: &[metal_graph::NodeKind], kind: &str, port: &str, side: Side) -> bool {
    catalog.iter().find(|k| k.kind == kind).is_some_and(|k| {
        let ports = match side {
            Side::Input => &k.inputs,
            Side::Output => &k.outputs,
        };
        ports.iter().any(|p| p.name == port && p.ty == metal_graph::PortType::Image)
    })
}

/// The first free spot `anchor.x + dx, anchor.y + 40·k` (k = 0, 1, …): no
/// node within the reach `free_spot` uses. A node made beside another never
/// lands on top of a third.
fn spot_near(g: &metal_graph::Graph, anchor: [f32; 2], dx: f32) -> [f32; 2] {
    let mut k = 0usize;
    loop {
        let pos = [anchor[0] + dx, anchor[1] + 40.0 * k as f32];
        let clear = g
            .nodes
            .iter()
            .all(|n| (n.pos[0] - pos[0]).abs() >= 100.0 || (n.pos[1] - pos[1]).abs() >= 60.0);
        if clear {
            return pos;
        }
        k += 1;
    }
}

/// Adds a node of `kind` at `pos`, seeded like `op: "add"` does, and then
/// `config` (when given) replaces the seed. Returns its id.
fn add_kind(
    g: &mut metal_graph::Graph,
    kind: &str,
    pos: [f32; 2],
    config: Option<&Value>,
) -> Result<String, CallError> {
    let prefix = kind.split('.').next().unwrap_or("n");
    let id = g.add_node(prefix, kind, pos);
    if let Some(seed) = metal_perform::script_preset::seed_config(kind) {
        if let (Some(n), Ok(cfg)) = (g.node_mut(&id), serde_json::to_string(&seed)) {
            if let Ok(cfg) = metal_graph::Config::from_json_str(&cfg) {
                n.config = cfg;
            }
        }
    }
    if let Some(cfg) = config.filter(|c| !c.is_null()) {
        let parsed = metal_graph::Config::from_json_str(&cfg.to_string())
            .map_err(|e| CallError::Bad(e.to_string()))?;
        if let Some(n) = g.node_mut(&id) {
            n.config = parsed;
        }
    }
    Ok(id)
}

/// The `ai.*` kinds a source of `kind` offers as an embedded reaction (plan
/// A1.3). Only `ai.colourist` exists as a node kind today; every `gen.*`
/// offers it. When `ai.router` and `ai.curator` are built, a `router` source
/// offers `ai.router` and a `reel.*` source offers `ai.curator` — added here,
/// the one place that answers the question, so no caller spells the mapping
/// a second time.
fn offered_ai_for(kind: &str) -> &'static [&'static str] {
    if kind.starts_with("gen.") {
        &[metal_graph::AI_COLOURIST_KIND]
    } else {
        &[]
    }
}

/// The newest trained weights for an `ai.*` reaction (plan A2.1), as the
/// **name** the runtime's `ai_weights` named branch resolves: a file
/// `<data_root>/reels/<name>.json` for the colourist. Only a grade fitted for
/// the model this reaction drives counts — `Grade::fits` refuses the rest, so
/// pinning another model's grade would make the reaction silently do nothing
/// — and among those the newest by mtime wins. `None` when there is none,
/// which leaves `weights: null` (the runtime's own per-model default, the file
/// the trainer writes) and its "untrained" fallback as today.
fn newest_weights(root: &std::path::Path, ai: &str, model: &str) -> Option<String> {
    // A2.2: `ai.router` reads `data/router/gate.json` for the current song and
    // its weights live under `data/router/`; the node is not built yet, so
    // this arm answers nothing until it is. TODO(A2.2).
    if ai != metal_graph::AI_COLOURIST_KIND {
        return None;
    }
    let dir = root.join(metal_reel::index::DIR);
    let prefix = format!("colourist-{model}");
    let mut best: Option<(web_time::SystemTime, String)> = None;
    for entry in metal_vfs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem != prefix && !stem.starts_with(&format!("{prefix}-")) {
            continue;
        }
        let at = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(web_time::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(t, _)| at > *t) {
            best = Some((at, stem.to_string()));
        }
    }
    best.map(|(_, name)| name)
}

/// The audio node an embedded chain hangs off (plan A1.2): the graph's only
/// `audio.live`; if there are several, the one the source already listens to;
/// if there is none, a new `audio.live {source: null}` on the default input.
/// Mirrors `metal_graph::migrate::wire_to_audio`'s choice of node and position.
fn embed_audio_node(g: &mut metal_graph::Graph, source: &str) -> String {
    let live: Vec<String> = g
        .nodes
        .iter()
        .filter(|n| n.kind == "audio.live")
        .map(|n| n.id.clone())
        .collect();
    if let [only] = live.as_slice() {
        return only.clone();
    }
    if let Some(from) = g
        .edges
        .iter()
        .find(|e| {
            e.to.node == source
                && e.to.port == "control"
                && g.node(&e.from.node).is_some_and(|n| n.kind.starts_with("audio."))
        })
        .map(|e| e.from.node.clone())
    {
        return from;
    }
    if let Some(id) = g
        .nodes
        .iter()
        .find(|n| n.kind == "audio.live" || n.kind == "audio.file")
        .map(|n| n.id.clone())
    {
        return id;
    }
    let x = g.nodes.iter().map(|n| n.pos[0]).fold(f32::INFINITY, f32::min);
    let x = if x.is_finite() { x - 240.0 } else { 0.0 };
    let id = g.add_node("audio", "audio.live", [x, 0.0]);
    if let Some(n) = g.node_mut(&id) {
        n.config = metal_graph::Config::from_json_str(r#"{"source":null}"#)
            .expect("a literal config always parses");
    }
    id
}

/// A held value off a request body: a number, or `null` to release it.
fn held_value(v: &Value) -> Result<Option<f32>, CallError> {
    match v {
        Value::Null => Ok(None),
        v => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| CallError::Bad(format!("a held value is a number or null, not {v}"))),
    }
}

/// `GET /graph/io?id=<graph>&node=<node>` — one node's whole I/O, read off
/// the graph file and, while the graph is being rendered, the engine's last
/// frame (`State::live`): the cables in (`inputs`) and out (`outputs`), its
/// parameters with their binding and live value (`params`), and every upstream
/// node's variables with whether this node uses them (`sources`). The shape is
/// `metal_graph::bind::NodeIo`'s, plus `graph`; the AI's `get_io` answers the
/// same read.
pub fn graph_io(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(id) = params.param("id") else {
        return Err(CallError::Bad("which graph".into()));
    };
    let Some(node) = params.param("node") else {
        return Err(CallError::Bad("which node".into()));
    };
    let g = crate::load_graph(id).map_err(CallError::Bad)?;
    let catalog = crate::node_catalog().map_err(CallError::Bad)?;
    let live = live_frame(&ctx.state.live, &g).map(|l| l as &dyn metal_graph::bind::LiveValues);
    let io = metal_graph::bind::io(&g, node, &catalog, live)
        .ok_or_else(|| CallError::Bad(format!("no such node: {node}")))?;
    let mut reply = serde_json::to_value(&io).map_err(|e| CallError::Internal(e.to_string()))?;
    reply["graph"] = json!(id);
    Ok(reply)
}

/// `GET /graph/live?id=<graph>` — every node's values in the last frame the
/// engine drew this graph: `{graph, t, age_ms, nodes: {<node>: {in: {param:
/// value}, out: {var: value}}}}`. `in` is each parameter's effective value
/// (bound, held or driven), `out` each variable's. A graph nobody renders
/// answers `t: null` and no nodes; `age_ms` tells a reader polling at 4–10 Hz
/// that the graph stopped being drawn.
pub fn graph_live(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(id) = params.param("id") else {
        return Err(CallError::Bad("which graph".into()));
    };
    let graph = crate::load_graph(id).ok();
    let frame = match &graph {
        Some(g) => live_frame(&ctx.state.live, g),
        None => ctx.state.live.graph(id),
    };
    Ok(match frame {
        Some(frame) => {
            let mut reply = frame.to_json();
            reply["graph"] = json!(id);
            reply
        }
        None => json!({ "graph": id, "t": null, "age_ms": null, "nodes": {} }),
    })
}

/// The last frame the engine drew for `g`. The engine draws the published
/// `device-<screen>` graphs, not a collection, so a collection answers with the
/// frame of the first screen it feeds that has been drawn (the nodes share ids
/// across a collection's screens, `metal_graph::collection`).
fn live_frame<'a>(live: &'a metal_perform::bind::Live, g: &metal_graph::Graph) -> Option<&'a metal_perform::bind::LiveGraph> {
    live.graph(&g.id).or_else(|| {
        metal_graph::collection::bound_devices(g)
            .iter()
            .find_map(|device| live.graph(&metal_graph::collection::device_graph_id(device)))
    })
}

/// Whether a node kind may carry the device mode: a `compose.*` kind (LAY.7b)
/// or the legacy `stack.mode` node it superseded. Anything else — a `gen.*`
/// node's config is `NodeLayerConfig`'s — would be silently ignored by
/// everything that reads the mode, so the op refuses it by name.
pub(crate) fn accepts_device_mode(kind: &str) -> bool {
    kind == metal_graph::STACK_MODE_KIND || metal_graph::is_compose_kind(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::DeviceMode;

    /// The op's writer is `main.rs::set_graph_device_mode_at`, and what it
    /// writes is exactly what `graph_device_mode` parses — every mode,
    /// through the writer, reads back as itself off the output's config
    /// (R22.5b).
    #[test]
    fn every_device_mode_written_by_the_route_reads_back_as_itself() {
        let mut g = metal_graph::Graph::new("device-x", "device-x", metal_graph::Domain::Live);
        let out = g.add_node("out", "output.device", [800.0, 0.0]);
        for mode in [
            DeviceMode::Automix,
            DeviceMode::Own,
            DeviceMode::Collection("noche".to_string()),
        ] {
            crate::set_graph_device_mode_at(&mut g, Some(&out), &mode).unwrap();
            assert_eq!(
                crate::graph_device_mode(&g),
                Some(mode.clone()),
                "{mode:?} did not survive the write"
            );
        }
        // Disabled is written too — a screen switched off — and reads back.
        crate::set_graph_device_mode_at(&mut g, Some(&out), &DeviceMode::Disabled).unwrap();
        assert_eq!(crate::graph_device_mode(&g), Some(DeviceMode::Disabled));
        assert!(
            g.nodes.iter().all(|n| !metal_graph::is_compose_kind(&n.kind)),
            "no compose node is ever minted"
        );
    }

    /// R22.5b: a legacy `stack.mode` node pointed at by the op is retired and
    /// the mode lands on the output's own config — the graph never carries a
    /// second place for the mode.
    #[test]
    fn a_legacy_stack_node_is_retired_and_the_mode_lands_on_the_output() {
        let mut g = metal_graph::Graph::new("device-x", "device-x", metal_graph::Domain::Live);
        g.add_node("out", "output.device", [800.0, 0.0]);
        let id = g.add_node("mode", metal_graph::STACK_MODE_KIND, [0.0, 0.0]);
        crate::set_graph_device_mode_at(&mut g, Some(&id), &DeviceMode::Collection("noche".into()))
            .unwrap();
        assert!(g.node(&id).is_none(), "the legacy node is retired");
        assert_eq!(
            crate::graph_device_mode(&g),
            Some(DeviceMode::Collection("noche".into()))
        );
    }

    /// The op is for composition nodes only — a generator's config is
    /// `NodeLayerConfig`'s, and the guard refuses it by name.
    #[test]
    fn a_generator_is_not_a_composition_node() {
        assert!(accepts_device_mode(metal_graph::STACK_MODE_KIND));
        for (kind, _) in metal_graph::COMPOSE_KINDS {
            assert!(accepts_device_mode(kind), "{kind}");
        }
        assert!(!accepts_device_mode("gen.turing"));
        assert!(!accepts_device_mode("fx.shake"));
    }

    /// R22.5a: the domain query answers the menu's question and only it — the
    /// retired kinds (`stack.mode`, `compose.*`) never come back, a Live-only
    /// kind is not offered Offline and vice versa, and no `domain` is the
    /// whole catalog so the callers that omit it (the properties badge, the
    /// harness's route mapping) are untouched.
    #[test]
    fn the_domain_query_answers_what_the_menu_offers_in_that_domain() {
        let kinds = crate::node_catalog().expect("the repo's data/schema.json parses");
        let has = |ks: &[metal_graph::NodeKind], kind: &str| ks.iter().any(|k| k.kind == kind);

        let live = menu_kinds(kinds.clone(), Some("live")).unwrap();
        assert!(has(&live, "gen.turing"), "a normal kind is offered Live");
        assert!(has(&live, "output.device"), "a screen is a Live thing");
        assert!(!has(&live, "export.video"), "export.video only renders Offline");
        assert!(!has(&live, "audio.file"), "audio.file is an Offline graph's audio");
        assert!(!has(&live, metal_graph::STACK_MODE_KIND), "the legacy mode is retired from the menu");
        assert!(!has(&live, "compose.layers"), "the composition is retired from the menu");
        assert!(
            !has(&live, metal_graph::AI_COLOURIST_KIND),
            "the colourist is the gen.* sub-feature, never offered as a node"
        );

        let offline = menu_kinds(kinds.clone(), Some("offline")).unwrap();
        assert!(has(&offline, "export.video"));
        assert!(has(&offline, "audio.file"));
        assert!(!has(&offline, "audio.live"), "a live audio source cannot render to a file");
        assert!(!has(&offline, "output.device"));
        assert!(!has(&offline, "capture"));
        assert!(!has(&offline, "out.sacn"));
        assert!(!has(&offline, metal_graph::STACK_MODE_KIND));

        let all = menu_kinds(kinds.clone(), None).unwrap();
        assert_eq!(all.len(), kinds.len(), "no domain is the whole catalog, compat");
        assert!(has(&all, metal_graph::STACK_MODE_KIND));

        assert!(menu_kinds(kinds, Some("both")).is_err(), "an unknown domain is refused by name");
    }
}

pub fn graph_edge(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(gid) = body["graph"].as_str() else {
        return Err(CallError::Bad("which graph".into()));
    };
    let mut g = match crate::load_graph(gid) {
        Ok(g) => g,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let catalog = match crate::node_catalog() {
        Ok(c) => c,
        Err(e) => return Err(CallError::Bad(e)),
    };
    match body["op"].as_str().unwrap_or_default() {
        "add" => {
            let (Some(fn_), Some(fp), Some(tn), Some(tp)) = (
                body["from"]["node"].as_str(),
                body["from"]["port"].as_str(),
                body["to"]["node"].as_str(),
                body["to"]["port"].as_str(),
            ) else {
                return Err(CallError::Bad("expected from/to {node, port}".into()));
            };
            let result = crate::add_edge_checked(
                &mut g,
                metal_graph::Endpoint::new(fn_, fp),
                metal_graph::Endpoint::new(tn, tp),
                &catalog,
            );
            let edge_id = match result {
                Ok(id) => id,
                Err(errors) => return Err(CallError::Bad(errors.join("; "))),
            };
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            let mut reply = crate::graph_reply(&g, &catalog);
            reply["edge"] = json!(edge_id);
            Ok(reply)
        }
        "remove" => {
            let Some(id) = body["edge"].as_str() else {
                return Err(CallError::Bad("which edge".into()));
            };
            g.remove_edge(id);
            if let Err(e) = crate::save_graph(&g) {
                return Err(CallError::Bad(e));
            }
            Ok(crate::graph_reply(&g, &catalog))
        }
        other => Err(CallError::Bad(format!("no such op: {other}"))),
    }
}

pub fn effects_get(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(id) = params.param("id") else {
        return Err(CallError::Bad("which graph".into()));
    };
    let Some(node) = params.param("node") else {
        return Err(CallError::Bad("which node".into()));
    };
    let g = match crate::load_graph(id) {
        Ok(g) => g,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let catalog = match crate::node_catalog() {
        Ok(c) => c,
        Err(e) => return Err(CallError::Bad(e)),
    };
    Ok(crate::effects_reply(&g, node, &catalog))
}

pub fn effects_post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    let Some(gid) = body["graph"].as_str() else {
        return Err(CallError::Bad("which graph".into()));
    };
    let Some(node) = body["node"].as_str() else {
        return Err(CallError::Bad("which node".into()));
    };
    let Some(effect) = body["effect"].as_str() else {
        return Err(CallError::Bad("which effect".into()));
    };
    let mut g = match crate::load_graph(gid) {
        Ok(g) => g,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let catalog = match crate::node_catalog() {
        Ok(c) => c,
        Err(e) => return Err(CallError::Bad(e)),
    };
    let state = match body["state"].as_str().unwrap_or_default() {
        "off" => metal_schema::EffectState::Off,
        "reactive" | "reactivo" => metal_schema::EffectState::Reactive,
        "fixed" | "fijo" => metal_schema::EffectState::Fixed(body["value"].as_f64().unwrap_or(0.0) as f32),
        other => return Err(CallError::Bad(format!("no such state: {other}"))),
    };
    let source = match body["source"].as_str() {
        Some("row") | Some("router") => metal_graph::effects::ControlSource::Row,
        _ => metal_graph::effects::ControlSource::RawIngest,
    };
    if let Err(e) = metal_graph::effects::set_effect(&mut g, node, effect, state, source) {
        return Err(CallError::Bad(e));
    }
    if let Err(e) = crate::save_graph(&g) {
        return Err(CallError::Bad(e));
    }
    let mut reply = crate::graph_reply(&g, &catalog);
    reply["effects"] = crate::effects_reply(&g, node, &catalog)["effects"].clone();
    Ok(reply)
}

/// R22.5a: `GET /catalog?domain=live|offline` — the catalog as the `+ node`
/// menu offers it *for that domain*: a kind is in the answer when it is in
/// the menu at all ([`metal_graph::NodeKind::menu`]) and names the domain in its
/// own `domains` (the list `metal_graph::kind_domains` stamps and `lint`
/// enforces), which is exactly [`metal_graph::NodeKind::offered_in`].
///
/// No `domain` answers the whole catalog — the shape every caller that wants
/// "what kinds exist" already reads (`properties.rs`'s family/ports badge,
/// `tool_route`'s mapping) — so adding the query broke nobody. An unknown
/// domain is refused by name rather than answered with the unfiltered list: a
/// caller that asked "what serves Offline" and got `output.device` back would
/// wire a graph `lint` rejects.
fn menu_kinds(
    kinds: Vec<metal_graph::NodeKind>,
    domain: Option<&str>,
) -> Result<Vec<metal_graph::NodeKind>, CallError> {
    let Some(domain) = domain else {
        return Ok(kinds);
    };
    let wanted = match domain {
        "live" => metal_graph::Domain::Live,
        "offline" => metal_graph::Domain::Offline,
        other => return Err(CallError::Bad(format!("no such domain: {other}"))),
    };
    Ok(kinds.into_iter().filter(|k| k.offered_in(wanted)).collect())
}

/// How often each node kind is used across the saved graphs, the `usage`
/// term of [`metal_graph::relevance::rank`]. Read from the graphs dir and
/// kept for [`USAGE_TTL`]: the menus ask on every open, the graphs change
/// on a save, and a few seconds of staleness is no visible difference.
pub fn kind_usage() -> std::collections::HashMap<String, u32> {
    static CACHE: std::sync::Mutex<Option<(web_time::Instant, std::collections::HashMap<String, u32>)>> =
        std::sync::Mutex::new(None);
    const USAGE_TTL: std::time::Duration = std::time::Duration::from_secs(30);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, counts)) = cache.as_ref() {
        if at.elapsed() < USAGE_TTL {
            return counts.clone();
        }
    }
    let dir = metal_perform::engine::graphs_dir(&crate::output::data_root());
    let counts = usage_in(&dir);
    *cache = Some((web_time::Instant::now(), counts.clone()));
    counts
}

/// Counts node kinds over every graph saved directly under `dir`. A file
/// that does not parse as a graph counts for nothing.
fn usage_in(dir: &std::path::Path) -> std::collections::HashMap<String, u32> {
    let mut counts = std::collections::HashMap::new();
    let Ok(entries) = metal_vfs::read_dir(dir) else {
        return counts;
    };
    for e in entries.flatten() {
        if e.path().extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Some(g) = metal_vfs::read_to_string(e.path())
            .ok()
            .and_then(|text| metal_graph::Graph::from_json(&text).ok())
        else {
            continue;
        };
        for n in &g.nodes {
            *counts.entry(n.kind.clone()).or_insert(0) += 1;
        }
    }
    counts
}

/// `GET /catalog?graph=&anchor=&port=`. With `graph` (and optionally `anchor`,
/// a node of that graph, and `port`, a material input of the anchor's kind
/// the source is wanted for) every entry carries `relevance` and the list is
/// in relevance order; without `graph` the answer is the catalog as it was.
/// Every answer carries `shaders` (the generator shader ids on disk) and
/// `fixtures`, the two lists a source dropdown offers beside the kinds.
pub fn catalog(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let manifest = crate::node_manifest().map_err(CallError::Bad)?;
    let kinds = metal_graph::NodeKind::catalog(&manifest);
    let kinds = menu_kinds(kinds, params.param("domain"))?;
    let shaders = metal_perform::forge::generators(&crate::output::data_root());
    if let Some(gid) = params.param("graph").filter(|s| !s.is_empty()) {
        let g = crate::load_graph(gid).map_err(CallError::Bad)?;
        let anchor = params.param("anchor").filter(|s| !s.is_empty());
        let port = params.param("port").filter(|s| !s.is_empty());
        let scenes = metal_graph::relevance::scene_kinds(&manifest);
        let ranked = metal_graph::relevance::rank_for_port(
            &kinds,
            Some(&g),
            anchor,
            port,
            &kind_usage(),
            &scenes,
        );
        let by_name: std::collections::HashMap<&str, &metal_graph::NodeKind> =
            kinds.iter().map(|k| (k.kind.as_str(), k)).collect();
        let entries: Vec<Value> = ranked
            .iter()
            .filter_map(|(name, score)| {
                let mut v = serde_json::to_value(by_name.get(name.as_str())?).ok()?;
                v["relevance"] = json!(score);
                Some(v)
            })
            .collect();
        return Ok(json!({ "catalog": entries, "fixtures": fixtures_json(), "shaders": shaders }));
    }
    Ok(json!({ "catalog": kinds, "fixtures": fixtures_json(), "shaders": shaders }))
}

/// O.20/S.AC: the fixture profiles are data too, and the catalog is where a
/// UI or the harness reads what kinds exist. Listing them beside the node
/// kinds keeps one answer for "what can I wire".
fn fixtures_json() -> Vec<Value> {
    crate::fixture::load(&crate::output::data_root())
        .iter()
        .map(|f| {
            json!({
                "model": f.model,
                "label": f.label,
                "universe": f.universe,
                "address": f.address,
                "channels": f.channels.iter().map(|c| json!({
                    "signal": c.signal.name(),
                    "coarse": c.coarse,
                    "fine": c.fine,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

#[cfg(test)]
mod param_tests {
    //! The param-edge routes through their handlers, on a graph saved to the
    //! test build's scratch graphs dir (`metal_perform::engine::graphs_dir`).
    use super::*;
    use std::collections::HashMap;

    fn fixture() -> (Box<dyn crate::paint::Paint>, crate::State, crate::listeners::Listeners, crate::Author) {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let author = {
            let _chdir = crate::chdir_lock();
            let prior = std::env::current_dir().expect("cwd");
            std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
            let author = crate::Author::new();
            std::env::set_current_dir(&prior).expect("restore cwd");
            author
        };
        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        (paint, state, crate::listeners::Listeners::stand_in(), author)
    }

    fn body(v: Value) -> Params {
        Params { query: HashMap::new(), body: v.to_string().into_bytes(), tail: None }
    }

    fn query(pairs: &[(&str, &str)]) -> Params {
        Params {
            query: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body: Vec::new(),
            tail: None,
        }
    }

    struct Ids {
        graph: String,
        audio: String,
        gen: String,
        fx: String,
        script: String,
    }

    /// audio → gen.turing → fx.vignette → output, plus a script fed by the
    /// audio that declares one output `x` and one float `gain`.
    #[test]
    fn a_second_live_collection_is_refused_and_an_offline_graph_is_not() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let err = graph_post(&mut ctx, &body(json!({ "op": "create", "id": "collection-9", "name": "collection 9" }))).unwrap_err();
        let CallError::Bad(why) = err else { panic!("refused as a bad request") };
        assert!(why.contains("one live collection"), "{why}");
        assert!(!metal_vfs::exists(&crate::graph_path("collection-9")), "nothing was written");
        let offline = format!("offline-create-{}", std::process::id());
        graph_post(&mut ctx, &body(json!({ "op": "create", "id": offline, "domain": "offline" }))).unwrap();
        let _ = metal_vfs::remove_file(crate::graph_path(&offline));
    }

    fn saved_graph(tag: &str) -> (metal_graph::Graph, Ids) {
        let graph = format!("param-{tag}-{}", std::process::id());
        let mut g = metal_graph::Graph::new(&graph, "params", metal_graph::Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [200.0, 0.0]);
        let fx = g.add_node("fx", "fx.vignette", [400.0, 0.0]);
        let out = g.add_node("out", "output.device", [600.0, 0.0]);
        let script = g.add_node("script", "script.py", [200.0, 200.0]);
        g.node_mut(&script).unwrap().config = metal_graph::Config::from_json_str(
            r#"{"code": "def process(floats, row):\n    return [0.5]\n",
                "floats": [{"name": "gain", "min": 0.0, "max": 1.0, "default": 0.5}],
                "outputs": ["x"]}"#,
        )
        .unwrap();
        let wire = |g: &mut metal_graph::Graph, a: &str, ap: &str, b: &str, bp: &str| {
            g.add_edge(metal_graph::Endpoint::new(a, ap), metal_graph::Endpoint::new(b, bp));
        };
        wire(&mut g, &audio, "control", &gen, "control");
        wire(&mut g, &gen, "image", &fx, "image");
        wire(&mut g, &fx, "image", &out, "image");
        wire(&mut g, &audio, "control", &out, "control");
        wire(&mut g, &audio, "control", &script, "control");
        crate::save_graph(&g).expect("scratch graphs dir");
        let ids = Ids { graph, audio, gen, fx, script };
        (g, ids)
    }

    /// `POST /graph/node {op: "bind"}` writes the param edge, and
    /// `GET /graph/io` reads a transitive upstream's variables back with
    /// what this node uses.
    #[test]
    fn graph_io_lists_a_transitive_upstreams_vars_with_used_by() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (_, ids) = saved_graph("io");

        let from = json!({ "node": ids.script, "port": "control", "var": "x" });
        let reply = graph_node(
            &mut ctx,
            &body(json!({ "op": "bind", "graph": ids.graph, "node": ids.fx, "param": "amount", "from": from })),
        )
        .expect("bind");
        assert!(reply["edge"].is_string(), "{}", reply["edge"]);

        let io = graph_io(&mut ctx, &query(&[("id", &ids.graph), ("node", &ids.fx)])).expect("io");
        assert_eq!(io["graph"], json!(ids.graph));
        let sources = io["sources"].as_array().unwrap();
        let hops: Vec<(&str, u64)> =
            sources.iter().map(|s| (s["node"].as_str().unwrap(), s["hops"].as_u64().unwrap())).collect();
        assert_eq!(hops, vec![(ids.gen.as_str(), 1), (ids.script.as_str(), 1), (ids.audio.as_str(), 2)]);
        let x = sources[1]["vars"].as_array().unwrap().iter().find(|v| v["name"] == "x").unwrap();
        assert_eq!((x["used"].clone(), x["used_by"].clone()), (json!(true), json!(["amount"])));
        assert_eq!(x["port_ref"], json!("control.x"));
        let image = sources[0]["vars"].as_array().unwrap().iter().find(|v| v["kind"] == "image").unwrap();
        assert_eq!(image["via"], json!(["image"]), "the render arriving counts as used");
        assert!(sources[2]["vars"].as_array().unwrap().iter().all(|v| v["used"] == json!(false)));
        let amount = &io["params"][0];
        assert_eq!((amount["name"].clone(), amount["bound"]["var"].clone()), (json!("amount"), json!("x")));

        // Unbinding, and a refused binding, through the same op.
        let bad = graph_node(
            &mut ctx,
            &body(json!({ "op": "bind", "graph": ids.graph, "node": ids.fx, "param": "amount",
                          "from": { "node": ids.audio, "port": "control", "var": "nope" } })),
        );
        assert!(matches!(bad, Err(CallError::Bad(_))));
        let unbound = graph_node(
            &mut ctx,
            &body(json!({ "op": "bind", "graph": ids.graph, "node": ids.fx, "param": "amount", "from": null })),
        )
        .unwrap();
        assert!(unbound["edge"].is_null());
        assert!(matches!(graph_io(&mut ctx, &query(&[("id", &ids.graph)])), Err(CallError::Bad(_))));
    }

    /// F9: `add` a `venue.fixture` node merges the caller's `config` pointer
    /// over the seed and autowires its `image` from whatever feeds the
    /// device's `output.device` (here `fx`), so the default `frame` colour
    /// works at once.
    #[test]
    fn adding_a_venue_fixture_node_merges_config_and_autowires_the_image() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (_, ids) = saved_graph("fixture-auto");

        let reply = graph_node(
            &mut ctx,
            &body(json!({ "op": "add", "graph": ids.graph, "kind": "venue.fixture",
                          "config": { "venue": "bar", "fixture": "wash-1", "colour_from": "model" } })),
        )
        .expect("add the fixture node");
        let node = reply["node"].as_str().expect("a node id").to_string();

        let g = crate::load_graph(&ids.graph).expect("load it back");
        let n = g.node(&node).expect("the node is there");
        let cfg: Value = serde_json::from_str(&n.config.to_json_str()).unwrap();
        assert_eq!(cfg["venue"], json!("bar"));
        assert_eq!(cfg["fixture"], json!("wash-1"));
        assert_eq!(cfg["colour_from"], json!("model"));
        assert_eq!(cfg["priority"], json!(100), "the seed's default survives the merge");
        assert!(
            g.edges.iter().any(|e| e.from.node == ids.fx && e.to.node == node && e.to.port == "image"),
            "image is autowired from the device's compose output"
        );
    }

    /// `GET /graph/live` answers the last frame the engine recorded for the
    /// graph — nothing before one — and `/graph/io` picks the same values up.
    #[test]
    fn graph_live_answers_the_last_frame() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let (g, ids) = saved_graph("live");
        let gen_params = metal_perform::bind::gen_params(&g, &paint.ids(), &paint.schemas());
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };

        let none = graph_live(&mut ctx, &query(&[("id", &ids.graph)])).unwrap();
        assert!(none["t"].is_null() && none["nodes"].as_object().unwrap().is_empty(), "{none}");

        let schema = gen_params[&ids.gen].clone();
        let width = schema.iter().filter_map(|p| p.index).max().unwrap() + 64;
        let rows = HashMap::from([(ids.gen.clone(), vec![0.0f32; width])]);
        let scripts = HashMap::from([(ids.script.clone(), vec![0.25f32])]);
        let floats = HashMap::from([(ids.script.clone(), vec![0.5f32])]);
        let mut raw = [0.0f32; 16];
        raw[2] = 0.75;
        ctx.state.live.record(&g, &rows, &scripts, &floats, &gen_params, &raw, 2.5);

        let live = graph_live(&mut ctx, &query(&[("id", &ids.graph)])).unwrap();
        assert_eq!(live["t"], json!(2.5));
        assert_eq!(live["nodes"][&ids.audio]["out"]["onset"], json!(0.75));
        assert_eq!(live["nodes"][&ids.script]["out"]["x"], json!(0.25));
        assert_eq!(live["nodes"][&ids.script]["in"]["gain"], json!(0.5));
        let io = graph_io(&mut ctx, &query(&[("id", &ids.graph), ("node", &ids.script)])).unwrap();
        assert_eq!(io["params"][0]["live"], json!(0.5), "io reads the same frame");
        assert!(matches!(graph_live(&mut ctx, &query(&[])), Err(CallError::Bad(_))));
    }

    /// `POST /graph/node {op: "param"}`: one write per kind, each into that
    /// kind's own slot, out-of-range refused; `op: "config"`'s `floats`
    /// object on a generator goes through the same writer.
    #[test]
    fn op_param_writes_each_kinds_held_value() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (_, ids) = saved_graph("op");
        let param = |ctx: &mut Ctx, node: &str, name: &str, value: Value| {
            graph_node(ctx, &body(json!({ "op": "param", "graph": ids.graph, "node": node, "name": name, "value": value })))
        };

        param(&mut ctx, &ids.fx, "amount", json!(0.5)).expect("fx");
        param(&mut ctx, &ids.gen, "scale", json!(3.0)).expect("gen");
        param(&mut ctx, &ids.script, "gain", json!(0.6)).expect("script");
        let g = crate::load_graph(&ids.graph).unwrap();
        assert_eq!(metal_graph::effects::state_of(&g, &ids.fx), metal_schema::EffectState::Fixed(0.5));
        assert_eq!(metal_graph::params::gen_held(g.node(&ids.gen).unwrap()).get("scale"), Some(&3.0));
        assert_eq!(g.node(&ids.script).unwrap().config.get("floats").unwrap()[0]["value"], json!(0.6_f32));

        assert!(matches!(param(&mut ctx, &ids.gen, "scale", json!(99.0)), Err(CallError::Bad(_))));
        assert!(matches!(param(&mut ctx, &ids.gen, "nope", json!(1.0)), Err(CallError::Bad(_))));

        graph_node(
            &mut ctx,
            &body(json!({ "op": "config", "graph": ids.graph, "node": ids.gen,
                          "config": { "floats": { "scale": null, "step": 20.0 } } })),
        )
        .expect("config floats on a generator");
        let held = metal_graph::params::gen_held(crate::load_graph(&ids.graph).unwrap().node(&ids.gen).unwrap());
        assert_eq!(held, HashMap::from([("step".to_string(), 20.0)]), "escala released, paso held, nothing else");
    }

    /// `POST /graph/node {op: "params"}`: every update lands in one write, and
    /// one refused value refuses the whole batch — the graph on disk keeps
    /// every value it had before the batch.
    #[test]
    fn op_params_writes_a_batch_all_or_nothing() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (_, ids) = saved_graph("batch");
        let batch = |ctx: &mut Ctx, updates: Value| {
            graph_node(ctx, &body(json!({ "op": "params", "graph": ids.graph, "updates": updates })))
        };

        batch(
            &mut ctx,
            json!([
                { "node": ids.fx, "name": "amount", "value": 0.5 },
                { "node": ids.gen, "name": "scale", "value": 3.0 },
                { "node": ids.script, "name": "gain", "value": 0.6 }
            ]),
        )
        .expect("a valid batch");
        let g = crate::load_graph(&ids.graph).unwrap();
        assert_eq!(metal_graph::effects::state_of(&g, &ids.fx), metal_schema::EffectState::Fixed(0.5));
        assert_eq!(metal_graph::params::gen_held(g.node(&ids.gen).unwrap()).get("scale"), Some(&3.0));
        assert_eq!(g.node(&ids.script).unwrap().config.get("floats").unwrap()[0]["value"], json!(0.6_f32));

        // The second update is out of range: the batch is refused and the
        // first update, already valid, is not written either.
        let refused = batch(
            &mut ctx,
            json!([
                { "node": ids.fx, "name": "amount", "value": 0.9 },
                { "node": ids.gen, "name": "scale", "value": 99.0 }
            ]),
        );
        assert!(matches!(refused, Err(CallError::Bad(_))));
        let after = crate::load_graph(&ids.graph).unwrap();
        assert_eq!(metal_graph::effects::state_of(&after, &ids.fx), metal_schema::EffectState::Fixed(0.5), "nothing from a refused batch");

        // A null releases a generator float, as `op: "param"` does.
        batch(&mut ctx, json!([{ "node": ids.gen, "name": "scale", "value": null }])).expect("release");
        let released = crate::load_graph(&ids.graph).unwrap();
        assert!(metal_graph::params::gen_held(released.node(&ids.gen).unwrap()).get("scale").is_none());
    }

    /// O.8: a graph another peer owns refuses `take` and every write, the
    /// owner writes and releases, and a free graph is writable as before.
    /// The owner record lands in the test-isolated scratch graphs dir.
    #[test]
    fn one_owner_per_graph() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (g, ids) = saved_graph("owner");
        let dir = crate::graphs_dir(&crate::output::data_root());
        let me = metal_graph::owner::local_identity();

        // Free: writable, status says no owner.
        crate::save_graph(&g).expect("a free graph is writable");
        let st = graph_post(&mut ctx, &body(json!({ "op": "owner", "id": ids.graph }))).unwrap();
        assert!(st["owner"].is_null(), "{st}");

        // Another peer owns it: take and writes are refused, with its name.
        metal_graph::owner::take(&dir, &ids.graph, "other-peer").unwrap();
        let Err(CallError::Bad(why)) = graph_post(&mut ctx, &body(json!({ "op": "take", "id": ids.graph }))) else {
            panic!("taking an owned graph must be refused");
        };
        assert!(why.contains("other-peer"), "{why}");
        let err = crate::save_graph(&g).unwrap_err();
        assert!(err.contains("owned by \"other-peer\""), "{err}");
        assert!(graph_post(&mut ctx, &body(json!({ "op": "release", "id": ids.graph }))).is_err());
        assert!(graph_post(&mut ctx, &body(json!({ "op": "delete", "id": ids.graph }))).is_err());

        // They release; we take, write, release.
        metal_graph::owner::release(&dir, &ids.graph, "other-peer").unwrap();
        let took = graph_post(&mut ctx, &body(json!({ "op": "take", "id": ids.graph }))).unwrap();
        assert_eq!((took["owner"].clone(), took["mine"].clone()), (json!(me), json!(true)));
        crate::save_graph(&g).expect("the owner writes");
        graph_post(&mut ctx, &body(json!({ "op": "release", "id": ids.graph }))).unwrap();
        assert!(metal_graph::owner::status(&dir, &ids.graph).is_none());
        let _ = metal_vfs::remove_file(crate::graph_path(&ids.graph));
    }

    // -- A1: the embedded reaction (`ai.embed`) ---------------------------

    fn has_edge(g: &metal_graph::Graph, a: &str, ap: &str, b: &str, bp: &str) -> bool {
        g.edges.iter().any(|e| {
            e.from.node == a && e.from.port == ap && e.to.node == b && e.to.port == bp
        })
    }

    fn embed(ctx: &mut Ctx, ids: &Ids, on: bool) -> Value {
        graph_node(
            ctx,
            &body(json!({
                "op": "ai.embed", "graph": ids.graph, "node": ids.gen,
                "ai": metal_graph::AI_COLOURIST_KIND, "on": on,
            })),
        )
        .expect("ai.embed")
    }

    /// Turning the reaction on mints exactly one owned `ai.colourist` node
    /// (embedded in the source) and the one chain
    /// `audio → ai → source.control`, folding away the source's own direct
    /// cable so its control port has a single driver. Its `weights` is the
    /// newest trained grade for the model it drives, or `null` when there is
    /// none — either way a name the runtime can resolve.
    #[test]
    fn embedding_creates_one_owned_node_and_its_wires() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (_, ids) = saved_graph("embed-on");

        let reply = embed(&mut ctx, &ids, true);
        let ai_id = reply["node"].as_str().expect("the reply names the new node").to_string();

        let g = crate::load_graph(&ids.graph).unwrap();
        let ai = g.node(&ai_id).expect("the owned node exists");
        assert_eq!(ai.kind, metal_graph::AI_COLOURIST_KIND);
        assert_eq!(ai.embedded_in.as_deref(), Some(ids.gen.as_str()));
        match ai.config.get("weights") {
            Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::String(name)) => assert!(
                crate::output::data_root()
                    .join(metal_reel::index::DIR)
                    .join(format!("{name}.json"))
                    .exists(),
                "a pinned weights name must resolve to a real grade file: {name}"
            ),
            other => panic!("weights is a name or null, got {other:?}"),
        }
        assert_eq!(
            g.nodes.iter().filter(|n| n.embedded_in.as_deref() == Some(ids.gen.as_str())).count(),
            1,
            "one owned node per source"
        );
        assert!(has_edge(&g, &ids.audio, "control", &ai_id, "control"), "audio → ai");
        assert!(has_edge(&g, &ai_id, "control", &ids.gen, "control"), "ai → source.control");
        assert!(
            !has_edge(&g, &ids.audio, "control", &ids.gen, "control"),
            "the source's own cable is folded into the chain, not left beside it"
        );

        // A repeat is a no-op: the toggle cannot mint a second chip.
        let again = embed(&mut ctx, &ids, true);
        assert_eq!(again["node"], json!(ai_id));
        let g = crate::load_graph(&ids.graph).unwrap();
        assert_eq!(g.nodes.iter().filter(|n| n.embedded_in.as_deref() == Some(ids.gen.as_str())).count(), 1);
    }

    /// A2: the weights default is the newest trained grade for the model the
    /// reaction drives, and only that model's — a grade fitted for another
    /// model is never pinned (`Grade::fits` would refuse it anyway). None of
    /// this reads the real `data/`: it takes a scratch root (rule 8).
    #[test]
    fn the_newest_weights_are_the_model_s_own_and_never_another_s() {
        let root = std::env::temp_dir().join(format!("fw-ai-embed-weights-{}", std::process::id()));
        let reels = root.join(metal_reel::index::DIR);
        metal_vfs::create_dir_all(&reels).unwrap();
        metal_vfs::write(reels.join("colourist-turing.json"), "{}").unwrap();
        metal_vfs::write(reels.join("colourist-estela.json"), "{}").unwrap();
        metal_vfs::write(reels.join("colourist-turing-warm.json"), "{}").unwrap();
        metal_vfs::write(reels.join("index.json"), "{}").unwrap();

        let picked = newest_weights(&root, metal_graph::AI_COLOURIST_KIND, "turing");
        assert!(
            matches!(picked.as_deref(), Some("colourist-turing" | "colourist-turing-warm")),
            "only the driven model's grades count, got {picked:?}"
        );
        assert_eq!(
            newest_weights(&root, metal_graph::AI_COLOURIST_KIND, "forge"),
            None,
            "no grade for this model is not a grade for another"
        );
        // A2.2: the router arm answers nothing until `ai.router` is built.
        assert_eq!(newest_weights(&root, "ai.router", "turing"), None);

        let _ = metal_vfs::remove_dir_all(&root);
    }

    /// Deleting the source deletes its embedded `ai.*` and its wires with it.
    #[test]
    fn deleting_the_source_deletes_its_embedded_ai() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (_, ids) = saved_graph("embed-del");

        let reply = embed(&mut ctx, &ids, true);
        let ai_id = reply["node"].as_str().unwrap().to_string();

        graph_node(&mut ctx, &body(json!({ "op": "remove", "graph": ids.graph, "node": ids.gen })))
            .expect("remove the source");

        let g = crate::load_graph(&ids.graph).unwrap();
        assert!(g.node(&ids.gen).is_none(), "the source is gone");
        assert!(g.node(&ai_id).is_none(), "its embedded ai went with it");
        assert!(
            !g.edges.iter().any(|e| e.from.node == ai_id || e.to.node == ai_id),
            "and none of its wires are left dangling"
        );
    }

    /// `detach` (off) detaches: the owned node stays and keeps its wires, it
    /// just stops being owned.
    #[test]
    fn detach_keeps_the_wires_and_clears_embedded_in() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (_, ids) = saved_graph("embed-off");

        let reply = embed(&mut ctx, &ids, true);
        let ai_id = reply["node"].as_str().unwrap().to_string();
        embed(&mut ctx, &ids, false);

        let g = crate::load_graph(&ids.graph).unwrap();
        let ai = g.node(&ai_id).expect("the detached node stays");
        assert_eq!(ai.embedded_in, None, "it is no longer owned");
        assert!(has_edge(&g, &ids.audio, "control", &ai_id, "control"), "audio → ai stays");
        assert!(has_edge(&g, &ai_id, "control", &ids.gen, "control"), "ai → source.control stays");
    }
    /// A graph saved to the scratch dir with `gen.turing → output.device` on
    /// its image, and a `mask` on its own: the two shapes the source ops
    /// are tested on.
    fn source_ops_graph(tag: &str) -> (String, String, String, String) {
        let graph = format!("source-{tag}-{}", std::process::id());
        let mut g = metal_graph::Graph::new(&graph, "source ops", metal_graph::Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [400.0, 0.0]);
        let out = g.add_node("out", "output.device", [800.0, 0.0]);
        let mask = g.add_node("mask", "mask", [400.0, 240.0]);
        // A stroke has no outline to paint without a width (the lint says so).
        g.node_mut(&mask).unwrap().config =
            metal_graph::Config::from_json_str(r#"{"stroke_width": 2.0}"#).unwrap();
        g.add_edge(metal_graph::Endpoint::new(&gen, "image"), metal_graph::Endpoint::new(&out, "image"));
        crate::save_graph(&g).expect("scratch graphs dir");
        (graph, gen, out, mask)
    }

    /// `attach` on `mask.stroke` makes a new source to the mask's left and
    /// wires it in; a second attach replaces it; a bare attach detaches.
    /// A port that is not an image input is refused.
    #[test]
    fn attach_wires_a_new_source_into_a_material_port_and_detaches_it() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (graph, _gen, _out, mask) = source_ops_graph("attach");

        let reply = graph_node(
            &mut ctx,
            &body(json!({ "op": "attach", "graph": graph, "node": mask, "port": "stroke", "kind": "gen.turing" })),
        )
        .expect("attach a new source");
        let first = reply["node"].as_str().expect("the source's id").to_string();
        let g = crate::load_graph(&graph).unwrap();
        assert_eq!(g.node(&first).unwrap().kind, "gen.turing");
        assert!(g.node(&first).unwrap().pos[0] < 400.0, "placed to the left of the mask");
        assert!(has_edge(&g, &first, "image", &mask, "stroke"));

        let reply = graph_node(
            &mut ctx,
            &body(json!({ "op": "attach", "graph": graph, "node": mask, "port": "stroke", "from": first })),
        )
        .expect("attach an existing node");
        assert_eq!(reply["node"], json!(first));

        // A second source replaces the first: one edge into the port.
        let second = graph_node(
            &mut ctx,
            &body(json!({ "op": "attach", "graph": graph, "node": mask, "port": "stroke", "kind": "gen.rings" })),
        )
        .expect("replace the source")["node"]
            .as_str()
            .unwrap()
            .to_string();
        let g = crate::load_graph(&graph).unwrap();
        let into_stroke: Vec<_> = g.edges.iter().filter(|e| e.to.node == mask && e.to.port == "stroke").collect();
        assert_eq!(into_stroke.len(), 1, "whatever fed the port is replaced");
        assert!(has_edge(&g, &second, "image", &mask, "stroke"));

        let detached = graph_node(
            &mut ctx,
            &body(json!({ "op": "attach", "graph": graph, "node": mask, "port": "stroke" })),
        )
        .expect("detach");
        assert!(detached["node"].is_null());
        let g = crate::load_graph(&graph).unwrap();
        assert!(g.edges.iter().all(|e| !(e.to.node == mask && e.to.port == "stroke")));

        let refused = graph_node(
            &mut ctx,
            &body(json!({ "op": "attach", "graph": graph, "node": mask, "port": "control", "kind": "gen.turing" })),
        );
        assert!(matches!(refused, Err(CallError::Bad(_))), "control is not an image input");
    }

    /// A source that a rewired port leaves with no other use is removed with
    /// its edges; one still feeding something else stays.
    #[test]
    fn attach_removes_a_source_orphaned_by_the_rewire() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (graph, gen, out, mask) = source_ops_graph("attach-orphan");

        let first = graph_node(
            &mut ctx,
            &body(json!({ "op": "attach", "graph": graph, "node": mask, "port": "stroke", "kind": "gen.turing" })),
        )
        .expect("attach a source")["node"]
            .as_str()
            .unwrap()
            .to_string();
        graph_node(
            &mut ctx,
            &body(json!({ "op": "attach", "graph": graph, "node": mask, "port": "stroke", "kind": "gen.rings" })),
        )
        .expect("replace the source");
        let g = crate::load_graph(&graph).unwrap();
        assert!(g.node(&first).is_none(), "the replaced source had no other use: removed");
        assert_eq!(g.nodes.iter().filter(|n| n.kind == "gen.rings").count(), 1);

        // Now wire the device's generator into the port: it is the previous
        // source of nothing, so the rings node goes; the generator stays.
        graph_node(
            &mut ctx,
            &body(json!({ "op": "attach", "graph": graph, "node": mask, "port": "stroke", "from": gen })),
        )
        .expect("attach the generator that feeds the device");
        let g = crate::load_graph(&graph).unwrap();
        assert!(g.nodes.iter().all(|n| n.kind != "gen.rings"), "the replaced rings node is gone");
        assert!(g.node(&gen).is_some());

        // Detaching the port: the generator still feeds the device, so it stays.
        graph_node(
            &mut ctx,
            &body(json!({ "op": "attach", "graph": graph, "node": mask, "port": "stroke" })),
        )
        .expect("detach");
        let g = crate::load_graph(&graph).unwrap();
        assert!(g.node(&gen).is_some(), "the generator still feeds the device: kept");
        assert!(has_edge(&g, &gen, "image", &out, "image"));
        assert!(!has_edge(&g, &gen, "image", &mask, "stroke"));
    }

    /// `insert` puts an fx between a generator and its device: the old
    /// consumer now reads the new node, and the generator feeds only the new
    /// node. A kind with no image ports is refused.
    #[test]
    fn insert_puts_an_effect_between_a_source_and_its_consumers() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let (graph, gen, out, _mask) = source_ops_graph("insert");

        let reply = graph_node(
            &mut ctx,
            &body(json!({ "op": "insert", "graph": graph, "after": gen, "kind": "fx.vignette" })),
        )
        .expect("insert");
        let fx = reply["node"].as_str().expect("the new node's id").to_string();
        let g = crate::load_graph(&graph).unwrap();
        assert_eq!(g.node(&fx).unwrap().kind, "fx.vignette");
        assert!(g.node(&fx).unwrap().pos[0] > 400.0, "placed to the right of the generator");
        assert!(has_edge(&g, &gen, "image", &fx, "image"));
        assert!(has_edge(&g, &fx, "image", &out, "image"), "the device now reads the effect");
        assert!(!has_edge(&g, &gen, "image", &out, "image"), "no bypass edge is left behind");

        let no_image = graph_node(
            &mut ctx,
            &body(json!({ "op": "insert", "graph": graph, "after": gen, "kind": "audio.live" })),
        );
        assert!(matches!(no_image, Err(CallError::Bad(_))));
        let missing = graph_node(
            &mut ctx,
            &body(json!({ "op": "insert", "graph": graph, "after": "nope", "kind": "fx.vignette" })),
        );
        assert!(matches!(missing, Err(CallError::Bad(_))));
    }

    /// The `reel.batch` walk: an absolute video path is kept as is, a relative
    /// one is joined under the root, and only `kind == "video"` entries with a
    /// non-empty string path survive — stills, scene/group entries and unnamed
    /// paths are skipped. Pure: no ffmpeg, no `data/`.
    #[test]
    fn reel_batch_paths_keep_videos_resolve_relative_and_skip_the_rest() {
        let root = std::path::Path::new("/srv/metal/data");
        let cfg = json!({
            "items": [
                { "kind": "video", "path": "/abs/clip.mp4" },
                { "kind": "video", "path": "reels/rel.mp4" },
                { "kind": "image", "path": "still.jpg" },
                { "kind": "scene", "clip": "clip.mp4", "start": 0.0, "end": 1.0 },
                { "kind": "group", "name": "club" },
                { "kind": "video", "path": "" },
                { "kind": "video" },
                { "path": "nameless.mp4" },
            ],
        });
        assert_eq!(
            reel_batch_paths(&cfg, root),
            vec!["/abs/clip.mp4".to_string(), "/srv/metal/data/reels/rel.mp4".to_string()]
        );
    }

    /// A config with no `items`, or a non-array one, names no videos.
    #[test]
    fn reel_batch_paths_of_a_config_without_items_is_empty() {
        let root = std::path::Path::new("/srv/metal/data");
        assert!(reel_batch_paths(&json!({}), root).is_empty());
        assert!(reel_batch_paths(&json!({ "items": 7 }), root).is_empty());
    }

}

/// The naga gate for a runtime shader saved from the shell: the id is a
/// file name, the WGSL must check, and it must be an effect (`fx.forge` takes
/// a picture; a generator has none). Without the `gpu` feature the gate is
/// `metal-forge`, which is not built, so the save is refused with its reason.
#[cfg(feature = "gpu")]
fn save_runtime_shader(id: &str, wgsl: &str) -> Result<(), CallError> {
    if !metal_perform::forge::valid_id(id) {
        return Err(CallError::Bad(format!("{id:?} is not a shader name (letters, digits, `-`, `_`)")));
    }
    let checked = metal_forge::check(wgsl).map_err(|e| CallError::Bad(e.to_string()))?;
    if !checked.effect {
        return Err(CallError::Bad(format!("{id}.wgsl is a generator: fx.forge needs a shader that reads `src`")));
    }
    let dir = metal_perform::forge::dir(&metal_perform::output::data_root());
    std::fs::create_dir_all(&dir).map_err(|e| CallError::Bad(e.to_string()))?;
    let path = dir.join(format!("{id}.wgsl"));
    std::fs::write(&path, wgsl).map_err(|e| CallError::Bad(format!("{}: {e}", path.display())))
}

#[cfg(not(feature = "gpu"))]
fn save_runtime_shader(_id: &str, _wgsl: &str) -> Result<(), CallError> {
    Err(CallError::Bad("loading a script needs the gpu feature (the naga check)".into()))
}
