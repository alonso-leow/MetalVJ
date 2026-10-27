//! Screenshots of the shell's real states (2026-09-30) — the library
//! (folders, `archivos`, the folder menu with the curator, a picked file in
//! `propiedades`) and every lanes / overlays surface (the grid, the `+` menu,
//! `nuevo botón` in both behaviours with per-node states, the dials, the ticks
//! timeline). Each state is the real `Shell` painted through kittest's wgpu
//! renderer against a scripted door, then written as a PNG.
//!
//! Test-only, behind the `shots` feature, and only when `FW_SHOTS_DIR` names
//! where to write:
//!
//! ```text
//! FW_SHOTS_DIR=$PWD/docs/shots cargo test -p metal-dash -F shots shots_ -- --test-threads=1
//! ```

use std::path::PathBuf;
use std::rc::Rc;

use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::panels::{FileRef, Selection};
use crate::shell::{FakeDoor, Shell};

type Harness = egui_kittest::Harness<'static, Shell>;

fn out_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("FW_SHOTS_DIR")?);
    metal_vfs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn harness(door: Box<dyn Call>, w: f32, h: f32) -> Harness {
    let theme = Theme::phosphor();
    let harness = egui_kittest::Harness::builder()
        .with_size(egui::Vec2::new(w, h))
        .wgpu()
        .build_ui_state(|ui, shell: &mut Shell| shell.ui(ui), Shell::new(theme.clone(), door));
    theme.apply(&harness.ctx);
    harness
}

fn shot(h: &mut Harness, dir: &std::path::Path, name: &str) {
    h.run_steps(4);
    let img = h.render().expect("wgpu render");
    img.save(dir.join(format!("{name}.png"))).expect("write png");
}

