//! `Graph::lint` — validation that never panics and never returns a `bool`.
//!
//! **Why never a `bool`.** The editor calls this on every drag (see
//! `the design notes` A4's warning), and "invalid" is not one fact, it is
//! a list of facts an operator needs to see at once: a dangling cable *and*
//! a cycle *and* a missing digest are three different repairs, not one
//! failure. A `Vec<Issue>` is also what makes "no cycles" and "orphan nodes
//! are warnings, not errors" the same kind of finding instead of two
//! different code paths.
//!
//! **Why it must not panic on a mid-drag graph.** A graph under construction
//! in the editor can have edges pointing at a node that was just deleted, or
//! two edges into a port that only accepts one — `lint` is what tells the
//! editor that, so it cannot itself assume the graph is already valid.

use std::collections::{HashMap, HashSet};

use crate::{stage_edge_allowed, Capability, Domain, Graph, Node, NodeKind, PortType, Stage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    /// The node this issue is about, when it's about one node in particular
    /// rather than the graph as a whole (a cycle, a mixed domain).
    pub node: Option<String>,
    pub message: String,
}

impl Issue {
    fn error(node: Option<&str>, message: impl Into<String>) -> Issue {
        Issue { severity: Severity::Error, node: node.map(str::to_string), message: message.into() }
    }
    fn warning(node: Option<&str>, message: impl Into<String>) -> Issue {
        Issue {
            severity: Severity::Warning,
            node: node.map(str::to_string),
            message: message.into(),
        }
    }
}

impl Graph {
    /// Runs every rule in `the design notes` §2/A4 and returns every
    /// finding — never stops at the first one, because the editor wants to
    /// show all of them at once.
    pub fn lint(&self, catalog: &[NodeKind]) -> Vec<Issue> {
        let mut issues = Vec::new();
        let by_kind: HashMap<&str, &NodeKind> = catalog.iter().map(|k| (k.kind.as_str(), k)).collect();
        let node_ids: HashSet<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();

        lint_dangling_and_port_types(self, &by_kind, &node_ids, &mut issues);
        lint_stage_order(self, &by_kind, &mut issues);
        lint_cycles(self, &mut issues);
        lint_domain(self, &by_kind, &mut issues);
        lint_output_device_control(self, &by_kind, &mut issues);
        lint_domain_restricted_kinds(self, &mut issues);
        lint_capability_requirements(self, &by_kind, &mut issues);
        lint_text_effects(self, &mut issues);
        lint_mask_stroke(self, &mut issues);
        lint_colour_ports(self, &by_kind, &mut issues);
        lint_ai_ports(self, &by_kind, &mut issues);
        lint_venue_surface(self, &by_kind, &mut issues);
        lint_venue_device_port(self, &mut issues);
        lint_venue_fixture(self, &mut issues);
        lint_material_surfaces(self, &by_kind, &mut issues);
        lint_bindings(self, &mut issues);
        lint_orphans(self, &node_ids, &mut issues);

        issues
    }
}

fn lint_dangling_and_port_types(
    g: &Graph,
    by_kind: &HashMap<&str, &NodeKind>,
    node_ids: &HashSet<&str>,
    issues: &mut Vec<Issue>,
) {
    for e in &g.edges {
        if !node_ids.contains(e.from.node.as_str()) {
            issues.push(Issue::error(None, format!("edge {}: source node {} does not exist", e.id, e.from.node)));
            continue;
        }
        if !node_ids.contains(e.to.node.as_str()) {
            issues.push(Issue::error(None, format!("edge {}: target node {} does not exist", e.id, e.to.node)));
            continue;
        }
        // A binding (`crate::bind`): its ends are a variable and a parameter,
        // not two catalog ports — checked against what the two nodes expose.
        if crate::params::is_param_edge(e) {
            if let Err(message) = crate::bind::check_param_edge(g, by_kind, e) {
                issues.push(Issue::error(None, format!("edge {}: {message}", e.id)));
            }
            continue;
        }
        let from_ty = port_type(g, by_kind, &e.from.node, &e.from.port, true);
        let to_ty = port_type(g, by_kind, &e.to.node, &e.to.port, false);
        match (from_ty, to_ty) {
            (Some(a), Some(b)) if a != b => {
                issues.push(Issue::error(
                    None,
                    format!("edge {}: port type mismatch ({:?} -> {:?})", e.id, a, b),
                ));
            }
            (None, _) => issues.push(Issue::error(
                None,
                format!("edge {}: {} has no output port {}", e.id, e.from.node, e.from.port),
            )),
            (_, None) => issues.push(Issue::error(
                None,
                format!("edge {}: {} has no input port {}", e.id, e.to.node, e.to.port),
            )),
            _ => {}
        }
    }
}

fn port_type(
    g: &Graph,
    by_kind: &HashMap<&str, &NodeKind>,
    node_id: &str,
    port: &str,
    output: bool,
) -> Option<PortType> {
    let node = g.node(node_id)?;
    let kind = by_kind.get(node.kind.as_str())?;
    let ports = if output { &kind.outputs } else { &kind.inputs };
    ports.iter().find(|p| p.name == port).map(|p| p.ty)
}

/// The stage contract of `docs/plan-architecture.md` §2.3: **every edge obeys
/// `rank(from) ≤ rank(to)`**, with the declared [`crate::STAGE_EDGE_EXCEPTIONS`]
/// table. A cable that goes backwards can never render, so this is an error
/// (not a warning) and it is the same rule `metal-perform`'s `compile` will
/// enforce at execution time (LAY.9) — see `docs/plan-architecture.md` §2.4.
///
/// Nodes whose kind is not in the catalog are skipped: `lint_dangling_and_port_types`
/// already reports a cable whose endpoint has no ports, and this rule must not
/// add a second, less useful message for the same graph mid-drag.
fn lint_stage_order(
    g: &Graph,
    by_kind: &HashMap<&str, &NodeKind>,
    issues: &mut Vec<Issue>,
) {
    let stage_of = |id: &str| -> Option<Stage> {
        let node = g.node(id)?;
        let kind = by_kind.get(node.kind.as_str())?;
        Some(kind.stage())
    };
    for e in &g.edges {
        // A binding carries one float that arrives before the picture is
        // drawn, so a script (Modulate) may drive a generator (Source); a
        // loop is still `lint_cycles`' to refuse.
        if crate::params::is_param_edge(e) {
            continue;
        }
        let (Some(from), Some(to)) = (stage_of(&e.from.node), stage_of(&e.to.node)) else {
            continue;
        };
        if let Err(message) = stage_edge_allowed(from, to) {
            issues.push(Issue::error(
                Some(&e.to.node),
                format!("edge {} ({} → {}): {}", e.id, e.from.node, e.to.node, message),
            ));
        }
    }
}

/// DFS with three-colour marks. Dangling edges (checked above) are simply
/// skipped here rather than followed, so a half-built graph can never make
/// this loop or panic.
fn lint_cycles(g: &Graph, issues: &mut Vec<Issue>) {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Gray,
        Black,
    }
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &g.edges {
        adj.entry(e.from.node.as_str()).or_default().push(e.to.node.as_str());
    }
    let mut mark: HashMap<&str, Mark> =
        g.nodes.iter().map(|n| (n.id.as_str(), Mark::White)).collect();
    let mut found_cycle = false;

    // Sorted node order so the walk (and which node gets blamed) is
    // deterministic across runs of the same graph.
    let mut ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
    ids.sort_unstable();

    fn visit<'a>(
        node: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        mark: &mut HashMap<&'a str, Mark>,
        found_cycle: &mut bool,
    ) {
        if *found_cycle {
            return;
        }
        match mark.get(node) {
            Some(Mark::Black) | None => return,
            Some(Mark::Gray) => {
                *found_cycle = true;
                return;
            }
            Some(Mark::White) => {}
        }
        mark.insert(node, Mark::Gray);
        if let Some(next) = adj.get(node) {
            for &n in next {
                visit(n, adj, mark, found_cycle);
                if *found_cycle {
                    break;
                }
            }
        }
        mark.insert(node, Mark::Black);
    }

    for id in ids {
        visit(id, &adj, &mut mark, &mut found_cycle);
        if found_cycle {
            break;
        }
    }

    if found_cycle {
        issues.push(Issue::error(None, "the graph has a cycle; feedback loops are not supported"));
    }
}

