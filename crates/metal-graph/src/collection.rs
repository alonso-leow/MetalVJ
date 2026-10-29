//! One graph per collection, the outputs inside it as nodes
//! (`the design notes` §9).
//!
//! The engine still runs one authored graph per device, `device-<id>`. A
//! **collection graph** is any graph that is not one of those: its
//! `output.device` nodes each name the screen they feed in `config.device`, and
//! saving it **publishes** to every device it feeds — the device graph becomes
//! the collection graph minus the other screens' output nodes, with
//! [`Graph::source`] pointing back. Node ids are kept, so everything that
//! addresses a node by `(device, node)` (previews, live values, scenes,
//! bindings) keeps working unchanged. The device file is generated; the canvas
//! edits the collection.
//!
//! **One collection holds several screens.** Every output device joins the
//! central collection ([`CENTRAL_ID`]) as an `output.device` node, not a
//! collection of its own: adding a screen calls [`ensure_device`], boot folds
//! the per-device collections an earlier version minted with
//! [`merge_device_collections`], and adopts an unowned device graph into the
//! same central collection ([`adopt_device_graphs`]).
//!
//! **Who owns a screen.** A screen is driven by one collection at a time.
//! Binding a screen to a collection (setting the output node's device)
//! *claims* it; plain edits afterwards only republish screens the collection
//! already owns, so two collections that name one screen do not fight over it
//! on every save.

use std::collections::BTreeSet;
use std::path::Path;

use crate::{Config, Graph};

/// The prefix of the engine's per-device graphs.
pub const DEVICE_PREFIX: &str = "device-";

/// The id of the one collection every output device joins. A screen added to
/// the show becomes an `output.device` node here, not a collection of its own:
/// one collection can feed several screens (`project`/`publish` already do).
pub const CENTRAL_ID: &str = "col-main";

/// The name the central collection opens with. The operator may rename it;
/// the id is what `ensure_device` and the migrations key on.
pub const CENTRAL_NAME: &str = "Main";

/// Whether `id` names a per-device graph.
pub fn is_device_graph_id(id: &str) -> bool {
    id.starts_with(DEVICE_PREFIX)
}

/// The id of the per-device graph for `device`.
pub fn device_graph_id(device: &str) -> String {
    format!("{DEVICE_PREFIX}{device}")
}

/// The device an `output.device` node feeds, when it names one.
pub fn node_device(node: &crate::Node) -> Option<&str> {
    node.config.get("device")?.as_str().filter(|d| !d.is_empty())
}

/// Every screen a collection graph feeds. Empty for a device graph.
pub fn bound_devices(g: &Graph) -> BTreeSet<String> {
    if is_device_graph_id(&g.id) {
        return BTreeSet::new();
    }
    g.nodes
        .iter()
        .filter(|n| n.kind == "output.device")
        .filter_map(|n| node_device(n).map(str::to_string))
        .collect()
}

/// Names `device` on `node`'s config (merging into what is there).
pub fn bind_node(node: &mut crate::Node, device: &str) {
    let mut cfg: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str::<serde_json::Value>(&node.config.to_json_str())
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
    cfg.insert("device".to_string(), serde_json::Value::String(device.to_string()));
    node.config = Config::from_json_str(&serde_json::Value::Object(cfg).to_string())
        .expect("an object serializes");
}

/// The device graph a collection graph publishes for `device`: every node,
/// minus the `output.device` nodes that feed another screen.
pub fn project(g: &Graph, device: &str) -> Graph {
    let mut out = g.clone();
    out.id = device_graph_id(device);
    out.name = device.to_string();
    out.source = Some(g.id.clone());
    let other: Vec<String> = g
        .nodes
        .iter()
        .filter(|n| n.kind == "output.device" && node_device(n) != Some(device))
        .map(|n| n.id.clone())
        .collect();
    for id in other {
        out.remove_node(&id);
    }
    out
}

fn read(dir: &Path, id: &str) -> Option<Graph> {
    let text = metal_vfs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
    Graph::from_json(&text).ok()
}