/// A door with the data a real session has: a music folder in use and
/// curated, songs in every digest status, movements, sequences, switches and
/// a small graph whose `gen` has float params.
fn realistic_door() -> Rc<FakeDoor> {
    let d = Rc::new(FakeDoor::new());
    d.respond(
        "GET",
        "/state",
        json!({
            "outputs": [{ "id": "screen-1", "layers": [{ "id": "l1" }], "masks": [] }],
            "reel": { "library": { "name": "metraje" } }
        }),
    );
    d.respond("GET", "/fs", json!({
        "parent": "/home/operator",
        "dirs": ["metraje", "musica", "samples"],
        "files": [
            { "file": "Gasolina - Remix.mp3", "kind": "audio" },
            { "file": "El Preso - Club Edit.mp3", "kind": "audio" },
            { "file": "BbY WOW - Hype Intro.mp3", "kind": "audio" },
            { "file": "Get Busy - Afro Bros.mp3", "kind": "audio" },
            { "file": "drone-3.mp4", "kind": "video", "seconds": 42.0 },
            { "file": "portada.png", "kind": "image" },
            { "file": "notas.txt", "kind": "other" }
        ]
    }));
    d.respond("GET", "/author/songs", json!({ "songs": [
        { "name": "Gasolina - Remix", "status": "digested", "open": true },
        { "name": "El Preso - Club Edit", "status": "failed", "error": "not a readable audio file: end of stream" },
        { "name": "BbY WOW - Hype Intro", "status": "digesting" }
    ]}));
    d.respond("GET", "/author/curator/state", json!({ "libraries": ["metraje"], "trained": true, "picks": 128, "songs": 6 }));
    d.respond("GET", "/author/models/library", json!({ "movements": [
        { "name": "drop", "kind": "model", "ticks": 32, "favourite": true },
        { "name": "intro-lenta", "kind": "model", "ticks": 64, "favourite": false }
    ]}));
    d.respond("GET", "/sequences", json!({ "sequences": [{ "id": "seq-1", "name": "coro" }], "running": [] }));
    d.respond("GET", "/switch", json!({ "switches": [
        { "id": "sw-1", "name": "strobo", "kind": "hold", "on": false },
        { "id": "sw-2", "name": "fondo on", "kind": "toggle", "on": true }
    ]}));
    d.respond("GET", "/overlay/blackout", json!({ "on": false }));
    d.respond("GET", "/graphs", json!({ "graphs": [{ "id": "g1", "name": "g1", "domain": "live" }] }));
    d.respond("GET", "/graph", json!({ "graph": { "id": "g1", "nodes": [
        { "id": "gen-1", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false, "config": {} },
        { "id": "fx-1", "kind": "fx.ghost", "pos": [200.0, 0.0], "label": "", "bypass": false, "solo": false, "config": {} },
        { "id": "out-0", "kind": "output.device", "pos": [400.0, 0.0], "label": "", "bypass": false, "solo": false, "config": {} }
    ], "edges": [] }, "primitives": [], "issues": [] }));
    d.respond("GET", "/catalog", json!({ "catalog": [] }));
    d.respond("GET", "/graph/io", json!({
        "node": "gen-1", "kind": "gen.turing", "label": "", "inputs": [], "outputs": [], "sources": [],
        "params": [
            { "name": "scale", "min": 1.0, "max": 12.0, "default": 1.0, "kind": "float", "live": 4.5 },
            { "name": "speed", "min": 0.0, "max": 2.0, "default": 1.0, "kind": "float", "live": 0.6 },
            { "name": "shine", "min": 0.0, "max": 1.0, "default": 0.8, "kind": "float", "live": null }
        ]
    }));
    d.respond("GET", "/graph/live", json!({ "graph": "g1", "t": 1.0, "age_ms": 10, "nodes": {} }));
    d.respond("GET", "/author/models/state", json!({ "models": [
        { "id": "turing", "label": "turing", "width": 4, "defaults": [0.5, 0.5, 0.5, 0.5],
          "blocks": [{ "type": "floats", "fields": [
              { "name": "scale", "min": 0.0, "max": 1.0 }, { "name": "speed", "min": 0.0, "max": 1.0 },
              { "name": "shine", "min": 0.0, "max": 1.0 }, { "name": "grain", "min": 0.0, "max": 1.0 }] }] },
        { "id": "mandel", "label": "mandel", "width": 2, "defaults": [0.5, 0.5],
          "blocks": [{ "type": "floats", "fields": [{ "name": "zoom" }, { "name": "turn" }] }] }
    ]}));
    // A song's digest with the shape music has: a pulse every half second,
    // a slower swell, chroma that moves between two chords.
    let n = 1000usize;
    let wave = |t: usize, period: f32| ((t as f32 / period * std::f32::consts::TAU).sin() * 0.5 + 0.5);
    let onset: Vec<f32> = (0..n).map(|t| if t % 4 == 0 { 0.6 + 0.3 * wave(t, 64.0) } else { 0.05 }).collect();
    let chroma: Vec<[f32; 12]> = (0..n)
        .map(|t| {
            let mut c = [0.05f32; 12];
            let (a, b) = if (t / 32) % 2 == 0 { (0, 7) } else { (5, 9) };
            c[a] = 0.9;
            c[b] = 0.6 * wave(t, 16.0);
            c[(a + 4) % 12] = 0.4;
            c
        })
        .collect();
    d.respond("GET", "/author/grade/matrix", json!({
        "song": "Gasolina - Remix", "rate": 8.0, "ticks": n,
        "rms": (0..n).map(|t| 0.25 + 0.35 * wave(t, 256.0)).collect::<Vec<f32>>(),
        "bandwidth": (0..n).map(|t| 0.3 + 0.2 * wave(t, 96.0)).collect::<Vec<f32>>(),
        "onset": onset,
        "centroid": (0..n).map(|t| 0.5 + 0.2 * wave(t + 30, 128.0)).collect::<Vec<f32>>(),
        "chroma": chroma,
    }));
    d.respond("GET", "/author/models/example", json!({ "ticks": [
        { "tick": 16, "floats": [0.2, 0.1, 0.9, 0.3] },
        { "tick": 48, "floats": [0.8, 0.5, 0.4, 0.3] },
        { "tick": 80, "floats": [0.4, 0.9, 0.7, 0.8] },
        { "tick": 128, "floats": [0.6, 0.2, 0.2, 0.5] }
    ]}));
    d.respond("GET", "/author/models/song-hits", json!({ "ticks": [16, 48, 64, 80, 112, 128] }));
    d.respond("GET", "/author/router/take", json!({ "song": "Gasolina - Remix", "picks": [
        { "tick": 0, "model": "turing" }, { "tick": 96, "model": "mandel" }
    ]}));
    d
}

fn overlays_plus(h: &Harness) -> egui_kittest::Node<'_> {
    let theme = Theme::phosphor();
    h.query_all_by_label("+")
        .find(|n| (n.rect().width() - theme.overlay_cell.x).abs() < 1.0)
        .expect("the overlays grid paints its + cell")
}