/// Rule 3: the domain is inferred from which audio kind is present, not
/// chosen. A graph declaring both, or declaring one and carrying the wrong
/// `domain` field, is an error.
fn lint_domain(g: &Graph, by_kind: &HashMap<&str, &NodeKind>, issues: &mut Vec<Issue>) {
    let _ = by_kind;
    let has_live = g.nodes.iter().any(|n| n.kind == "audio.live");
    let has_file = g.nodes.iter().any(|n| n.kind == "audio.file");

    if has_live && has_file {
        issues.push(Issue::error(
            None,
            "a graph cannot have both audio.live and audio.file — the domain is ambiguous",
        ));
        return;
    }

    // `has_live`/`has_file` are already known not to both be true here, so
    // this is exactly the answer `Graph::inferred_domain` would give — asking
    // it directly is what keeps this rule and anything showing the operator
    // "what domain is this graph becoming" from computing it two different
    // ways that could drift apart.
    if let Some(inferred) = g.inferred_domain() {
        if inferred != g.domain {
            issues.push(Issue::error(
                None,
                format!(
                    "declared domain {:?} does not match the audio node present ({:?})",
                    g.domain, inferred
                ),
            ));
        }
    }
}

/// Rule 2: `output.device` requires a `control` connected.
fn lint_output_device_control(g: &Graph, by_kind: &HashMap<&str, &NodeKind>, issues: &mut Vec<Issue>) {
    let _ = by_kind;
    for n in &g.nodes {
        if n.kind != "output.device" {
            continue;
        }
        let has_control = g
            .edges
            .iter()
            .any(|e| e.to.node == n.id && e.to.port == "control");
        if !has_control {
            issues.push(Issue::error(
                Some(&n.id),
                "output.device requires a control (digest) connected",
            ));
        }
    }
}

/// Rule 4: `export.video` only in `Offline`, `output.device` only in `Live`
/// — and `capture` only in `Live` (Conjunto G3).
///
/// **Where the rule lives.** The allowed domains are
/// [`crate::kind_domains`]'s list, not a second one kept here: this function
/// reads it to decide *whether* to complain, and only spells out *how* to
/// repair each case. The catalog stamps the same list on every
/// [`NodeKind`](crate::NodeKind), which is what `GET /catalog?domain=` and
/// the `+ nodo` menu filter by (R22.5a).
///
/// **The one exception the doc calls out is decided here, not deferred to the
/// editor.** A capture device is a thing happening *now*; a graph rendered
/// offline, tick by tick with no wall clock, has nothing for "now" to mean.
/// The apparent exception — freezing a captured frame and exporting it — is
/// not actually one: the operator freezes the frame *while the graph is
/// still Live* and saves it as an image, and that image is read back by a
/// `still` node, which carries no domain restriction at all. So the crossing
/// happens through a file on disk, never by letting a `capture` node itself
/// sit in an `Offline` graph.
fn lint_domain_restricted_kinds(g: &Graph, issues: &mut Vec<Issue>) {
    for n in &g.nodes {
        // Whether this kind can be here at all is the catalog's own answer:
        // `crate::kind_domains` is the one list `NodeKind` stamps on every
        // catalog entry (and the menu filters by), never a second match here
        // that could drift from it. This is R22.5a's "un solo lugar".
        if crate::kind_domains(&n.kind).contains(&g.domain) {
            continue;
        }
        // A lamp in the room is a thing happening now, the same way a capture
        // is — there is no wall clock in an Offline graph for it to be lit
        // at, and a `.mp4` has no channel to write a DMX universe into. The
        // *authoring* of a light cue is not restricted at all: those are the
        // `sig_*` floats on the model's row, and modo edit keyframes them in
        // an Offline graph exactly like any other float. It is only the wire
        // out of the building that has to be Live.
        if n.kind.starts_with("out.") && g.domain == Domain::Offline {
            issues.push(Issue::error(
                Some(&n.id),
                format!(
                    "{} only exists in Live graphs — author the signal on the model's \
                     sig_* floats here, and let a Live graph send it",
                    n.kind
                ),
            ));
            continue;
        }
        match (n.kind.as_str(), g.domain) {
            ("export.video", Domain::Live) => {
                issues.push(Issue::error(Some(&n.id), "export.video only exists in Offline graphs"))
            }
            ("output.device", Domain::Offline) => {
                issues.push(Issue::error(Some(&n.id), "output.device only exists in Live graphs"))
            }
            ("capture", Domain::Offline) => issues.push(Issue::error(
                Some(&n.id),
                "capture only exists in Live graphs — freeze a frame to a still image \
                 and read it back with a still node in an Offline graph instead",
            )),
            // A device mode is a property of a screen in a room. An Offline
            // graph is a timeline being rendered to a file: it has no device
            // to be in a mode, and automix — which picks what to show from
            // what the router hears right now — has nothing to listen to.
            (kind, Domain::Offline) if kind == crate::STACK_MODE_KIND || crate::is_compose_kind(kind) => {
                issues.push(Issue::error(
                    Some(&n.id),
                    format!(
                        "{kind} only exists in Live graphs — a timeline renders the segments \
                         it holds, it does not choose a stack"
                    ),
                ))
            }
            // The pixel map drives the rig in the room; an Offline graph has
            // no wires to send on (M3/F1).
            (kind, Domain::Offline)
                if kind == crate::VENUE_FIXTURES_KIND || kind == crate::VENUE_FIXTURE_KIND =>
            {
                issues.push(Issue::error(
                    Some(&n.id),
                    format!(
                        "{kind} only exists in Live graphs — a fixture node drives the rig in the room"
                    ),
                ))
            }
            // `audio.live`/`audio.file` are `lint_domain`'s business: the
            // declared-domain mismatch it reports already names exactly which
            // audio node is wrong, and a second, differently-worded error for
            // the same fact would be noise. `kind_domains` still knows their
            // one domain, which is what the menu filters by.
            _ => {}
        }
    }

    // One composition, one statement. Two is not a device that is in two
    // states; it is a graph that cannot answer the question. Flagged rather
    // than resolved by position, because "the first one wins" is a rule
    // nobody can see on the canvas.
    //
    // A legacy `stack.mode` node counts as a composition for this rule: the
    // render path reads `compose.*` first and `stack.mode` only as the
    // fallback, so a graph carrying both is exactly the ambiguous case — the
    // migration converts in place rather than adding a second node, and this
    // is what catches a file where something else did not.
    let compositions: Vec<&Node> = g
        .nodes
        .iter()
        .filter(|n| n.kind == crate::STACK_MODE_KIND || crate::is_compose_kind(&n.kind))
        .collect();
    if compositions.len() > 1 {
        for n in &compositions {
            issues.push(Issue::error(
                Some(&n.id),
                format!(
                    "{} composition nodes in one graph — a device is in one mode, so delete \
                     all but one",
                    compositions.len()
                ),
            ));
        }
    }
}

/// Capability checks (LAY.3, `docs/plan-architecture.md` D4).
///
/// **Warnings, not errors, unless a combination can never work at all.** A
/// stage that expects an image and has been given none, or a signal
/// destination with no signal, is usually a graph still being wired — the same
/// reason `lint_orphans` is a warning. Nothing here is yet an error, by
/// design: the plan says existing stacks must not be pushed from warning to
/// error without a case that proves they cannot render, and no such case
/// exists for these two.
///
/// Orphan nodes are skipped: `lint_orphans` already names them, and adding a
/// second message about the same unconnected node is noise on every mid-drag
/// graph.
fn lint_capability_requirements(
    g: &Graph,
    by_kind: &HashMap<&str, &NodeKind>,
    issues: &mut Vec<Issue>,
) {
    let touched = |id: &str| g.edges.iter().any(|e| e.from.node == id || e.to.node == id);
    let has_in = |id: &str, port: &str| {
        g.edges.iter().any(|e| e.to.node == id && e.to.port == port)
    };
    for n in &g.nodes {
        let Some(kind) = by_kind.get(n.kind.as_str()) else { continue };
        if !touched(&n.id) {
            continue;
        }
        let caps = kind.capabilities();
        if caps.has(Capability::ImageIn) && !has_in(&n.id, "image") {
            issues.push(Issue::warning(
                Some(&n.id),
                format!("{} expects an image but none is connected", n.kind),
            ));
        }
        if caps.has(Capability::SignalOut) && !has_in(&n.id, "signal") {
            issues.push(Issue::warning(
                Some(&n.id),
                format!("{} sends a signal but none is connected", n.kind),
            ));
        }
    }
}

