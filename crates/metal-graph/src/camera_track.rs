//! §13.2's "config-association" mechanism: which generator node(s) a
//! `cam.track` node writes its reserved floats into.
//!
//! Deliberately the mirror image of §3.2's effect materialization: a `fx.*`
//! node earns its place by being spliced into the image chain, but a
//! `cam.track` node is *never* wired into the image chain at all (§13.2:
//! "here the node is not inserted into the image chain; it is attached to the
//! config of one or more generators") — `cam.track` has no image output in
//! the catalog (see [`crate::NodeKind::catalog`]), so there is nothing to
//! splice it into. Association happens purely through this node's `config`,
//! resolved here.
//!
//! **What this module does not know.** Nothing about `Control`, sensors, or
//! what `eye_x` means — that is `metal-sense`'s job, one layer up, which this
//! crate does not depend on (see this crate's own top-level doc: no
//! knowledge that an app, or a sensor, exists). This module answers exactly
//! one question — "which generator node ids is this `cam.track` node
//! allowed to write into" — using only the graph's own topology and the
//! manifest's `camera_track` capability flag, the same flag
//! [`crate::NodeKind::catalog`] already reads `footage` from off the same
//! [`metal_schema::Manifest`].

use crate::{Graph, NodeId};

/// A `cam.track` node's config: which generator node ids it drives.
///
/// Node ids, not kinds and not positions — the same reasoning as
/// [`crate::NodeId`]'s own doc comment: a config naming "the turing node" by
/// kind would attach to *every* `gen.turing` in the graph, and one naming it
/// by position would silently reattach to a different node once an earlier
/// one was deleted. Naming the id is the only form that means "that specific
/// node, forever" — ids are never reused after a delete.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CamTrackConfig {
    #[serde(default)]
    pub targets: Vec<NodeId>,
    /// How much of the sensor's viewer offset reaches the camera, `0..1`. The
    /// default is 1.0 — a `cam.track` written before this key existed moved the
    /// camera at full strength, so a missing key must keep doing exactly that.
    #[serde(default = "full_strength")]
    pub strength: f32,
    /// Flip the offset on each axis (x, y, z), for a camera mounted the other
    /// way round. All false by default.
    #[serde(default)]
    pub invert: [bool; 3],
}

fn full_strength() -> f32 {
    1.0
}

impl Default for CamTrackConfig {
    fn default() -> Self {
        CamTrackConfig { targets: Vec::new(), strength: 1.0, invert: [false; 3] }
    }
}

impl CamTrackConfig {
    /// Packs this into the opaque [`crate::Config`] a [`crate::Node`]
    /// carries — what a graph builder (a test, the editor, `migrate`) uses to
    /// set a `cam.track` node's config without hand-writing JSON.
    pub fn to_config(&self) -> crate::Config {
        // unwrap: `CamTrackConfig` is one named `Vec<String>` field — nothing
        // in it can fail to serialize.
        crate::Config::from_value(
            serde_json::to_value(self).expect("CamTrackConfig always serializes"),
        )
    }

    /// The empty config (no targets) for anything that fails to parse as
    /// this shape — a `cam.track` node with no config set yet included. Same
    /// "never panic on a graph mid-construction" discipline as `Graph::lint`.
    fn from_config(config: &crate::Config) -> CamTrackConfig {
        serde_json::from_str(&config.to_json_str()).unwrap_or_default()
    }
}

/// Which of `cam_track_node`'s configured targets are real: present in
/// `graph`, spelling a `gen.*` node, whose manifest model declares
/// `camera_track: true`.
///
/// A target failing any of those checks is silently dropped rather than
/// erroring — `the design notes` §13.2's own words: "the `cam.track` node
/// simply does not list it as a valid target — once again the UX falls back
/// to typing, not to a checkbox." A stale id (its node got deleted), a typo,
/// and a target that used to declare `camera_track` and lost it all read the
/// same from here: not a valid destination right now.
///
/// Returns targets in the order the config lists them, deduplicated —
/// `cam.track` writes the same three floats into each one, so a target
/// listed twice would just mean writing them twice.
///
/// Never panics: a missing node, a node that is not a `cam.track`, or a
/// `cam.track` with no config yet all resolve to an empty list, the same way
/// `Graph::lint` tolerates a graph mid-drag rather than assuming it is
/// already valid.
pub fn targets(graph: &Graph, cam_track_node: &str, manifest: &metal_schema::Manifest) -> Vec<NodeId> {
    let Some(node) = graph.node(cam_track_node) else {
        return Vec::new();
    };
    if node.kind != "cam.track" {
        return Vec::new();
    }

    let config = CamTrackConfig::from_config(&node.config);
    let mut out = Vec::new();
    for id in &config.targets {
        if !out.contains(id) && is_valid_target(graph, id, manifest) {
            out.push(id.clone());
        }
    }
    out
}