/// Every library state worth looking at.
#[test]
fn shots_library() {
    let Some(dir) = out_dir() else { return };
    let door = realistic_door();
    let (w, h) = (crate::shell::DESKTOP_VIEWPORT.0 as f32, crate::shell::DESKTOP_VIEWPORT.1 as f32);
    let mut hs = harness(Box::new(Rc::clone(&door)), w, h);
    hs.run_steps(4);
    hs.state_mut().library_state_mut().set_cwd("/home/operator/musica");
    hs.get_by_label(crate::shell::panels::Region::Library.title()).click();
    shot(&mut hs, &dir, "library-01-folders-and-archivos");

    // The folder in use's `⋮`: use/remove/reload + train curator.
    if let Some(menu) = hs.query_all_by_label("⋮").next() {
        menu.click();
    }
    shot(&mut hs, &dir, "library-02-folder-menu-with-curator");
    hs.key_press(egui::Key::Escape);

    // A picked file shows in properties (the failed digest, with its error).
    hs.state_mut().set_selection_for_test(Selection::File(FileRef {
        name: "El Preso - Club Edit.mp3".into(),
        path: "/home/operator/musica/El Preso - Club Edit.mp3".into(),
        kind: "audio".into(),
    }));
    shot(&mut hs, &dir, "library-03-file-selected-in-properties");

    hs.state_mut().library_state_mut().set_confirm_open("BbY WOW - Hype Intro", "BbY WOW - Hype Intro.mp3");
    shot(&mut hs, &dir, "library-04-confirm-switch-song");
}

/// Every lanes / overlays state.
#[test]
fn shots_lanes_and_overlays() {
    let Some(dir) = out_dir() else { return };
    let door = realistic_door();
    let w = crate::shell::DESKTOP_VIEWPORT.0 as f32;
    let mut hs = harness(Box::new(Rc::clone(&door)), w, 1400.0);
    hs.run_steps(4);
    hs.state_mut().nodes_state_mut().set_selected_graph_for_test("g1");
    shot(&mut hs, &dir, "overlays-01-grid");

    overlays_plus(&hs).click();
    shot(&mut hs, &dir, "overlays-02-plus-menu");

    hs.get_by_label("controller button").click();
    shot(&mut hs, &dir, "overlays-03-controller-editor");
    {
        let state = hs.state_mut().lanes_state_mut();
        state.controllers.insert(
            "g1".into(),
            serde_json::json!({
                "graph": "g1",
                "name": "escena",
                "mode": "hold",
                "targets": [
                    { "node": "gen-1", "on": true },
                    { "node": "fx-1", "on": false }
                ]
            }),
        );
    }
    shot(&mut hs, &dir, "overlays-04-controller-scene-hold");
    hs.state_mut().lanes_state_mut().controllers.clear();

    // 2026-10: one controller is one scene cell — the whole stored state
    // (nodes, fixtures, parameters) behind one button.
    hs.state_mut().lanes_state_mut().controllers.insert(
        "g1".into(),
        serde_json::json!({
            "graph": "g1",
            "name": "escena",
            "targets": [
                { "node": "gen-1", "on": true },
                { "node": "gen-1", "param": "scale", "value": 3.0 }
            ]
        }),
    );
    shot(&mut hs, &dir, "overlays-05-controller-scene");
    hs.state_mut().lanes_state_mut().controllers.clear();

    // The sequence editor: a digested song, two stacked tracks (turing
    // expanded with its float rows, mandel collapsed to where it plays), the
    // router picks and the song's hits.
    hs.state_mut().set_selection_for_test(Selection::None);
    hs.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
    {
        let ingest = hs.state_mut().ingest_state_mut();
        ingest.song = "Gasolina - Remix".into();
        ingest.switch_model("turing", vec![0.5; 4]);
        ingest.device = Some("screen-1".into());
    }
    hs.run_steps(4);
    {
        let ingest = hs.state_mut().ingest_state_mut();
        ingest.add_track();
        ingest.switch_model("mandel", vec![0.5; 2]);
        ingest.keyframes.insert(96, vec![0.1, 0.9]);
        ingest.keyframes.insert(150, vec![0.9, 0.2]);
        ingest.expanded = false;
        ingest.select_track(0);
        ingest.goto(64);
        ingest.mark = Some((48, 80));
    }
    shot(&mut hs, &dir, "lanes-01-sequence-editor");
    // Focused: propiedades is the active track's floats at the playhead.
    hs.get_by_label(crate::shell::panels::Region::Lanes.title()).click();
    shot(&mut hs, &dir, "lanes-02-sequence-editor-focused-properties");
    hs.state_mut().ingest_state_mut().set_expanded(0, false);
    hs.state_mut().ingest_state_mut().span = 1000;
    shot(&mut hs, &dir, "lanes-03-collapsed-whole-song");
}