/// 22-sept-2026 (tarde): a `mask` node whose `stroke` port is wired needs
/// `config.stroke_width > 0` — otherwise the cable is drawn but the outline
/// it was meant to fill has no width and paints nothing, which is a silent
/// dead cable rather than a visible one. The reverse is not an error: a
/// width with no material (and no colour) simply has nothing to paint, and
/// saying so here would fight the operator mid-edit, when the width is set
/// first and the cable arrives a moment later.
fn lint_mask_stroke(g: &Graph, issues: &mut Vec<Issue>) {
    for n in &g.nodes {
        if n.kind != "mask" {
            continue;
        }
        let wired = g.edges.iter().any(|e| e.to.node == n.id && e.to.port == "stroke");
        if !wired {
            continue;
        }
        let width = n.config.get("stroke_width").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        if width <= 0.0 {
            issues.push(Issue::error(
                Some(&n.id),
                "mask has a source wired to `stroke` but no `stroke_width` — \
                 set a width above 0 for the outline to exist",
            ));
        }
    }
}

/// S.W: an `fx.text.*` node transforms glyphs, so it can only hang off a
/// `text.source` — never off a `gen.*`. A pixel-level effect on a generator
/// belongs in the `fx.*` vocabulary instead, and the design is explicit:
/// "lint rechaza uno que no cuelgue de `text.source`". An error, not a
/// warning: the node can never do anything where it is.
fn lint_text_effects(g: &Graph, issues: &mut Vec<Issue>) {
    for n in &g.nodes {
        if !n.kind.starts_with("fx.text.") {
            continue;
        }
        // Bindings are skipped: a float driving the amount says nothing about
        // which picture the effect hangs off.
        let feeds = |id: &str| -> Vec<&str> {
            g.edges
                .iter()
                .filter(|e| e.to.node == id && !crate::params::is_param_edge(e))
                .map(|e| e.from.node.as_str())
                .collect()
        };
        let mut seen: HashSet<&str> = HashSet::new();
        let mut queue: Vec<&str> = feeds(&n.id);
        let mut from_text = false;
        while let Some(id) = queue.pop() {
            if !seen.insert(id) {
                continue;
            }
            match g.node(id).map(|m| m.kind.as_str()) {
                Some(crate::TEXT_SOURCE_KIND) => {
                    from_text = true;
                    break;
                }
                _ => queue.extend(feeds(id)),
            }
        }
        if !from_text {
            issues.push(Issue::error(
                Some(&n.id),
                format!("{} can only transform a text.source", n.kind),
            ));
        }
    }
}

/// K: a colour source drives a parameter, nothing else. A `color` or `palette`
/// node's `control` output carries colours, so the only cable it may feed is a
/// `param:` binding — a picture input would be a colour where an image belongs,
/// and a stage's `control` input would be a colour read as a digest column.
/// An error, not a warning: the cable can never do anything where it lands.
///
/// The colour *pair* (a colour var onto a colour slot, never onto a float) is
/// `crate::bind::check_param_edge`'s business; this rule is the other half, an
/// ordinary cable from a colour node anywhere.
fn lint_colour_ports(g: &Graph, by_kind: &HashMap<&str, &NodeKind>, issues: &mut Vec<Issue>) {
    let is_colour = |id: &str| {
        g.node(id)
            .and_then(|n| by_kind.get(n.kind.as_str()))
            .map(|k| k.family == crate::NodeFamily::Colour)
            .unwrap_or(false)
    };
    for e in &g.edges {
        if !is_colour(&e.from.node) {
            continue;
        }
        if crate::params::is_param_edge(e) {
            continue;
        }
        issues.push(Issue::error(
            Some(&e.from.node),
            format!(
                "{} feeds {}'s {} port — a colour only lands on a parameter (`param:<name>`)",
                e.from.node, e.to.node, e.to.port
            ),
        ));
    }
}

/// S.AM: a trained reaction hears the music and drives a picture — nothing
/// else. An `ai.*` node's `control` input may only come from an `audio.*` or
/// `sensor.*` source (the digest row it was trained on), and its `control`
/// output may only enter a `gen.*`/`text.source`/`fx.*` consumer (a source's
/// own floats, a text setting or an effect's amount). Anything else is a
/// cable that can never mean anything, so it is an error naming the node.
///
/// **No input is a warning, not an error.** An `ai.*` node dropped on the
/// canvas before its `audio.live` is wired is the ordinary mid-edit state,
/// and the plan is explicit: "an `ai.*` with no input is a warning, not an
/// error". `lint_orphans` covers the wholly unconnected node; this warning is
/// the narrower "it has an output but nothing to hear".
fn lint_ai_ports(g: &Graph, _by_kind: &HashMap<&str, &NodeKind>, issues: &mut Vec<Issue>) {
    let kind_of = |id: &str| g.node(id).map(|n| n.kind.as_str()).unwrap_or_default();
    for n in &g.nodes {
        if !n.kind.starts_with("ai.") {
            continue;
        }
        // A wholly unconnected node is `lint_orphans`'s to name; adding a
        // second warning for the same fact is noise on every mid-drag graph
        // (the same reason `lint_capability_requirements` skips untouched
        // nodes). "No input" here means "it is wired, but not to a source".
        if !g.edges.iter().any(|e| e.from.node == n.id || e.to.node == n.id) {
            continue;
        }
        // Bindings carry one float and are not a digest cable — the same
        // exclusion `lint_colour_ports` makes for `param:` edges.
        let mut has_input = false;
        for e in &g.edges {
            if crate::params::is_param_edge(e) {
                continue;
            }
            if e.to.node == n.id && e.to.port == "control" {
                has_input = true;
                let from = kind_of(&e.from.node);
                if !(from.starts_with("audio.") || from.starts_with("sensor.")) {
                    issues.push(Issue::error(
                        Some(&n.id),
                        format!(
                            "{} hears {}'s {} — an ai node only accepts control from audio.* or sensor.*",
                            n.kind, e.from.node, e.from.port
                        ),
                    ));
                }
            }
            if e.from.node == n.id && e.from.port == "control" {
                let to = kind_of(&e.to.node);
                if !(to.starts_with("gen.")
                    || to == crate::TEXT_SOURCE_KIND
                    || to.starts_with("fx."))
                {
                    issues.push(Issue::error(
                        Some(&n.id),
                        format!(
                            "{} feeds {}'s {} — an ai node's control only enters gen.*/text.source/fx.*",
                            n.kind, e.to.node, e.to.port
                        ),
                    ));
                }
            }
        }
        if !has_input {
            issues.push(Issue::warning(
                Some(&n.id),
                format!("{} has no control input — wire an audio.* or sensor.* source into it", n.kind),
            ));
        }
    }
}

/// P1: the `venue.surface` node. Three rules, all about the space a contour
/// lives in:
///
/// - a `venue.surface` that never reaches an `output.device` warps a picture
///   for nothing — there is no screen at the end of its chain. Error.
/// - a `mask` downstream of a `venue.surface` cuts what the surface has
///   already warped into device space, while the mask's own contour is in
///   source pixels: the two disagree about which space the points are in.
///   Error, and the message says where the mask belongs.
/// - two `venue.surface` in series warp a plane that is already device space;
///   the second has no surface geometry left to stand on. Error.
///
/// The walks follow `image` edges only and skip bindings
/// ([`crate::params::is_param_edge`]): a float driving a parameter says
/// nothing about which picture a node is part of.
fn lint_venue_surface(g: &Graph, by_kind: &HashMap<&str, &NodeKind>, issues: &mut Vec<Issue>) {
    let is_surface = |id: &str| g.node(id).is_some_and(|n| n.kind == crate::VENUE_SURFACE_KIND);
    let is_device = |id: &str| {
        g.node(id)
            .and_then(|n| by_kind.get(n.kind.as_str()))
            .is_some_and(|k| k.family == crate::NodeFamily::OutputDevice)
    };

    for n in &g.nodes {
        if n.kind == crate::VENUE_SURFACE_KIND {
            if !image_reaches(g, &n.id, false, &is_device) {
                issues.push(Issue::error(
                    Some(&n.id),
                    "venue.surface never reaches an output.device — it warps a picture with no screen to land on",
                ));
            }
            if image_reaches(g, &n.id, true, &is_surface) {
                issues.push(Issue::error(
                    Some(&n.id),
                    "two venue.surface in series — the first has already warped into device space",
                ));
            }
        }
        if n.kind == "mask" && image_reaches(g, &n.id, true, &is_surface) {
            issues.push(Issue::error(
                Some(&n.id),
                "masks are cut in source space; put the mask before the surface",
            ));
        }
    }
}