/// How a caller writes a graph file, and who may. The engine passes its atomic
/// writer and the peer-ownership check, the agent harness its own; the
/// collection logic is the same for both.
pub struct Io<'a> {
    pub write: &'a dyn Fn(&Path, &str) -> Result<(), String>,
    pub check: &'a dyn Fn(&str) -> Result<(), String>,
}

/// A plain atomic write, for callers with nothing of their own.
pub fn plain_write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = metal_vfs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    metal_vfs::write(&tmp, text)
        .and_then(|_| metal_vfs::rename(&tmp, path))
        .map_err(|e| {
            let _ = metal_vfs::remove_file(&tmp);
            e.to_string()
        })
}

fn write(dir: &Path, g: &Graph, io: &Io) -> Result<(), String> {
    (io.write)(&dir.join(format!("{}.json", g.id)), &g.to_json())
}

/// The collection graph `col` after its screen `device` was edited through
/// the published `edited` graph: everything the projection carries is taken
/// from `edited`; the other screens' output nodes (and the cables into them)
/// stay as `col` had them.
pub fn unproject(col: &Graph, edited: &Graph, device: &str) -> Graph {
    let mut out = edited.clone();
    out.id = col.id.clone();
    out.name = col.name.clone();
    out.source = None;
    out.ids = edited.ids.merged(&col.ids);
    let others: BTreeSet<&str> = col
        .nodes
        .iter()
        .filter(|n| n.kind == "output.device" && node_device(n) != Some(device))
        .map(|n| n.id.as_str())
        .collect();
    for node in col.nodes.iter().filter(|n| others.contains(n.id.as_str())) {
        out.nodes.push(node.clone());
    }
    for edge in col
        .edges
        .iter()
        .filter(|e| others.contains(e.to.node.as_str()) || others.contains(e.from.node.as_str()))
    {
        out.edges.push(edge.clone());
    }
    out
}

/// A graph save with collection semantics — the one entry point every writer
/// calls instead of writing the file:
///
/// - a **published device graph** (`source` set) is an edit of its collection:
///   it is merged back ([`unproject`]), the collection saved, and every screen
///   it feeds republished — so a route that still addresses `device-<id>`
///   (venue, outputs, loops…) edits the one real graph;
/// - a **collection graph** is saved and [`publish`]ed;
/// - anything else is written as it is.
///
/// A published graph whose collection is gone is written plain, released.
pub fn store(dir: &Path, g: &Graph, io: &Io) -> Result<(), String> {
    (io.check)(&g.id)?;
    if let Some(col_id) = g.source.as_deref() {
        let device = g.id.trim_start_matches(DEVICE_PREFIX);
        if let Some(col) = read(dir, col_id) {
            (io.check)(col_id)?;
            let before = bound_devices(&col);
            let merged = unproject(&col, g, device);
            write(dir, &merged, io)?;
            publish(dir, &merged, &before, io);
            return Ok(());
        }
        let mut released = g.clone();
        released.source = None;
        return write(dir, &released, io);
    }
    if is_device_graph_id(&g.id) {
        return write(dir, g, io);
    }
    let before = bound_in_store(dir, &g.id);
    write(dir, g, io)?;
    publish(dir, g, &before, io);
    Ok(())
}

/// What a save of collection graph `g` publishes, given the screens its
/// previous version fed (`before`). Returns the devices written.
///
/// - a screen `g` already fed and still owns is republished;
/// - a screen `g` newly names is claimed: its device graph is replaced;
/// - a screen another collection owns, which `g` already named, is left alone;
/// - a screen `g` stopped naming keeps its last graph but is released
///   ([`Graph::source`] cleared), so it is never silently lost.
pub fn publish(dir: &Path, g: &Graph, before: &BTreeSet<String>, io: &Io) -> Vec<String> {
    if is_device_graph_id(&g.id) {
        return Vec::new();
    }
    let now = bound_devices(g);
    let mut written = Vec::new();
    for device in &now {
        let existing = read(dir, &device_graph_id(device));
        let mine = existing.as_ref().is_some_and(|e| e.source.as_deref() == Some(g.id.as_str()));
        let claimed = !before.contains(device);
        if mine || claimed {
            if (io.check)(&device_graph_id(device)).is_ok() && write(dir, &project(g, device), io).is_ok() {
                written.push(device.clone());
            }
        }
    }
    for device in before.difference(&now) {
        if let Some(mut e) = read(dir, &device_graph_id(device)) {
            if e.source.as_deref() == Some(g.id.as_str()) {
                e.source = None;
                let _ = write(dir, &e, io);
            }
        }
    }
    written
}