/// 2026-10-09: the reworked `fx.grade` page, every section open, a few
/// floats off their defaults — the layout check for the grade rework.
#[test]
fn shots_grade() {
    let Some(dir) = out_dir() else { return };
    let door = realistic_door();
    door.respond("GET", "/graphs", json!({ "graphs": [{ "id": "empty-1", "name": "empty-1", "domain": "live" }] }));
    door.respond("GET", "/graph", json!({
        "graph": { "id": "empty-1", "nodes": [
            { "id": "gen-0", "kind": "gen.turing", "pos": [0.0, 0.0], "label": "", "bypass": false, "solo": false, "config": null },
            { "id": "grade-0", "kind": "fx.grade", "pos": [1.0, 0.0], "label": "", "bypass": false, "solo": false,
              "config": { "grade_contrast": 35.0, "grade_temperature": -20.0, "grade_curve_darks": -30.0,
                          "grade_grading_shadows_hue": 210.0, "grade_grading_shadows_sat": 40.0,
                          "grade_mixer_orange_sat": 25.0 } }
        ],
        "edges": [ { "id": "e0", "from": { "node": "gen-0", "port": "image" }, "to": { "node": "grade-0", "port": "image" } } ]},
        "primitives": [], "issues": [],
    }));
    let params: Vec<_> = metal_schema::GRADE_FLOATS
        .iter()
        .map(|f| json!({ "name": f.name, "min": f.min, "max": f.max, "default": f.default, "kind": "float", "value": f.default, "bound": null, "live": null }))
        .collect();
    door.respond("GET", "/graph/io", json!({
        "node": "grade-0", "kind": "fx.grade", "label": "", "inputs": [], "outputs": [], "sources": [], "params": params,
    }));
    door.respond("GET", "/graph/live", json!({ "graph": "empty-1", "nodes": {} }));
    let (w, h) = (crate::shell::DESKTOP_VIEWPORT.0 as f32, crate::shell::DESKTOP_VIEWPORT.1 as f32 * 2.5);
    let mut hs = harness(Box::new(Rc::clone(&door)), w, h);
    hs.run_steps(4);
    hs.state_mut().set_selection_for_test(Selection::Node("grade-0".into()));
    hs.run_steps(4);
    for title in ["\u{25b8} Tone Curve", "\u{25b8} Color Mixer", "\u{25b8} Color Grading"] {
        if let Some(n) = hs.query_all_by_label(title).next() {
            n.click();
        }
        hs.run_steps(2);
    }
    shot(&mut hs, &dir, "grade-01-all-sections");
}

/// The pairing overlay (`panels::pairing`) over the real grid: a machine
/// already in the club, one nearby that is not, one request arriving here
/// with its code, and the sync chip open in the header.
#[test]
fn shots_pairing() {
    let Some(dir) = out_dir() else { return };
    // The operator's own language, as the shell paints it (the default is English).
    let before = crate::shell::i18n::lang();
    crate::shell::i18n::set_lang(crate::shell::i18n::Lang::Es);
    let door = realistic_door();
    door.respond("GET", "/sync/status", json!({ "running": true, "name": "sala" }));
    door.respond("GET", "/sync/peers", json!({ "peers": [
        { "node_id": "9f3ac2d14e7b", "name": "cocina", "online": true }
    ]}));
    door.respond("GET", "/sync/discover", json!({ "nearby": [
        { "node_id": "9f3ac2d14e7b", "name": "cocina", "member": true },
        { "node_id": "c04e1b8a2f6d", "name": "escenario", "member": false }
    ]}));
    door.respond("GET", "/sync/pair", json!({
        "running": true,
        "pending": [{ "node_id": "c04e1b8a2f6d", "name": "escenario", "code": "048213" }],
        "outgoing": null,
    }));
    let (w, h) = (crate::shell::DESKTOP_VIEWPORT.0 as f32, crate::shell::DESKTOP_VIEWPORT.1 as f32);
    let mut hs = harness(Box::new(Rc::clone(&door)), w, h);
    hs.run_steps(4);
    hs.get_by_label(&crate::shell::panels::pairing::chip_text(
        crate::shell::panels::pairing::Summary::Paired(1),
    ))
    .click();
    shot(&mut hs, &dir, "pairing");
    crate::shell::i18n::set_lang(before);
}