/// F1: the `venue.fixture` node. Three rules, all about its pointer:
///
/// - a node that names no venue or no fixture has an empty pointer and drives
///   nothing — a warning, the mid-drag state of a node just dropped before the
///   operator points it at a row;
/// - two nodes naming the same `(venue, fixture)` both write the same head and
///   the canvas cannot tell them apart — an error;
/// - `colour_from = "model"` reads the node's `colour` input, so with nothing
///   wired there it has no colour to send — an error naming the port.
///
/// **What this rule cannot see.** `lint` is a reading of the graph alone
/// ([`Graph::lint`] takes a catalog, not a venue store), so it cannot know
/// whether the row a pointer names still exists. That half is the runtime's
/// (a `venue.fixture` node whose row is gone drives nothing and sends nothing
/// — `metal_perform::wire`); this rule flags the pointer that names nothing at
/// all (decision 1).
fn lint_venue_fixture(g: &Graph, issues: &mut Vec<Issue>) {
    let str_at = |n: &Node, key: &str| {
        n.config
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let has_in =
        |id: &str, port: &str| g.edges.iter().any(|e| e.to.node == id && e.to.port == port);

    let mut by_row: HashMap<(String, String), Vec<String>> = HashMap::new();
    for n in &g.nodes {
        if n.kind != crate::VENUE_FIXTURE_KIND {
            continue;
        }
        let venue = str_at(n, "venue");
        let fixture = str_at(n, "fixture");
        if venue.is_empty() || fixture.is_empty() {
            issues.push(Issue::warning(
                Some(&n.id),
                format!("{} names no venue fixture — an orphan node drives nothing", n.kind),
            ));
            continue;
        }
        by_row.entry((venue, fixture)).or_default().push(n.id.clone());
        if str_at(n, "colour_from") == "model" && !has_in(&n.id, "colour") {
            issues.push(Issue::error(
                Some(&n.id),
                format!(
                    "{} is set to colour_from=model but nothing is wired to its `colour` input",
                    n.kind
                ),
            ));
        }
    }
    let mut rows: Vec<(&(String, String), &Vec<String>)> = by_row.iter().collect();
    rows.sort();
    for ((_, fixture), ids) in rows {
        if ids.len() > 1 {
            for id in ids {
                issues.push(Issue::error(
                    Some(id),
                    format!(
                        "{} nodes drive the same venue fixture {fixture:?} — keep one, or give them \
                         different fixtures",
                        ids.len()
                    ),
                ));
            }
        }
    }
}

/// The wire after the device (2026-10-22): `output.device.venue` is an
/// `image` output that carries the finished picture of *this hardware
/// output*, and exactly one kind may read it — `venue.surface`, whose picture
/// is that frame warped onto one surface of the room.
///
/// **The direction.** `the design notes` fixes the chain
/// (`gen → fx… → mask → venue.surface → output.device`; the device is last)
/// and the operator asked for "a wire after the device". Both hold with an
/// **output** on the device read by a surface, never an input on it:
/// `venue.surface.image → output.device.image` keeps *driving* the screen;
/// `output.device.venue → venue.surface.image` lets the room's grid read the
/// finished frame back. The second cable is a backward edge by stage rank
/// (the device is `Output`, the surface `Mask`), so it is one entry in
/// [`crate::STAGE_EDGE_EXCEPTIONS`] and this rule is what keeps that
/// exception to one pair and one port.
///
/// What this rule refuses, by name:
///
/// - a `venue` cable from a device into anything but a `venue.surface`. A
///   `venue.fixtures`/`venue.fixture` node samples the frame it is *fed*, at
///   its footprints; reading the device's frame there would be a second path
///   to one thing (rule 6), not a new capability.
/// - a `venue` cable onto anything but the surface's `image`: a device's
///   frame lands on a surface, it is not a control row.
/// - `venue.surface → output.device → venue.surface`: the surface already
///   carries this device's picture, so the back edge closes a two-node cycle.
///   `compile` would see it and `lint_cycles` names it; this names *why*.
fn lint_venue_device_port(g: &Graph, issues: &mut Vec<Issue>) {
    let venue_port = |e: &crate::Edge| {
        g.node(&e.from.node).is_some_and(|n| n.kind == "output.device") && e.from.port == "venue"
    };
    for e in &g.edges {
        if crate::params::is_param_edge(e) || !venue_port(e) {
            continue;
        }
        let target = g.node(&e.to.node);
        if target.map(|n| n.kind.as_str()) != Some(crate::VENUE_SURFACE_KIND) {
            issues.push(Issue::error(
                Some(&e.from.node),
                format!(
                    "output.device → {} (port {}): the device's `venue` output may only be read by a \
                     venue.surface — the surface is the view of this device's frame",
                    target.map(|n| n.kind.as_str()).unwrap_or("?"),
                    e.to.port
                ),
            ));
            continue;
        }
        if e.to.port != "image" {
            issues.push(Issue::error(
                Some(&e.from.node),
                format!(
                    "output.device → venue.surface (port {}): the frame lands on the surface's \
                     `image`, not on `{}`",
                    e.to.port, e.to.port
                ),
            ));
            continue;
        }
        let surface_feeds_device = g.edges.iter().any(|other| {
            other.from.node == e.to.node
                && other.from.port == "image"
                && other.to.node == e.from.node
                && other.to.port == "image"
        });
        if surface_feeds_device {
            issues.push(Issue::error(
                Some(&e.from.node),
                "venue.surface → output.device → venue.surface is a cycle: the surface already carries \
                 this device's picture, so the back edge reads the same frame twice — the device is \
                 last (rule 3: one origin, an output is a view of the frame)",
            ));
        }
    }
}

/// Whether `start` reaches a node `want` accepts, walking `image` edges
/// (`upstream` when true, downstream otherwise) and skipping bindings
/// ([`crate::params::is_param_edge`]): a float driving a parameter says
/// nothing about which picture a node is part of. A cycle is guarded by the
/// visited set, so a mid-drag graph can never make this loop.
fn image_reaches(g: &Graph, start: &str, upstream: bool, want: &dyn Fn(&str) -> bool) -> bool {
    let step = |id: &str| -> Vec<&str> {
        g.edges
            .iter()
            .filter(|e| !crate::params::is_param_edge(e) && if upstream {
                e.to.node == id && e.to.port == "image"
            } else {
                e.from.node == id && e.from.port == "image"
            })
            .map(|e| if upstream { e.from.node.as_str() } else { e.to.node.as_str() })
            .collect()
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut queue: Vec<&str> = step(start);
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        if want(id) {
            return true;
        }
        queue.extend(step(id));
    }
    false
}


/// One driver per parameter (`crate::bind`): a `param:` port with more than one
/// cable, or an effect whose `amount` is bound while its legacy `control`
/// cable still drives the same float, is a parameter the graph cannot give
/// one value — an error on the node, never a silent "last cable wins".
fn lint_bindings(g: &Graph, issues: &mut Vec<Issue>) {
    let mut drivers: HashMap<(&str, &str), usize> = HashMap::new();
    for e in &g.edges {
        if let Some(param) = crate::params::param_name(&e.to.port) {
            *drivers.entry((e.to.node.as_str(), param)).or_default() += 1;
        }
    }
    let mut keys: Vec<(&(&str, &str), &usize)> = drivers.iter().collect();
    keys.sort();
    for ((node, param), count) in keys {
        if *count > 1 {
            issues.push(Issue::error(
                Some(node),
                format!("parameter {param} of {node} is driven by {count} cables — keep one"),
            ));
        }
        let kind = g.node(node).map(|n| n.kind.as_str()).unwrap_or_default();
        if *param == "amount"
            && crate::bind::control_drives_amount(kind)
            && g.edges.iter().any(|e| e.to.node == *node && e.to.port == "control")
        {
            issues.push(Issue::error(
                Some(node),
                format!(
                    "the amount of {node} is driven twice — by its control cable and by a binding; \
                     remove one"
                ),
            ));
        }
    }
}

/// M4 of the materials lote: a `mat.*` node acts on a 3D model's surfaces, so
/// it only does anything when the model it hangs off declares at least one
/// (`tex.<part>` inputs in the catalog). On a model with no surfaces it is a
/// **warning and a no-op**, never an error (the materials brief, Decision 4):
/// the node is harmless where it sits, it just has nothing to paint. The model
/// is found by walking the `image` cable upstream to the feeding `gen.*`.
fn lint_material_surfaces(
    g: &Graph,
    by_kind: &HashMap<&str, &NodeKind>,
    issues: &mut Vec<Issue>,
) {
    for n in &g.nodes {
        if !crate::materials::is_material_kind(&n.kind) {
            continue;
        }
        let Some(gen) = upstream_gen(g, &n.id) else { continue };
        let Some(model) = g.node(&gen) else { continue };
        let has_parts = by_kind
            .get(model.kind.as_str())
            .map(|k| k.inputs.iter().any(|p| p.name.starts_with("tex.")))
            .unwrap_or(false);
        if !has_parts {
            issues.push(Issue::warning(
                Some(&n.id),
                format!(
                    "{} hangs off {}, which declares no surfaces — the material is a no-op",
                    n.kind, model.kind
                ),
            ));
        }
    }
}

/// The first `gen.*` node upstream of `node` over the picture cables, skipping
/// bindings (a float driving a material says nothing about which model it acts
/// on). `None` when nothing upstream is a generator.
fn upstream_gen(g: &Graph, node: &str) -> Option<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = vec![g.node(node).map(|n| n.id.as_str())?];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for e in g.edges.iter().filter(|e| e.to.node == id && !crate::params::is_param_edge(e)) {
            match g.node(&e.from.node).map(|n| n.kind.as_str()) {
                Some(k) if k.starts_with("gen.") => return Some(e.from.node.clone()),
                Some(_) => stack.push(e.from.node.as_str()),
                None => {}
            }
        }
    }
    None
}