/// The screens the saved version of collection graph `id` feeds — the
/// `before` a save passes to [`publish`].
pub fn bound_in_store(dir: &Path, id: &str) -> BTreeSet<String> {
    read(dir, id).map(|g| bound_devices(&g)).unwrap_or_default()
}

/// Adds an `output.device` node bound to `device` to `g` when it has none,
/// so a collection can still reach the screen. Returns whether it added one.
fn ensure_output_node(g: &mut Graph, device: &str) -> bool {
    if g.nodes
        .iter()
        .any(|n| n.kind == "output.device" && node_device(n) == Some(device))
    {
        return false;
    }
    let out = g.add_node("out", "output.device", [800.0, 0.0]);
    if let Some(node) = g.node_mut(&out) {
        bind_node(node, device);
    }
    true
}

/// Copies every node and edge of `other` into `target`, minting a fresh id for
/// any node id `target` already uses, so two graphs can share one collection
/// without two nodes answering the same id. An `output.device` node that names
/// no screen is bound to `device` (the graph `other` was the screen's own).
fn absorb(target: &mut Graph, other: &Graph, device: Option<&str>) {
    let mut rename: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for node in &other.nodes {
        let id = if target.node(&node.id).is_some() {
            let prefix = node.id.rsplit_once('-').map(|(p, _)| p).unwrap_or("node");
            target.ids.next(prefix)
        } else {
            node.id.clone()
        };
        rename.insert(node.id.clone(), id);
    }
    for node in &other.nodes {
        let mut node = node.clone();
        node.id = rename[&node.id].clone();
        if let (Some(device), "output.device", None) = (device, node.kind.as_str(), node_device(&node)) {
            bind_node(&mut node, device);
        }
        target.nodes.push(node);
    }
    for edge in &other.edges {
        let (Some(from), Some(to)) = (rename.get(&edge.from.node), rename.get(&edge.to.node)) else {
            continue;
        };
        target.add_edge(
            crate::Endpoint::new(from, &edge.from.port),
            crate::Endpoint::new(to, &edge.to.port),
        );
    }
    target.ids = target.ids.merged(&other.ids);
}

/// The screen a per-device collection an earlier boot minted feeds: one bound
/// device whose id is the collection's own suffix (`col-<device>`). `None` for
/// any other graph.
fn per_device_collection_device(g: &Graph) -> Option<String> {
    let devices = bound_devices(g);
    let device = devices.iter().next()?;
    (devices.len() == 1 && g.id == format!("col-{device}")).then(|| device.clone())
}

/// Every device graph that carries no collection yet is merged into **the
/// one** central collection (`col-main`), keeping each screen's nodes and its
/// `output.device` node, and publishing the merged collection back to every
/// screen. Run once at boot; a device graph that already has a [`Graph::source`]
/// is left alone. Returns the collection ids it wrote (just `col-main`), or
/// empty when there was nothing left to adopt.
pub fn adopt_device_graphs(dir: &Path, io: &Io) -> Vec<String> {
    let Ok(entries) = metal_vfs::read_dir(dir) else {
        return Vec::new();
    };
    let mut central = read(dir, CENTRAL_ID)
        .unwrap_or_else(|| Graph::new(CENTRAL_ID, CENTRAL_NAME, crate::Domain::Live));
    let before = bound_devices(&central);
    let mut adopted = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json").filter(|i| is_device_graph_id(i)) else {
            continue;
        };
        let Some(device_graph) = read(dir, id) else { continue };
        if device_graph.source.is_some() {
            continue;
        }
        let device = id.trim_start_matches(DEVICE_PREFIX).to_string();
        if before.contains(&device) {
            continue;
        }
        absorb(&mut central, &device_graph, Some(&device));
        ensure_output_node(&mut central, &device);
        adopted = true;
    }
    if !adopted {
        return Vec::new();
    }
    if write(dir, &central, io).is_err() {
        return Vec::new();
    }
    // Every screen central names is central's: claim them all, so a device
    // graph left unowned by a half-finished migration is republished.
    publish(dir, &central, &BTreeSet::new(), io);
    vec![CENTRAL_ID.to_string()]
}