fn is_valid_target(graph: &Graph, id: &str, manifest: &metal_schema::Manifest) -> bool {
    let Some(node) = graph.node(id) else { return false };
    let Some(model_id) = node.kind.strip_prefix("gen.") else { return false };
    manifest.models.iter().any(|m| m.id == model_id && m.camera_track)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Domain, Graph};

    fn manifest() -> metal_schema::Manifest {
        let text = metal_vfs::read_to_string(schema_path()).expect("data/schema.json readable");
        metal_schema::Manifest::parse(&text).expect("schema.json parses")
    }

    fn schema_path() -> std::path::PathBuf {
        // Same real file `lib.rs`'s own catalog tests read — see its
        // `schema_path()` for why: no data dir of this crate's own.
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json")
    }

    #[test]
    fn turing_declares_camera_track_in_the_real_manifest() {
        // Ground truth for every test below: if this ever fails, either
        // Turing's `camera_track` capability regressed, or `emit-schema`
        // was not re-run after touching it — not that this module broke.
        let m = manifest();
        let turing = m.models.iter().find(|m| m.id == "turing").expect("turing is a model");
        assert!(
            turing.camera_track,
            "turing must declare camera_track: true — see metal_render::models::turing"
        );
    }

    #[test]
    fn a_cam_track_node_resolves_its_configured_target() {
        let manifest = manifest();
        let mut g = Graph::new("g", "t", Domain::Live);
        let turing = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let cam = g.add_node("cam", "cam.track", [1.0, 0.0]);
        g.node_mut(&cam).unwrap().config = CamTrackConfig { targets: vec![turing.clone()], ..Default::default() }.to_config();

        assert_eq!(targets(&g, &cam, &manifest), vec![turing]);
    }

    #[test]
    fn a_target_without_camera_track_is_dropped_not_errored() {
        let manifest = manifest();
        let mut g = Graph::new("g", "t", Domain::Live);
        // Ascii has no 3D camera concept — camera_track must be false for
        // it, per §13.2's audit ("not all 32... audit which qualify").
        let ascii = g.add_node("gen", "gen.ascii", [0.0, 0.0]);
        let cam = g.add_node("cam", "cam.track", [1.0, 0.0]);
        g.node_mut(&cam).unwrap().config = CamTrackConfig { targets: vec![ascii], ..Default::default() }.to_config();

        assert_eq!(targets(&g, &cam, &manifest), Vec::<NodeId>::new());
    }

    #[test]
    fn a_deleted_target_is_dropped_not_errored() {
        let manifest = manifest();
        let mut g = Graph::new("g", "t", Domain::Live);
        let turing = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let cam = g.add_node("cam", "cam.track", [1.0, 0.0]);
        g.node_mut(&cam).unwrap().config = CamTrackConfig { targets: vec![turing.clone()], ..Default::default() }.to_config();
        g.remove_node(&turing);

        assert_eq!(targets(&g, &cam, &manifest), Vec::<NodeId>::new());
    }

    #[test]
    fn a_node_that_is_not_cam_track_resolves_to_nothing() {
        let manifest = manifest();
        let mut g = Graph::new("g", "t", Domain::Live);
        let turing = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        g.node_mut(&turing).unwrap().config =
            CamTrackConfig { targets: vec![turing.clone()], ..Default::default() }.to_config();

        assert_eq!(targets(&g, &turing, &manifest), Vec::<NodeId>::new());
    }

    #[test]
    fn a_missing_node_id_resolves_to_nothing_and_does_not_panic() {
        let manifest = manifest();
        let g = Graph::new("g", "t", Domain::Live);
        assert_eq!(targets(&g, "does-not-exist", &manifest), Vec::<NodeId>::new());
    }

    #[test]
    fn duplicate_targets_in_config_are_deduplicated() {
        let manifest = manifest();
        let mut g = Graph::new("g", "t", Domain::Live);
        let turing = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let cam = g.add_node("cam", "cam.track", [1.0, 0.0]);
        g.node_mut(&cam).unwrap().config =
            CamTrackConfig { targets: vec![turing.clone(), turing.clone()], ..Default::default() }.to_config();

        assert_eq!(targets(&g, &cam, &manifest), vec![turing]);
    }

    #[test]
    fn a_cam_track_node_with_no_config_yet_resolves_to_nothing() {
        let manifest = manifest();
        let mut g = Graph::new("g", "t", Domain::Live);
        // add_node mutates the id generator, so `g` still needs `mut` even
        // though its config is left at the default.
        let cam = g.add_node("cam", "cam.track", [0.0, 0.0]);
        assert_eq!(targets(&g, &cam, &manifest), Vec::<NodeId>::new());
    }

    // -- H3: strength and invert (both defaulted, for old configs) ---------

    #[test]
    fn a_config_written_before_strength_existed_reads_as_full_strength() {
        // A `cam.track` saved with only `targets` must move the camera at 1.0
        // and invert nothing — exactly what it did before these keys existed.
        let old = crate::Config::from_json_str(r#"{"targets":["gen-1"]}"#).unwrap();
        let cfg: CamTrackConfig = serde_json::from_str(&old.to_json_str()).unwrap();
        assert_eq!(cfg.strength, 1.0);
        assert_eq!(cfg.invert, [false; 3]);
        assert_eq!(cfg.targets, vec!["gen-1".to_string()]);
    }

    #[test]
    fn strength_and_invert_round_trip_through_the_opaque_config() {
        let cfg = CamTrackConfig {
            targets: vec!["gen-1".into()],
            strength: 0.25,
            invert: [true, false, true],
        };
        let back: CamTrackConfig = serde_json::from_str(&cfg.to_config().to_json_str()).unwrap();
        assert_eq!(back, cfg);
    }
}