/// Nodes with no edge touching them at all — legitimate (a generator being
/// wired up mid-drag) but worth flagging, hence a warning and not an error.
fn lint_orphans(g: &Graph, node_ids: &HashSet<&str>, issues: &mut Vec<Issue>) {
    let _ = node_ids;
    let mut touched: HashSet<&str> = HashSet::new();
    for e in &g.edges {
        touched.insert(e.from.node.as_str());
        touched.insert(e.to.node.as_str());
    }
    for n in &g.nodes {
        // `stack.mode` and every `compose.*` kind have no ports, so
        // "unconnected" is their only possible state and warning about it
        // would be noise on every graph that has one — see
        // `NodeFamily::Stack`/`NodeFamily::Compose` for why they are outside
        // the chain.
        if n.kind == crate::STACK_MODE_KIND || crate::is_compose_kind(&n.kind) {
            continue;
        }
        // An embedded node (`ai.embed`) is owned by its source, not a free
        // node: "unconnected" is not its to answer, and the source's own
        // cables are what touch it.
        if n.embedded_in.is_some() {
            continue;
        }
        if !touched.contains(n.id.as_str()) {
            issues.push(Issue::warning(Some(&n.id), "node is not connected to anything"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Domain, Endpoint, Graph, NodeFamily, NodeKind, Port, PortType};

    fn catalog() -> Vec<NodeKind> {
        // Built through the same constructor the real catalog uses, so the
        // `menu`/`domains` fields R22.5a added cannot make this fixture drift
        // from what `NodeKind::catalog` stamps on a kind.
        vec![
            NodeKind {
                params: vec![
                    crate::params::Param {
                        name: "scale".to_string(),
                        min: 1.0,
                        max: 12.0,
                        default: 1.0,
                        value: None,
                        kind: crate::params::ParamKind::Float,
                        index: Some(0),
                    },
                    crate::params::Param {
                        name: "stroke".to_string(),
                        min: 0.0,
                        max: 1.0,
                        default: 1.0,
                        value: None,
                        kind: crate::params::ParamKind::Colour,
                        index: Some(1),
                    },
                ],
                ..NodeKind::new(
                    "gen.turing",
                    NodeFamily::Generator,
                    "Turing",
                    vec![Port::new("control", PortType::Control)],
                    vec![Port::new("image", PortType::Image)],
                )
            },
            NodeKind::new(
                "audio.live",
                NodeFamily::Audio,
                "Audio live",
                vec![],
                vec![Port::new("control", PortType::Control)],
            ),
            NodeKind::new(
                "audio.file",
                NodeFamily::Audio,
                "Audio file",
                vec![],
                vec![Port::new("control", PortType::Control)],
            ),
            NodeKind::new(
                "output.device",
                NodeFamily::OutputDevice,
                "Output",
                vec![Port::new("image", PortType::Image), Port::new("control", PortType::Control)],
                vec![Port::new("venue", PortType::Image)],
            ),
            NodeKind::new(
                "export.video",
                NodeFamily::ExportVideo,
                "Export",
                vec![Port::new("image", PortType::Image)],
                vec![],
            ),
            NodeKind::new(
                "capture",
                NodeFamily::Capture,
                "Capture",
                vec![],
                vec![Port::new("image", PortType::Image)],
            ),
            NodeKind::new(
                "fx.shake",
                NodeFamily::Effect,
                "Shake",
                vec![Port::new("image", PortType::Image)],
                vec![Port::new("image", PortType::Image)],
            ),
            NodeKind::new(
                "text.source",
                NodeFamily::Text,
                "Texto",
                vec![Port::new("control", PortType::Control)],
                vec![Port::new("image", PortType::Image)],
            ),
            NodeKind::new(
                "fx.text.wave",
                NodeFamily::Effect,
                "wave",
                vec![Port::new("image", PortType::Image), Port::new("control", PortType::Control)],
                vec![Port::new("image", PortType::Image)],
            ),
            NodeKind::new(
                "mask",
                NodeFamily::Mask,
                "Mask",
                vec![Port::new("image", PortType::Image)],
                vec![Port::new("image", PortType::Image)],
            ),
            NodeKind::new(
                crate::VENUE_SURFACE_KIND,
                NodeFamily::Venue,
                "Venue Surface",
                vec![Port::new("image", PortType::Image), Port::new("control", PortType::Control)],
                vec![Port::new("image", PortType::Image)],
            ),
            NodeKind::new(
                "out.sacn",
                NodeFamily::OutputSignal,
                "DMX",
                vec![Port::new("signal", PortType::Control), Port::new("image", PortType::Image)],
                vec![],
            ),
            NodeKind::new(
                crate::COLOR_KIND,
                NodeFamily::Colour,
                "Colour",
                vec![],
                vec![Port::new("control", PortType::Control)],
            ),
            NodeKind::new(
                crate::AI_COLOURIST_KIND,
                NodeFamily::Ai,
                "Colourist",
                vec![Port::new("control", PortType::Control)],
                vec![Port::new("control", PortType::Control)],
            ),
            NodeKind::new(
                crate::VENUE_FIXTURE_KIND,
                NodeFamily::Venue,
                "Venue fixture",
                vec![
                    Port::new("image", PortType::Image),
                    Port::new("colour", PortType::Control),
                    Port::new("signal", PortType::Control),
                ],
                vec![],
            ),
            // A 3D model that declares a surface part, for the material lint.
            NodeKind::new(
                "gen.monster",
                NodeFamily::Generator,
                "Monstruo",
                vec![Port::material("tex.skin", PortType::Image)],
                vec![Port::new("image", PortType::Image)],
            ),
            NodeKind::new(
                "mat.wood",
                NodeFamily::Material,
                "wood",
                vec![Port::new("image", PortType::Image), Port::new("control", PortType::Control)],
                vec![Port::new("image", PortType::Image)],
            ),
        ]
    }

    #[test]
    fn incompatible_port_types_are_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [1.0, 0.0]);
        // control -> image is a type mismatch.
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "control"));
        let issues = g.lint(&catalog());
        assert!(issues.iter().any(|i| i.severity == Severity::Error && i.message.contains("mismatch")));
    }

    /// A `mat.*` on a model with no surfaces is a **warning and a no-op**, and
    /// the same node on a model that declares a part is silent — the materials
    /// brief's Decision 4, the one lint rule this family adds.
    #[test]
    fn a_material_on_a_model_without_surfaces_is_a_warning_not_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let bare = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let mat = g.add_node("m", "mat.wood", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&bare, "image"), Endpoint::new(&mat, "image"));
        let issues = g.lint(&catalog());
        let hit = issues
            .iter()
            .find(|i| i.node.as_deref() == Some(mat.as_str()) && i.message.contains("no surfaces"));
        assert_eq!(
            hit.map(|i| i.severity),
            Some(Severity::Warning),
            "a material on a model with no surfaces must warn, never error"
        );

        let part = g.add_node("gen", "gen.monster", [0.0, 1.0]);
        let mat2 = g.add_node("m", "mat.wood", [1.0, 1.0]);
        g.add_edge(Endpoint::new(&part, "image"), Endpoint::new(&mat2, "image"));
        let issues = g.lint(&catalog());
        assert!(
            !issues
                .iter()
                .any(|i| i.node.as_deref() == Some(mat2.as_str()) && i.message.contains("no surfaces")),
            "a model that declares a part must not warn"
        );
    }

    #[test]
    fn a_cycle_is_rejected() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let a = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let b = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new(&b, "control"));
        g.add_edge(Endpoint::new(&b, "image"), Endpoint::new(&a, "control"));
        let issues = g.lint(&catalog());
        assert!(issues.iter().any(|i| i.message.contains("cycle")));
    }

    #[test]
    fn a_text_effect_on_a_text_source_is_fine() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let text = g.add_node("t", "text.source", [0.0, 0.0]);
        let wave = g.add_node("w", "fx.text.wave", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&text, "image"), Endpoint::new(&wave, "image"));
        let issues = g.lint(&catalog());
        assert!(
            !issues.iter().any(|i| i.message.contains("can only transform a text.source")),
            "a text effect hanging off a text.source must pass"
        );
    }

    #[test]
    fn a_text_effect_hanging_off_a_generator_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let wave = g.add_node("w", "fx.text.wave", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&wave, "image"));
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.message.contains("can only transform a text.source")),
            "a text effect on a generator can never do anything"
        );
    }

    #[test]
    fn mixed_audio_kinds_are_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("audio", "audio.live", [0.0, 0.0]);
        g.add_node("audio", "audio.file", [0.0, 1.0]);
        let issues = g.lint(&catalog());
        assert!(issues.iter().any(|i| i.message.contains("ambiguous")));
    }

    #[test]
    fn output_device_without_control_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("out", "output.device", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("requires a control")));
    }

    #[test]
    fn output_device_with_control_connected_has_no_control_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out, "control"));
        let issues = g.lint(&catalog());
        assert!(!issues.iter().any(|i| i.message.contains("requires a control")));
    }

    #[test]
    fn export_video_in_live_graph_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("exp", "export.video", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(issues.iter().any(|i| i.message.contains("export.video only exists in Offline")));
    }

    #[test]
    fn output_device_in_offline_graph_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Offline);
        g.add_node("out", "output.device", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(issues.iter().any(|i| i.message.contains("output.device only exists in Live")));
    }

    /// G3's domain restriction: `capture` is a live device, and an `Offline`
    /// graph has no "now" for it to read.
    #[test]
    fn capture_in_offline_graph_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Offline);
        g.add_node("cap", "capture", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("capture only exists in Live")),
            "{issues:?}"
        );
    }

    /// The mirror: a `Live` graph is exactly where `capture` belongs, so
    /// nothing about the rule above should fire against it.
    #[test]
    fn capture_in_live_graph_has_no_domain_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("cap", "capture", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(!issues.iter().any(|i| i.message.contains("capture")), "{issues:?}");
    }

    /// A lamp is lit now or not at all: an `Offline` graph renders tick by
    /// tick with no wall clock, and a `.mp4` has no DMX universe in it.
    #[test]
    fn a_signal_output_in_an_offline_graph_is_an_error_naming_where_to_author_instead() {
        // `S.O.3`: `out.bluetooth` joined the family in `e0678905`, generic
        // by `n.kind.starts_with("out.")` — this list is data for the test,
        // not a second place the rule lives, so it is kept in step with
        // `OUTPUT_SIGNAL_KINDS` by hand rather than by a shared constant
        // (`lint.rs` does not depend on the catalog module's private list).
        for kind in ["out.ddp", "out.json", "out.sacn", "out.osc", "out.http", "out.midi", "out.bluetooth"] {
            let mut g = Graph::new("g", "t", Domain::Offline);
            g.add_node("o", kind, [0.0, 0.0]);
            let issues = g.lint(&catalog());
            let hit = issues
                .iter()
                .find(|i| i.severity == Severity::Error && i.message.starts_with(kind));
            let hit = hit.unwrap_or_else(|| panic!("{kind} was allowed offline: {issues:?}"));
            // The error is only useful if it says where the work *does*
            // belong — the operator authoring a light cue offline is not
            // doing anything wrong, they just put the wire in the wrong graph.
            assert!(hit.message.contains("sig_*"), "{}", hit.message);
        }
    }

    /// The mirror, and the reason the rule is worth having rather than just
    /// forbidding the family everywhere.
    #[test]
    fn a_signal_output_in_a_live_graph_has_no_domain_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("o", "out.sacn", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error),
            "a live graph is where a lamp belongs: {issues:?}"
        );
    }

    #[test]
    fn an_unconnected_node_is_a_warning_not_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Warning);
    }

    #[test]
    fn lint_does_not_panic_on_a_dangling_edge_mid_drag() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let a = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new("does-not-exist", "control"));
        // The point of this test is simply that this call returns.
        let _issues = g.lint(&catalog());
    }

    #[test]
    fn a_clean_graph_has_no_errors() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&gen, "control"));
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out, "control"));
        let issues = g.lint(&catalog());
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
    }

    // -----------------------------------------------------------------
    // LAY.1: the stage contract (`docs/plan-architecture.md` §2)
    // -----------------------------------------------------------------

    #[test]
    fn a_backward_edge_is_an_error() {
        // output.device (Output, rank 4) feeding a generator (Source, rank 0)
        // can never render, so it is an error and not a warning.
        let mut g = Graph::new("g", "t", Domain::Live);
        let out = g.add_node("out", "output.device", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        // Give the output its required control so the only error is the
        // stage one, then add the backward cable on a second port.
        let audio = g.add_node("audio", "audio.live", [0.0, 1.0]);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out, "control"));
        let e = g.add_edge(Endpoint::new(&out, "image"), Endpoint::new(&gen, "control"));
        assert!(e.starts_with("edge"));
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error && i.message.contains("backward edge")),
            "{issues:?}"
        );
    }

    #[test]
    fn source_to_modulate_to_output_is_forward_and_fine() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [0.0, 1.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
        g.add_edge(Endpoint::new(&fx, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out, "control"));
        let issues = g.lint(&catalog());
        assert!(!issues.iter().any(|i| i.message.contains("backward edge")), "{issues:?}");
    }

    #[test]
    fn a_mask_may_wrap_a_source_and_feed_compose() {
        // `Mask → Compose` is the one declared backward exception: a masked
        // source is a layer in the composite. Compose has no catalog family
        // yet, so this asserts the base `Source → Mask` half plus the rule
        // function directly for the exception half.
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let mask = g.add_node("mask", "mask", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mask, "image"));
        let issues = g.lint(&catalog());
        assert!(!issues.iter().any(|i| i.message.contains("backward edge")), "{issues:?}");
        assert!(crate::stage_edge_allowed(Stage::Mask, Stage::Compose).is_ok());
        assert!(crate::stage_edge_allowed(Stage::Compose, Stage::Source).is_err());
    }

    // -----------------------------------------------------------------
    // LAY.3: capability-aware lint (`docs/plan-architecture.md` D4)
    // -----------------------------------------------------------------

    #[test]
    fn a_stage_given_no_image_is_warned_about_but_not_errored() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [0.0, 1.0]);
        // gen -> out directly: the effect is connected ("touched") but has no
        // image, and the output has a control but no picture.
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&fx, "control"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out, "control"));
        let issues = g.lint(&catalog());
        let warns: Vec<&Issue> = issues
            .iter()
            .filter(|i| i.message.contains("expects an image"))
            .collect();
        assert!(!warns.is_empty(), "{issues:?}");
        assert!(warns.iter().all(|i| i.severity == Severity::Warning), "not an error: {warns:?}");
    }

    #[test]
    fn a_signal_destination_with_no_signal_is_warned_about() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "out.sacn", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        let issues = g.lint(&catalog());
        let hit = issues
            .iter()
            .find(|i| i.message.contains("sends a signal"))
            .unwrap_or_else(|| panic!("{issues:?}"));
        assert_eq!(hit.severity, Severity::Warning);
    }

    #[test]
    fn an_unconnected_node_is_not_double_warned_by_capabilities() {
        // `lint_orphans` already names it; the capability rule must stay out
        // of the way so a mid-drag graph does not get two messages per node.
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("fx", "fx.shake", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].severity, Severity::Warning);
    }

    // -----------------------------------------------------------------
    // O.4: `stack.mode`
    // -----------------------------------------------------------------

    #[test]
    fn a_stack_mode_node_in_an_offline_graph_is_an_error() {
        // A timeline being rendered to a file has no device to be in a mode,
        // and automix has nothing to listen to.
        let mut g = Graph::new("t", "T", Domain::Offline);
        g.add_node("mode", crate::STACK_MODE_KIND, [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("stack.mode only exists in Live")),
            "{issues:?}"
        );
    }

    #[test]
    fn one_stack_mode_node_in_a_live_graph_is_fine_and_is_not_an_orphan() {
        let mut g = Graph::new("d", "D", Domain::Live);
        g.add_node("mode", crate::STACK_MODE_KIND, [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(issues.is_empty(), "a lone mode node is a complete, valid statement: {issues:?}");
    }

    #[test]
    fn two_stack_mode_nodes_are_an_error_on_both_rather_than_a_silent_first_wins() {
        // "The first one wins" is a rule nobody can see on the canvas, so the
        // graph says it cannot answer instead of picking.
        let mut g = Graph::new("d", "D", Domain::Live);
        g.add_node("a", crate::STACK_MODE_KIND, [0.0, 0.0]);
        g.add_node("b", crate::STACK_MODE_KIND, [2.0, 0.0]);
        let issues = g.lint(&catalog());
        let blamed: Vec<&str> = issues
            .iter()
            .filter(|i| i.message.contains("composition nodes in one graph"))
            .filter_map(|i| i.node.as_deref())
            .collect();
        assert_eq!(blamed.len(), 2, "both have to be named, or one looks innocent: {issues:?}");
    }

    // -----------------------------------------------------------------
    // LAY.7b: the compose family (`docs/plan-architecture.md` §2.4)
    // -----------------------------------------------------------------

    #[test]
    fn a_compose_node_in_an_offline_graph_is_an_error() {
        // The same reason `stack.mode` has one: an Offline graph is a timeline
        // rendered to a file, and it has no device whose composition this is.
        let mut g = Graph::new("t", "T", Domain::Offline);
        g.add_node("comp", "compose.automix", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("compose.automix only exists in Live")),
            "{issues:?}"
        );
    }

    #[test]
    fn one_compose_node_in_a_live_graph_is_a_complete_statement_and_not_an_orphan() {
        let mut g = Graph::new("d", "D", Domain::Live);
        g.add_node("comp", "compose.layers", [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert!(issues.is_empty(), "a lone compose node must lint clean: {issues:?}");
    }

    #[test]
    fn a_compose_node_beside_a_legacy_stack_mode_node_is_the_ambiguous_case() {
        // The render path reads `compose.*` first and `stack.mode` as the
        // fallback, so both in one graph is two answers to one question. The
        // migration converts in place; this catches a file where something
        // else did not.
        let mut g = Graph::new("d", "D", Domain::Live);
        g.add_node("comp", "compose.layers", [0.0, 0.0]);
        g.add_node("mode", crate::STACK_MODE_KIND, [2.0, 0.0]);
        let issues = g.lint(&catalog());
        let blamed: Vec<&str> = issues
            .iter()
            .filter(|i| i.message.contains("composition nodes in one graph"))
        .filter_map(|i| i.node.as_deref())
        .collect();
        assert_eq!(blamed.len(), 2, "{issues:?}");
    }

    // -- 22-sept-2026 (tarde): the mask stroke ------------------------------

    // -- K: a colour only lands on a parameter ------------------------------

    /// A colour cable into a picture port (or a stage's `control` input read
    /// as a digest) can never do anything, so it is an error naming the node.
    #[test]
    fn a_colour_cable_into_a_picture_port_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let colour = g.add_node("color", crate::COLOR_KIND, [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&colour, "control"), Endpoint::new(&gen, "control"));
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.node.as_deref() == Some(colour.as_str())
                && i.message.contains("only lands on a parameter")),
            "{issues:?}"
        );
    }

    #[test]
    fn a_colour_on_a_param_port_is_not_flagged_by_the_colour_rule() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let colour = g.add_node("color", crate::COLOR_KIND, [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&colour, "control.rgb"), Endpoint::new(&gen, "param:stroke"));
        let issues = g.lint(&catalog());
        assert!(
            !issues.iter().any(|i| i.message.contains("only lands on a parameter")),
            "a param binding is exactly where a colour may land: {issues:?}"
        );
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error),
            "a colour var on a colour slot is a valid pair: {issues:?}"
        );
    }

    /// A `stroke` cable without a width is a dead cable the operator cannot
    /// see: the lint says what is missing rather than letting the outline
    /// silently not exist.
    #[test]
    fn a_mask_with_a_stroke_source_but_no_width_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let still = g.add_node("still", "still", [0.0, 0.0]);
        let m = g.add_node("mask", "mask", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&still, "image"), Endpoint::new(&m, "stroke"));
        let issues = g.lint(&catalog());
        let hit = issues.iter().find(|i| i.severity == Severity::Error && i.node.as_deref() == Some(m.as_str()));
        let hit = hit.unwrap_or_else(|| panic!("no stroke-width error: {issues:?}"));
        assert!(hit.message.contains("stroke_width"), "{}", hit.message);
    }

    /// Setting `stroke_width > 0` answers the rule; the absence of any
    /// stroke source is not an error (a width with no material simply has
    /// nothing to paint yet — the set-first-then-wire order of the ask).
    #[test]
    fn a_mask_with_a_width_has_no_stroke_error_even_before_its_source_is_wired() {
        use crate::Config;
        let mut g = Graph::new("g", "t", Domain::Live);
        let m = g.add_node("mask", "mask", [0.0, 0.0]);
        g.node_mut(&m).unwrap().config = Config::from_value(serde_json::json!({ "stroke_width": 6.0 }));
        let issues = g.lint(&catalog());
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error && i.node.as_deref() == Some(m.as_str())),
            "a width answers the rule: {issues:?}"
        );
    }

    // -- S.AM: an ai.* node's cables -----------------------------------------

    /// The legal shape: `audio.live → ai.colourist → gen.turing.control`.
    #[test]
    fn an_ai_node_hears_audio_and_feeds_a_generator() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let ai = g.add_node("ai", crate::AI_COLOURIST_KIND, [1.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&ai, "control"));
        g.add_edge(Endpoint::new(&ai, "control"), Endpoint::new(&gen, "control"));
        let issues = g.lint(&catalog());
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error && i.node.as_deref() == Some(ai.as_str())),
            "audio in, gen out is exactly what an ai node is for: {issues:?}"
        );
    }

    /// A generator (or anything that is not `audio.*`/`sensor.*`) into an
    /// `ai.*` input is an error: it was trained on the digest row.
    #[test]
    fn an_ai_node_hearing_a_generator_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let ai = g.add_node("ai", crate::AI_COLOURIST_KIND, [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "row"), Endpoint::new(&ai, "control"));
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.node.as_deref() == Some(ai.as_str())
                && i.message.contains("only accepts control from audio")),
            "{issues:?}"
        );
    }

    /// An `ai.*` output into a screen is an error: its offsets drive a
    /// source's floats, not a picture.
    #[test]
    fn an_ai_node_feeding_an_output_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let ai = g.add_node("ai", crate::AI_COLOURIST_KIND, [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&ai, "control"));
        g.add_edge(Endpoint::new(&ai, "control"), Endpoint::new(&out, "control"));
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.node.as_deref() == Some(ai.as_str())
                && i.message.contains("only enters gen.*")),
            "{issues:?}"
        );
    }

    /// An `ai.*` with an output but no input is a warning, never an error —
    /// the ordinary mid-edit state.
    #[test]
    fn an_ai_node_with_no_input_is_a_warning_not_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let ai = g.add_node("ai", crate::AI_COLOURIST_KIND, [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&ai, "control"), Endpoint::new(&gen, "control"));
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Warning
                && i.node.as_deref() == Some(ai.as_str())
                && i.message.contains("no control input")),
            "{issues:?}"
        );
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error && i.node.as_deref() == Some(ai.as_str())),
            "an unwired ai node is not an error: {issues:?}"
        );
    }

    /// An embedded `ai.*` (`ai.embed`) is owned by its source, so it is never
    /// reported as an orphan even before its wires are read.
    #[test]
    fn an_embedded_ai_node_is_not_an_orphan() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let ai = g.add_node("ai", crate::AI_COLOURIST_KIND, [1.0, 0.0]);
        g.node_mut(&ai).unwrap().embedded_in = Some(gen.clone());
        let issues = g.lint(&catalog());
        assert!(
            !issues.iter().any(|i| i.node.as_deref() == Some(ai.as_str())),
            "an embedded ai is owned by its source, not an orphan: {issues:?}"
        );
    }

    /// A wholly unconnected `ai.*` node is `lint_orphans`'s one warning, not
    /// two — the same "no double warning" rule the capability check follows.
    #[test]
    fn an_orphan_ai_node_is_warned_once_by_orphans_not_twice() {
        let mut g = Graph::new("g", "t", Domain::Live);
        g.add_node("ai", crate::AI_COLOURIST_KIND, [0.0, 0.0]);
        let issues = g.lint(&catalog());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].message.contains("not connected"), "{issues:?}");
    }

    // -- P1: the venue.surface node -----------------------------------------

    /// The ordinary chain: `gen → mask → venue.surface → output.device` must
    /// pass the surface's own three rules. (The device still wants its
    /// `control`, which is a different rule and not what this checks.)
    #[test]
    fn a_venue_surface_reaches_a_device() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let mask = g.add_node("mask", "mask", [1.0, 0.0]);
        let surface = g.add_node("surface", crate::VENUE_SURFACE_KIND, [2.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&mask, "image"));
        g.add_edge(Endpoint::new(&mask, "image"), Endpoint::new(&surface, "image"));
        g.add_edge(Endpoint::new(&surface, "image"), Endpoint::new(&out, "image"));
        let issues = g.lint(&catalog());
        assert!(
            !issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("never reaches an output.device")),
            "{issues:?}"
        );
        assert!(
            !issues.iter().any(|i| i.message.contains("in series")
                || i.message.contains("masks are cut in source space")),
            "{issues:?}"
        );
    }

    /// A surface that feeds nothing — or only a dead end — has no screen to
    /// warp onto, so it is an error rather than a silent no-op.
    #[test]
    fn a_venue_surface_without_a_device_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let surface = g.add_node("surface", crate::VENUE_SURFACE_KIND, [1.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&surface, "image"));
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.node.as_deref() == Some(surface.as_str())
                && i.message.contains("never reaches an output.device")),
            "{issues:?}"
        );
    }

    #[test]
    fn a_mask_after_a_surface_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let surface = g.add_node("surface", crate::VENUE_SURFACE_KIND, [1.0, 0.0]);
        let mask = g.add_node("mask", "mask", [2.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&surface, "image"));
        g.add_edge(Endpoint::new(&surface, "image"), Endpoint::new(&mask, "image"));
        g.add_edge(Endpoint::new(&mask, "image"), Endpoint::new(&out, "image"));
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.node.as_deref() == Some(mask.as_str())
                && i.message.contains("masks are cut in source space")),
            "{issues:?}"
        );
    }

    #[test]
    fn two_surfaces_in_series_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let a = g.add_node("surface", crate::VENUE_SURFACE_KIND, [1.0, 0.0]);
        let b = g.add_node("surface", crate::VENUE_SURFACE_KIND, [2.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&a, "image"));
        g.add_edge(Endpoint::new(&a, "image"), Endpoint::new(&b, "image"));
        g.add_edge(Endpoint::new(&b, "image"), Endpoint::new(&out, "image"));
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.node.as_deref() == Some(b.as_str())
                && i.message.contains("in series")),
            "{issues:?}"
        );
    }

    // -- The wire after the device: `output.device.venue` ------------------

    /// The operator's request: a wire *after* the output device, into the
    /// room. `output.device.venue → venue.surface.image` is legal on its own —
    /// the surface is the view of this device's frame, and the device stays
    /// last in the chain that drives it: the view comes *after* the surface's
    /// own warp, so no picture is composed twice.
    #[test]
    /// The operator's request: a wire *after* the output device, into the
    /// room. `output.device.venue → venue.surface.image` is legal on its own —
    /// the surface reads this device's finished frame.
    ///
    /// A surface *upstream* of that device is required, because a surface
    /// that reaches no device is its own error (`lint_venue_surface`), and the
    /// gen → device path must stay open. The "two surfaces in series" rule
    /// reads the new `venue` cable exactly like any other image cable, so the
    /// same surface cannot be both — that is
    /// `the_back_edge_over_the_ordinary_chain_is_refused_as_a_cycle`.
    #[test]
    fn a_device_venue_output_read_back_by_a_second_surface_is_legal() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [1.0, 0.0]);
        let view = g.add_node("view", crate::VENUE_SURFACE_KIND, [2.0, 1.0]);
        let audio = g.add_node("audio", "audio.live", [0.0, 1.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out, "control"));
        let edge = g.add_edge_checked(
            Endpoint::new(&out, "venue"),
            Endpoint::new(&view, "image"),
            &catalog(),
        );
        assert!(edge.is_ok(), "the back view is the operator's new wire: {edge:?}");
        assert!(
            !g.lint(&catalog()).iter().any(|i| i.severity == Severity::Error
                && (i.message.contains("may only be read") || i.message.contains("is a cycle"))),
            "no rule of the new port may refuse it"
        );
    }

    /// The port feeds a surface, never a fixture: a fixture samples the frame
    /// it is fed, so a second path from the device would be rule 6's two
    /// paths to one thing. Refused by name.
    #[test]
    fn a_device_venue_output_into_a_fixture_is_refused() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let fixture = venue_fixture(&mut g, "fix", "bar", "front", "frame");
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        let err = g
            .add_edge_checked(
                Endpoint::new(&out, "venue"),
                Endpoint::new(&fixture, "image"),
                &catalog(),
            )
            .expect_err("a fixture is not a reader of the device's frame");
        assert!(
            err.iter().any(|m| m.contains("may only be read by a venue.surface")),
            "the refusal names the rule: {err:?}"
        );
    }

    /// Both directions at once is the loop `the design notes` forbids: the surface
    /// already carries this device's picture. Refused with the chain in the
    /// message, not just "the graph has a cycle".
    #[test]
    fn the_back_edge_over_the_ordinary_chain_is_refused_as_a_cycle() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let surface = g.add_node("surface", crate::VENUE_SURFACE_KIND, [1.0, 0.0]);
        let out = g.add_node("out", "output.device", [2.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [0.0, 1.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&surface, "image"));
        g.add_edge(Endpoint::new(&surface, "image"), Endpoint::new(&out, "image"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&out, "control"));
        let err = g
            .add_edge_checked(
                Endpoint::new(&out, "venue"),
                Endpoint::new(&surface, "image"),
                &catalog(),
            )
            .expect_err("the back edge over the chain is a cycle");
        assert!(
            err.iter().any(|m| m.contains("is a cycle") && m.contains("the device is last")),
            "the refusal says why: {err:?}"
        );
    }

    // -----------------------------------------------------------------
    // F1: the per-fixture node
    // -----------------------------------------------------------------

    fn venue_fixture(g: &mut Graph, id: &str, venue: &str, fixture: &str, colour_from: &str) -> String {
        let n = g.add_node(id, crate::VENUE_FIXTURE_KIND, [0.0, 0.0]);
        g.node_mut(&n).unwrap().config = crate::Config::from_json_str(
            &serde_json::json!({ "venue": venue, "fixture": fixture, "colour_from": colour_from })
                .to_string(),
        )
        .unwrap();
        n
    }

    /// A node whose pointer names no row is an orphan — a warning, the
    /// mid-drag state before the operator points it at a fixture.
    #[test]
    fn a_venue_fixture_naming_no_row_is_a_warning() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let n = venue_fixture(&mut g, "f", "bar", "", "frame");
        let issues = g.lint(&catalog());
        let hit = issues
            .iter()
            .find(|i| i.node.as_deref() == Some(n.as_str()))
            .unwrap_or_else(|| panic!("{issues:?}"));
        assert_eq!(hit.severity, Severity::Warning);
        assert!(hit.message.contains("names no venue fixture"), "{}", hit.message);
    }

    /// Two nodes on the same row fight on the bus with nothing to tell them
    /// apart on the canvas — an error.
    #[test]
    fn two_venue_fixture_nodes_on_the_same_row_are_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let a = venue_fixture(&mut g, "a", "bar", "wash-1", "frame");
        let b = venue_fixture(&mut g, "b", "bar", "wash-1", "frame");
        let issues = g.lint(&catalog());
        for id in [&a, &b] {
            assert!(
                issues.iter().any(|i| i.severity == Severity::Error
                    && i.node.as_deref() == Some(id.as_str())
                    && i.message.contains("drive the same venue fixture")),
                "{issues:?}"
            );
        }
    }

    /// Two different rows are fine; the duplicate check keys on the pointer.
    #[test]
    fn two_venue_fixture_nodes_on_different_rows_have_no_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        venue_fixture(&mut g, "a", "bar", "wash-1", "frame");
        venue_fixture(&mut g, "b", "bar", "wash-2", "frame");
        let issues = g.lint(&catalog());
        assert!(!issues.iter().any(|i| i.message.contains("drive the same venue fixture")), "{issues:?}");
    }

    /// `colour_from = "model"` reads the `colour` input; with nothing wired
    /// there the node has no colour to send.
    #[test]
    fn a_model_colour_source_with_nothing_on_colour_is_an_error() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let n = venue_fixture(&mut g, "f", "bar", "wash-1", "model");
        let issues = g.lint(&catalog());
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.node.as_deref() == Some(n.as_str())
                && i.message.contains("colour_from=model")),
            "{issues:?}"
        );

        // Wire something into `colour` and the error goes away.
        let colour = g.add_node("c", crate::COLOR_KIND, [1.0, 0.0]);
        g.add_edge(Endpoint::new(&colour, "control"), Endpoint::new(&n, "colour"));
        let issues = g.lint(&catalog());
        assert!(
            !issues.iter().any(|i| i.node.as_deref() == Some(n.as_str())
                && i.message.contains("colour_from=model")),
            "{issues:?}"
        );
    }
}