/// There is one collection. Every other live collection file (a per-device
/// `col-<screen>` an earlier boot minted, an operator's canvas, one naming
/// several screens, one naming none) is folded into the central collection,
/// its nodes and wiring kept under fresh ids, then its file is dropped. A
/// screen a folded collection names is claimed by the central one, unless
/// central already feeds it. Outputs that name no screen stay unbound (the
/// central canvas shows them as issues). Central's own duplicate node ids are
/// repaired too. Returns the ids it merged.
pub fn merge_device_collections(dir: &Path, io: &Io) -> Vec<String> {
    let Ok(entries) = metal_vfs::read_dir(dir) else {
        return Vec::new();
    };
    let mut central = read(dir, CENTRAL_ID)
        .unwrap_or_else(|| Graph::new(CENTRAL_ID, CENTRAL_NAME, crate::Domain::Live));
    let before = bound_devices(&central);
    let mut merged = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else { continue };
        if id == CENTRAL_ID || is_device_graph_id(id) {
            continue;
        }
        let Some(col) = read(dir, id) else { continue };
        match per_device_collection_device(&col) {
            Some(device) => {
                if !before.contains(&device) {
                    absorb(&mut central, &col, Some(&device));
                    ensure_output_node(&mut central, &device);
                }
            }
            None => {
                // An output naming a screen central already feeds is dropped
                // with its edges: one screen, one output node.
                let mut col = col;
                let taken = bound_devices(&central);
                col.nodes.retain(|n| !(n.kind == "output.device" && node_device(n).is_some_and(|d| taken.contains(d))));
                let kept: std::collections::BTreeSet<&str> = col.nodes.iter().map(|n| n.id.as_str()).collect();
                col.edges.retain(|e| kept.contains(e.from.node.as_str()) && kept.contains(e.to.node.as_str()));
                absorb(&mut central, &col, None);
            }
        }
        merged.push(id.to_string());
    }
    if merged.is_empty() {
        return Vec::new();
    }
    dedupe_node_ids(&mut central);
    if write(dir, &central, io).is_err() {
        return Vec::new();
    }
    // Publish before dropping the old files: every screen the folded
    // collections named is now central's.
    publish(dir, &central, &BTreeSet::new(), io);
    for id in &merged {
        let _ = metal_vfs::remove_file(dir.join(format!("{id}.json")));
        let _ = metal_vfs::remove_file(dir.join(format!("{id}.owner")));
    }
    merged
}

/// Gives a later copy of a node id that repeats an earlier one a fresh id (the
/// first keeps its id). Edges that name the repeated id stay on the first node.
fn dedupe_node_ids(g: &mut Graph) {
    let mut seen = BTreeSet::new();
    for i in 0..g.nodes.len() {
        let id = g.nodes[i].id.clone();
        if seen.insert(id.clone()) {
            continue;
        }
        let prefix = id.rsplit_once('-').map(|(p, _)| p).unwrap_or("node").to_string();
        let fresh = g.ids.next(&prefix);
        seen.insert(fresh.clone());
        g.nodes[i].id = fresh;
    }
}