/// 2026-10-16: the shell-UI pass — the floating canvas bars, the "new"
/// gallery, the settings window and the panel heads.
#[test]
fn shots_shell_ui_pass() {
    let Some(dir) = out_dir() else { return };
    let door = realistic_door();
    door.respond("GET", "/catalog", json!({ "catalog": [
        { "kind": "gen.turing", "family": "generator", "label": "Turing", "inputs": [{ "name": "control" }], "outputs": [{ "name": "image" }], "menu": true },
        { "kind": "gen.rings", "family": "generator", "label": "Anillos", "inputs": [{ "name": "control" }], "outputs": [{ "name": "image" }], "menu": true },
        { "kind": "fx.ghost", "family": "effect", "label": "Ghost", "inputs": [{ "name": "image" }, { "name": "control" }], "outputs": [{ "name": "image" }], "menu": true },
        { "kind": "fx.shake", "family": "effect", "label": "Shake", "inputs": [{ "name": "image" }, { "name": "control" }], "outputs": [{ "name": "image" }], "menu": true },
        { "kind": "mat.metal", "family": "material", "label": "Metal", "inputs": [{ "name": "image" }, { "name": "control" }], "outputs": [{ "name": "image" }], "menu": true },
        { "kind": "audio.live", "family": "audio", "label": "Audio (live)", "inputs": [], "outputs": [{ "name": "row" }], "menu": true },
        { "kind": "output.device", "family": "output_device", "label": "Output", "inputs": [{ "name": "image" }], "outputs": [], "menu": true }
    ] }));
    door.respond("GET", "/graph", json!({ "graph": { "id": "g1", "domain": "live", "nodes": [
        { "id": "gen-1", "kind": "gen.turing", "pos": [0.0, 60.0], "label": "", "bypass": false, "solo": true, "config": {} },
        { "id": "fx-1", "kind": "fx.ghost", "pos": [220.0, 60.0], "label": "", "bypass": true, "solo": false, "config": {} },
        { "id": "out-0", "kind": "output.device", "pos": [440.0, 60.0], "label": "", "bypass": false, "solo": false, "config": {} }
    ], "edges": [] }, "primitives": [], "issues": [] }));
    let (w, h) = (crate::shell::DESKTOP_VIEWPORT.0 as f32, crate::shell::DESKTOP_VIEWPORT.1 as f32);
    let mut hs = harness(Box::new(Rc::clone(&door)), w, h);
    hs.run_steps(4);
    hs.state_mut().nodes_state_mut().set_selected_graph_for_test("g1");
    shot(&mut hs, &dir, "shellui-01-canvas-hold-bars");
    hs.state_mut().set_selection_for_test(Selection::Node("gen-1".into()));
    shot(&mut hs, &dir, "shellui-02-canvas-focus-bar");
    hs.get_by_label("new").click();
    shot(&mut hs, &dir, "shellui-03-gallery");
    hs.get_by_label("materials").click();
    shot(&mut hs, &dir, "shellui-04-gallery-materials");
    hs.key_press(egui::Key::Escape);
    hs.run_steps(2);
    hs.get_by_label("ajustes").click();
    shot(&mut hs, &dir, "shellui-05-settings");
    hs.get_by_label("language").click();
    shot(&mut hs, &dir, "shellui-06-settings-language");
    hs.key_press(egui::Key::Escape);
    hs.state_mut().lanes_state_mut().force_mode(crate::shell::lanes::Mode::Lanes);
    {
        let ingest = hs.state_mut().ingest_state_mut();
        ingest.song = "Gasolina - Remix".into();
        ingest.switch_model("turing", vec![0.5; 4]);
        ingest.device = Some("screen-1".into());
    }
    shot(&mut hs, &dir, "shellui-07-sequence");
}
