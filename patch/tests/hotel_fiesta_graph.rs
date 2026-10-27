//! The hotel-fiesta graph (`web/wasm/data/graphs/hotel-fiesta.json`) against
//! the live catalog: it has to parse, use only real node kinds, and lint with
//! no errors. This is the graph the fork ships as its default: one generator
//! feeding the LED wall and the fixture rig, with the production-room webcam
//! wired into `sensor.yolo` -> `cam.track`.

use std::path::{Path, PathBuf};

use metal_graph::lint::Severity;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_hotel_fiesta_graph_parses_and_lints_clean() {
    let text = std::fs::read_to_string(repo().join("web/wasm/data/graphs/hotel-fiesta.json"))
        .expect("hotel-fiesta.json on disk");
    let g = metal_graph::Graph::from_json(&text).expect("graph parses");

    let manifest_text = std::fs::read_to_string(repo().join("data/schema.json")).expect("manifest");
    let manifest = metal_schema::Manifest::parse(&manifest_text).expect("manifest parses");
    let catalog = metal_graph::NodeKind::catalog(&manifest);

    for n in &g.nodes {
        assert!(
            catalog.iter().any(|k| k.kind == n.kind),
            "{} names kind {:?}, which the catalog does not have",
            n.id,
            n.kind
        );
    }

    let issues = g.lint(&catalog);
    let errors: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
    assert!(errors.is_empty(), "lint errors: {errors:#?}");
}

#[test]
fn the_camera_chain_is_wired_from_capture_to_track() {
    let text = std::fs::read_to_string(repo().join("web/wasm/data/graphs/hotel-fiesta.json"))
        .expect("hotel-fiesta.json on disk");
    let g = metal_graph::Graph::from_json(&text).expect("graph parses");

    let kind = |id: &str| g.node(id).map(|n| n.kind.as_str());
    let cap = g.nodes.iter().find(|n| n.kind == "capture").expect("a capture node");
    assert!(
        cap.config.to_json_str().contains("dshow:"),
        "the capture node names the webcam: {}",
        cap.config.to_json_str()
    );
    let sensor = g.nodes.iter().find(|n| n.kind == "sensor.yolo").expect("a sensor");
    let track = g.nodes.iter().find(|n| n.kind == "cam.track").expect("a track");

    // capture.image -> sensor.image
    assert!(g.edges.iter().any(|e| e.from.node == cap.id
        && e.from.port == "image"
        && e.to.node == sensor.id
        && e.to.port == "image"));
    // sensor.control -> track.control
    assert!(g.edges.iter().any(|e| e.from.node == sensor.id
        && e.from.port == "control"
        && e.to.node == track.id
        && e.to.port == "control"));

    assert_eq!(kind(&cap.id), Some("capture"));
    assert_eq!(kind(&sensor.id), Some("sensor.yolo"));
    assert_eq!(kind(&track.id), Some("cam.track"));
}

#[test]
fn the_patch_nodes_point_at_the_venue_and_its_groups() {
    let text = std::fs::read_to_string(repo().join("web/wasm/data/graphs/hotel-fiesta.json"))
        .expect("hotel-fiesta.json on disk");
    let g = metal_graph::Graph::from_json(&text).expect("graph parses");

    let rigs: Vec<_> = g.nodes.iter().filter(|n| n.kind == "venue.fixtures").collect();
    assert!(!rigs.is_empty(), "the patch is wired to the room");
    for n in rigs {
        let cfg = n.config.to_json_str();
        assert!(cfg.contains("fiesta-resort"), "{cfg}");
        assert!(cfg.contains("group"), "{cfg}");
    }

    // The screen surface names the venue's own surface id.
    let screen = g.nodes.iter().find(|n| n.kind == "venue.surface").expect("a surface node");
    let cfg = screen.config.to_json_str();
    assert!(cfg.contains("fiesta-resort"), "{cfg}");
    assert!(cfg.contains("screen-main"), "{cfg}");
}