/// `POST /outputs {op:"add"}`'s half: make `device` a screen of the central
/// collection and publish its device graph from it. An existing device graph
/// the device already has is absorbed rather than overwritten, and a screen
/// another collection owns is left alone. Returns the collection ids written.
pub fn ensure_device(dir: &Path, device: &str, io: &Io) -> Vec<String> {
    let mut central = read(dir, CENTRAL_ID)
        .unwrap_or_else(|| Graph::new(CENTRAL_ID, CENTRAL_NAME, crate::Domain::Live));
    let before = bound_devices(&central);
    let existing = read(dir, &device_graph_id(device));
    // A screen another collection owns is its own; never steal it here.
    if existing
        .as_ref()
        .is_some_and(|g| g.source.as_deref().is_some_and(|s| s != CENTRAL_ID))
    {
        return Vec::new();
    }
    if before.contains(device)
        && existing.as_ref().is_some_and(|g| g.source.as_deref() == Some(CENTRAL_ID))
    {
        return Vec::new();
    }
    if !before.contains(device) {
        if let Some(g) = existing.as_ref().filter(|g| g.source.is_none()) {
            absorb(&mut central, g, Some(device));
        }
    }
    ensure_output_node(&mut central, device);
    if write(dir, &central, io).is_err() {
        return Vec::new();
    }
    // The screen is central's now (the guard above kept us off any other
    // collection's): claim it, republishing its device graph.
    publish(dir, &central, &BTreeSet::new(), io);
    vec![CENTRAL_ID.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Domain;

    fn two_screens() -> Graph {
        let mut g = Graph::new("show", "Show", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let a = g.add_node("out", "output.device", [200.0, 0.0]);
        let b = g.add_node("out", "output.device", [200.0, 120.0]);
        bind_node(g.node_mut(&a).unwrap(), "A");
        bind_node(g.node_mut(&b).unwrap(), "B");
        let _ = gen;
        g
    }

    const IO: Io<'static> = Io { write: &|p, t| plain_write(p, t), check: &|_| Ok(()) };

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mvj-collection-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_collection_names_every_screen_its_outputs_feed() {
        let g = two_screens();
        assert_eq!(bound_devices(&g).into_iter().collect::<Vec<_>>(), ["A", "B"]);
        let mut device = g.clone();
        device.id = "device-A".into();
        assert!(bound_devices(&device).is_empty(), "a device graph feeds nothing else");
    }

    #[test]
    fn the_projection_keeps_ids_and_drops_the_other_screens_outputs() {
        let g = two_screens();
        let a = project(&g, "A");
        assert_eq!(a.id, "device-A");
        assert_eq!(a.source.as_deref(), Some("show"));
        let outs: Vec<_> = a.nodes.iter().filter(|n| n.kind == "output.device").collect();
        assert_eq!(outs.len(), 1);
        assert_eq!(node_device(outs[0]), Some("A"));
        assert!(a.node("gen-0").is_some(), "the shared nodes keep their ids");
    }

    #[test]
    fn saving_publishes_to_every_screen_and_a_second_collection_does_not_steal_on_edit() {
        let dir = scratch("publish");
        let g = two_screens();
        assert_eq!(publish(&dir, &g, &BTreeSet::new(), &IO), ["A", "B"]);
        assert_eq!(read(&dir, "device-B").unwrap().source.as_deref(), Some("show"));

        // Another collection claims B by naming it for the first time.
        let mut other = Graph::new("other", "Other", Domain::Live);
        let o = other.add_node("out", "output.device", [0.0, 0.0]);
        bind_node(other.node_mut(&o).unwrap(), "B");
        assert_eq!(publish(&dir, &other, &BTreeSet::new(), &IO), ["B"]);
        assert_eq!(read(&dir, "device-B").unwrap().source.as_deref(), Some("other"));

        // `show` is edited and saved again: A is still its own, B is not.
        let before = bound_devices(&g);
        assert_eq!(publish(&dir, &g, &before, &IO), ["A"]);
        assert_eq!(read(&dir, "device-B").unwrap().source.as_deref(), Some("other"));
    }

    #[test]
    fn unbinding_a_screen_releases_it_without_losing_its_graph() {
        let dir = scratch("release");
        let g = two_screens();
        publish(&dir, &g, &BTreeSet::new(), &IO);
        let mut g2 = g.clone();
        let b = g2.nodes.iter().find(|n| node_device(n) == Some("B")).unwrap().id.clone();
        g2.remove_node(&b);
        publish(&dir, &g2, &bound_devices(&g), &IO);
        let released = read(&dir, "device-B").expect("the graph stays");
        assert_eq!(released.source, None);
    }

    #[test]
    fn an_edit_through_a_published_graph_lands_in_the_collection_and_every_screen() {
        let dir = scratch("store");
        let g = two_screens();
        store(&dir, &g, &IO).unwrap();
        // A route edits `device-A` directly: it adds a node.
        let mut edited = read(&dir, "device-A").unwrap();
        let fx = edited.add_node("fx", "fx.shake", [100.0, 0.0]);
        store(&dir, &edited, &IO).unwrap();
        let col = read(&dir, "show").unwrap();
        assert!(col.node(&fx).is_some(), "the edit is in the collection");
        assert_eq!(bound_devices(&col).len(), 2, "the other screen's output survives");
        let b = read(&dir, "device-B").unwrap();
        assert!(b.node(&fx).is_some(), "and the other screen shares the node");
        assert_eq!(read(&dir, "device-A").unwrap().source.as_deref(), Some("show"));
    }

    /// Manual check against a real data folder:
    /// `MVJ_REAL_GRAPHS=~/.cache/metalvj/data/graphs cargo test -p metal-graph adopts_a_copy_of_the_real_graphs -- --ignored --nocapture`.
    /// Works on a scratch copy; nothing in the folder is touched.
    #[test]
    #[ignore]
    fn adopts_a_copy_of_the_real_graphs() {
        let Some(src) = std::env::var_os("MVJ_REAL_GRAPHS") else { return };
        let dir = scratch("real");
        for e in std::fs::read_dir(&src).unwrap().flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                std::fs::copy(e.path(), dir.join(e.file_name())).unwrap();
            }
        }
        let before: Vec<(String, usize)> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), read(&dir, e.file_name().to_str()?.strip_suffix(".json")?)?.nodes.len())))
            .collect();
        let made = adopt_device_graphs(&dir, &IO);
        eprintln!("made {made:?}");
        for (file, nodes) in before {
            let id = file.trim_end_matches(".json");
            let after = read(&dir, id).unwrap();
            assert!(after.nodes.len() >= nodes, "{id} keeps every node");
            eprintln!("{id}: source={:?} nodes={}", after.source, after.nodes.len());
        }
        assert!(adopt_device_graphs(&dir, &IO).is_empty());
    }

    #[test]
    fn device_graphs_without_a_collection_join_one_central_collection() {
        let dir = scratch("adopt");
        for device in ["HDMI", "DP"] {
            let mut g = Graph::new(format!("device-{device}"), device, Domain::Live);
            g.add_node("gen", "gen.turing", [0.0, 0.0]);
            g.add_node("out", "output.device", [200.0, 0.0]);
            write(&dir, &g, &IO).unwrap();
        }
        assert_eq!(adopt_device_graphs(&dir, &IO), [CENTRAL_ID]);
        let col = read(&dir, CENTRAL_ID).unwrap();
        assert_eq!(bound_devices(&col).into_iter().collect::<Vec<_>>(), ["DP", "HDMI"]);
        assert_eq!(
            col.nodes.iter().filter(|n| n.kind == "output.device").count(),
            2,
            "one output node per screen, in one collection"
        );
        for device in ["HDMI", "DP"] {
            let published = read(&dir, &format!("device-{device}")).unwrap();
            assert_eq!(published.source.as_deref(), Some(CENTRAL_ID));
        }
        assert!(adopt_device_graphs(&dir, &IO).is_empty(), "idempotent");
    }

    #[test]
    fn ensure_device_adds_a_screen_to_the_central_collection() {
        let dir = scratch("ensure");
        assert_eq!(ensure_device(&dir, "HDMI", &IO), [CENTRAL_ID]);
        assert_eq!(ensure_device(&dir, "DP", &IO), [CENTRAL_ID]);
        let col = read(&dir, CENTRAL_ID).unwrap();
        assert_eq!(bound_devices(&col).into_iter().collect::<Vec<_>>(), ["DP", "HDMI"]);
        assert_eq!(read(&dir, "device-HDMI").unwrap().source.as_deref(), Some(CENTRAL_ID));
        // Already a screen of central: nothing new is written.
        assert!(ensure_device(&dir, "DP", &IO).is_empty());
    }

    #[test]
    fn a_per_device_collection_is_merged_into_the_central_one() {
        let dir = scratch("merge");
        // What an earlier boot minted: one collection per screen.
        for device in ["HDMI", "DP"] {
            let mut col = Graph::new(format!("col-{device}"), device, Domain::Live);
            col.add_node("gen", "gen.turing", [0.0, 0.0]);
            let out = col.add_node("out", "output.device", [200.0, 0.0]);
            bind_node(col.node_mut(&out).unwrap(), device);
            store(&dir, &col, &IO).unwrap();
        }
        let mut merged = merge_device_collections(&dir, &IO);
        merged.sort();
        assert_eq!(merged, ["col-DP", "col-HDMI"]);
        let col = read(&dir, CENTRAL_ID).unwrap();
        assert_eq!(bound_devices(&col).into_iter().collect::<Vec<_>>(), ["DP", "HDMI"]);
        assert!(read(&dir, "col-HDMI").is_none(), "the per-device file is gone");
        assert!(read(&dir, "col-DP").is_none());
        for device in ["HDMI", "DP"] {
            assert_eq!(read(&dir, &format!("device-{device}")).unwrap().source.as_deref(), Some(CENTRAL_ID));
        }
        assert!(merge_device_collections(&dir, &IO).is_empty(), "idempotent");
    }

    #[test]
    fn every_other_collection_folds_into_the_central_one() {
        let dir = scratch("fold-all");
        let mut central = Graph::new(CENTRAL_ID, CENTRAL_NAME, Domain::Live);
        central.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = central.add_node("out", "output.device", [200.0, 0.0]);
        bind_node(central.node_mut(&out).unwrap(), "HDMI");
        // A duplicate id already in central (the shape the old boot left).
        central.nodes.push(crate::Node::new(out.clone(), "output.device", [0.0, 0.0]));
        store(&dir, &central, &IO).unwrap();
        // An operator's canvas with no screen: its oracle and an unbound output.
        let mut orphan = Graph::new("col-5", "collection 5", Domain::Live);
        let oracle = orphan.add_node("gen", "gen.oracle", [0.0, 0.0]);
        let out5 = orphan.add_node("output", "output.device", [200.0, 0.0]);
        orphan.add_edge(crate::Endpoint::new(&oracle, "image"), crate::Endpoint::new(&out5, "image"));
        store(&dir, &orphan, &IO).unwrap();

        assert_eq!(merge_device_collections(&dir, &IO), ["col-5"]);
        let col = read(&dir, CENTRAL_ID).unwrap();
        assert!(col.nodes.iter().any(|n| n.kind == "gen.oracle"), "the oracle is in central");
        let mut ids: Vec<&str> = col.nodes.iter().map(|n| n.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), col.nodes.len(), "every node id is unique");
        assert_eq!(bound_devices(&col).into_iter().collect::<Vec<_>>(), ["HDMI"], "the unbound output names no screen");
        assert!(read(&dir, "col-5").is_none(), "the orphan file is gone");
    }

    #[test]
    fn a_screen_another_collection_owns_is_not_taken_by_ensure_device() {
        let dir = scratch("owned");
        let mut other = Graph::new("col-other", "Other", Domain::Live);
        let out = other.add_node("out", "output.device", [0.0, 0.0]);
        bind_node(other.node_mut(&out).unwrap(), "HDMI");
        store(&dir, &other, &IO).unwrap();
        assert!(ensure_device(&dir, "HDMI", &IO).is_empty());
        assert_eq!(read(&dir, "device-HDMI").unwrap().source.as_deref(), Some("col-other"));
    }
}
